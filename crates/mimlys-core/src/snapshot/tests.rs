use super::*;

fn file(path: &str, content: &[u8]) -> Entry {
    Entry::File {
        path: path.into(),
        executable: false,
        mode: None,
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

/// REQ-WS-004 / LIM-U6, REQ-WS-006: a file entry may record its mode (0o777 mask), absent in a
/// manifest made before 0.11: such a manifest reads, serializes and hashes exactly as before (the
/// hash below was computed by 0.10), and writes no `mode` key; one that records modes hashes
/// otherwise. A mode that is not recorded is not a difference; one that is, is.
#[test]
fn req_ws_004_an_old_manifest_keeps_its_hash_and_a_mode_is_part_of_the_content() {
    let old = r#"{"entries":[{"type":"dir","path":"src"},{"type":"file","path":"src/lib.rs","executable":false,"size":10,"hash":"f3ea8f2a2e6215bd4b5c03ca09e9cfc6e81652ecc7349fb5e82165edd04b989b"},{"type":"file","path":"run.sh","executable":true,"size":3,"hash":"9dc177c2fde29dea8e7c29f7ddf147b7c449c99d049c62f3aac0a5933ecf76a3"},{"type":"symlink","path":"src/l","target":"lib.rs"}]}"#;
    let manifest: Manifest = serde_json::from_str(old).unwrap();
    assert_eq!(
        manifest.hash().as_str(),
        "5a43ee8a7a09bfd91a9b91c221a861a2c1a3394dbeb3d29f6eeefca8aeda8816"
    );
    assert!(
        manifest
            .entries
            .iter()
            .all(|e| !matches!(e, Entry::File { mode: Some(_), .. }))
    );
    assert!(
        !String::from_utf8(manifest.canonical_bytes())
            .unwrap()
            .contains("mode")
    );
    // The same manifest with the modes recorded is another manifest.
    let with_modes = Manifest::new(
        manifest
            .entries
            .iter()
            .cloned()
            .map(|e| match e {
                Entry::File {
                    path,
                    executable,
                    size,
                    hash,
                    ..
                } => Entry::File {
                    path,
                    executable,
                    mode: Some(if executable { 0o755 } else { 0o644 }),
                    size,
                    hash,
                },
                other => other,
            })
            .collect(),
    );
    assert_ne!(with_modes.hash(), manifest.hash());
    let json = serde_json::to_string(&with_modes).unwrap();
    assert!(
        json.contains("\"mode\":420") && json.contains("\"mode\":493"),
        "{json}"
    );
    assert_eq!(serde_json::from_str::<Manifest>(&json).unwrap(), with_modes);
    // `diff`: a mode alone is a change, an unrecorded mode is not.
    let with_mode = |mode: Option<u32>| {
        Manifest::new(vec![Entry::File {
            path: "f".into(),
            executable: false,
            mode,
            size: 1,
            hash: hash_bytes(b"x"),
        }])
    };
    let changes = |a: Option<u32>, b: Option<u32>| diff(&with_mode(a), &with_mode(b)).changes;
    assert_eq!(
        changes(Some(0o644), Some(0o600))
            .iter()
            .map(|c| (c.path.as_str(), c.kind))
            .collect::<Vec<_>>(),
        [("f", ChangeKind::Modified)]
    );
    assert!(changes(Some(0o644), Some(0o644)).is_empty());
    assert!(
        changes(None, Some(0o600)).is_empty(),
        "an old base records no mode"
    );
    assert!(changes(Some(0o600), None).is_empty());
}

/// REQ-WS-005: a link is resolved through the links of the tree before it is judged. The chain
/// of the audit, `s/x -> ..` and `y -> s/x/..`, is lexically inside the root (`s/x/..` is `s`)
/// and really the parent of the root, so `y` leaves; `s/x` itself does not. A chain is followed
/// through every link on the way, a bounded number of times: `MAX_LINK_HOPS` links are allowed
/// and one more is refused, which is also what ends a loop. A link on the way that is absolute
/// refuses it. A name that is not a link of the tree is taken as it is.
#[test]
fn req_ws_005_a_link_chain_is_resolved_through_the_trees_own_links() {
    let links = |pairs: &[(&'static str, &'static str)]| -> BTreeMap<&'static str, &'static str> {
        pairs.iter().copied().collect()
    };
    let chain = links(&[("s/x", ".."), ("y", "s/x/..")]);
    assert!(target_inside_root("y", "s/x/.."), "lexically inside");
    assert!(
        !target_resolves_inside_root("y", "s/x/..", &chain),
        "through s/x it is the parent of the root"
    );
    assert!(target_resolves_inside_root("s/x", "..", &chain));
    // The same shape with another set of names, and one level deeper.
    let deeper = links(&[("a/b/up", "../.."), ("c", "a/b/up/..")]);
    assert!(!target_resolves_inside_root("c", "a/b/up/..", &deeper));
    let inside = links(&[("a/b/up", ".."), ("c", "a/b/up/..")]);
    assert!(
        target_resolves_inside_root("c", "a/b/up/..", &inside),
        "`a/b/up` is `a`, and its parent is the root"
    );
    // A chain of plain links to a file is followed and inside.
    let mut pairs: Vec<(String, String)> = (0..MAX_LINK_HOPS - 1)
        .map(|i| (format!("l{i}"), format!("l{}", i + 1)))
        .collect();
    pairs.push((format!("l{}", MAX_LINK_HOPS - 1), "file".to_owned()));
    let owned: BTreeMap<&str, &str> = pairs
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();
    // `l0 -> l1 -> ... -> l39 -> file`: from `start`, 40 links are passed through.
    assert!(
        target_resolves_inside_root("start", "l0", &owned),
        "the bound is allowed"
    );
    // One more link on the way is refused.
    let mut longer = owned.clone();
    longer.insert("start2", "l0");
    assert!(
        !target_resolves_inside_root("start3", "start2", &longer),
        "{MAX_LINK_HOPS} + 1 links are refused"
    );
    // A loop ends at the bound and is refused.
    let looping = links(&[("a", "b"), ("b", "a")]);
    assert!(!target_resolves_inside_root("start", "a", &looping));
    // An absolute link on the way leaves; a drive does too.
    assert!(!target_resolves_inside_root(
        "y",
        "s",
        &links(&[("s", "/etc")])
    ));
    assert!(!target_resolves_inside_root(
        "y",
        "s",
        &links(&[("s", "C:\\x")])
    ));
    // Names that are not links are taken as they are, and the lexical rule holds first.
    let none = links(&[]);
    assert!(target_resolves_inside_root("a/b/l", "../c", &none));
    assert!(!target_resolves_inside_root("l", "../x", &none));
    assert!(!target_resolves_inside_root("l", "/etc", &none));
    // A backslash separates, on every platform.
    assert!(!target_resolves_inside_root(
        "y",
        "s\\x\\..",
        &links(&[("s/x", "..")])
    ));
}
