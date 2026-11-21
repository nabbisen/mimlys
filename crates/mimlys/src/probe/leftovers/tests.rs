use super::*;
use crate::workspace::LocalWorkspaces;
use mimlys_core::clock::SystemClock;
use mimlys_core::ids::IdSource;

fn scope_name() -> String {
    format!("{SCOPE_PREFIX}{}", SystemIds.next().hyphenated())
}

/// A data directory with the store and the directories a runtime makes, and the provider of its
/// workspaces.
struct Rig {
    dir: tempfile::TempDir,
    store: mimlys_persist::sqlite::Store,
    workspaces: LocalWorkspaces,
}

impl Rig {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        for sub in ["tmp", "workspaces"] {
            std::fs::create_dir_all(dir.path().join(sub)).unwrap();
        }
        let store = mimlys_persist::sqlite::open(dir.path()).unwrap();
        let workspaces = LocalWorkspaces::new(dir.path().to_owned(), vec![], 1 << 20, 100).unwrap();
        Self {
            dir,
            store,
            workspaces,
        }
    }

    fn tmp(&self) -> PathBuf {
        self.dir.path().join("tmp")
    }

    fn remove(&self) -> Removed {
        remove(&self.store, &self.workspaces, self.dir.path(), &SystemClock)
    }

    /// A workspace copy no task has, as a creation cut by a crash leaves it: `files` in its tree.
    fn copy(&self, files: &[(&str, &str)]) -> PathBuf {
        let name = SystemIds.next().hyphenated().to_string();
        let tree = self.dir.path().join("workspaces").join(name).join("tree");
        std::fs::create_dir_all(&tree).unwrap();
        for (file, text) in files {
            std::fs::write(tree.join(file), text).unwrap();
        }
        tree.parent().unwrap().to_owned()
    }
}

/// REQ-MDL-011: the name of a probe's scope is `probe-` and a hyphenated UUID, and nothing else
/// is taken for one: not the prefix alone, not another prefix, not a UUID that is not hyphenated
/// or is in upper case, not a name with more after it.
#[test]
fn req_mdl_011_only_the_name_the_probe_writes_is_a_probes_scope() {
    let id = SystemIds.next().hyphenated().to_string();
    assert!(is_scope_name(&format!("probe-{id}")));
    for not in [
        "probe-".to_owned(),
        "probe".to_owned(),
        format!("probe-{}", SystemIds.next().simple()),
        format!("probe-{}", id.to_uppercase()),
        format!("probe-{id}-x"),
        format!("probes-{id}"),
        format!("x-probe-{id}"),
        id.clone(),
    ] {
        assert!(!is_scope_name(&not), "{not}");
    }
}

/// REQ-MDL-011: the directories found under `tmp` are real directories of the probe's name: a file
/// of that name, a link of that name and the other things the runtime keeps in `tmp` are not.
#[test]
fn req_mdl_011_the_scopes_found_are_directories_of_that_name_only() {
    let rig = Rig::new();
    let mine = rig.tmp().join(scope_name());
    std::fs::create_dir(&mine).unwrap();
    std::fs::write(rig.tmp().join(scope_name()), "a file").unwrap();
    std::fs::create_dir(rig.tmp().join("a-task-scratch")).unwrap();
    std::fs::create_dir(rig.tmp().join("probe-not-an-id")).unwrap();
    #[cfg(unix)]
    {
        let elsewhere = rig.dir.path().join("elsewhere");
        std::fs::create_dir(&elsewhere).unwrap();
        std::os::unix::fs::symlink(&elsewhere, rig.tmp().join(scope_name())).unwrap();
    }
    assert_eq!(scope_dirs(&rig.tmp()), [mine]);
    assert!(scope_dirs(&rig.dir.path().join("none")).is_empty());
}

/// REQ-MDL-011, THR-40: with no probe's directory the removal finds nothing and touches nothing:
/// the user's workspaces and the rest of `tmp` stay as they are.
#[test]
fn req_mdl_011_a_start_with_no_probe_directory_removes_nothing() {
    let rig = Rig::new();
    let user_copy = rig.copy(&[("a.txt", "the user's\n")]);
    std::fs::create_dir(rig.tmp().join("scratch")).unwrap();
    let removed = rig.remove();
    assert_eq!(removed, Removed::default());
    assert!(!removed.found());
    assert!(user_copy.is_dir() && rig.tmp().join("scratch").is_dir());
}

/// REQ-MDL-011: a probe's directory is removed with what it holds; the workspace copy of a creation
/// the crash cut (no task has it) is removed only when it holds exactly the probe's file as the
/// probe wrote it, and a copy that holds anything else is left alone, as is every other directory
/// of `tmp`.
#[test]
fn req_mdl_011_the_next_start_removes_a_probes_scope_and_its_unfinished_copy_only() {
    let rig = Rig::new();
    let scope = rig.tmp().join(scope_name());
    std::fs::create_dir(&scope).unwrap();
    std::fs::write(scope.join(FILE), FILE_START).unwrap();
    let probes_copy = rig.copy(&[(FILE, FILE_START)]);
    let edited_copy = rig.copy(&[(FILE, "color: blue\n")]);
    let users_copy = rig.copy(&[("a.txt", "x\n")]);
    let two_files = rig.copy(&[(FILE, FILE_START), ("b.txt", "x\n")]);
    let other = rig.tmp().join("scratch");
    std::fs::create_dir(&other).unwrap();
    let removed = rig.remove();
    assert!(removed.found());
    assert_eq!(removed.scopes.len(), 1);
    assert_eq!((removed.tasks, removed.copies), (0, 1));
    assert!(removed.left.is_empty(), "{:?}", removed.left);
    assert!(!scope.exists(), "the probe's scope is gone");
    assert!(!probes_copy.exists(), "the probe's unfinished copy is gone");
    for kept in [&edited_copy, &users_copy, &two_files, &other] {
        assert!(kept.is_dir(), "{} stays", kept.display());
    }
    // And a second start finds nothing more.
    assert_eq!(rig.remove(), Removed::default());
}
