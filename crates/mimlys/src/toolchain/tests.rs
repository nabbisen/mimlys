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
