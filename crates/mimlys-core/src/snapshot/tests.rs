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

/// REQ-WS-007: a promotion record without `restored` reads as restored; the field is written
/// only when the restore of a failed promotion failed; a failed promotion can be reversed by its
/// backup only then.
#[test]
fn req_ws_007_the_restored_flag_defaults_to_true_and_marks_the_way_back() {
    let record = r#"{"promotion_id":"00000000-0000-7000-8000-00000000f00d","task_id":"00000000-0000-7000-8000-000000000001","backup_snapshot":"00000000-0000-7000-8000-00000000beef","project_root":"/p","files":[],"applied_at":"2025-01-01T00:00:00Z","reversed_at":null,"state":"failed"}"#;
    let mut promotion: Promotion = serde_json::from_str(record).unwrap();
    assert!(promotion.restored);
    assert!(!promotion.can_be_reversed(), "its project was restored");
    assert!(
        !serde_json::to_string(&promotion)
            .unwrap()
            .contains("restored")
    );
    promotion.restored = false;
    assert!(promotion.can_be_reversed());
    let written = serde_json::to_string(&promotion).unwrap();
    assert!(written.contains(r#""restored":false"#), "{written}");
    assert_eq!(
        serde_json::from_str::<Promotion>(&written).unwrap(),
        promotion
    );
    promotion.reversed_at = Some(promotion.applied_at);
    assert!(!promotion.can_be_reversed());
    promotion.reversed_at = None;
    promotion.state = PromotionState::Applying;
    assert!(!promotion.can_be_reversed());
}
