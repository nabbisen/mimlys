use super::*;
use crate::confine::{ConfinementSpec, task_spec};

/// A data directory with two tasks, each with a tree and a temporary directory.
fn data_dir() -> (tempfile::TempDir, PathBuf) {
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("data");
    for task in ["a", "b"] {
        std::fs::create_dir_all(data.join("workspaces").join(task).join("tree")).unwrap();
        std::fs::create_dir_all(data.join("workspaces").join(task).join("tmp")).unwrap();
    }
    std::fs::create_dir_all(data.join("tmp")).unwrap();
    (root, data)
}

fn spec(data: &Path) -> ConfinementSpec {
    ConfinementSpec {
        read_paths: vec![],
        write_paths: vec![data.join("workspaces"), data.join("tmp")],
        deny_network: true,
        memory_bytes: 0,
        max_processes: 0,
        data_dir: data.to_path_buf(),
    }
}

/// A link to `target` at `link`: a symbolic link on Unix, a junction on
/// Windows (no privilege needed for it).
fn link(target: &Path, link: &Path) {
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, link).unwrap();
    #[cfg(windows)]
    {
        let made = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .output()
            .unwrap();
        assert!(
            made.status.success(),
            "mklink {} {}: {}{}",
            link.display(),
            target.display(),
            String::from_utf8_lossy(&made.stdout),
            String::from_utf8_lossy(&made.stderr)
        );
    }
}

/// Removes a link made by [`link`] (the link, not its target).
fn unlink(path: &Path) {
    #[cfg(unix)]
    std::fs::remove_file(path).unwrap();
    #[cfg(windows)]
    std::fs::remove_dir(path).unwrap();
}

/// REQ-CONF-006, REQ-WS-005 (handoff 011n item 9): a plain name is something
/// a task id, `tree` or `tmp` can be; nothing that names another place is.
#[test]
fn req_conf_006_a_plain_name_names_no_other_place() {
    for plain in ["a", "tree", "tmp", "0198a7c2-5d6e", "t_1", "x.y", ".hidden"] {
        assert!(is_plain_name(OsStr::new(plain)), "{plain}");
    }
    for not_plain in [
        "", ".", "..", "a/b", "a\\b", "a:b", "C:", "a.", "a ", "tree.", "a\0b", "..\\x",
    ] {
        assert!(!is_plain_name(OsStr::new(not_plain)), "{not_plain:?}");
    }
}

/// REQ-CONF-006, REQ-WS-005 (handoff 011n item 9): the paths of a task are
/// taken apart and each component must be a plain name; `<data>/workspaces/../tree`
/// (which a lexical comparison took for the tree of a task, its parent's parent
/// being `<data>/workspaces`) is refused, so is a `.`-less detour of any kind,
/// a path outside the data directory and a temporary directory of another task.
#[test]
fn req_conf_006_a_non_plain_component_is_refused() {
    let (_root, data) = data_dir();
    let ws = data.join("workspaces");
    let (tree, tmp) = (ws.join("a/tree"), ws.join("a/tmp"));
    let names = |t: &Path, m: &Path| task_names(&data, t, m);
    let (tree_names, tmp_names) = names(&tree, &tmp).expect("one task");
    assert_eq!(tree_names, ["workspaces", "a", "tree"]);
    assert_eq!(tmp_names, ["workspaces", "a", "tmp"]);
    // The execution's directory inside the task's temporary directory is one task's too.
    assert!(names(&tree, &ws.join("a/tmp/exec1")).is_ok());
    // The traversal the lexical test let through.
    let sneaky_tree = ws.join("..").join("tree");
    let e = names(&sneaky_tree, &tmp).unwrap_err();
    assert!(
        e.contains("not a plain name") || e.contains("not the tree"),
        "{e}"
    );
    assert!(names(&ws.join("a/../b/tree"), &ws.join("b/tmp")).is_err());
    assert!(names(&tree, &ws.join("a/tmp/../../b/tmp")).is_err());
    assert!(names(&tree, &ws.join("a/tmp/..")).is_err());
    // Not the tree of a task.
    assert!(names(&ws.join("a"), &tmp).is_err());
    assert!(names(&ws, &tmp).is_err());
    assert!(names(&ws.join("a/other"), &tmp).is_err());
    assert!(names(&ws.join("a/tree/deeper"), &tmp).is_err());
    // Not beneath the data directory.
    let outside = data.parent().unwrap().join("elsewhere");
    assert!(names(&outside.join("workspaces/a/tree"), &tmp).is_err());
    // Another task's temporary directory; the task directory itself.
    assert!(names(&tree, &ws.join("b/tmp")).is_err());
    assert!(names(&tree, &ws.join("a")).is_err());
    // The temporary directory is `tmp` or inside it, never anything else in the
    // task's directory (handoff 011o, D7: that directory holds the workspace
    // record, and the tree is not a temporary directory).
    for elsewhere in [
        "a/INFO",
        "a/elsewhere",
        "a/elsewhere/exec1",
        "a/tree",
        "a/tree/x",
        "a/tmpx",
    ] {
        let e = names(&tree, &ws.join(elsewhere)).unwrap_err();
        assert!(e.contains("do not belong"), "{elsewhere}: {e}");
    }
    assert!(names(&tree, &ws.join("a/tmp/exec1/deeper")).is_ok());
    // A name that is not plain on some platform is refused on all.
    assert!(names(&ws.join("a:b/tree"), &ws.join("a:b/tmp")).is_err());
    assert!(names(&ws.join("a./tree"), &ws.join("a./tmp")).is_err());
}

