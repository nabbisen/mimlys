use super::*;
use mimlys_core::ids::ScriptedIds;
use mimlys_core::limits::{LIM_W4_MATERIALIZE_EXCLUSIONS, LIM_W5_SECRET_PATTERNS};
use mimlys_core::policy::Policy;

fn patterns() -> Vec<String> {
    let p = Policy::shipped();
    p.exclusions
        .materialize
        .iter()
        .chain(p.exclusions.secrets.iter())
        .cloned()
        .collect()
}

fn scope() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("src/deep")).unwrap();
    std::fs::write(root.join("src/lib.rs"), "fn a() {}\n").unwrap();
    std::fs::write(root.join("src/deep/x.txt"), "xx").unwrap();
    std::fs::create_dir_all(root.join("target/debug")).unwrap();
    std::fs::write(root.join("target/debug/big"), vec![0u8; 1000]).unwrap();
    std::fs::create_dir_all(root.join("node_modules/m")).unwrap();
    std::fs::write(root.join("node_modules/m/index.js"), "x").unwrap();
    std::fs::write(root.join(".env"), "SECRET=1").unwrap();
    std::fs::write(root.join("id_rsa"), "key").unwrap();
    std::fs::write(root.join("server.pem"), "pem").unwrap();
    std::fs::create_dir_all(root.join(".git/objects")).unwrap();
    std::fs::write(root.join(".git/HEAD"), "ref").unwrap();
    dir
}

fn provider(data: &Path) -> LocalWorkspaces {
    LocalWorkspaces::new(data.to_path_buf(), patterns(), 10_000, 100).unwrap()
}

/// REQ-WS-002 / REQ-WS-003: the copy has the project files and none of the
/// LIM-W4 or LIM-W5 entries; the measured numbers count what was copied.
#[test]
fn req_ws_002_copy_with_exclusions() {
    let scope = scope();
    let data = tempfile::tempdir().unwrap();
    let ids = ScriptedIds::default();
    let task = TaskId::new(&ids);
    let p = provider(data.path());
    let info = p
        .materialize(
            task,
            &ScopeSpec {
                root: scope.path().to_string_lossy().into_owned(),
                include: vec![],
            },
        )
        .unwrap();
    let tree = p.tree_path(task);
    assert!(tree.join("src/lib.rs").is_file());
    assert!(tree.join("src/deep/x.txt").is_file());
    for excluded in [
        "target",
        "node_modules",
        ".env",
        "id_rsa",
        "server.pem",
        ".git",
    ] {
        assert!(
            !tree.join(excluded).exists(),
            "{excluded} must not be materialized"
        );
    }
    assert_eq!(info.files, 2);
    assert_eq!(info.bytes, 12);
    assert_eq!(info.symlinks_omitted, 0);
    assert_eq!(p.open(task).unwrap(), info);
    assert_eq!(p.remaining_bytes(task).unwrap(), Some(10_000 - 12));
    let tools = p.tools(task).unwrap();
    assert_eq!(tools.definitions().len(), 6);
    assert!(
        LIM_W4_MATERIALIZE_EXCLUSIONS.contains(&"target/")
            && LIM_W5_SECRET_PATTERNS.contains(&".env")
    );
    assert!(tree.join("..").join("tmp").is_dir());
}

/// REQ-WS-002: a scope above the limits is refused with the measured size
/// before anything is written.
#[test]
fn req_ws_002_size_refusal_before_writing() {
    let scope = scope();
    std::fs::write(scope.path().join("src/large.bin"), vec![1u8; 20_000]).unwrap();
    let data = tempfile::tempdir().unwrap();
    let task = TaskId::new(&ScriptedIds::default());
    let p = provider(data.path());
    let err = p
        .materialize(
            task,
            &ScopeSpec {
                root: scope.path().to_string_lossy().into_owned(),
                include: vec![],
            },
        )
        .unwrap_err();
    assert!(
        matches!(err, WorkspaceError::TooLarge { bytes, .. } if bytes > 10_000),
        "{err:?}"
    );
    let tree = p.tree_path(task);
    assert!(
        std::fs::read_dir(&tree).unwrap().next().is_none(),
        "nothing is copied when the scope is refused"
    );
}

