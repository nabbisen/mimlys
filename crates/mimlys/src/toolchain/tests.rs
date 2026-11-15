//! The one spelling of a directory in the record and the commands.

use super::{
    Access, Entry, FoundBy, Outcome, Planned, Record, Source, normalized, plan_revoke_all,
    record_after, same_path,
};

/// REQ-CONF-008: one directory under two spellings is one entry (a `..`
/// segment here; the short and the long forms on Windows), and a missing
/// directory keeps its name.
#[test]
fn req_conf_008_one_spelling_per_directory() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("a")).unwrap();
    let plain = dir.path().join("a");
    let roundabout = dir.path().join("a").join("..").join("a");
    assert!(same_path(
        plain.to_str().unwrap(),
        roundabout.to_str().unwrap()
    ));
    assert_eq!(normalized(&roundabout), normalized(&plain));
    assert!(!normalized(&plain).starts_with(r"\\?\"));
    let missing = dir.path().join("missing");
    assert_eq!(normalized(&missing), missing.to_string_lossy());
    assert!(!same_path(
        plain.to_str().unwrap(),
        missing.to_str().unwrap()
    ));
}

fn entry(path: &str, source: Source) -> Entry {
    Entry {
        path: path.into(),
        access: Access::Read,
        source,
        applied_at: "2026-11-07T00:00:00Z".into(),
    }
}

fn record(entries: Vec<Entry>) -> Record {
    Record {
        profile: "mimlys-test".into(),
        sid: "S-1-15-2-1".into(),
        entries,
    }
}

/// REQ-CONF-008: the list of `revoke --all` is every recorded entry plus
/// every candidate that carries the entry now although the record lacks it;
/// a candidate without the entry is not listed; nothing appears twice.
#[test]
fn req_conf_008_plan_holds_the_record_and_what_the_scan_finds() {
    let dir = tempfile::tempdir().unwrap();
    let p = |n: &str| dir.path().join(n).to_string_lossy().into_owned();
    let rec = record(vec![
        entry(&p("recorded"), Source::User),
        entry(&p("both"), Source::Runtime),
    ]);
    let candidates = vec![
        (p("both"), Source::Runtime),
        (p("unrecorded"), Source::Runtime),
        (p("clean"), Source::User),
    ];
    let in_force = |path: &str| path == p("both") || path == p("unrecorded");
    let plan = plan_revoke_all(&rec, &candidates, in_force);
    let found: Vec<(String, FoundBy)> = plan
        .iter()
        .map(|x| {
            (
                x.path.rsplit(['/', '\\']).next().unwrap().to_owned(),
                x.found_by,
            )
        })
        .collect();
    assert_eq!(
        found,
        vec![
            ("recorded".into(), FoundBy::Record),
            ("both".into(), FoundBy::Record),
            ("unrecorded".into(), FoundBy::Scan),
        ],
        "a recorded entry is listed whether or not it is in force; a scanned one only if it is"
    );
    // Without the scan (the mutation of handoff 011e): the unrecorded entry is missed.
    let record_only = plan_revoke_all(&rec, &[], |_| false);
    assert_eq!(record_only.len(), 2);
}

/// REQ-CONF-008: the record after a run holds exactly what failed (a scanned
/// entry that failed is added to it, so a second run retries it); nothing is
/// left when everything was removed.
#[test]
fn req_conf_008_record_after_keeps_exactly_what_failed() {
    let rec = record(vec![
        entry("/a", Source::User),
        entry("/b", Source::Runtime),
    ]);
    let planned = |path: &str, found_by| Planned {
        path: path.into(),
        source: Source::Runtime,
        found_by,
    };
    let all_removed = Outcome {
        removed: vec![
            planned("/a", FoundBy::Record),
            planned("/b", FoundBy::Record),
        ],
        ..Outcome::default()
    };
    assert!(all_removed.complete());
    assert_eq!(record_after(&rec, &all_removed), None);
    let partial = Outcome {
        removed: vec![planned("/a", FoundBy::Record)],
        failed: vec![
            (planned("/b", FoundBy::Record), "access denied".into()),
            (planned("/c", FoundBy::Scan), "in use".into()),
        ],
        ..Outcome::default()
    };
    assert!(!partial.complete());
    let kept = record_after(&rec, &partial).unwrap();
    let paths: Vec<&str> = kept.entries.iter().map(|e| e.path.as_str()).collect();
    assert_eq!(paths, ["/b", "/c"]);
    assert_eq!(
        kept.entries[0].source,
        Source::Runtime,
        "the recorded entry is kept as it was"
    );
    assert_eq!(
        (kept.profile.as_str(), kept.sid.as_str()),
        ("mimlys-test", "S-1-15-2-1")
    );
}

/// REQ-CONF-008 / external design §19.2: the JSON shape of `revoke --all`.
#[test]
fn req_conf_008_outcome_json_has_the_documented_shape() {
    let outcome = Outcome {
        removed: vec![Planned {
            path: "/a".into(),
            source: Source::User,
            found_by: FoundBy::Scan,
        }],
        failed: vec![(
            Planned {
                path: "/b".into(),
                source: Source::Runtime,
                found_by: FoundBy::Record,
            },
            "denied".into(),
        )],
        profile_deleted: false,
        record_deleted: false,
    };
    let v = outcome.to_json();
    assert_eq!(
        v,
        serde_json::json!({
            "removed": [{"path": "/a", "source": "user", "found_by": "scan"}],
            "failed": [{"path": "/b", "reason": "denied"}],
            "profile_deleted": false,
            "record_deleted": false,
        })
    );
}

/// REQ-CONF-008 (A-23): a record that is corrupt or cannot be read stops the
/// start with the file and the remedy named, and the file is left exactly as
/// it was; no record at all is a new installation; a good one loads.
#[test]
fn req_conf_008_a_corrupt_record_stops_the_start_and_is_never_rewritten() {
    let dir = tempfile::tempdir().unwrap();
    // Nothing there: a new installation.
    assert_eq!(Record::load(dir.path()).unwrap(), Record::default());
    assert!(super::load_record(dir.path()).is_ok());
    let file = dir.path().join("toolchain-acls.json");
    // Corrupt: truncated, then not JSON at all, then empty, then JSON of another shape.
    for bytes in [
        &b"{\"profile\": \"mimlys-x\", \"entries\": [{\"path\""[..],
        b"not json",
        b"",
        b"{\"profile\": 3}",
        b"[1, 2]",
    ] {
        std::fs::write(&file, bytes).unwrap();
        let error = Record::load(dir.path()).unwrap_err();
        assert!(
            matches!(error, super::RecordError::Corrupt { .. }),
            "{error:?}"
        );
        let app = super::load_record(dir.path()).unwrap_err();
        let text = app.to_string();
        assert!(
            text.contains("toolchain-acls.json"),
            "names the file: {text}"
        );
        assert!(text.contains("Remedy"), "names the remedy: {text}");
        assert!(text.contains(".bad"), "names the move aside: {text}");
        assert_eq!(std::fs::read(&file).unwrap(), bytes, "never rewritten");
    }
    // Unreadable: a directory where the file should be.
    std::fs::remove_file(&file).unwrap();
    std::fs::create_dir(&file).unwrap();
    let error = Record::load(dir.path()).unwrap_err();
    assert!(
        matches!(error, super::RecordError::Unreadable { .. }),
        "{error:?}"
    );
    assert!(error.to_string().contains("toolchain-acls.json"));
    assert!(error.to_string().contains("Remedy"));
    assert!(file.is_dir(), "left as it was");
    std::fs::remove_dir(&file).unwrap();
    // A good record round-trips through the atomic save.
    let good = record(vec![
        entry("/a", Source::User),
        entry("/b", Source::Runtime),
    ]);
    good.save(dir.path()).unwrap();
    assert_eq!(Record::load(dir.path()).unwrap(), good);
}

/// REQ-CONF-008 (A-23): the record is written by a replace. The new content
/// takes the old one's place whole; no temporary file stays; and when a step
/// fails the record is what it was (here the temporary file cannot be made).
#[test]
fn req_conf_008_the_record_is_saved_by_a_replace() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("toolchain-acls.json");
    super::write_replacing(&target, b"first").unwrap();
    super::write_replacing(&target, b"second, longer than the first").unwrap();
    assert_eq!(
        std::fs::read(&target).unwrap(),
        b"second, longer than the first"
    );
    let names = |d: &std::path::Path| -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(d)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    };
    assert_eq!(
        names(dir.path()),
        ["toolchain-acls.json"],
        "no temporary file stays"
    );
    // The temporary file cannot be created: the old record is untouched.
    let temp = dir
        .path()
        .join(format!("toolchain-acls.json.tmp-{}", std::process::id()));
    std::fs::create_dir(&temp).unwrap();
    assert!(super::write_replacing(&target, b"third").is_err());
    assert_eq!(
        std::fs::read(&target).unwrap(),
        b"second, longer than the first"
    );
    std::fs::remove_dir(&temp).unwrap();
    // The rename fails (the target is a directory): the temporary file is removed.
    let blocked = dir.path().join("blocked");
    std::fs::create_dir(&blocked).unwrap();
    assert!(super::write_replacing(&blocked, b"x").is_err());
    assert!(blocked.is_dir());
    assert_eq!(names(dir.path()), ["blocked", "toolchain-acls.json"]);
}
