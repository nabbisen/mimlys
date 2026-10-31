use super::*;

/// The `/proc` walk finds a child and a grandchild of this process and
/// kills them within the grace period (clause K's fallback rung).
#[test]
fn rfc_011_descendant_walk_kills_the_tree() {
    let mut child = std::process::Command::new("sh")
        .args(["-c", "sleep 30 & sleep 30; wait"])
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(200));
    let me = std::process::id();
    let found = descendants_of(me);
    assert!(found.contains(&child.id()), "{found:?}");
    assert!(found.len() >= 2, "a grandchild too: {found:?}");
    let remaining = kill_descendants(me, Duration::from_secs(2));
    assert_eq!(remaining, 0);
    let _ = child.wait();
    assert!(descendants_of(me).is_empty());
}

/// The stat parser copes with a comm that has spaces and parentheses.
#[test]
fn ppid_parser_handles_odd_comm() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("stat"),
        "123 (my (odd) prog) S 77 123 123 0 -1 4194304\n",
    )
    .unwrap();
    assert_eq!(ppid_of(dir.path()), Some(77));
    std::fs::write(
        dir.path().join("stat"),
        "124 (gone) Z 77 123 123 0 -1 4194304\n",
    )
    .unwrap();
    assert_eq!(
        ppid_of(dir.path()),
        None,
        "a zombie is not a descendant to kill"
    );
}