/// REQ-CONF-006, REQ-WS-005 (handoff 011n item 9), on every job: `task_spec`
/// opens the paths. A `..` component is refused (not resolved to a directory
/// that happens to exist), a task directory, a tree or a temporary directory
/// that is a link is refused naming the link, a missing directory is refused,
/// and the plain case still narrows the write set to the two directories.
#[test]
fn req_conf_006_task_spec_opens_the_paths_and_refuses_links() {
    let (root, data) = data_dir();
    let ws = data.join("workspaces");
    let base = spec(&data);
    let (tree, tmp) = (ws.join("a/tree"), ws.join("a/tmp"));
    let granted = task_spec(&base, &tree, &tmp).expect("one task, plain");
    assert!(granted.write_paths.contains(&tree) && granted.write_paths.contains(&tmp));
    assert!(
        !granted
            .write_paths
            .iter()
            .any(|p| p.starts_with(&data) && p != &tree && p != &tmp),
        "nothing of the data directory but the two: {:?}",
        granted.write_paths
    );
    // `..` where a directory of that name exists to be reached: `<data>/tree`.
    std::fs::create_dir_all(data.join("tree")).unwrap();
    let e = task_spec(&base, &ws.join("..").join("tree"), &tmp).unwrap_err();
    assert!(
        e.contains("not a plain name") || e.contains("not the tree"),
        "{e}"
    );
    assert!(task_spec(&base, &tree, &ws.join("a/tmp/../../a/tmp")).is_err());
    // A missing temporary directory.
    assert!(task_spec(&base, &tree, &ws.join("a/tmp/missing")).is_err());
    // No data directory in the specification.
    let mut none = base.clone();
    none.data_dir = PathBuf::new();
    assert!(task_spec(&none, &tree, &tmp).is_err());

    // A linked task directory: `b` replaced by a link to `a` (a tool that can
    // write `<data>/workspaces` could do this).
    let elsewhere = root.path().join("elsewhere");
    std::fs::create_dir_all(elsewhere.join("tree")).unwrap();
    std::fs::create_dir_all(elsewhere.join("tmp")).unwrap();
    std::fs::remove_dir_all(ws.join("b")).unwrap();
    link(&elsewhere, &ws.join("b"));
    let e = task_spec(&base, &ws.join("b/tree"), &ws.join("b/tmp")).unwrap_err();
    assert!(e.contains("link"), "a linked task directory: {e}");
    // A linked tree, in a task of its own.
    std::fs::create_dir_all(ws.join("c/tmp")).unwrap();
    link(&elsewhere.join("tree"), &ws.join("c").join("tree"));
    let e = task_spec(&base, &ws.join("c/tree"), &ws.join("c/tmp")).unwrap_err();
    assert!(e.contains("link"), "a linked tree: {e}");
    // A linked temporary directory.
    std::fs::create_dir_all(ws.join("d/tree")).unwrap();
    link(&elsewhere.join("tmp"), &ws.join("d").join("tmp"));
    let e = task_spec(&base, &ws.join("d/tree"), &ws.join("d/tmp")).unwrap_err();
    assert!(e.contains("link"), "a linked tmp: {e}");
    // The workspaces directory itself replaced by a link.
    let other = root.path().join("other");
    std::fs::create_dir_all(other.join("a/tree")).unwrap();
    std::fs::create_dir_all(other.join("a/tmp")).unwrap();
    let moved = data.join("workspaces-moved");
    std::fs::rename(&ws, &moved).unwrap();
    link(&other, &ws);
    let e = task_spec(&base, &ws.join("a/tree"), &ws.join("a/tmp")).unwrap_err();
    assert!(e.contains("link"), "a linked workspaces directory: {e}");
    // The data directory is opened as it is spelled: reached through a link, it works.
    unlink(&ws);
    std::fs::rename(&moved, &ws).unwrap();
    let through = root.path().join("data-link");
    link(&data, &through);
    let spelled = spec(&through);
    task_spec(
        &spelled,
        &through.join("workspaces/a/tree"),
        &through.join("workspaces/a/tmp"),
    )
    .expect("a data directory reached through a link is the administrator's choice");
}