/// REQ-WS-002: symbolic links are recreated only when their target stays
/// inside the scope; others are omitted and counted.
#[cfg(unix)]
#[test]
fn req_ws_002_symlink_rule() {
    let scope = scope();
    let root = scope.path();
    std::os::unix::fs::symlink("lib.rs", root.join("src/inside.rs")).unwrap();
    std::os::unix::fs::symlink("../src/lib.rs", root.join("src/deep/up.rs")).unwrap();
    std::os::unix::fs::symlink("/etc/passwd", root.join("src/absolute")).unwrap();
    std::os::unix::fs::symlink("../../outside", root.join("src/escape")).unwrap();
    let data = tempfile::tempdir().unwrap();
    let task = TaskId::new(&ScriptedIds::default());
    let p = provider(data.path());
    let info = p
        .materialize(
            task,
            &ScopeSpec {
                root: root.to_string_lossy().into_owned(),
                include: vec![],
            },
        )
        .unwrap();
    let tree = p.tree_path(task);
    assert!(
        tree.join("src/inside.rs")
            .symlink_metadata()
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(
        tree.join("src/deep/up.rs")
            .symlink_metadata()
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(!tree.join("src/absolute").symlink_metadata().is_ok());
    assert!(!tree.join("src/escape").symlink_metadata().is_ok());
    assert_eq!(info.symlinks_omitted, 2);
    assert_eq!(info.files, 2, "links are not counted as files");
}

/// REQ-TASK-001: the scope must exist and lie outside the data directory.
#[test]
fn req_task_001_scope_rules() {
    let data = tempfile::tempdir().unwrap();
    let task = TaskId::new(&ScriptedIds::default());
    let p = provider(data.path());
    let missing = ScopeSpec {
        root: data.path().join("nope").to_string_lossy().into_owned(),
        include: vec![],
    };
    assert_eq!(
        p.materialize(task, &missing).unwrap_err(),
        WorkspaceError::ScopeNotFound
    );
    let inside = ScopeSpec {
        root: data.path().to_string_lossy().into_owned(),
        include: vec![],
    };
    assert_eq!(
        p.materialize(task, &inside).unwrap_err(),
        WorkspaceError::InsideDataDir
    );
    let parent = ScopeSpec {
        root: data.path().parent().unwrap().to_string_lossy().into_owned(),
        include: vec![],
    };
    assert_eq!(
        p.materialize(task, &parent).unwrap_err(),
        WorkspaceError::ContainsDataDir
    );
    assert_eq!(p.open(task).unwrap_err(), WorkspaceError::Missing);
}

mod snapshots {
    use super::*;
    use mimlys_core::clock::Instant;
    use mimlys_core::snapshot::Entry;
    use mimlys_core::snapshot::{ChangeKind, Promotion, SnapshotReason, SnapshotRecord};
    use mimlys_core::spi::DeletedRows;

    struct Fx {
        _data: tempfile::TempDir,
        scope: tempfile::TempDir,
        p: LocalWorkspaces,
        task: TaskId,
        ids: ScriptedIds,
        n: std::sync::atomic::AtomicU32,
    }

    impl Fx {
        fn new() -> Self {
            let data = tempfile::tempdir().unwrap();
            let scope = scope();
            let ids = ScriptedIds::default();
            let task = TaskId::new(&ids);
            let p = provider(data.path());
            p.materialize(
                task,
                &ScopeSpec {
                    root: scope.path().to_string_lossy().into_owned(),
                    include: vec![],
                },
            )
            .unwrap();
            Self {
                _data: data,
                scope,
                p,
                task,
                ids,
                n: std::sync::atomic::AtomicU32::new(0),
            }
        }

        fn now(&self) -> Instant {
            let n = self.n.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            Instant::from_unix_millis(1_700_000_000_000 + i64::from(n)).unwrap()
        }

        fn snap(
            &self,
            reason: SnapshotReason,
            previous: Option<&mimlys_core::snapshot::SnapshotRecord>,
        ) -> mimlys_core::spi::PreparedSnapshot {
            let prepared = self
                .p
                .snapshot(
                    self.task,
                    mimlys_core::ids::SnapshotId::new(&self.ids),
                    reason,
                    None,
                    previous,
                    self.now(),
                )
                .unwrap();
            if !prepared.reused {
                self.p.seal(prepared.record.snapshot_id).unwrap();
            }
            prepared
        }

        fn tree(&self) -> PathBuf {
            self.p.tree_path(self.task)
        }

        fn data(&self) -> &Path {
            self._data.path()
        }
    }

    /// REQ-WS-004 / REQ-WS-006 / REQ-TEST-009: snapshot, change, restore: the
    /// tree's hashes equal the manifest's; extras are deleted; a second
    /// snapshot of an unchanged tree reuses the first.
    #[test]
    fn req_ws_006_snapshot_and_restore_by_hash() {
        let fx = Fx::new();
        let base = fx.snap(SnapshotReason::Base, None);
        assert!(!base.reused);
        assert_eq!(base.record.files, 2);
        let again = fx.snap(SnapshotReason::Before, Some(&base.record));
        assert!(again.reused, "an unchanged tree shares the snapshot");
        assert_eq!(again.record.snapshot_id, base.record.snapshot_id);
        std::fs::write(fx.tree().join("src/lib.rs"), "fn a() { changed }\n").unwrap();
        std::fs::write(fx.tree().join("src/new.rs"), "new").unwrap();
        std::fs::create_dir_all(fx.tree().join("extra/dir")).unwrap();
        std::fs::write(fx.tree().join("extra/dir/f"), "x").unwrap();
        std::fs::remove_file(fx.tree().join("src/deep/x.txt")).unwrap();
        let after = fx.snap(SnapshotReason::After, Some(&base.record));
        assert!(!after.reused);
        let manifest = fx.p.manifest(&after.record).unwrap();
        let d = mimlys_core::snapshot::diff(&fx.p.manifest(&base.record).unwrap(), &manifest);
        let kinds: Vec<(String, ChangeKind)> =
            d.changes.iter().map(|c| (c.path.clone(), c.kind)).collect();
        assert_eq!(
            kinds,
            [
                ("extra".to_owned(), ChangeKind::Added),
                ("extra/dir".to_owned(), ChangeKind::Added),
                ("extra/dir/f".to_owned(), ChangeKind::Added),
                ("src/deep/x.txt".to_owned(), ChangeKind::Deleted),
                ("src/lib.rs".to_owned(), ChangeKind::Modified),
                ("src/new.rs".to_owned(), ChangeKind::Added),
            ]
        );
        let report = fx.p.restore(fx.task, &base.record).unwrap();
        assert!(report.deleted >= 2 && report.written >= 2, "{report:?}");
        let restored = walk(&fx);
        assert_eq!(
            restored.hash(),
            base.record.manifest_hash,
            "the tree equals the base manifest"
        );
        assert!(!fx.tree().join("extra").exists());
        assert_eq!(
            std::fs::read_to_string(fx.tree().join("src/lib.rs")).unwrap(),
            "fn a() {}\n"
        );
        assert_eq!(
            std::fs::read_to_string(fx.tree().join("src/deep/x.txt")).unwrap(),
            "xx"
        );
        // Forward again: restore the after snapshot.
        fx.p.restore(fx.task, &after.record).unwrap();
        assert_eq!(walk(&fx).hash(), after.record.manifest_hash);
        assert_eq!(
            std::fs::read_to_string(fx.tree().join("src/new.rs")).unwrap(),
            "new"
        );
    }

    /// REQ-WS-004, REQ-SEC-012 (handoff 008c): the hashes of a walked tree are those
    /// 0.9.0 computed, on a short file and on a file of several read blocks.
    #[test]
    fn req_ws_004_golden_walk_hashes_of_0_9_0() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("tools")).unwrap();
        std::fs::write(dir.path().join("a.txt"), b"fix the failing test\n").unwrap();
        let big: Vec<u8> = (0..70_000u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(dir.path().join("tools/big.bin"), &big).unwrap();
        let handle =
            cap_std::fs::Dir::open_ambient_dir(dir.path(), cap_std::ambient_authority()).unwrap();
        let manifest = super::super::snapshot::walk_manifest(&handle, None).unwrap();
        let hashes: Vec<(String, String)> = manifest
            .entries
            .iter()
            .filter_map(|e| match e {
                mimlys_core::snapshot::Entry::File { path, hash, .. } => {
                    Some((path.clone(), hash.to_string()))
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            hashes,
            [
                (
                    "a.txt".to_owned(),
                    "f3ea8f2a2e6215bd4b5c03ca09e9cfc6e81652ecc7349fb5e82165edd04b989b".to_owned()
                ),
                (
                    "tools/big.bin".to_owned(),
                    "9dc177c2fde29dea8e7c29f7ddf147b7c449c99d049c62f3aac0a5933ecf76a3".to_owned()
                )
            ]
        );
    }

    /// Gives the permissions back when dropped, so that a failed assertion leaves a tree
    /// that can be removed.
    #[cfg(unix)]
    struct Restore(PathBuf);

    #[cfg(unix)]
    impl Drop for Restore {
        fn drop(&mut self) {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o755));
        }
    }

    /// REQ-WS-004, REQ-WS-006, REQ-WS-008: a directory the task cannot read is named by every
    /// walk of the tree (the snapshot, the restore, the quota check) as `Unreadable` with its
    /// path, relative to the tree, and so is a file; none of them is the storage failure
    /// `walk`. With the permissions restored, each works again. Unix only; the permissions do
    /// not stop the superuser, and the test then has nothing to show and returns.
    #[cfg(unix)]
    #[test]
    fn req_ws_004_an_unreadable_directory_or_file_is_named_by_every_walk() {
        use std::os::unix::fs::PermissionsExt;
        let fx = Fx::new();
        let base = fx.snap(SnapshotReason::Base, None);
        let unreadable = |path: &str| WorkspaceError::Unreadable { path: path.into() };
        for (path, mode_on_directory) in [("src/deep", true), ("src/lib.rs", false)] {
            let target = fx.tree().join(path);
            let _restore = Restore(target.clone());
            std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o000)).unwrap();
            let readable = if mode_on_directory {
                std::fs::read_dir(&target).is_ok()
            } else {
                std::fs::read(&target).is_ok()
            };
            if readable {
                eprintln!("skipped: the permissions do not apply to this user");
                return;
            }
            let dir = cap_std::fs::Dir::open_ambient_dir(fx.tree(), cap_std::ambient_authority())
                .unwrap();
            assert_eq!(
                super::super::snapshot::walk_manifest(&dir, None).unwrap_err(),
                unreadable(path)
            );
            assert_eq!(
                fx.p.snapshot(
                    fx.task,
                    mimlys_core::ids::SnapshotId::new(&fx.ids),
                    SnapshotReason::Before,
                    None,
                    Some(&base.record),
                    fx.now(),
                )
                .unwrap_err(),
                unreadable(path),
                "a snapshot names {path}"
            );
            assert_eq!(
                fx.p.restore(fx.task, &base.record).unwrap_err(),
                unreadable(path),
                "a restore names {path}"
            );
            if mode_on_directory {
                assert_eq!(
                    fx.p.remaining_bytes(fx.task).unwrap_err(),
                    unreadable(path),
                    "the quota check names {path}"
                );
            } else {
                // The size of a file is its metadata: its permissions do not stop it.
                fx.p.remaining_bytes(fx.task).unwrap();
            }
            std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755)).unwrap();
            fx.p.remaining_bytes(fx.task).unwrap();
            fx.p.restore(fx.task, &base.record).unwrap();
            assert!(
                fx.p.snapshot(
                    fx.task,
                    mimlys_core::ids::SnapshotId::new(&fx.ids),
                    SnapshotReason::Before,
                    None,
                    Some(&base.record),
                    fx.now(),
                )
                .is_ok()
            );
        }
    }

    /// The path of an `Unreadable` error is one clean line: a name with control or
    /// bidirectional characters is cleaned, and a long one is bounded.
    #[test]
    fn req_ws_004_the_path_of_an_unreadable_error_is_cleaned_and_bounded() {
        let error = super::super::snapshot::unreadable("a/\u{1b}[31mred\u{202e}b\nc")(
            std::io::ErrorKind::PermissionDenied.into(),
        );
        let WorkspaceError::Unreadable { path } = error else {
            panic!("{error:?}")
        };
        assert!(!path.contains('\u{1b}') && !path.contains('\u{202e}') && !path.contains('\n'));
        let long = "x".repeat(10_000);
        let WorkspaceError::Unreadable { path } =
            super::super::snapshot::unreadable(&long)(std::io::ErrorKind::PermissionDenied.into())
        else {
            panic!()
        };
        assert!(path.chars().count() <= mimlys_core::limits::LIM_I5_MESSAGE_CHARS);
        let WorkspaceError::Unreadable { path } =
            super::super::snapshot::unreadable("")(std::io::ErrorKind::PermissionDenied.into())
        else {
            panic!()
        };
        assert_eq!(path, ".", "the root of the tree");
    }

    /// REQ-WS-004: only a refused permission is `Unreadable`, which asks the user to restore
    /// the permissions. Any other failure of a read of the walk names the path with the kind
    /// of the failure and gives no such advice. The errors are made up where the walk maps
    /// them.
    #[test]
    fn req_ws_004_only_a_permission_error_asks_to_restore_permissions() {
        use std::io::ErrorKind;
        let map = super::super::snapshot::unreadable("src/deep");

        let permission = map(ErrorKind::PermissionDenied.into());
        assert_eq!(
            permission,
            WorkspaceError::Unreadable {
                path: "src/deep".into()
            }
        );
        assert_eq!(
            permission.to_string(),
            "part of the workspace could not be read: src/deep; restore its permissions, then try again"
        );

        for kind in [
            ErrorKind::NotFound,
            ErrorKind::TimedOut,
            ErrorKind::InvalidData,
            ErrorKind::Other,
        ] {
            let error = map(kind.into());
            let words = kind.to_string();
            assert_eq!(
                error,
                WorkspaceError::ReadFailed {
                    path: "src/deep".into(),
                    kind: words.clone()
                },
                "{kind:?}"
            );
            assert_eq!(
                error.to_string(),
                format!("part of the workspace could not be read: src/deep ({words})"),
                "{kind:?}"
            );
            assert!(!words.is_empty() && !error.to_string().contains("permission"));
        }
        assert_eq!(
            map(ErrorKind::NotFound.into()).to_string(),
            "part of the workspace could not be read: src/deep (entity not found)"
        );
    }

    fn walk(fx: &Fx) -> mimlys_core::snapshot::Manifest {
        let dir =
            cap_std::fs::Dir::open_ambient_dir(fx.tree(), cap_std::ambient_authority()).unwrap();
        super::super::snapshot::walk_manifest(&dir, None).unwrap()
    }

    /// RFC 006 §3: a file rewritten with the same size and modification time
    /// is still seen, because every file is hashed every time.
    #[test]
    fn rfc_006_same_size_same_mtime_is_seen() {
        let fx = Fx::new();
        let base = fx.snap(SnapshotReason::Base, None);
        let path = fx.tree().join("src/lib.rs");
        let mtime = std::fs::metadata(&path).unwrap().modified().unwrap();
        std::fs::write(&path, "fn b() {}\n").unwrap(); // same length as "fn a() {}\n"
        let file = std::fs::File::options().write(true).open(&path).unwrap();
        file.set_modified(mtime).unwrap();
        let next = fx.snap(SnapshotReason::Before, Some(&base.record));
        assert!(!next.reused, "the change is seen without any index");
    }

    /// REQ-WS-005 / THR-27 / INV-08 (external design §21.2): links are
    /// entries; a link whose target leaves the root is stored but never
    /// followed (`/etc/passwd` is not hashed into the snapshot), and refused
    /// at restore (never recreated). INV-08's symbolic-link clause for both
    /// snapshot and restore.
    #[cfg(unix)]
    #[test]
    fn req_ws_005_symlinks_both_directions() {
        let fx = Fx::new();
        std::os::unix::fs::symlink("lib.rs", fx.tree().join("src/inside")).unwrap();
        std::os::unix::fs::symlink("/etc/passwd", fx.tree().join("src/escape")).unwrap();
        let snap = fx.snap(SnapshotReason::After, None);
        let manifest = fx.p.manifest(&snap.record).unwrap();
        let entries: Vec<&Entry> = manifest
            .entries
            .iter()
            .filter(|e| matches!(e, Entry::Symlink { .. }))
            .collect();
        assert_eq!(entries.len(), 2);
        assert!(
            manifest
                .entries
                .iter()
                .all(|e| !matches!(e, Entry::File { path, .. } if path == "src/escape")),
            "the target was not followed"
        );
        assert_eq!(
            manifest.files(),
            2,
            "/etc/passwd was not hashed into the snapshot"
        );
        std::fs::remove_file(fx.tree().join("src/inside")).unwrap();
        std::fs::remove_file(fx.tree().join("src/escape")).unwrap();
        let report = fx.p.restore(fx.task, &snap.record).unwrap();
        assert_eq!(report.links_omitted, ["src/escape"]);
        assert!(
            fx.tree()
                .join("src/inside")
                .symlink_metadata()
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(
            fx.tree().join("src/escape").symlink_metadata().is_err(),
            "an escaping link is never recreated"
        );
    }

    /// REQ-WS-007 / REQ-SEC-015 / INV-10: promotion diffs against the base,
    /// refuses conflicts, backs up, applies only the plan, and reverses.
    #[test]
    fn req_ws_007_promotion_conflicts_backup_and_reversal() {
        let fx = Fx::new();
        let base = fx.snap(SnapshotReason::Base, None);
        let root = fx.scope.path().to_string_lossy().into_owned();
        std::fs::write(fx.tree().join("src/lib.rs"), "fn a() -> i32 { 1 }\n").unwrap();
        std::fs::write(fx.tree().join("src/added.rs"), "added").unwrap();
        std::fs::remove_file(fx.tree().join("src/deep/x.txt")).unwrap();
        std::fs::write(fx.tree().join(".env"), "SECRET=2").unwrap();
        // A conflict: the user edited a target after the task started.
        std::fs::write(fx.scope.path().join("src/lib.rs"), "fn a() { user edit }\n").unwrap();
        let err =
            fx.p.promotion_plan(fx.task, &base.record, &root)
                .unwrap_err();
        assert_eq!(
            err,
            WorkspaceError::Conflict {
                paths: vec!["src/lib.rs".into()]
            }
        );
        std::fs::write(fx.scope.path().join("src/lib.rs"), "fn a() {}\n").unwrap();
        let plan = fx.p.promotion_plan(fx.task, &base.record, &root).unwrap();
        let paths: Vec<&str> = plan.changes.iter().map(|c| c.path.as_str()).collect();
        assert_eq!(paths, ["src/added.rs", "src/deep/x.txt", "src/lib.rs"]);
        assert_eq!(plan.excluded, [".env"], "LIM-W5 paths are never promoted");
        assert!(
            plan.text_diffs
                .iter()
                .any(|(p, d)| p == "src/lib.rs" && d.contains("+fn a() -> i32 { 1 }"))
        );
        let (backup, promotion) = fx.promote(&plan, &base.record).unwrap();
        fx.p.seal(backup.record.snapshot_id).unwrap();
        assert_eq!(backup.record.reason, SnapshotReason::Backup);
        assert_eq!(
            std::fs::read_to_string(fx.scope.path().join("src/lib.rs")).unwrap(),
            "fn a() -> i32 { 1 }\n"
        );
        assert_eq!(
            std::fs::read_to_string(fx.scope.path().join("src/added.rs")).unwrap(),
            "added"
        );
        assert!(!fx.scope.path().join("src/deep/x.txt").exists());
        assert_eq!(
            std::fs::read_to_string(fx.scope.path().join(".env")).unwrap(),
            "SECRET=1",
            "excluded, untouched"
        );
        assert_eq!(promotion.files.len(), 3);
        // A stale plan is refused.
        std::fs::write(fx.tree().join("src/lib.rs"), "fn a() -> i32 { 2 }\n").unwrap();
        let err = fx.promote(&plan, &base.record).unwrap_err();
        assert!(matches!(err, WorkspaceError::Refused { .. }));
        // Reversal from the backup.
        let report =
            fx.p.revert_promotion(
                &promotion,
                &backup.record,
                mimlys_core::spi::EditedPaths::Keep,
            )
            .unwrap();
        assert!(report.written >= 2 && report.deleted == 1, "{report:?}");
        assert_eq!(
            std::fs::read_to_string(fx.scope.path().join("src/lib.rs")).unwrap(),
            "fn a() {}\n"
        );
        assert!(!fx.scope.path().join("src/added.rs").exists());
        assert_eq!(
            std::fs::read_to_string(fx.scope.path().join("src/deep/x.txt")).unwrap(),
            "xx"
        );
    }

    /// The names of the temporary files left under `root` (none are left by a write that ended).
    fn temp_leftovers(root: &Path) -> Vec<String> {
        let mut found = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap().flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with(".mimlys-tmp-") {
                    found.push(name);
                }
                if entry.file_type().unwrap().is_dir() {
                    stack.push(entry.path());
                }
            }
        }
        found
    }

    /// REQ-WS-006, REQ-WS-007, REQ-WS-003: a tree that holds a name of 255 bytes restores, forks and
    /// promotes; the temporary name of a write does not depend on the name of the entry.
    #[test]
    fn req_ws_006_a_name_of_255_bytes_restores_forks_and_promotes() {
        let fx = Fx::new();
        let base = fx.snap(SnapshotReason::Base, None);
        let name = "n".repeat(255);
        std::fs::write(fx.tree().join(&name), "v1").unwrap();
        std::fs::write(fx.tree().join("src").join(&name), "v1 nested").unwrap();
        let after = fx.snap(SnapshotReason::After, Some(&base.record));
        // Restore: the entries are changed, then made equal to the snapshot again.
        std::fs::write(fx.tree().join(&name), "v2").unwrap();
        std::fs::write(fx.tree().join("src").join(&name), "v2").unwrap();
        fx.p.restore(fx.task, &after.record).unwrap();
        assert_eq!(
            std::fs::read_to_string(fx.tree().join(&name)).unwrap(),
            "v1"
        );
        assert_eq!(
            std::fs::read_to_string(fx.tree().join("src").join(&name)).unwrap(),
            "v1 nested"
        );
        assert_eq!(walk(&fx).hash(), after.record.manifest_hash);
        assert!(temp_leftovers(&fx.tree()).is_empty());
        // Fork.
        let new_task = TaskId::new(&fx.ids);
        let (_, prepared) =
            fx.p.materialize_from(
                new_task,
                mimlys_core::ids::SnapshotId::new(&fx.ids),
                &after.record,
                &ScopeSpec {
                    root: fx.scope.path().to_string_lossy().into_owned(),
                    include: vec![],
                },
                fx.now(),
            )
            .unwrap();
        fx.p.seal(prepared.record.snapshot_id).unwrap();
        assert_eq!(
            std::fs::read_to_string(fx.p.tree_path(new_task).join(&name)).unwrap(),
            "v1"
        );
        assert!(temp_leftovers(&fx.p.tree_path(new_task)).is_empty());
        // Promote, and reverse.
        let root = fx.scope.path().to_string_lossy().into_owned();
        let plan = fx.p.promotion_plan(fx.task, &base.record, &root).unwrap();
        assert!(plan.changes.iter().any(|c| c.path == name));
        let (backup, promotion) = fx.promote(&plan, &base.record).unwrap();
        fx.p.seal(backup.record.snapshot_id).unwrap();
        assert_eq!(
            std::fs::read_to_string(fx.scope.path().join(&name)).unwrap(),
            "v1"
        );
        assert_eq!(
            std::fs::read_to_string(fx.scope.path().join("src").join(&name)).unwrap(),
            "v1 nested"
        );
        assert!(temp_leftovers(fx.scope.path()).is_empty());
        fx.p.revert_promotion(
            &promotion,
            &backup.record,
            mimlys_core::spi::EditedPaths::Keep,
        )
        .unwrap();
        assert!(!fx.scope.path().join(&name).exists());
        assert!(temp_leftovers(fx.scope.path()).is_empty());
    }

    /// REQ-WS-006, REQ-WS-007: an entry whose name ends as the old temporary names did is an entry
    /// like any other: a restore and a promotion leave it as it is, and the entry beside it is right.
    #[test]
    fn req_ws_006_an_entry_named_like_a_temporary_file_is_untouched() {
        let fx = Fx::new();
        for root in [fx.tree(), fx.scope.path().to_path_buf()] {
            std::fs::write(root.join("foo"), "foo v1").unwrap();
            std::fs::write(root.join("foo.mimlys-tmp"), "keep").unwrap();
            std::fs::write(root.join("other"), "other v1").unwrap();
            #[cfg(unix)]
            {
                std::fs::write(root.join("bar"), "bar v1").unwrap();
                std::os::unix::fs::symlink("other", root.join("bar.mimlys-tmp")).unwrap();
            }
        }
        let base = fx.snap(SnapshotReason::Base, None);
        // Restore.
        std::fs::write(fx.tree().join("foo"), "foo v2").unwrap();
        #[cfg(unix)]
        std::fs::write(fx.tree().join("bar"), "bar v2").unwrap();
        let after = fx.snap(SnapshotReason::After, Some(&base.record));
        std::fs::write(fx.tree().join("foo"), "foo v3").unwrap();
        #[cfg(unix)]
        std::fs::write(fx.tree().join("bar"), "bar v3").unwrap();
        fx.p.restore(fx.task, &after.record).unwrap();
        let check = |root: &Path, foo: &str, bar: &str| {
            assert_eq!(std::fs::read_to_string(root.join("foo")).unwrap(), foo);
            assert_eq!(
                std::fs::read_to_string(root.join("foo.mimlys-tmp")).unwrap(),
                "keep"
            );
            assert_eq!(
                std::fs::read_to_string(root.join("other")).unwrap(),
                "other v1"
            );
            #[cfg(unix)]
            {
                assert_eq!(std::fs::read_to_string(root.join("bar")).unwrap(), bar);
                assert!(
                    root.join("bar.mimlys-tmp")
                        .symlink_metadata()
                        .unwrap()
                        .file_type()
                        .is_symlink(),
                    "the link is still a link"
                );
                assert_eq!(
                    std::fs::read_link(root.join("bar.mimlys-tmp")).unwrap(),
                    Path::new("other")
                );
            }
            #[cfg(not(unix))]
            let _ = bar;
        };
        check(&fx.tree(), "foo v2", "bar v2");
        assert!(temp_leftovers(&fx.tree()).is_empty());
        // Promote what the tree holds now; the project holds the base.
        let root = fx.scope.path().to_string_lossy().into_owned();
        let plan = fx.p.promotion_plan(fx.task, &base.record, &root).unwrap();
        let (backup, promotion) = fx.promote(&plan, &base.record).unwrap();
        fx.p.seal(backup.record.snapshot_id).unwrap();
        check(fx.scope.path(), "foo v2", "bar v2");
        fx.p.revert_promotion(
            &promotion,
            &backup.record,
            mimlys_core::spi::EditedPaths::Keep,
        )
        .unwrap();
        check(fx.scope.path(), "foo v1", "bar v1");
        assert!(temp_leftovers(fx.scope.path()).is_empty());
    }

    /// REQ-WS-006: a write that fails leaves no temporary file behind (an object that does not
    /// match its name is refused after it was written to its temporary file).
    #[test]
    fn req_ws_006_a_failed_write_leaves_no_temporary_file() {
        let fx = Fx::new();
        let base = fx.snap(SnapshotReason::Base, None);
        std::fs::write(fx.tree().join("src/lib.rs"), "fn a() { changed }\n").unwrap();
        // The stored object of the file the restore will write is damaged.
        let manifest = fx.p.manifest(&base.record).unwrap();
        let hash = manifest
            .entries
            .iter()
            .find_map(|e| match e {
                Entry::File { path, hash, .. } if path == "src/lib.rs" => Some(hash.clone()),
                _ => None,
            })
            .unwrap();
        let object = fx.data().join("snapshots/objects").join(hash.as_str());
        let mut perms = std::fs::metadata(&object).unwrap().permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        perms.set_readonly(false);
        std::fs::set_permissions(&object, perms).unwrap();
        std::fs::write(&object, "damaged").unwrap();
        let err = fx.p.restore(fx.task, &base.record).unwrap_err();
        assert!(matches!(err, WorkspaceError::Storage { .. }), "{err:?}");
        assert!(temp_leftovers(&fx.tree()).is_empty());
        assert_eq!(
            std::fs::read_to_string(fx.tree().join("src/lib.rs")).unwrap(),
            "fn a() { changed }\n",
            "the file is as it was"
        );
    }

    /// REQ-WS-006: the temporary file of a write is removed when the write is not completed.
    #[test]
    fn req_ws_006_a_temporary_file_that_is_not_placed_is_removed() {
        let fx = Fx::new();
        let dir =
            cap_std::fs::Dir::open_ambient_dir(fx.tree(), cap_std::ambient_authority()).unwrap();
        let (a, _) = super::super::snapshot::TempFile::create(&dir, "src/lib.rs").unwrap();
        let (b, _) = super::super::snapshot::TempFile::create(&dir, "src/lib.rs").unwrap();
        assert_eq!(temp_leftovers(&fx.tree()).len(), 2, "two names, both new");
        drop(a);
        drop(b);
        assert!(temp_leftovers(&fx.tree()).is_empty());
    }

    /// Writes `text` at `rel` under `root`, making the directories.
    fn put(root: &Path, rel: &str, text: &str) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn read(root: &Path, rel: &str) -> String {
        std::fs::read_to_string(root.join(rel)).unwrap()
    }

    impl Fx {
        fn root(&self) -> String {
            self.scope.path().to_string_lossy().into_owned()
        }

        fn plan(
            &self,
            base: &SnapshotRecord,
        ) -> Result<mimlys_core::snapshot::PromotionPlan, WorkspaceError> {
            self.p.promotion_plan(self.task, base, &self.root())
        }

        /// Phase 2 as the runtime runs it, without its records: the backup, its seal, the apply.
        /// An error leaves the project as the apply left it.
        fn promote(
            &self,
            plan: &mimlys_core::snapshot::PromotionPlan,
            base: &SnapshotRecord,
        ) -> Result<(mimlys_core::spi::PreparedSnapshot, Promotion), WorkspaceError> {
            let (backup, mut promotion) = self.promote_backup(plan, base)?;
            self.p.seal(backup.record.snapshot_id)?;
            self.p.promote_apply(self.task, &promotion, base)?;
            promotion.state = mimlys_core::snapshot::PromotionState::Applied;
            Ok((backup, promotion))
        }

        /// Step 1: the backup, and the promotion `applying`.
        fn promote_backup(
            &self,
            plan: &mimlys_core::snapshot::PromotionPlan,
            base: &SnapshotRecord,
        ) -> Result<(mimlys_core::spi::PreparedSnapshot, Promotion), WorkspaceError> {
            self.p.promote_backup(
                self.task,
                &mimlys_core::spi::PromotionRequest {
                    plan,
                    base,
                    project_root: &self.root(),
                    backup_id: mimlys_core::ids::SnapshotId::new(&self.ids),
                    promotion_id: mimlys_core::ids::PromotionId::new(&self.ids),
                    now: self.now(),
                },
            )
        }

        /// Installs the step into the apply of a promotion.
        fn on_apply(
            &self,
            hook: impl Fn(&str) -> Result<(), WorkspaceError> + Send + Sync + 'static,
        ) {
            *self.p.apply_hook.lock().unwrap() = Some(Box::new(hook));
        }
    }

    /// Runs `work` on a thread of its own and returns its result. A named pipe with no other end
    /// holds a blocking open for ever, so a regression ends the test binary here, saying what
    /// blocked, instead of hanging the run; what the test decides is the state it finds.
    #[cfg(unix)]
    fn without_blocking<T: Send>(what: &str, work: impl FnOnce() -> T + Send) -> T {
        let (done, result) = std::sync::mpsc::channel();
        std::thread::scope(|scope| {
            scope.spawn(move || {
                let _ = done.send(work());
            });
            match result.recv_timeout(std::time::Duration::from_secs(120)) {
                Ok(value) => value,
                Err(_) => {
                    eprintln!("blocked: {what}");
                    std::process::exit(1)
                }
            }
        })
    }

    #[cfg(unix)]
    fn mkfifo(path: &Path) {
        let status = std::process::Command::new("mkfifo")
            .arg(path)
            .status()
            .expect("mkfifo runs");
        assert!(status.success(), "mkfifo");
    }

    /// Makes a socket at `at`. The path of a socket may not be longer than the system allows
    /// (104 bytes on macOS), which the path of a test's tree can be: it is bound where the path is
    /// short, in a directory of its own beside the test's, and moved.
    #[cfg(unix)]
    fn make_socket(at: &Path) {
        let short = tempfile::tempdir().unwrap();
        let from = short.path().join("s");
        let listener = std::os::unix::net::UnixListener::bind(&from).unwrap();
        std::fs::rename(&from, at).unwrap();
        drop(listener);
    }

    #[cfg(unix)]
    fn is_special(path: &Path) -> bool {
        use std::os::unix::fs::FileTypeExt;
        let t = std::fs::symlink_metadata(path).unwrap().file_type();
        t.is_fifo() || t.is_socket()
    }

    /// REQ-WS-007: a path of the plan that is a named pipe or a socket in the project is refused
    /// in the plan, naming it and what it is, and nothing blocks (the open of a named pipe
    /// waits for its other end). The path may be one the workspace changed and the base had, or
    /// one the workspace added. The project is as it was.
    #[cfg(unix)]
    #[test]
    fn req_ws_007_a_named_pipe_or_socket_at_a_path_of_the_plan_refuses_the_plan() {
        for (name, kind, make) in [
            ("src/lib.rs", "named pipe", true),
            ("src/new.rs", "named pipe", true),
            ("src/lib.rs", "socket", false),
            ("src/new.rs", "socket", false),
        ] {
            let fx = Fx::new();
            let base = fx.snap(SnapshotReason::Base, None);
            put(&fx.tree(), "src/lib.rs", "changed in the workspace");
            put(&fx.tree(), "src/new.rs", "new");
            let target = fx.scope.path().join(name);
            let _ = std::fs::remove_file(&target);
            if make {
                mkfifo(&target);
            } else {
                make_socket(&target);
            }
            let (p, task, root) = (&fx.p, fx.task, fx.root());
            let err = without_blocking("the plan", || p.promotion_plan(task, &base.record, &root))
                .unwrap_err();
            let WorkspaceError::Refused { reason } = &err else {
                panic!("{name} ({kind}): {err:?}");
            };
            assert!(
                reason.contains(name) && reason.contains(kind),
                "{name} ({kind}): {reason}"
            );
            assert!(is_special(&target), "{name} ({kind}) is untouched");
        }
    }

    /// REQ-WS-007: a named pipe that appears at a path of the plan after the plan was made is
    /// refused by the backup and by the apply, as the plan refuses it; nothing blocks, and what
    /// the apply had written is restored by the runtime as for any error.
    #[cfg(unix)]
    #[test]
    fn req_ws_007_a_named_pipe_that_appears_after_the_plan_is_refused_without_blocking() {
        // Before the backup: the stale plan is refused, naming the path.
        let fx = Fx::new();
        let base = fx.snap(SnapshotReason::Base, None);
        put(&fx.tree(), "src/new.rs", "new");
        let plan = fx.plan(&base.record).unwrap();
        let target = fx.scope.path().join("src/new.rs");
        mkfifo(&target);
        let err = without_blocking("the backup", || fx.promote_backup(&plan, &base.record))
            .map(|_| ())
            .unwrap_err();
        assert!(
            matches!(&err, WorkspaceError::Refused { reason } if reason.contains("src/new.rs") && reason.contains("named pipe")),
            "{err:?}"
        );
        assert!(is_special(&target));
        // During the apply: the path becomes a named pipe just before it is written.
        let fx = Fx::new();
        let base = fx.snap(SnapshotReason::Base, None);
        put(&fx.tree(), "src/new.rs", "new");
        let plan = fx.plan(&base.record).unwrap();
        let target = fx.scope.path().join("src/new.rs");
        let hook_target = target.clone();
        fx.on_apply(move |path| {
            if path == "src/new.rs" {
                mkfifo(&hook_target);
            }
            Ok(())
        });
        let err = without_blocking("the apply", || fx.promote(&plan, &base.record))
            .map(|_| ())
            .unwrap_err();
        assert!(
            matches!(&err, WorkspaceError::Refused { reason } if reason.contains("src/new.rs") && reason.contains("named pipe")),
            "{err:?}"
        );
        assert!(is_special(&target), "the pipe is not written over");
    }

    /// REQ-WS-007: the open of a project file does not wait for a named pipe (`O_NONBLOCK`) and
    /// looks at what it got through the handle: a named pipe is refused by name, a directory too.
    /// Here the path was not looked at first, which is the race the look cannot cover.
    #[cfg(unix)]
    #[test]
    fn req_ws_007_the_open_of_a_project_file_never_waits_for_a_named_pipe() {
        let dir = tempfile::tempdir().unwrap();
        mkfifo(&dir.path().join("pipe"));
        std::fs::create_dir(dir.path().join("directory")).unwrap();
        std::fs::write(dir.path().join("file"), "x").unwrap();
        let handle =
            cap_std::fs::Dir::open_ambient_dir(dir.path(), cap_std::ambient_authority()).unwrap();
        for (name, what) in [("pipe", "named pipe"), ("directory", "directory")] {
            let error = without_blocking(name, || {
                super::super::snapshot::open_file(&handle, name).map(|_| ())
            })
            .unwrap_err();
            assert!(
                matches!(&error, WorkspaceError::Refused { reason } if reason.contains(name) && reason.contains(what)),
                "{name}: {error:?}"
            );
        }
        assert!(super::super::snapshot::open_file(&handle, "file").is_ok());
    }

    /// REQ-WS-007: a directory the promotion would remove, or replace by a file, holds entries the
    /// plan does not list (a file of the project that was never materialized, a file added since):
    /// the plan is refused, names them, and the project is unchanged. Once they are dealt with,
    /// the plan applies.
    #[test]
    fn req_ws_007_a_deleted_directory_with_unlisted_entries_is_refused() {
        for replaced_by_file in [false, true] {
            let fx = Fx::new();
            for root in [fx.tree(), fx.scope.path().to_path_buf()] {
                put(&root, "d/a.txt", "a");
            }
            put(fx.scope.path(), "d/.env", "SECRET=3");
            let base = fx.snap(SnapshotReason::Base, None);
            put(fx.scope.path(), "d/new.txt", "added since");
            std::fs::remove_dir_all(fx.tree().join("d")).unwrap();
            if replaced_by_file {
                put(&fx.tree(), "d", "now a file");
            }
            let err = fx.plan(&base.record).unwrap_err();
            assert_eq!(
                err,
                WorkspaceError::Unlisted {
                    paths: vec!["d/.env".into(), "d/new.txt".into()]
                },
                "replaced_by_file: {replaced_by_file}"
            );
            let text = err.to_string();
            assert!(
                text.contains("d/.env") && text.contains("d/new.txt"),
                "{text}"
            );
            assert_eq!(read(fx.scope.path(), "d/a.txt"), "a");
            assert_eq!(read(fx.scope.path(), "d/.env"), "SECRET=3");
            assert_eq!(read(fx.scope.path(), "d/new.txt"), "added since");
            // The user deals with the two entries; the plan then lists what it removes.
            std::fs::remove_file(fx.scope.path().join("d/.env")).unwrap();
            std::fs::remove_file(fx.scope.path().join("d/new.txt")).unwrap();
            let plan = fx.plan(&base.record).unwrap();
            fx.promote(&plan, &base.record).unwrap();
            if replaced_by_file {
                assert_eq!(read(fx.scope.path(), "d"), "now a file");
            } else {
                assert!(!fx.scope.path().join("d").exists());
            }
        }
    }

    /// REQ-WS-007: the apply removes a directory only when it is empty, whatever the plan said:
    /// an entry that appeared after the plan stays.
    #[test]
    fn req_ws_007_an_entry_that_appeared_after_the_plan_is_not_removed() {
        let fx = Fx::new();
        for root in [fx.tree(), fx.scope.path().to_path_buf()] {
            put(&root, "d/a.txt", "a");
        }
        let base = fx.snap(SnapshotReason::Base, None);
        std::fs::remove_dir_all(fx.tree().join("d")).unwrap();
        let plan = fx.plan(&base.record).unwrap();
        put(fx.scope.path(), "d/late.txt", "written after the plan");
        let err = fx.promote(&plan, &base.record).unwrap_err();
        assert!(matches!(err, WorkspaceError::Refused { .. }), "{err:?}");
        assert_eq!(
            read(fx.scope.path(), "d/late.txt"),
            "written after the plan"
        );
    }

    /// REQ-WS-007: reversing a promotion removes only the paths it wrote; a directory it made that
    /// has since received an entry of the user's stays, and is reported.
    #[test]
    fn req_ws_007_a_reversal_removes_only_what_the_promotion_wrote() {
        let fx = Fx::new();
        let base = fx.snap(SnapshotReason::Base, None);
        put(&fx.tree(), "n/f.txt", "promoted");
        let plan = fx.plan(&base.record).unwrap();
        let (backup, promotion) = fx.promote(&plan, &base.record).unwrap();
        fx.p.seal(backup.record.snapshot_id).unwrap();
        put(fx.scope.path(), "n/user.txt", "the user's");
        let report =
            fx.p.revert_promotion(
                &promotion,
                &backup.record,
                mimlys_core::spi::EditedPaths::Keep,
            )
            .unwrap();
        assert!(!fx.scope.path().join("n/f.txt").exists());
        assert_eq!(read(fx.scope.path(), "n/user.txt"), "the user's");
        assert_eq!(report.left, ["n"]);
        assert_eq!(report.deleted, 1);
    }

    /// REQ-WS-007: an edit made in the project after the plan, here while the promotion is being
    /// applied, stops the promotion at that entry, names it, and is not overwritten.
    #[test]
    fn req_ws_007_an_edit_made_after_the_plan_stops_the_promotion_and_names_it() {
        let fx = Fx::new();
        let base = fx.snap(SnapshotReason::Base, None);
        put(&fx.tree(), "src/lib.rs", "fn a() -> i32 { 1 }\n");
        put(&fx.tree(), "src/added.rs", "added");
        let plan = fx.plan(&base.record).unwrap();
        let project = fx.scope.path().to_path_buf();
        fx.on_apply(move |path| {
            if path == "src/added.rs" {
                put(&project, "src/lib.rs", "fn a() { edited meanwhile }\n");
            }
            Ok(())
        });
        let err = fx.promote(&plan, &base.record).unwrap_err();
        assert_eq!(
            err,
            WorkspaceError::Conflict {
                paths: vec!["src/lib.rs".into()]
            }
        );
        assert_eq!(
            read(fx.scope.path(), "src/lib.rs"),
            "fn a() { edited meanwhile }\n"
        );
    }

    /// REQ-WS-007: an edit made after the plan and before the backup is refused before anything
    /// is written, naming the entry.
    #[test]
    fn req_ws_007_an_edit_made_before_the_backup_is_refused_before_any_write() {
        let fx = Fx::new();
        let base = fx.snap(SnapshotReason::Base, None);
        put(&fx.tree(), "src/lib.rs", "fn a() -> i32 { 1 }\n");
        put(&fx.tree(), "src/added.rs", "added");
        let plan = fx.plan(&base.record).unwrap();
        put(fx.scope.path(), "src/lib.rs", "fn a() { edited }\n");
        let err = fx.promote(&plan, &base.record).unwrap_err();
        assert_eq!(
            err,
            WorkspaceError::Conflict {
                paths: vec!["src/lib.rs".into()]
            }
        );
        assert!(!fx.scope.path().join("src/added.rs").exists());
    }

    /// REQ-WS-005, REQ-WS-007: a link in the project is never a parent of a path the promotion
    /// writes: the plan is refused and names the link, and nothing is written behind it.
    #[cfg(unix)]
    #[test]
    fn req_ws_007_a_path_below_a_link_in_the_project_is_refused_at_plan_time() {
        let fx = Fx::new();
        std::fs::create_dir_all(fx.scope.path().join(".git/hooks")).unwrap();
        for root in [fx.tree(), fx.scope.path().to_path_buf()] {
            std::os::unix::fs::symlink(".git/hooks", root.join("hk")).unwrap();
        }
        let base = fx.snap(SnapshotReason::Base, None);
        std::fs::remove_file(fx.tree().join("hk")).unwrap();
        put(&fx.tree(), "hk/pre-commit", "#!/bin/sh\n");
        let err = fx.plan(&base.record).unwrap_err();
        let WorkspaceError::Refused { reason } = &err else {
            panic!("{err:?}")
        };
        assert!(reason.contains("hk"), "{reason}");
        assert!(!fx.scope.path().join(".git/hooks/pre-commit").exists());
    }

    /// REQ-WS-005, REQ-WS-007: a parent that became a link after the plan is not written through
    /// either: the apply checks each parent as it writes.
    #[cfg(unix)]
    #[test]
    fn req_ws_007_a_parent_that_became_a_link_is_not_written_through() {
        let fx = Fx::new();
        std::fs::create_dir_all(fx.scope.path().join(".git/hooks")).unwrap();
        let base = fx.snap(SnapshotReason::Base, None);
        put(&fx.tree(), "newdir/file", "content");
        let plan = fx.plan(&base.record).unwrap();
        let project = fx.scope.path().to_path_buf();
        fx.on_apply(move |path| {
            if path == "newdir/file" {
                std::fs::remove_dir(project.join("newdir")).unwrap();
                std::os::unix::fs::symlink(".git/hooks", project.join("newdir")).unwrap();
            }
            Ok(())
        });
        let err = fx.promote(&plan, &base.record).unwrap_err();
        assert!(matches!(err, WorkspaceError::Refused { .. }), "{err:?}");
        assert!(!fx.scope.path().join(".git/hooks/file").exists());
    }

    /// A Windows junction at `link` to the directory `target` (no privilege is needed).
    #[cfg(windows)]
    fn make_junction(link: &Path, target: &Path) {
        // mklink refuses a path with a forward slash.
        let native = |p: &Path| p.to_string_lossy().replace('/', "\\");
        let out = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(native(link))
            .arg(native(target))
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "mklink /J failed: {} {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// REQ-WS-005, REQ-WS-007 (Windows): a junction in the project, like a link, is never a
    /// parent of a path the promotion writes: the plan is refused and names it, and nothing is
    /// written behind it.
    #[cfg(windows)]
    #[test]
    fn req_ws_007_a_path_below_a_junction_in_the_project_is_refused_at_plan_time() {
        let fx = Fx::new();
        let other = fx.scope.path().join("other");
        std::fs::create_dir_all(&other).unwrap();
        make_junction(&fx.scope.path().join("jn"), &other);
        let base = fx.snap(SnapshotReason::Base, None);
        put(&fx.tree(), "jn/inner.txt", "content");
        let err = fx.plan(&base.record).unwrap_err();
        let WorkspaceError::Refused { reason } = &err else {
            panic!("{err:?}")
        };
        assert!(reason.contains("jn"), "{reason}");
        assert!(!other.join("inner.txt").exists());
    }

    /// REQ-WS-005, REQ-WS-007 (Windows): a parent that became a junction after the plan is not
    /// written through either: the apply checks each parent as it writes.
    #[cfg(windows)]
    #[test]
    fn req_ws_007_a_parent_that_became_a_junction_is_not_written_through() {
        let fx = Fx::new();
        let other = fx.scope.path().join("other");
        std::fs::create_dir_all(&other).unwrap();
        let base = fx.snap(SnapshotReason::Base, None);
        put(&fx.tree(), "newdir/file", "content");
        let plan = fx.plan(&base.record).unwrap();
        let project = fx.scope.path().to_path_buf();
        fx.on_apply(move |path| {
            if path == "newdir/file" {
                std::fs::remove_dir(project.join("newdir")).unwrap();
                make_junction(&project.join("newdir"), &project.join("other"));
            }
            Ok(())
        });
        let err = fx.promote(&plan, &base.record).unwrap_err();
        assert!(matches!(err, WorkspaceError::Refused { .. }), "{err:?}");
        assert!(!other.join("file").exists());
    }

    /// REQ-WS-007: a directory replaced by a file, a file replaced by a directory, and (Unix) a
    /// directory replaced by a link are applied, and reversed.
    #[test]
    fn req_ws_007_type_changes_are_applied_and_reversed() {
        let fx = Fx::new();
        for root in [fx.tree(), fx.scope.path().to_path_buf()] {
            put(&root, "dir_to_file/a", "a");
            put(&root, "file_to_dir", "was a file");
            put(&root, "dir_to_link/b", "b");
            put(&root, "target.txt", "t");
        }
        let base = fx.snap(SnapshotReason::Base, None);
        std::fs::remove_dir_all(fx.tree().join("dir_to_file")).unwrap();
        put(&fx.tree(), "dir_to_file", "now a file");
        std::fs::remove_file(fx.tree().join("file_to_dir")).unwrap();
        put(&fx.tree(), "file_to_dir/x", "now a directory");
        #[cfg(unix)]
        {
            std::fs::remove_dir_all(fx.tree().join("dir_to_link")).unwrap();
            std::os::unix::fs::symlink("target.txt", fx.tree().join("dir_to_link")).unwrap();
        }
        let plan = fx.plan(&base.record).unwrap();
        let (backup, promotion) = fx.promote(&plan, &base.record).unwrap();
        fx.p.seal(backup.record.snapshot_id).unwrap();
        let project = fx.scope.path();
        assert_eq!(read(project, "dir_to_file"), "now a file");
        assert_eq!(read(project, "file_to_dir/x"), "now a directory");
        #[cfg(unix)]
        assert_eq!(
            std::fs::read_link(project.join("dir_to_link")).unwrap(),
            Path::new("target.txt")
        );
        fx.p.revert_promotion(
            &promotion,
            &backup.record,
            mimlys_core::spi::EditedPaths::Keep,
        )
        .unwrap();
        assert_eq!(read(project, "dir_to_file/a"), "a");
        assert_eq!(read(project, "file_to_dir"), "was a file");
        assert_eq!(read(project, "dir_to_link/b"), "b");
        assert!(temp_leftovers(project).is_empty());
    }

    /// The project as it is: its manifest hash, every entry hashed.
    fn project_hash(fx: &Fx) -> mimlys_core::hash::Hash {
        let dir = cap_std::fs::Dir::open_ambient_dir(fx.scope.path(), cap_std::ambient_authority())
            .unwrap();
        super::super::snapshot::walk_manifest(&dir, None)
            .unwrap()
            .hash()
    }

    /// Fails the apply of the promotion at the `n`th entry it is about to write (counting from 1).
    fn fail_at(fx: &Fx, n: usize) {
        let calls = std::sync::atomic::AtomicUsize::new(0);
        fx.on_apply(move |_| {
            if calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1 == n {
                Err(WorkspaceError::Storage {
                    detail: "injected".into(),
                })
            } else {
                Ok(())
            }
        });
    }

    /// REQ-WS-007 (external design §14.5): an error after the first write restores the project from
    /// the backup, and the backup is there.
    #[test]
    fn req_ws_007_an_error_after_the_first_write_restores_the_project() {
        let fx = Fx::new();
        let base = fx.snap(SnapshotReason::Base, None);
        put(&fx.tree(), "src/lib.rs", "fn a() -> i32 { 1 }\n");
        put(&fx.tree(), "src/added.rs", "added");
        std::fs::remove_file(fx.tree().join("src/deep/x.txt")).unwrap();
        let before = project_hash(&fx);
        let plan = fx.plan(&base.record).unwrap();
        // Every position of the failure: after 0, 1, 2 writes.
        for n in 1..=3 {
            fail_at(&fx, n);
            let (backup, promotion) = fx.promote_backup(&plan, &base.record).unwrap();
            fx.p.seal(backup.record.snapshot_id).unwrap();
            let err =
                fx.p.promote_apply(fx.task, &promotion, &base.record)
                    .unwrap_err();
            assert!(matches!(err, WorkspaceError::Storage { .. }), "{err:?}");
            if n > 1 {
                assert_ne!(project_hash(&fx), before, "n = {n}: something was written");
            }
            let report =
                fx.p.revert_promotion(
                    &promotion,
                    &backup.record,
                    mimlys_core::spi::EditedPaths::Keep,
                )
                .unwrap();
            assert!(report.left.is_empty(), "{report:?}");
            assert_eq!(
                project_hash(&fx),
                before,
                "n = {n}: the project is restored"
            );
            assert!(
                fx.p.manifest(&backup.record).is_ok(),
                "the backup is present"
            );
            assert!(temp_leftovers(fx.scope.path()).is_empty());
        }
    }

    /// REQ-WS-007: a failure in the middle of a type change restores the directory and what it
    /// held, and the file it was to become is gone.
    #[test]
    fn req_ws_007_an_error_in_the_middle_of_a_type_change_restores_the_project() {
        let fx = Fx::new();
        for root in [fx.tree(), fx.scope.path().to_path_buf()] {
            put(&root, "d/a", "a");
            put(&root, "d/b", "b");
            put(&root, "f", "was a file");
        }
        let base = fx.snap(SnapshotReason::Base, None);
        std::fs::remove_dir_all(fx.tree().join("d")).unwrap();
        put(&fx.tree(), "d", "now a file");
        std::fs::remove_file(fx.tree().join("f")).unwrap();
        put(&fx.tree(), "f/x", "now a directory");
        let before = project_hash(&fx);
        let plan = fx.plan(&base.record).unwrap();
        // d/a, d/b, d and f are removed, then d, f and f/x are written: fail at each of the seven.
        for n in 1..=7 {
            fail_at(&fx, n);
            let (backup, promotion) = fx.promote_backup(&plan, &base.record).unwrap();
            fx.p.seal(backup.record.snapshot_id).unwrap();
            let applied = fx.p.promote_apply(fx.task, &promotion, &base.record);
            assert!(applied.is_err(), "n = {n}");
            fx.p.revert_promotion(
                &promotion,
                &backup.record,
                mimlys_core::spi::EditedPaths::Keep,
            )
            .unwrap();
            assert_eq!(project_hash(&fx), before, "n = {n}");
        }
    }

    /// REQ-WS-007 (external design §16.3): a crash between `applying` and `applied` leaves the
    /// project as the apply left it and no more; reverting from the backup, as the start does,
    /// restores it. Here the apply stops after its first removals and writes, and is never
    /// reverted by its own process.
    #[test]
    fn req_ws_007_a_promotion_the_apply_never_finished_is_restored_from_its_backup() {
        let fx = Fx::new();
        let base = fx.snap(SnapshotReason::Base, None);
        put(&fx.tree(), "src/lib.rs", "fn a() -> i32 { 1 }\n");
        put(&fx.tree(), "src/added.rs", "added");
        std::fs::remove_file(fx.tree().join("src/deep/x.txt")).unwrap();
        let before = project_hash(&fx);
        let plan = fx.plan(&base.record).unwrap();
        fail_at(&fx, 3);
        let (backup, promotion) = fx.promote_backup(&plan, &base.record).unwrap();
        fx.p.seal(backup.record.snapshot_id).unwrap();
        assert!(
            fx.p.promote_apply(fx.task, &promotion, &base.record)
                .is_err()
        );
        assert_ne!(project_hash(&fx), before);
        // The next start: the same restore from the record and the backup alone.
        let report =
            fx.p.revert_promotion(
                &promotion,
                &backup.record,
                mimlys_core::spi::EditedPaths::Keep,
            )
            .unwrap();
        assert!(report.left.is_empty());
        assert_eq!(project_hash(&fx), before);
    }

    /// REQ-WS-007: the restore of a promotion leaves a path the user edited since: the apply that
    /// stopped at an edit does not overwrite it, and the rest is restored.
    #[test]
    fn req_ws_007_a_restore_leaves_a_path_edited_since() {
        let fx = Fx::new();
        let base = fx.snap(SnapshotReason::Base, None);
        put(&fx.tree(), "src/lib.rs", "fn a() -> i32 { 1 }\n");
        put(&fx.tree(), "src/added.rs", "added");
        let plan = fx.plan(&base.record).unwrap();
        let project = fx.scope.path().to_path_buf();
        fx.on_apply(move |path| {
            if path == "src/lib.rs" {
                put(&project, "src/lib.rs", "fn a() { edited meanwhile }\n");
            }
            Ok(())
        });
        let (backup, promotion) = fx.promote_backup(&plan, &base.record).unwrap();
        fx.p.seal(backup.record.snapshot_id).unwrap();
        let err =
            fx.p.promote_apply(fx.task, &promotion, &base.record)
                .unwrap_err();
        assert_eq!(
            err,
            WorkspaceError::Conflict {
                paths: vec!["src/lib.rs".into()]
            }
        );
        let report =
            fx.p.revert_promotion(
                &promotion,
                &backup.record,
                mimlys_core::spi::EditedPaths::Keep,
            )
            .unwrap();
        assert_eq!(report.left, ["src/lib.rs"]);
        assert_eq!(
            read(fx.scope.path(), "src/lib.rs"),
            "fn a() { edited meanwhile }\n",
            "the edit is kept"
        );
        assert!(!fx.scope.path().join("src/added.rs").exists());
    }

    /// REQ-WS-012 (handoff 006b): a read-only directory under the task's workspace is made writable
    /// and removed; the manifests and objects go; the journal of the deletion is closed.
    #[cfg(unix)]
    #[test]
    fn req_ws_012_read_only_directories_are_removed_with_the_workspace() {
        use std::os::unix::fs::PermissionsExt;
        let fx = Fx::new();
        let base = fx.snap(SnapshotReason::Base, None);
        let cache = fx.tree().join("pkg/mod");
        std::fs::create_dir_all(cache.join("v1")).unwrap();
        std::fs::write(cache.join("v1/a.zip"), "a").unwrap();
        for path in [cache.join("v1/a.zip"), cache.join("v1"), cache.clone()] {
            let mode = if path.is_dir() { 0o555 } else { 0o444 };
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
        }
        let manifest = fx.p.manifest(&base.record).unwrap();
        let rows = DeletedRows {
            objects_at_zero: manifest.objects().keys().cloned().collect(),
            snapshots: vec![base.record.snapshot_id],
            blobs: vec![],
        };
        fx.p.files.begin_deletion(fx.task, &rows).unwrap();
        fx.p.delete_files(fx.task, &rows).unwrap();
        assert!(!fx.p.task_dir(fx.task).exists());
        assert!(fx.p.manifest(&base.record).is_err());
        for hash in &rows.objects_at_zero {
            assert!(!fx.p.files.has_object(hash));
        }
        assert!(fx.p.files.pending_deletions().unwrap().is_empty());
    }

    /// REQ-WS-012 (handoff 006b): every removal is attempted when one fails, each failure is named
    /// by path, and the journal of the deletion stays for the next start.
    #[test]
    fn req_ws_012_every_removal_is_attempted_and_each_failure_is_named() {
        let fx = Fx::new();
        let base = fx.snap(SnapshotReason::Base, None);
        let manifest = fx.p.manifest(&base.record).unwrap();
        let rows = DeletedRows {
            objects_at_zero: manifest.objects().keys().cloned().collect(),
            snapshots: vec![base.record.snapshot_id],
            blobs: vec![],
        };
        // The manifest's path holds a non-empty directory, which no removal of a file can take.
        let manifest_path = fx
            .data()
            .join("snapshots/manifests")
            .join(format!("{}.json", base.record.snapshot_id));
        std::fs::remove_file(&manifest_path).unwrap();
        std::fs::create_dir_all(manifest_path.join("held")).unwrap();
        fx.p.files.begin_deletion(fx.task, &rows).unwrap();
        let err = fx.p.delete_files(fx.task, &rows).unwrap_err();
        let expected = format!("snapshots/manifests/{}.json", base.record.snapshot_id);
        assert_eq!(
            err,
            WorkspaceError::NotRemoved {
                task_id: fx.task,
                paths: vec![expected.clone()]
            }
        );
        // The message says first that the task is deleted, then what is left to the next start.
        assert_eq!(
            err.to_string(),
            format!(
                "task {} is deleted; 1 path(s) could not be removed and are left to the next start: {expected}",
                fx.task
            )
        );
        assert!(
            !fx.p.task_dir(fx.task).exists(),
            "the workspace went although one removal failed"
        );
        for hash in &rows.objects_at_zero {
            assert!(!fx.p.files.has_object(hash), "the objects went too");
        }
        let pending = fx.p.files.pending_deletions().unwrap();
        assert_eq!(pending.len(), 1, "the journal stays");
        assert_eq!(pending[0].1, rows);
    }

    /// REQ-WS-005, REQ-WS-007: a link whose target leaves the project is refused at plan time and
    /// named, never skipped and recorded as promoted; a link inside the project is promoted.
    #[cfg(unix)]
    #[test]
    fn req_ws_005_a_link_to_outside_the_project_refuses_the_plan() {
        let fx = Fx::new();
        let base = fx.snap(SnapshotReason::Base, None);
        put(&fx.tree(), "src/ok.rs", "ok");
        std::os::unix::fs::symlink("lib.rs", fx.tree().join("src/inside")).unwrap();
        std::os::unix::fs::symlink("../../outside", fx.tree().join("src/leaves")).unwrap();
        let err = fx.plan(&base.record).unwrap_err();
        let WorkspaceError::Refused { reason } = &err else {
            panic!("{err:?}")
        };
        assert!(reason.contains("src/leaves"), "{reason}");
        assert!(
            !fx.scope.path().join("src/ok.rs").exists(),
            "nothing is written"
        );
        // Without it the plan lists the link inside the project and applies it.
        std::fs::remove_file(fx.tree().join("src/leaves")).unwrap();
        let plan = fx.plan(&base.record).unwrap();
        assert!(plan.changes.iter().any(|c| c.path == "src/inside"));
        fx.promote(&plan, &base.record).unwrap();
        assert_eq!(
            std::fs::read_link(fx.scope.path().join("src/inside")).unwrap(),
            Path::new("lib.rs")
        );
    }

    /// REQ-WS-007: the plan of a reversal lists what it restores and removes and which paths were
    /// edited since; those are kept, or restored when the user confirmed.
    #[test]
    fn req_ws_007_the_plan_of_a_reversal_lists_its_paths_and_the_edits_since() {
        use mimlys_core::snapshot::{RevertAction, RevertEntry};
        use mimlys_core::spi::EditedPaths;
        let fx = Fx::new();
        let base = fx.snap(SnapshotReason::Base, None);
        put(&fx.tree(), "src/lib.rs", "fn a() -> i32 { 1 }\n");
        put(&fx.tree(), "src/added.rs", "added");
        std::fs::remove_file(fx.tree().join("src/deep/x.txt")).unwrap();
        let plan = fx.plan(&base.record).unwrap();
        let (backup, promotion) = fx.promote(&plan, &base.record).unwrap();
        fx.p.seal(backup.record.snapshot_id).unwrap();
        let revert =
            fx.p.promotion_revert_plan(&promotion, &backup.record)
                .unwrap();
        assert!(revert.conflicts.is_empty() && revert.edits_detected);
        let entries: Vec<(&str, RevertAction)> = revert
            .entries
            .iter()
            .map(|RevertEntry { path, action }| (path.as_str(), *action))
            .collect();
        assert_eq!(
            entries,
            [
                ("src/added.rs", RevertAction::Remove),
                ("src/deep/x.txt", RevertAction::Restore),
                ("src/lib.rs", RevertAction::Restore),
            ]
        );
        // The user edits a promoted file.
        put(fx.scope.path(), "src/lib.rs", "fn a() { edited since }\n");
        let revert =
            fx.p.promotion_revert_plan(&promotion, &backup.record)
                .unwrap();
        assert_eq!(revert.conflicts, ["src/lib.rs"]);
        assert!(revert.entries.iter().all(|e| e.path != "src/lib.rs"));
        // Kept: the edit stays, the rest is restored.
        let report =
            fx.p.revert_promotion(&promotion, &backup.record, EditedPaths::Keep)
                .unwrap();
        assert_eq!(report.left, ["src/lib.rs"]);
        assert_eq!(
            read(fx.scope.path(), "src/lib.rs"),
            "fn a() { edited since }\n"
        );
        assert!(!fx.scope.path().join("src/added.rs").exists());
        assert_eq!(read(fx.scope.path(), "src/deep/x.txt"), "xx");
        // Confirmed: the edit is overwritten too.
        let report =
            fx.p.revert_promotion(&promotion, &backup.record, EditedPaths::Overwrite)
                .unwrap();
        assert!(report.left.is_empty());
        assert_eq!(read(fx.scope.path(), "src/lib.rs"), "fn a() {}\n");
    }

    /// REQ-WS-007: a path of a promotion that holds a named pipe when the promotion is reversed is
    /// an edit made since: listed as such, kept, never opened, and the rest is restored; confirmed,
    /// it is replaced by the backup's entry.
    #[cfg(unix)]
    #[test]
    fn req_ws_007_a_named_pipe_in_the_place_of_a_promoted_file_is_an_edit_since() {
        use mimlys_core::spi::EditedPaths;
        let fx = Fx::new();
        let base = fx.snap(SnapshotReason::Base, None);
        put(&fx.tree(), "src/lib.rs", "fn a() -> i32 { 1 }\n");
        put(&fx.tree(), "src/added.rs", "added");
        let plan = fx.plan(&base.record).unwrap();
        let (backup, promotion) = fx.promote(&plan, &base.record).unwrap();
        fx.p.seal(backup.record.snapshot_id).unwrap();
        let pipe = fx.scope.path().join("src/lib.rs");
        std::fs::remove_file(&pipe).unwrap();
        mkfifo(&pipe);
        let (p, promotion_ref, backup_ref) = (&fx.p, &promotion, &backup.record);
        let revert = without_blocking("the plan of the reversal", || {
            p.promotion_revert_plan(promotion_ref, backup_ref)
        })
        .unwrap();
        assert_eq!(revert.conflicts, ["src/lib.rs"]);
        let report = without_blocking("the reversal", || {
            p.revert_promotion(promotion_ref, backup_ref, EditedPaths::Keep)
        })
        .unwrap();
        assert_eq!(report.left, ["src/lib.rs"]);
        assert!(is_special(&pipe), "the pipe is kept");
        assert!(!fx.scope.path().join("src/added.rs").exists());
        let report = without_blocking("the confirmed reversal", || {
            p.revert_promotion(promotion_ref, backup_ref, EditedPaths::Overwrite)
        })
        .unwrap();
        assert!(report.left.is_empty());
        assert_eq!(read(fx.scope.path(), "src/lib.rs"), "fn a() {}\n");
    }

    /// The names of the staging directories and temporary files left under `root`.
    fn leftovers(root: &Path) -> Vec<String> {
        let mut found = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap().flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with(".mimlys-tmp-") || name.starts_with(".mimlys-stage-") {
                    found.push(name);
                }
                if entry.file_type().unwrap().is_dir() {
                    stack.push(entry.path());
                }
            }
        }
        found
    }

    /// The path of an object of the store.
    fn object(fx: &Fx, hash: &mimlys_core::hash::Hash) -> PathBuf {
        fx.data().join("snapshots/objects").join(hash.as_str())
    }

    /// The hash of the object of the file at `path` in `manifest`.
    fn hash_at(manifest: &mimlys_core::snapshot::Manifest, path: &str) -> mimlys_core::hash::Hash {
        manifest
            .entries
            .iter()
            .find_map(|e| match e {
                Entry::File { path: p, hash, .. } if p == path => Some(hash.clone()),
                _ => None,
            })
            .unwrap()
    }

    /// Makes an object of the store unreadable (`remove`) or damaged: the store keeps them read-only.
    fn spoil(object: &Path, remove: bool) {
        let mut permissions = std::fs::metadata(object).unwrap().permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        permissions.set_readonly(false);
        std::fs::set_permissions(object, permissions).unwrap();
        if remove {
            std::fs::remove_file(object).unwrap();
        } else {
            std::fs::write(object, b"damaged").unwrap();
        }
    }

    /// REQ-WS-006: a restore verifies every object it needs before it removes anything: with an
    /// object of the snapshot missing, and then damaged, the restore fails and the tree is as it
    /// was, entry for entry (nothing removed, nothing written, no staging directory or temporary
    /// file left).
    #[test]
    fn req_ws_006_a_missing_or_damaged_object_leaves_the_tree_as_it_was() {
        for remove in [true, false] {
            let fx = Fx::new();
            let base = fx.snap(SnapshotReason::Base, None);
            let manifest = fx.p.manifest(&base.record).unwrap();
            put(&fx.tree(), "src/lib.rs", "changed\n");
            put(&fx.tree(), "extra/dir/f", "x");
            std::fs::remove_file(fx.tree().join("src/deep/x.txt")).unwrap();
            let before = walk(&fx).hash();
            // `src/lib.rs` is written back last in the old order, after the extras were removed.
            spoil(&object(&fx, &hash_at(&manifest, "src/lib.rs")), remove);
            let err = fx.p.restore(fx.task, &base.record).unwrap_err();
            assert!(
                matches!(&err, WorkspaceError::Storage { .. }),
                "remove: {remove}: {err:?}"
            );
            assert_eq!(
                walk(&fx).hash(),
                before,
                "remove: {remove}: the tree changed"
            );
            assert_eq!(read(&fx.tree(), "src/lib.rs"), "changed\n");
            assert!(fx.tree().join("extra/dir/f").exists());
            assert!(leftovers(&fx.tree()).is_empty());
        }
    }

    /// REQ-WS-006: a fork, which restores into a new tree, fails the same way and leaves nothing.
    #[test]
    fn req_ws_006_a_damaged_object_fails_a_fork_and_leaves_no_staging_directory() {
        let fx = Fx::new();
        let base = fx.snap(SnapshotReason::Base, None);
        let manifest = fx.p.manifest(&base.record).unwrap();
        spoil(&object(&fx, &hash_at(&manifest, "src/lib.rs")), false);
        let new_task = TaskId::new(&fx.ids);
        let result = fx.p.materialize_from(
            new_task,
            mimlys_core::ids::SnapshotId::new(&fx.ids),
            &base.record,
            &ScopeSpec {
                root: fx.root(),
                include: vec![],
            },
            fx.now(),
        );
        assert!(result.is_err());
        assert!(leftovers(&fx.p.tree_path(new_task)).is_empty());
    }

    /// REQ-WS-006, REQ-WS-007: a reversal verifies every object it needs before it removes or
    /// writes anything in the project: with an object of the backup damaged, and then missing,
    /// the reversal fails and the project is as the promotion left it, and nothing is left in it.
    #[test]
    fn req_ws_006_a_damaged_or_missing_object_leaves_the_project_as_it_was_in_a_reversal() {
        use mimlys_core::spi::EditedPaths;
        for remove in [false, true] {
            let fx = Fx::new();
            let base = fx.snap(SnapshotReason::Base, None);
            put(&fx.tree(), "src/lib.rs", "fn a() -> i32 { 1 }\n");
            put(&fx.tree(), "src/added.rs", "added");
            let plan = fx.plan(&base.record).unwrap();
            let (backup, promotion) = fx.promote(&plan, &base.record).unwrap();
            fx.p.seal(backup.record.snapshot_id).unwrap();
            let manifest = fx.p.manifest(&backup.record).unwrap();
            let project = fx.scope.path();
            spoil(&object(&fx, &hash_at(&manifest, "src/lib.rs")), remove);
            let err =
                fx.p.revert_promotion(&promotion, &backup.record, EditedPaths::Keep)
                    .unwrap_err();
            assert!(
                matches!(&err, WorkspaceError::Storage { .. }),
                "remove: {remove}: {err:?}"
            );
            // `src/added.rs` is removed before `src/lib.rs` is written back in the old order.
            assert_eq!(read(project, "src/added.rs"), "added", "remove: {remove}");
            assert_eq!(read(project, "src/lib.rs"), "fn a() -> i32 { 1 }\n");
            assert!(leftovers(project).is_empty(), "remove: {remove}");
        }
    }

    /// REQ-WS-006: a staging directory a crash left in the root is an extra entry, and the next
    /// restore of the tree removes it with the rest; the next reversal of a promotion removes the
    /// one in the project's root, and only a directory of exactly that form.
    #[test]
    fn req_ws_006_a_staging_directory_left_behind_is_removed_by_the_next_restore() {
        use mimlys_core::spi::EditedPaths;
        let fx = Fx::new();
        let base = fx.snap(SnapshotReason::Base, None);
        put(&fx.tree(), ".mimlys-stage-1f-2/0", "left by a crash");
        let report = fx.p.restore(fx.task, &base.record).unwrap();
        assert!(report.deleted >= 2, "{report:?}");
        assert!(leftovers(&fx.tree()).is_empty());
        assert_eq!(walk(&fx).hash(), base.record.manifest_hash);
        // The project's root, at a reversal.
        put(&fx.tree(), "src/lib.rs", "fn a() -> i32 { 1 }\n");
        let plan = fx.plan(&base.record).unwrap();
        let (backup, promotion) = fx.promote(&plan, &base.record).unwrap();
        fx.p.seal(backup.record.snapshot_id).unwrap();
        let project = fx.scope.path();
        put(project, ".mimlys-stage-1f-2/0", "left by a crash");
        put(project, ".mimlys-stage-notes/0", "the user's own");
        fx.p.revert_promotion(&promotion, &backup.record, EditedPaths::Keep)
            .unwrap();
        assert!(!project.join(".mimlys-stage-1f-2").exists());
        assert_eq!(read(project, ".mimlys-stage-notes/0"), "the user's own");
        assert_eq!(read(project, "src/lib.rs"), "fn a() {}\n");
    }

    /// REQ-WS-006, REQ-WS-007: the restore of a promotion after a crash also removes the
    /// temporary files the promotion made beside the paths it wrote (`.mimlys-tmp-<pid>-<n>`),
    /// so that a directory the promotion made is not kept for a file nobody will place; a file of
    /// the user's of another name in the same directory is left, and so is the directory then.
    #[test]
    fn req_ws_007_the_restore_after_a_crash_removes_the_temporary_files_of_the_promotion() {
        use mimlys_core::spi::EditedPaths;
        for user_file in [false, true] {
            let fx = Fx::new();
            let base = fx.snap(SnapshotReason::Base, None);
            put(&fx.tree(), "made/by/promotion.txt", "new");
            let plan = fx.plan(&base.record).unwrap();
            let (backup, mut promotion) = fx.promote_backup(&plan, &base.record).unwrap();
            fx.p.seal(backup.record.snapshot_id).unwrap();
            // The crash: `made/by` exists, a temporary file was written in it, and nothing else.
            let project = fx.scope.path();
            put(
                project,
                "made/by/.mimlys-tmp-1a2b-3",
                "half of the new file",
            );
            put(project, ".mimlys-tmp-1a2b-4", "in the root");
            if user_file {
                put(project, "made/by/notes.txt", "the user's");
            }
            promotion.state = mimlys_core::snapshot::PromotionState::Applying;
            let report =
                fx.p.revert_promotion(&promotion, &backup.record, EditedPaths::Keep)
                    .unwrap();
            assert!(leftovers(project).is_empty(), "user_file: {user_file}");
            if user_file {
                assert_eq!(read(project, "made/by/notes.txt"), "the user's");
                assert!(!report.left.is_empty(), "{report:?}");
            } else {
                assert!(!project.join("made").exists(), "{report:?}");
            }
        }
    }

    /// REQ-WS-004, REQ-WS-008: the walk of a tree deeper than any recursion would bear. The chain
    /// is made with handles, one directory in the next, `DEPTH` levels with a file at the bottom;
    /// the quota check (`remaining_bytes`) reads it in the stack of the test thread (2 MiB), and
    /// so does the snapshot walk of a chain below the system's limit on a path. A recursive walk
    /// overflowed the stack of this thread at the depth the request names.
    #[cfg(unix)]
    #[test]
    fn req_ws_004_a_tree_deeper_than_any_recursion_would_bear_is_walked() {
        // Linux: far past what a recursive walk bore (it overflowed this thread's stack between 700
        // and 1000 levels). Elsewhere a smaller chain: a system that finds the path of a deep
        // directory by walking up from it makes every operation on one cost its depth.
        const DEPTH: usize = if cfg!(target_os = "linux") {
            30_000
        } else {
            300
        };
        const CHAIN: usize = if cfg!(target_os = "linux") {
            1_500
        } else {
            100
        };
        let fx = Fx::new();
        let deep = fx.tree().join("deep");
        std::fs::create_dir(&deep).unwrap();
        let mut dir =
            cap_std::fs::Dir::open_ambient_dir(&deep, cap_std::ambient_authority()).unwrap();
        for _ in 0..DEPTH {
            dir.create_dir("d").unwrap();
            dir = dir.open_dir("d").unwrap();
        }
        dir.write("bottom", b"12345").unwrap();
        let used = fx.p.remaining_bytes(fx.task).unwrap().unwrap();
        assert_eq!(used, 10_000 - 12 - 5, "the file at the bottom is counted");
        assert_eq!(fx.p.tree_bytes(fx.task).unwrap(), 12 + 5);
        // Taken off by the system's own tool: the tree is deeper than a path may be.
        let status = std::process::Command::new("rm")
            .arg("-rf")
            .arg(&deep)
            .status()
            .unwrap();
        assert!(status.success());
        // The walk of the snapshot (hashes, manifest) on a chain shorter than a path may be.
        let mut dir =
            cap_std::fs::Dir::open_ambient_dir(fx.tree(), cap_std::ambient_authority()).unwrap();
        for _ in 0..CHAIN {
            dir.create_dir("d").unwrap();
            dir = dir.open_dir("d").unwrap();
        }
        let manifest = walk(&fx);
        let dirs = manifest
            .entries
            .iter()
            .filter(|e| matches!(e, Entry::Dir { .. }))
            .count();
        assert_eq!(dirs, CHAIN + 2, "the chain, `src` and `src/deep`");
        let status = std::process::Command::new("rm")
            .arg("-rf")
            .arg(fx.tree().join("d"))
            .status()
            .unwrap();
        assert!(status.success());
    }

    /// REQ-WS-012, REQ-WS-004 (handoff 006c-6): the deletion of a task removes a tree deeper than
    /// the stack bears and than a path may be, whole, and `NotRemoved` does not occur for it.
    /// The chain is `DEPTH` levels of one-character names; on the way there are a read-only
    /// directory (`ro`, 0555) and a directory nothing can enter (`locked`, 0000), each with a file;
    /// a name that is not valid UTF-8; a link to a file outside the tree, which is not followed
    /// (the file stays); and a named pipe. The previous `remove_tree` was recursive and worked by
    /// path: it could not name the chain past 2 048 levels (a path of 4 096 bytes), and listed
    /// every level above the first it could not reach as left.
    #[cfg(unix)]
    #[test]
    fn req_ws_012_a_tree_deeper_than_a_path_may_be_is_deleted_whole() {
        use std::os::unix::ffi::OsStrExt;
        // Linux: five times what a path of 4 096 bytes holds here (2 048 levels), and a system's
        // rmdir of a deep directory costs about 0.3 ms, so no more. Elsewhere a modest chain, as
        // for the walk (see above).
        const DEPTH: usize = if cfg!(target_os = "linux") {
            10_000
        } else {
            300
        };
        let fx = Fx::new();
        let base = fx.snap(SnapshotReason::Base, None);
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("keep"), "kept").unwrap();
        let deep = fx.tree().join("deep");
        std::fs::create_dir(&deep).unwrap();
        mkfifo(&deep.join("pipe"));
        let mut dir =
            cap_std::fs::Dir::open_ambient_dir(&deep, cap_std::ambient_authority()).unwrap();
        for level in 0..DEPTH {
            if level == DEPTH / 3 {
                dir.create_dir("ro").unwrap();
                dir.write("ro/f", b"f").unwrap();
                let ro = cap_std::fs::Permissions::from_std(
                    std::os::unix::fs::PermissionsExt::from_mode(0o555),
                );
                dir.set_permissions("ro", ro).unwrap();
                dir.create_dir("locked").unwrap();
                dir.write("locked/f", b"f").unwrap();
                let locked = cap_std::fs::Permissions::from_std(
                    std::os::unix::fs::PermissionsExt::from_mode(0o000),
                );
                dir.set_permissions("locked", locked).unwrap();
            }
            if level == DEPTH / 2 {
                dir.write(std::ffi::OsStr::from_bytes(b"caf\xE9.txt"), b"x")
                    .unwrap();
                // Absolute targets: the capability handle refuses to make these, rustix does.
                rustix::fs::symlinkat(outside.path().join("keep"), &dir, "link").unwrap();
                rustix::fs::symlinkat(outside.path(), &dir, "dirlink").unwrap();
            }
            dir.create_dir("d").unwrap();
            dir = dir.open_dir("d").unwrap();
        }
        dir.write("bottom", b"12345").unwrap();
        let manifest = fx.p.manifest(&base.record).unwrap();
        let rows = DeletedRows {
            objects_at_zero: manifest.objects().keys().cloned().collect(),
            snapshots: vec![base.record.snapshot_id],
            blobs: vec![],
        };
        fx.p.files.begin_deletion(fx.task, &rows).unwrap();
        let result = fx.p.delete_files(fx.task, &rows);
        let shown = match &result {
            Ok(()) => String::new(),
            Err(e) => e.to_string().chars().take(300).collect(),
        };
        assert!(result.is_ok(), "the deep tree is deleted whole: {shown}");
        assert!(!fx.p.task_dir(fx.task).exists());
        assert!(
            fx.p.files.pending_deletions().unwrap().is_empty(),
            "the journal of the deletion is closed"
        );
        assert_eq!(
            std::fs::read_to_string(outside.path().join("keep")).unwrap(),
            "kept",
            "a link is removed, never followed"
        );
        assert!(outside.path().exists());
    }

    /// REQ-WS-006, REQ-WS-005: a named pipe or a socket an action made in the tree (at a path the
    /// snapshot does not hold, in a directory it does not hold, or in the place of a file) is
    /// removed by a rollback, the restore does not block on it, and the file it replaced comes
    /// back. The snapshot of a tree that holds one holds neither.
    #[cfg(unix)]
    #[test]
    fn req_ws_006_a_named_pipe_or_socket_made_by_an_action_is_removed_by_a_rollback() {
        let fx = Fx::new();
        let base = fx.snap(SnapshotReason::Base, None);
        std::fs::remove_file(fx.tree().join("src/lib.rs")).unwrap();
        mkfifo(&fx.tree().join("src/lib.rs"));
        mkfifo(&fx.tree().join("pipe"));
        make_socket(&fx.tree().join("src/socket"));
        std::fs::create_dir_all(fx.tree().join("made/inside")).unwrap();
        mkfifo(&fx.tree().join("made/inside/pipe"));
        let (p, task) = (&fx.p, fx.task);
        // A snapshot of the tree is taken without opening any of them, and holds none.
        let after = without_blocking("the snapshot", || {
            p.snapshot(
                task,
                mimlys_core::ids::SnapshotId::new(&fx.ids),
                SnapshotReason::After,
                None,
                None,
                fx.now(),
            )
        })
        .unwrap();
        let manifest = fx.p.manifest(&after.record).unwrap();
        assert!(
            manifest
                .entries
                .iter()
                .all(|e| !e.path().ends_with("pipe") && e.path() != "src/socket")
        );
        let record = base.record.clone();
        let report = without_blocking("the rollback", || p.restore(task, &record)).unwrap();
        assert!(report.deleted >= 4, "{report:?}");
        assert_eq!(walk(&fx).hash(), base.record.manifest_hash);
        assert_eq!(read(&fx.tree(), "src/lib.rs"), "fn a() {}\n");
        for gone in ["pipe", "src/socket", "made"] {
            assert!(
                std::fs::symlink_metadata(fx.tree().join(gone)).is_err(),
                "{gone}"
            );
        }
        assert!(leftovers(&fx.tree()).is_empty());
    }

    /// REQ-WS-002, REQ-WS-004: a name that is not valid UTF-8 refuses the creation of the
    /// workspace, naming the path with the name written out byte by byte and saying why;
    /// nothing is copied. The same walk refuses it in a task's tree, so a snapshot of one is a
    /// stated failure and not a lossy name that is later "not found". Linux; other systems
    /// restrict the names a file may have.
    #[cfg(target_os = "linux")]
    #[test]
    fn req_ws_002_a_name_that_is_not_utf8_refuses_the_creation_and_names_the_path() {
        use std::os::unix::ffi::OsStrExt;
        let scope = scope();
        let name = std::ffi::OsStr::from_bytes(b"caf\xe9.txt");
        std::fs::write(scope.path().join("src").join(name), "x").unwrap();
        let data = tempfile::tempdir().unwrap();
        let task = TaskId::new(&ScriptedIds::default());
        let p = provider(data.path());
        let err = p
            .materialize(
                task,
                &ScopeSpec {
                    root: scope.path().to_string_lossy().into_owned(),
                    include: vec![],
                },
            )
            .unwrap_err();
        let WorkspaceError::ReadFailed { path, kind } = &err else {
            panic!("{err:?}");
        };
        assert_eq!(path, "src/caf\\xE9.txt");
        assert!(kind.contains("not valid UTF-8"), "{kind}");
        assert!(
            std::fs::read_dir(p.tree_path(task))
                .unwrap()
                .next()
                .is_none(),
            "nothing is copied"
        );
        // In a task's tree: the snapshot refuses it the same way.
        let fx = Fx::new();
        std::fs::write(fx.tree().join(name), "x").unwrap();
        let dir =
            cap_std::fs::Dir::open_ambient_dir(fx.tree(), cap_std::ambient_authority()).unwrap();
        assert_eq!(
            super::super::snapshot::walk_manifest(&dir, None).unwrap_err(),
            WorkspaceError::ReadFailed {
                path: "caf\\xE9.txt".into(),
                kind: "the name is not valid UTF-8".into()
            }
        );
    }

    /// REQ-WS-005, REQ-WS-006: a link's target is kept byte for byte: a backslash in it, a
    /// character that is a name on Unix, is not rewritten to a slash, in the copy, the manifest
    /// and the restore. (Windows reads a backslash as its separator and writes the manifest's
    /// slash; the same test is Unix only.)
    #[cfg(unix)]
    #[test]
    fn req_ws_005_a_link_target_with_a_backslash_survives() {
        let scope = scope();
        std::os::unix::fs::symlink("odd\\name", scope.path().join("src/l")).unwrap();
        let data = tempfile::tempdir().unwrap();
        let ids = ScriptedIds::default();
        let task = TaskId::new(&ids);
        let p = provider(data.path());
        let info = p
            .materialize(
                task,
                &ScopeSpec {
                    root: scope.path().to_string_lossy().into_owned(),
                    include: vec![],
                },
            )
            .unwrap();
        assert_eq!(info.symlinks_omitted, 0);
        let link = p.tree_path(task).join("src/l");
        assert_eq!(std::fs::read_link(&link).unwrap(), Path::new("odd\\name"));
        let prepared = p
            .snapshot(
                task,
                mimlys_core::ids::SnapshotId::new(&ids),
                SnapshotReason::Base,
                None,
                None,
                Instant::from_unix_millis(1_700_000_000_000).unwrap(),
            )
            .unwrap();
        p.seal(prepared.record.snapshot_id).unwrap();
        let manifest = p.manifest(&prepared.record).unwrap();
        assert!(
            manifest.entries.iter().any(|e| matches!(e, Entry::Symlink { path, target } if path == "src/l" && target == "odd\\name")),
            "{manifest:?}"
        );
        std::fs::remove_file(&link).unwrap();
        p.restore(task, &prepared.record).unwrap();
        assert_eq!(std::fs::read_link(&link).unwrap(), Path::new("odd\\name"));
    }

    /// REQ-WS-002, REQ-WS-004: a part of the scope that cannot be read refuses the creation with
    /// its path (and nothing is copied), where it was the internal `walk` failure; with the
    /// permissions back, the creation works. Unix; the superuser reads anything and the test then
    /// has nothing to show.
    #[cfg(unix)]
    #[test]
    fn req_ws_002_an_unreadable_directory_of_the_scope_refuses_the_creation_with_its_path() {
        use std::os::unix::fs::PermissionsExt;
        let scope = scope();
        let locked = scope.path().join("src/deep");
        let _restore = Restore(locked.clone());
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        if std::fs::read_dir(&locked).is_ok() {
            eprintln!("skipped: the permissions do not apply to this user");
            return;
        }
        let data = tempfile::tempdir().unwrap();
        let task = TaskId::new(&ScriptedIds::default());
        let p = provider(data.path());
        let spec = ScopeSpec {
            root: scope.path().to_string_lossy().into_owned(),
            include: vec![],
        };
        let err = p.materialize(task, &spec).unwrap_err();
        assert_eq!(
            err,
            WorkspaceError::Unreadable {
                path: "src/deep".into()
            }
        );
        assert!(
            std::fs::read_dir(p.tree_path(task))
                .unwrap()
                .next()
                .is_none()
        );
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(p.materialize(task, &spec).is_ok());
    }

    /// REQ-WS-006: which kind of link a target is, from the manifest's own entries: a directory
    /// (also through a chain of links) is one, a file is not, and a target that is absolute or
    /// leaves the root is neither. It decides, on Windows, between a directory link and a file link;
    /// the other systems do not ask, so this is checked here.
    #[test]
    fn req_ws_006_the_kind_of_a_link_is_found_in_the_manifest() {
        use super::super::{points_at_directory, resolve_target};
        let entries = [
            Entry::Dir { path: "d".into() },
            Entry::Dir {
                path: "d/sub".into(),
            },
            Entry::File {
                path: "d/f".into(),
                executable: false,
                mode: None,
                size: 0,
                hash: mimlys_core::hash::hash_bytes(b""),
            },
            Entry::Symlink {
                path: "l".into(),
                target: "d".into(),
            },
            Entry::Symlink {
                path: "d/up".into(),
                target: "../l".into(),
            },
            Entry::Symlink {
                path: "loop".into(),
                target: "loop".into(),
            },
        ];
        let manifest = mimlys_core::snapshot::Manifest::new(entries.to_vec());
        let known = manifest.by_path();
        let dst = tempfile::tempdir().unwrap();
        let dst =
            cap_std::fs::Dir::open_ambient_dir(dst.path(), cap_std::ambient_authority()).unwrap();
        let kind = |link: &str, target: &str| points_at_directory(&dst, link, target, Some(&known));
        assert!(kind("x", "d"));
        assert!(kind("x", "./d/sub"));
        assert!(kind("d/x", ".."), "the root is a directory");
        assert!(kind("x", "l"), "a link to a link to a directory");
        assert!(kind("x", "d/up"), "through a link that goes up");
        assert!(!kind("x", "d/f"));
        assert!(!kind("x", "/d"));
        assert!(!kind("x", "../d"), "leaves the root");
        assert!(!kind("x", "loop"), "a loop ends");
        assert!(!kind("x", "missing"));
        assert_eq!(resolve_target("a/b/l", "../c").as_deref(), Some("a/c"));
        assert_eq!(resolve_target("l", "..\\x"), None);
    }

    /// REQ-WS-006: a directory link is restored as a directory link (Windows makes the two kinds of
    /// link differently). The tree is made with a directory and a link to it, snapshotted, emptied,
    /// and restored. Needs the right to make links, which the Windows job has; where the system
    /// refuses it, the test says so and has nothing to show.
    #[cfg(windows)]
    #[test]
    fn req_ws_006_a_directory_link_is_restored_as_a_directory_link() {
        use std::os::windows::fs::FileTypeExt;
        let fx = Fx::new();
        std::fs::create_dir_all(fx.tree().join("target_dir/inner")).unwrap();
        std::fs::write(fx.tree().join("target_dir/inner/f.txt"), "x").unwrap();
        std::fs::write(fx.tree().join("a_file.txt"), "y").unwrap();
        let made = std::os::windows::fs::symlink_dir("target_dir", fx.tree().join("dir_link"))
            .and_then(|()| {
                std::os::windows::fs::symlink_file("a_file.txt", fx.tree().join("file_link"))
            });
        if let Err(e) = made {
            // The CI job runs with the right to make links: a refusal there is a failure.
            assert!(
                std::env::var_os("CI").is_none(),
                "the system refuses to make links: {e}"
            );
            eprintln!("skipped: the system refuses to make links here: {e}");
            return;
        }
        let base = fx.snap(SnapshotReason::Base, None);
        std::fs::remove_dir(fx.tree().join("dir_link")).unwrap();
        std::fs::remove_file(fx.tree().join("file_link")).unwrap();
        fx.p.restore(fx.task, &base.record).unwrap();
        let dir_link = std::fs::symlink_metadata(fx.tree().join("dir_link")).unwrap();
        assert!(dir_link.file_type().is_symlink_dir(), "{dir_link:?}");
        assert!(fx.tree().join("dir_link/inner/f.txt").exists());
        let file_link = std::fs::symlink_metadata(fx.tree().join("file_link")).unwrap();
        assert!(file_link.file_type().is_symlink_file(), "{file_link:?}");
        // Forgotten and restored again from the manifest alone (the destination holds nothing).
        std::fs::remove_dir_all(fx.tree().join("target_dir")).unwrap();
        std::fs::remove_dir(fx.tree().join("dir_link")).unwrap();
        fx.p.restore(fx.task, &base.record).unwrap();
        let dir_link = std::fs::symlink_metadata(fx.tree().join("dir_link")).unwrap();
        assert!(dir_link.file_type().is_symlink_dir(), "{dir_link:?}");
    }

    #[cfg(unix)]
    fn chmod(path: &Path, mode: u32) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    }

    #[cfg(unix)]
    fn mode_of(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    /// REQ-WS-006: the manifest records a file's mode (0o777 mask) and a restore gives it back
    /// exactly: a 0600 and a 0755 file whose modes were changed come back with their modes, though
    /// their content is the same. A directory-level umask does not change what is restored.
    #[cfg(unix)]
    #[test]
    fn req_ws_006_a_restore_gives_back_the_recorded_mode() {
        let fx = Fx::new();
        put(&fx.tree(), "private/key.txt", "k");
        put(&fx.tree(), "bin/run.sh", "#!/bin/sh\n");
        put(&fx.tree(), "group.txt", "g");
        chmod(&fx.tree().join("private/key.txt"), 0o600);
        chmod(&fx.tree().join("bin/run.sh"), 0o755);
        chmod(&fx.tree().join("group.txt"), 0o664);
        let base = fx.snap(SnapshotReason::Base, None);
        let manifest = fx.p.manifest(&base.record).unwrap();
        let recorded = |path: &str| {
            manifest.entries.iter().find_map(|e| match e {
                Entry::File { path: p, mode, .. } if p == path => Some(*mode),
                _ => None,
            })
        };
        assert_eq!(recorded("private/key.txt"), Some(Some(0o600)));
        assert_eq!(recorded("bin/run.sh"), Some(Some(0o755)));
        assert_eq!(recorded("group.txt"), Some(Some(0o664)));
        for (path, other) in [
            ("private/key.txt", 0o644),
            ("bin/run.sh", 0o644),
            ("group.txt", 0o600),
        ] {
            chmod(&fx.tree().join(path), other);
        }
        // Content is unchanged, the modes are not: the snapshot is not the base.
        assert_ne!(walk(&fx).hash(), base.record.manifest_hash);
        let report = fx.p.restore(fx.task, &base.record).unwrap();
        assert_eq!(report.written, 3, "{report:?}");
        assert_eq!(mode_of(&fx.tree().join("private/key.txt")), 0o600);
        assert_eq!(mode_of(&fx.tree().join("bin/run.sh")), 0o755);
        assert_eq!(mode_of(&fx.tree().join("group.txt")), 0o664);
        assert_eq!(walk(&fx).hash(), base.record.manifest_hash);
    }

    /// REQ-WS-006: a mode that cannot be set is an error, never ignored; an entry from a manifest
    /// made before 0.11, which records none, gets 0o755 when it is executable and 0o644 when it is
    /// not, as it always did.
    #[cfg(unix)]
    #[test]
    fn req_ws_006_a_failed_chmod_is_an_error_and_an_absent_mode_gives_the_old_default() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a"), "x").unwrap();
        std::fs::write(dir.path().join("b"), "x").unwrap();
        let handle =
            cap_std::fs::Dir::open_ambient_dir(dir.path(), cap_std::ambient_authority()).unwrap();
        let set = super::super::snapshot::set_file_mode;
        assert!(matches!(
            set(&handle, "missing", false, Some(0o600)),
            Err(WorkspaceError::Storage { detail }) if detail == "set_permissions"
        ));
        set(&handle, "a", true, None).unwrap();
        set(&handle, "b", false, None).unwrap();
        assert_eq!(mode_of(&dir.path().join("a")), 0o755);
        assert_eq!(mode_of(&dir.path().join("b")), 0o644);
        set(&handle, "b", false, Some(0o4755)).unwrap();
        assert_eq!(mode_of(&dir.path().join("b")), 0o755, "the mask is 0o777");
    }

    /// REQ-WS-007: a promotion keeps the project file's own mode: a 0600 file of the project,
    /// rewritten in the workspace by a tool that recreated it with the default mode, stays 0600
    /// in the project through the promotion and through its reversal (the content comes back and
    /// so does the mode). The promotion forced 0644 before.
    #[cfg(unix)]
    #[test]
    fn req_ws_007_a_0600_file_stays_0600_through_a_promotion_and_a_reversal() {
        use mimlys_core::spi::EditedPaths;
        let fx = Fx::new();
        for root in [fx.tree(), fx.scope.path().to_path_buf()] {
            put(&root, "private/notes.txt", "secret v1\n");
            chmod(&root.join("private/notes.txt"), 0o600);
        }
        let base = fx.snap(SnapshotReason::Base, None);
        // The tool recreated the file: new content, the default mode.
        std::fs::remove_file(fx.tree().join("private/notes.txt")).unwrap();
        put(&fx.tree(), "private/notes.txt", "secret v2\n");
        chmod(&fx.tree().join("private/notes.txt"), 0o644);
        let plan = fx.plan(&base.record).unwrap();
        let (backup, promotion) = fx.promote(&plan, &base.record).unwrap();
        fx.p.seal(backup.record.snapshot_id).unwrap();
        let project_file = fx.scope.path().join("private/notes.txt");
        assert_eq!(read(fx.scope.path(), "private/notes.txt"), "secret v2\n");
        assert_eq!(
            mode_of(&project_file),
            0o600,
            "the project's own mode is kept"
        );
        // Somebody loosened it since the promotion: that is an edit since, kept, not undone.
        chmod(&project_file, 0o644);
        let report =
            fx.p.revert_promotion(&promotion, &backup.record, EditedPaths::Keep)
                .unwrap();
        assert_eq!(report.left, ["private/notes.txt"], "{report:?}");
        assert_eq!(mode_of(&project_file), 0o644);
        // Back as the promotion wrote it, the reversal restores content and mode.
        chmod(&project_file, 0o600);
        let report =
            fx.p.revert_promotion(&promotion, &backup.record, EditedPaths::Keep)
                .unwrap();
        assert!(report.left.is_empty(), "{report:?}");
        assert_eq!(read(fx.scope.path(), "private/notes.txt"), "secret v1\n");
        assert_eq!(mode_of(&project_file), 0o600);
    }

    /// REQ-WS-007: a new file takes the workspace file's mode without the group's and others'
    /// write bits: 0666 is promoted as 0644 and 0775 as 0755, 0600 stays 0600; the reversal removes
    /// them.
    #[cfg(unix)]
    #[test]
    fn req_ws_007_a_new_file_of_0666_is_promoted_as_0644() {
        let fx = Fx::new();
        let base = fx.snap(SnapshotReason::Base, None);
        for (name, mode) in [("w.txt", 0o666), ("x.sh", 0o775), ("p.txt", 0o600)] {
            put(&fx.tree(), name, "new");
            chmod(&fx.tree().join(name), mode);
        }
        let plan = fx.plan(&base.record).unwrap();
        let (_backup, _promotion) = fx.promote(&plan, &base.record).unwrap();
        assert_eq!(mode_of(&fx.scope.path().join("w.txt")), 0o644);
        assert_eq!(mode_of(&fx.scope.path().join("x.sh")), 0o755);
        assert_eq!(mode_of(&fx.scope.path().join("p.txt")), 0o600);
    }

    /// REQ-WS-007 (external design §14.5; handoff 006c-6): the plan shows exactly what the promotion
    /// does. A change of mode alone is in it only when the promotion carries it over, and is shown
    /// with the mode it will give: a script made executable in the workspace becomes executable in
    /// the project (0640 becomes 0750, the project's own bits otherwise), and the reversal restores
    /// the mode it had. A change of mode that promotes nothing is not listed: a mode that is not of
    /// the executable bit (the project keeps its own), and an executable bit the project already
    /// has (set, or cleared). Nor does such a path hold up the plan, whatever the project holds there.
    #[cfg(unix)]
    #[test]
    fn req_ws_007_a_mode_only_change_is_in_the_plan_only_when_it_is_carried() {
        use mimlys_core::spi::EditedPaths;
        let fx = Fx::new();
        for root in [fx.tree(), fx.scope.path().to_path_buf()] {
            put(&root, "src/set.sh", "echo set\n");
            put(&root, "src/has.sh", "echo has\n");
            put(&root, "src/clear.sh", "echo clear\n");
            chmod(&root.join("src/lib.rs"), 0o640);
            chmod(&root.join("src/deep/x.txt"), 0o644);
            chmod(&root.join("src/set.sh"), 0o644);
            chmod(&root.join("src/has.sh"), 0o644);
            chmod(&root.join("src/clear.sh"), 0o755);
        }
        // The project already has the bit the task is going to set, and lacks the one it clears.
        chmod(&fx.scope.path().join("src/has.sh"), 0o755);
        chmod(&fx.scope.path().join("src/clear.sh"), 0o644);
        let base = fx.snap(SnapshotReason::Base, None);
        chmod(&fx.tree().join("src/lib.rs"), 0o755);
        chmod(&fx.tree().join("src/deep/x.txt"), 0o600);
        chmod(&fx.tree().join("src/set.sh"), 0o755);
        chmod(&fx.tree().join("src/has.sh"), 0o755);
        chmod(&fx.tree().join("src/clear.sh"), 0o644);
        // The project has an edit the task knows nothing of, at a path that promotes nothing: it
        // does not conflict, for nothing of it is written.
        put(fx.scope.path(), "src/has.sh", "echo edited\n");
        chmod(&fx.scope.path().join("src/has.sh"), 0o755);
        let plan = fx.plan(&base.record).unwrap();
        let changed: Vec<(&str, ChangeKind)> = plan
            .changes
            .iter()
            .map(|c| (c.path.as_str(), c.kind))
            .collect();
        assert_eq!(
            changed,
            [
                ("src/lib.rs", ChangeKind::Modified),
                ("src/set.sh", ChangeKind::Modified)
            ],
            "only what the promotion changes: x.txt (not an executable bit), has.sh (already set) and \
             clear.sh (already clear) are not listed"
        );
        for absent in ["src/deep/x.txt", "src/has.sh", "src/clear.sh"] {
            assert!(
                plan.text_diffs.iter().all(|(p, _)| p != absent),
                "{absent} has nothing to show: {:?}",
                plan.text_diffs
            );
        }
        let diff_of = |path: &str| {
            plan.text_diffs
                .iter()
                .find(|(p, _)| p == path)
                .map(|(_, d)| d.clone())
        };
        let shown = diff_of("src/lib.rs").unwrap();
        assert!(
            shown.contains("old mode 100640") && shown.contains("new mode 100750"),
            "{shown}"
        );
        let shown = diff_of("src/set.sh").unwrap();
        assert!(
            shown.contains("old mode 100644") && shown.contains("new mode 100755"),
            "{shown}"
        );
        let (backup, promotion) = fx.promote(&plan, &base.record).unwrap();
        fx.p.seal(backup.record.snapshot_id).unwrap();
        assert_eq!(mode_of(&fx.scope.path().join("src/lib.rs")), 0o750);
        assert_eq!(mode_of(&fx.scope.path().join("src/set.sh")), 0o755);
        assert_eq!(mode_of(&fx.scope.path().join("src/deep/x.txt")), 0o644);
        assert_eq!(mode_of(&fx.scope.path().join("src/has.sh")), 0o755);
        assert_eq!(mode_of(&fx.scope.path().join("src/clear.sh")), 0o644);
        assert_eq!(read(fx.scope.path(), "src/lib.rs"), "fn a() {}\n");
        assert_eq!(
            read(fx.scope.path(), "src/has.sh"),
            "echo edited\n",
            "a path that promotes nothing is not written"
        );
        fx.p.revert_promotion(&promotion, &backup.record, EditedPaths::Keep)
            .unwrap();
        assert_eq!(mode_of(&fx.scope.path().join("src/lib.rs")), 0o640);
        assert_eq!(mode_of(&fx.scope.path().join("src/set.sh")), 0o644);
        assert_eq!(mode_of(&fx.scope.path().join("src/deep/x.txt")), 0o644);
    }

    /// REQ-WS-007: the mode a promoted file is given, by cases (the project's own, the executable
    /// bit carried over where the project allows reading, the workspace's mode without write bits
    /// for others for a new file).
    #[test]
    fn req_ws_007_the_mode_a_promoted_file_is_given() {
        let file = |mode: Option<u32>, executable: bool| Entry::File {
            path: "f".into(),
            executable,
            mode,
            size: 0,
            hash: mimlys_core::hash::hash_bytes(b""),
        };
        let given = super::super::snapshot::promoted_mode;
        // No file in the project: the workspace's, without group and others' write.
        assert_eq!(given(&file(Some(0o666), false), None, None), Some(0o644));
        assert_eq!(given(&file(Some(0o775), true), None, None), Some(0o755));
        assert_eq!(given(&file(Some(0o600), false), None, None), Some(0o600));
        // A file in the project: its own, whatever the workspace holds.
        let own = file(Some(0o600), false);
        let base = file(Some(0o600), false);
        assert_eq!(
            given(&file(Some(0o666), false), Some(&base), Some(&own)),
            Some(0o600)
        );
        // The executable bit the task changed is carried over.
        let base = file(Some(0o640), false);
        let own = file(Some(0o640), false);
        assert_eq!(
            given(&file(Some(0o755), true), Some(&base), Some(&own)),
            Some(0o750)
        );
        let base = file(Some(0o755), true);
        let own = file(Some(0o755), true);
        assert_eq!(
            given(&file(Some(0o644), false), Some(&base), Some(&own)),
            Some(0o644)
        );
        // Windows: none.
        assert_eq!(given(&file(None, false), None, None), None);
        assert_eq!(
            given(
                &file(None, true),
                Some(&file(None, false)),
                Some(&file(None, false))
            ),
            None
        );
    }

    /// REQ-WS-005: a chain of links that leaves the root through its own links is refused like a
    /// link that leaves it: at the copy of the scope (omitted and counted), at a restore (omitted and
    /// reported), at the plan of a promotion (the plan is refused, naming the link). The chain is
    /// `mid/up -> ..` and `out -> mid/up/..`, which is lexically `mid` and really the parent of the
    /// root; `mid/up` itself stays inside and is kept.
    #[cfg(unix)]
    #[test]
    fn req_ws_005_a_link_chain_that_leaves_the_root_is_refused() {
        // The copy of the scope.
        let scope = scope();
        std::fs::create_dir(scope.path().join("mid")).unwrap();
        std::os::unix::fs::symlink("..", scope.path().join("mid/up")).unwrap();
        std::os::unix::fs::symlink("mid/up/..", scope.path().join("out")).unwrap();
        let data = tempfile::tempdir().unwrap();
        let task = TaskId::new(&ScriptedIds::default());
        let p = provider(data.path());
        let info = p
            .materialize(
                task,
                &ScopeSpec {
                    root: scope.path().to_string_lossy().into_owned(),
                    include: vec![],
                },
            )
            .unwrap();
        assert_eq!(info.symlinks_omitted, 1, "{info:?}");
        let tree = p.tree_path(task);
        assert!(std::fs::symlink_metadata(tree.join("mid/up")).is_ok());
        assert!(std::fs::symlink_metadata(tree.join("out")).is_err());
        // A restore: the chain was made in the tree by an action, and is in a snapshot.
        let fx = Fx::new();
        std::fs::create_dir(fx.tree().join("mid")).unwrap();
        std::os::unix::fs::symlink("..", fx.tree().join("mid/up")).unwrap();
        std::os::unix::fs::symlink("mid/up/..", fx.tree().join("out")).unwrap();
        let snapshot = fx.snap(SnapshotReason::Base, None);
        std::fs::remove_file(fx.tree().join("out")).unwrap();
        std::fs::remove_file(fx.tree().join("mid/up")).unwrap();
        let report = fx.p.restore(fx.task, &snapshot.record).unwrap();
        assert_eq!(report.links_omitted, ["out"], "{report:?}");
        assert!(std::fs::symlink_metadata(fx.tree().join("mid/up")).is_ok());
        assert!(std::fs::symlink_metadata(fx.tree().join("out")).is_err());
        // The plan of a promotion that holds the chain.
        let fx = Fx::new();
        let base = fx.snap(SnapshotReason::Base, None);
        std::fs::create_dir(fx.tree().join("mid")).unwrap();
        std::os::unix::fs::symlink("..", fx.tree().join("mid/up")).unwrap();
        std::os::unix::fs::symlink("mid/up/..", fx.tree().join("out")).unwrap();
        let err = fx.plan(&base.record).unwrap_err();
        let WorkspaceError::Refused { reason } = &err else {
            panic!("{err:?}")
        };
        assert!(
            reason.contains("out") && reason.contains("through other links"),
            "{reason}"
        );
        assert!(!fx.scope.path().join("mid").exists(), "nothing is written");
        // Without the link that leaves, the plan lists the one that stays.
        std::fs::remove_file(fx.tree().join("out")).unwrap();
        let plan = fx.plan(&base.record).unwrap();
        assert!(plan.changes.iter().any(|c| c.path == "mid/up"));
    }

    /// REQ-TASK-004: a fork materializes the snapshot into a new tree.
    #[test]
    fn req_task_004_materialize_from() {
        let fx = Fx::new();
        std::fs::write(fx.tree().join("src/lib.rs"), "forked content\n").unwrap();
        let snap = fx.snap(SnapshotReason::After, None);
        let new_task = TaskId::new(&fx.ids);
        let (info, prepared) =
            fx.p.materialize_from(
                new_task,
                mimlys_core::ids::SnapshotId::new(&fx.ids),
                &snap.record,
                &ScopeSpec {
                    root: fx.scope.path().to_string_lossy().into_owned(),
                    include: vec![],
                },
                fx.now(),
            )
            .unwrap();
        fx.p.seal(prepared.record.snapshot_id).unwrap();
        assert_eq!(prepared.record.reason, SnapshotReason::Fork);
        assert_eq!(prepared.record.manifest_hash, snap.record.manifest_hash);
        assert_eq!(info.files, 2);
        assert_eq!(
            std::fs::read_to_string(fx.p.tree_path(new_task).join("src/lib.rs")).unwrap(),
            "forked content\n"
        );
        assert_eq!(
            std::fs::read_to_string(fx.tree().join("src/lib.rs")).unwrap(),
            "forked content\n",
            "the source is untouched"
        );
    }

    /// RFC 006 §6: deleting removes the tree, the manifests and the released objects, nothing else.
    /// REQ-WS-012 (the files): the working tree and the manifest are removed and the workspace can no longer be opened (the blob and object files are covered by `req_ws_012_task_delete_removes_the_task_and_what_only_it_held`).
    #[test]
    fn rfc_006_delete_files() {
        let fx = Fx::new();
        let base = fx.snap(SnapshotReason::Base, None);
        let manifest = fx.p.manifest(&base.record).unwrap();
        let objects: Vec<_> = manifest.objects().keys().cloned().collect();
        let rows = DeletedRows {
            objects_at_zero: objects.clone(),
            snapshots: vec![base.record.snapshot_id],
            blobs: vec![],
        };
        fx.p.delete_files(fx.task, &rows).unwrap();
        assert!(!fx.tree().exists());
        assert!(fx.p.manifest(&base.record).is_err());
        assert!(fx.p.open(fx.task).is_err());
    }
}

