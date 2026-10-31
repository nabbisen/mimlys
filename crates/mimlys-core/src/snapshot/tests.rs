use super::*;

fn file(path: &str, content: &[u8]) -> Entry {
    Entry::File {
        path: path.into(),
        executable: false,
        size: content.len() as u64,
        hash: hash_bytes(content),
    }
}

/// REQ-WS-004 / LIM-U6: a manifest's hash covers its sorted entries and nothing else.
#[test]
fn req_ws_004_manifest_canonical_hash() {
    let a = Manifest::new(vec![
        file("src/lib.rs", b"a"),
        Entry::Dir { path: "src".into() },
        Entry::Symlink {
            path: "link".into(),
            target: "src/lib.rs".into(),
        },
    ]);
    let b = Manifest::new(vec![
        Entry::Symlink {
            path: "link".into(),
            target: "src/lib.rs".into(),
        },
        Entry::Dir { path: "src".into() },
        file("src/lib.rs", b"a"),
    ]);
    assert_eq!(a, b, "order of construction does not matter");
    assert_eq!(a.hash(), b.hash());
    assert_eq!(
        a.entries.iter().map(Entry::path).collect::<Vec<_>>(),
        ["link", "src", "src/lib.rs"]
    );
    let c = Manifest::new(vec![file("src/lib.rs", b"b")]);
    assert_ne!(a.hash(), c.hash());
    assert_eq!(a.bytes(), 1);
    assert_eq!(a.files(), 1);
    assert_eq!(a.objects().len(), 1);
    let json = serde_json::to_string(&a).unwrap();
    assert!(json.contains("\"type\":\"symlink\""));
    assert_eq!(serde_json::from_str::<Manifest>(&json).unwrap(), a);
}

/// External design §14.5: the diff names added, modified and deleted paths.
#[test]
fn ed_14_5_diff() {
    let base = Manifest::new(vec![
        file("a", b"1"),
        file("b", b"2"),
        file("c", b"3"),
        Entry::Dir { path: "d".into() },
    ]);
    let current = Manifest::new(vec![
        file("a", b"1"),
        file("b", b"changed"),
        file("e", b"5"),
        Entry::Dir { path: "d".into() },
    ]);
    let d = diff(&base, &current);
    assert_eq!(
        d.changes,
        vec![
            Change {
                path: "b".into(),
                kind: ChangeKind::Modified,
                bytes: 7
            },
            Change {
                path: "c".into(),
                kind: ChangeKind::Deleted,
                bytes: 0
            },
            Change {
                path: "e".into(),
                kind: ChangeKind::Added,
                bytes: 1
            },
        ]
    );
    assert!(diff(&base, &base).is_empty());
    // A file replaced by a link of the same path is a modification.
    let linked = Manifest::new(vec![
        file("a", b"1"),
        Entry::Symlink {
            path: "b".into(),
            target: "a".into(),
        },
        file("c", b"3"),
        Entry::Dir { path: "d".into() },
    ]);
    assert_eq!(diff(&base, &linked).changes[0].kind, ChangeKind::Modified);
}

/// REQ-WS-005: a link's target stays inside the root only when relative and
/// never above the root, lexically.
#[test]
fn req_ws_005_target_rule() {
    assert!(target_inside_root("a/b/link", "../c"));
    assert!(target_inside_root("a/b/link", "../../c"));
    assert!(!target_inside_root("a/b/link", "../../../c"));
    assert!(!target_inside_root("link", "../x"));
    assert!(target_inside_root("link", "./x/../y"));
    assert!(!target_inside_root("link", "/etc/passwd"));
    assert!(!target_inside_root("link", "C:\\Windows"));
    assert!(target_inside_root("deep/er/link", "../../top"));
    assert!(!target_inside_root("a/link", "..\\..\\x"));
    assert!(!target_inside_root("link", "\\\\server\\share"));
}