/// REQ-CONF-006 (handoff 011o, D5), on every job: each of the runtime's own
/// directories is refused when it is a link (a symbolic link, or a junction on
/// Windows), naming it; an ordinary directory and a missing one pass.
#[test]
fn req_conf_006_the_runtimes_own_directories_are_never_links() {
    let (root, data) = data_dir();
    let elsewhere = root.path().join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    for name in ["workspaces", "tmp", "interpreters"] {
        let dir = data.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(refuse_link(&dir), Ok(()), "{name} as a directory");
        let moved = data.join(format!("{name}-moved"));
        std::fs::rename(&dir, &moved).unwrap();
        link(&elsewhere, &dir);
        let e = refuse_link(&dir).unwrap_err();
        assert!(
            e.contains(&dir.display().to_string()) && e.contains("link"),
            "{name} as a link: {e}"
        );
        unlink(&dir);
        std::fs::rename(&moved, &dir).unwrap();
        assert_eq!(refuse_link(&dir), Ok(()), "{name} restored");
    }
    assert_eq!(
        refuse_link(&data.join("not-yet")),
        Ok(()),
        "a missing directory"
    );
}

/// REQ-CONF-006 (handoff 011n item 9), Unix: the descriptors are those of the
/// directories (`O_PATH` on Linux), and a plain task opens as itself.
#[cfg(unix)]
#[test]
fn req_conf_006_the_opened_directories_are_the_tasks_own() {
    use std::os::unix::fs::MetadataExt;
    let (_root, data) = data_dir();
    let ws = data.join("workspaces");
    let opened = open_task(&data, &ws.join("a/tree"), &ws.join("a/tmp")).unwrap();
    #[allow(clippy::unnecessary_cast)]
    let same = |fd: &std::os::fd::OwnedFd, path: &Path| {
        let stat = rustix::fs::fstat(fd).unwrap();
        let meta = std::fs::metadata(path).unwrap();
        stat.st_ino as u64 == meta.ino() && stat.st_dev as u64 == meta.dev()
    };
    assert!(same(&opened.tree.fd, &ws.join("a/tree")));
    assert!(same(&opened.tmp.fd, &ws.join("a/tmp")));
    assert!(!same(&opened.tree.fd, &ws.join("b/tree")));
}