/// RFC 006 §3 / the M3 report: the cost of hashing every file on every
/// snapshot, and of a restore, on a 10 000-file tree (prints the numbers).
#[test]
fn rfc_006_snapshot_cost_report() {
    let data = tempfile::tempdir().unwrap();
    let scope = tempfile::tempdir().unwrap();
    for i in 0..10_000u32 {
        let dir = scope.path().join(format!("d{:02}", i % 100));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("f{i}.txt")), vec![b'x'; 1024]).unwrap();
    }
    let p = LocalWorkspaces::new(data.path().to_path_buf(), patterns(), 1 << 30, 100_000).unwrap();
    let ids = ScriptedIds::default();
    let task = TaskId::new(&ids);
    let t0 = std::time::Instant::now();
    p.materialize(
        task,
        &ScopeSpec {
            root: scope.path().to_string_lossy().into_owned(),
            include: vec![],
        },
    )
    .unwrap();
    let copy_ms = t0.elapsed().as_millis();
    let now = mimlys_core::clock::Instant::from_unix_millis(1_700_000_000_000).unwrap();
    let t1 = std::time::Instant::now();
    let first = p
        .snapshot(
            task,
            mimlys_core::ids::SnapshotId::new(&ids),
            mimlys_core::snapshot::SnapshotReason::Base,
            None,
            None,
            now,
        )
        .unwrap();
    p.seal(first.record.snapshot_id).unwrap();
    let first_ms = t1.elapsed().as_millis();
    let t2 = std::time::Instant::now();
    let second = p
        .snapshot(
            task,
            mimlys_core::ids::SnapshotId::new(&ids),
            mimlys_core::snapshot::SnapshotReason::Before,
            None,
            Some(&first.record),
            now,
        )
        .unwrap();
    let second_ms = t2.elapsed().as_millis();
    assert!(second.reused);
    std::fs::write(p.tree_path(task).join("d00/f0.txt"), "changed").unwrap();
    let t3 = std::time::Instant::now();
    p.restore(task, &first.record).unwrap();
    let restore_ms = t3.elapsed().as_millis();
    println!(
        "M3 cost report (10 000 files, 10 MiB): materialize {copy_ms} ms; first snapshot (objects written) {first_ms} ms; snapshot of an unchanged tree (hash only) {second_ms} ms; restore of one changed file {restore_ms} ms"
    );
    assert_eq!(first.record.files, 10_000);
}
