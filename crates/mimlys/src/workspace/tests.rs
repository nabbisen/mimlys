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

#[test]
fn target_inside_scope_is_lexical() {
    assert!(target_inside_scope("a/b/link", Path::new("../c")));
    assert!(target_inside_scope("a/b/link", Path::new("../../c")));
    assert!(!target_inside_scope("a/b/link", Path::new("../../../c")));
    assert!(!target_inside_scope("link", Path::new("../x")));
    assert!(target_inside_scope("link", Path::new("./x/../y")));
    assert!(!target_inside_scope("link", Path::new("/etc")));
}

mod snapshots {
    use super::*;
    use mimlys_core::clock::Instant;
    use mimlys_core::snapshot::Entry;
    use mimlys_core::snapshot::{ChangeKind, SnapshotReason};
    use mimlys_core::spi::DeletedRows;

    struct Fx {
        _data: tempfile::TempDir,
        scope: tempfile::TempDir,
        p: LocalWorkspaces,
        task: TaskId,
        ids: ScriptedIds,
        n: std::cell::Cell<u32>,
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
                n: std::cell::Cell::new(0),
            }
        }

        fn now(&self) -> Instant {
            self.n.set(self.n.get() + 1);
            Instant::from_unix_millis(1_700_000_000_000 + i64::from(self.n.get())).unwrap()
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
        let (backup, promotion) =
            fx.p.promote(
                fx.task,
                &plan,
                &root,
                mimlys_core::ids::SnapshotId::new(&fx.ids),
                mimlys_core::ids::PromotionId::new(&fx.ids),
                fx.now(),
            )
            .unwrap();
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
        let err =
            fx.p.promote(
                fx.task,
                &plan,
                &root,
                mimlys_core::ids::SnapshotId::new(&fx.ids),
                mimlys_core::ids::PromotionId::new(&fx.ids),
                fx.now(),
            )
            .unwrap_err();
        assert!(matches!(err, WorkspaceError::Refused { .. }));
        // Reversal from the backup.
        let report = fx.p.revert_promotion(&promotion, &backup.record).unwrap();
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
        let (backup, promotion) =
            fx.p.promote(
                fx.task,
                &plan,
                &root,
                mimlys_core::ids::SnapshotId::new(&fx.ids),
                mimlys_core::ids::PromotionId::new(&fx.ids),
                fx.now(),
            )
            .unwrap();
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
        fx.p.revert_promotion(&promotion, &backup.record).unwrap();
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
        let (backup, promotion) =
            fx.p.promote(
                fx.task,
                &plan,
                &root,
                mimlys_core::ids::SnapshotId::new(&fx.ids),
                mimlys_core::ids::PromotionId::new(&fx.ids),
                fx.now(),
            )
            .unwrap();
        fx.p.seal(backup.record.snapshot_id).unwrap();
        check(fx.scope.path(), "foo v2", "bar v2");
        fx.p.revert_promotion(&promotion, &backup.record).unwrap();
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
