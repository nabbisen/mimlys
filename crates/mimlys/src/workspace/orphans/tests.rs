use super::*;

const A: &str = "00000000-0000-0000-0000-00000000000a";
const B: &str = "00000000-0000-0000-0000-00000000000b";

fn data() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("workspaces")).unwrap();
    dir
}

fn workspace(data: &Path, name: &str, files: &[(&str, usize)]) {
    let dir = data.join("workspaces").join(name);
    for (path, len) in files {
        let path = dir.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, vec![b'x'; *len]).unwrap();
    }
}

fn names(found: &[Orphan]) -> Vec<&str> {
    found.iter().map(|o| o.name.as_str()).collect()
}

/// REQ-WS-014: a directory of `workspaces` named like a task id that no task refers to is
/// listed, in name order, with the bytes of the regular files below it; one a task refers to
/// is not.
#[test]
fn req_ws_014_a_workspace_no_task_refers_to_is_found_with_its_footprint() {
    let data = data();
    workspace(data.path(), B, &[("tree/src/a.rs", 100), ("tmp/x", 20)]);
    workspace(data.path(), A, &[("tree/b", 5)]);
    let held = "00000000-0000-0000-0000-00000000000c";
    workspace(data.path(), held, &[("tree/c", 1)]);
    let referenced = BTreeSet::from([held.to_owned()]);
    let found = find(data.path(), &referenced).unwrap();
    assert_eq!(names(&found), [A, B]);
    assert_eq!((found[0].bytes, found[1].bytes), (5, 120));
    assert!(found.iter().all(|o| o.reason == REASON));
    // Nothing is removed by looking.
    assert!(data.path().join("workspaces").join(held).exists());
    assert!(data.path().join("workspaces").join(A).exists());
}

/// REQ-WS-014: only what a creation makes is looked at: a name that is not a task id as this
/// program writes it (another name, upper case, no hyphens), and a file with a task's name,
/// are neither listed nor removed. No `workspaces` directory is no workspace.
#[test]
fn req_ws_014_only_directories_named_like_a_task_are_looked_at() {
    // Not the same name as another written here with other letters: macOS and Windows
    // keep one name for both.
    const FILE: &str = "00000000-0000-0000-0000-000000000010";
    let data = data();
    let root = data.path().join("workspaces");
    for odd in [
        "notes",
        "00000000-0000-0000-0000-00000000000F",
        "0000000000000000000000000000000d",
    ] {
        std::fs::create_dir_all(root.join(odd).join("tree")).unwrap();
    }
    std::fs::write(root.join(FILE), b"a file with a task's name").unwrap();
    assert_eq!(find(data.path(), &BTreeSet::new()).unwrap(), vec![]);
    assert!(remove(data.path(), "notes").is_empty());
    assert!(root.join("notes").exists(), "not a task's name: kept");
    assert!(remove(data.path(), FILE).is_empty());
    assert!(root.join(FILE).is_file(), "not a directory: kept");
    let none = tempfile::tempdir().unwrap();
    assert_eq!(find(none.path(), &BTreeSet::new()).unwrap(), vec![]);
}

/// REQ-WS-014: a link is never followed: a link named like a task, to a directory outside,
/// is neither listed nor removed, and what it points at is untouched; a link inside a
/// workspace is removed as a link, and its target is not counted or removed.
#[cfg(unix)]
#[test]
fn req_ws_014_a_link_is_never_followed() {
    let data = data();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("precious"), vec![b'p'; 1000]).unwrap();
    let root = data.path().join("workspaces");
    std::os::unix::fs::symlink(outside.path(), root.join(A)).unwrap();
    assert_eq!(find(data.path(), &BTreeSet::new()).unwrap(), vec![]);
    assert!(remove(data.path(), A).is_empty());
    assert!(root.join(A).symlink_metadata().is_ok(), "the link is kept");
    // A link inside a workspace.
    workspace(data.path(), B, &[("tree/f", 10)]);
    std::os::unix::fs::symlink(outside.path(), root.join(B).join("tree/link")).unwrap();
    let found = find(data.path(), &BTreeSet::new()).unwrap();
    assert_eq!(names(&found), [B]);
    assert_eq!(found[0].bytes, 10, "the target of a link is not counted");
    assert!(remove(data.path(), B).is_empty());
    assert!(!root.join(B).exists());
    assert!(outside.path().join("precious").exists());
}

/// REQ-WS-014: the removal takes the whole directory, a read-only part included, and says
/// nothing failed.
#[test]
fn req_ws_014_a_workspace_is_removed_whole() {
    let data = data();
    workspace(data.path(), A, &[("tree/deep/er/f", 3), ("tmp/g", 4)]);
    workspace(data.path(), B, &[("tree/f", 1)]);
    assert!(remove(data.path(), A).is_empty());
    assert!(!data.path().join("workspaces").join(A).exists());
    assert!(
        data.path().join("workspaces").join(B).exists(),
        "the other stays"
    );
}
