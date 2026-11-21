use super::*;

fn tree_with(file: &str, content: &[u8]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(file), content).unwrap();
    dir
}

/// REQ-MDL-011: the file is as asked when it holds the wanted line, a line ending after it or not
/// (a model that writes the whole file does not know which the goal wanted).
#[test]
fn req_mdl_011_the_file_as_asked_is_the_wanted_line() {
    for content in ["color: blue", "color: blue\n", "color: blue\r\n"] {
        let dir = tree_with(crate::probe::FILE, content.as_bytes());
        assert!(file_is_as_asked(dir.path()), "{content:?}");
    }
}

/// REQ-MDL-011: anything else is not: the file as it started, a word more, a line more, an empty
/// file, another file, no file, a directory by its name, a file past the bound.
#[test]
fn req_mdl_011_anything_else_is_not_as_asked() {
    for content in [
        crate::probe::FILE_START,
        "color: blue!",
        "color: blue\nmore\n",
        "color: blue\n\n\nx",
        "Color: blue",
        "",
        " color: blue",
    ] {
        let dir = tree_with(crate::probe::FILE, content.as_bytes());
        assert!(!file_is_as_asked(dir.path()), "{content:?}");
    }
    let other = tree_with("other.txt", b"color: blue\n");
    assert!(!file_is_as_asked(other.path()), "another file");
    let empty = tempfile::tempdir().unwrap();
    assert!(!file_is_as_asked(empty.path()), "no file");
    assert!(
        !file_is_as_asked(&empty.path().join("gone")),
        "no tree at all"
    );
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir(directory.path().join(crate::probe::FILE)).unwrap();
    assert!(
        !file_is_as_asked(directory.path()),
        "a directory by its name"
    );
    let mut long = String::from("color: blue\n");
    long.push_str(&" ".repeat(crate::probe::READ_AT_MOST as usize));
    let big = tree_with(crate::probe::FILE, long.as_bytes());
    assert!(!file_is_as_asked(big.path()), "past the bound");
}

/// REQ-MDL-011, THR-40: the runtime reads through a handle on the tree that cannot leave it: a
/// link in the tree that points at a file outside it, which holds the wanted line, is not the file
/// as asked (Unix: a link needs no privilege there).
#[cfg(unix)]
#[test]
fn thr_40_a_link_out_of_the_tree_is_not_followed() {
    let outside = tree_with("elsewhere.txt", b"color: blue\n");
    let tree = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(
        outside.path().join("elsewhere.txt"),
        tree.path().join(crate::probe::FILE),
    )
    .unwrap();
    assert!(!file_is_as_asked(tree.path()));
}
