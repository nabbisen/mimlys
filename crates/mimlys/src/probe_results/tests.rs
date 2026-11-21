use super::*;

fn write(dir: &Path, text: &str) {
    std::fs::write(dir.join(FILE), text).unwrap();
}

/// REQ-MDL-010: no file is no probe: nothing was probed, which is not an error.
#[test]
fn req_mdl_010_no_record_is_not_probed() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(read(dir.path()), Results::Read(BTreeMap::new()));
    assert_eq!(read(dir.path()).of("default"), None);
}

/// REQ-MDL-010, REQ-MDL-011: the record as the probe writes it is read, per backend, with the
/// day of its date; a field a reader does not need may be absent.
#[test]
fn req_mdl_010_the_record_is_read_per_backend() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        r#"{"version": 1, "backends": {
            "default": {"class": "acts", "steps": 4, "model_ms_per_step": 820,
                        "transport": "tool_calls", "model": "m", "at": "2026-11-22T10:15:00Z"},
            "small": {"class": "incomplete", "detail": "gave_up", "at": "2026-11-21T00:00:00Z"}
        }}"#,
    );
    let results = read(dir.path());
    let default = results.of("default").unwrap();
    assert_eq!(default.class, "acts");
    assert_eq!(default.steps, 4);
    assert_eq!(default.model_ms_per_step, Some(820));
    assert_eq!(default.date(), "2026-11-22");
    assert!(default.class_known());
    let small = results.of("small").unwrap();
    assert_eq!(small.detail.as_deref(), Some("gave_up"));
    assert_eq!(small.steps, 0);
    assert_eq!(results.of("other"), None);
}

/// REQ-MDL-010: every class the probe records is known; one this release does not know is
/// carried as it is, so that a reader never invents a class or drops a result.
#[test]
fn req_mdl_010_the_classes_are_the_five_and_an_unknown_one_is_kept() {
    for class in CLASSES {
        let probe = LastProbe {
            class: class.to_owned(),
            detail: None,
            steps: 1,
            model_ms_per_step: None,
            transport: None,
            model: None,
            at: "2026-11-22T00:00:00Z".into(),
        };
        assert!(probe.class_known(), "{class}");
    }
    let newer = LastProbe {
        class: "reasons_aloud".into(),
        detail: None,
        steps: 1,
        model_ms_per_step: None,
        transport: None,
        model: None,
        at: "not a date".into(),
    };
    assert!(!newer.class_known());
    assert_eq!(
        newer.date(),
        "not a date",
        "an instant without a date is shown as it is"
    );
}

/// REQ-MDL-010: a record that cannot be read is said, with the reason, and is never repaired or
/// read as empty: not JSON, not this shape, a newer version, a file that is not a file.
#[test]
fn req_mdl_010_a_record_that_cannot_be_read_is_said() {
    let dir = tempfile::tempdir().unwrap();
    for (text, reason) in [
        ("{not json", "is not a record of the fitness probe"),
        ("[]", "is not a record of the fitness probe"),
        (
            r#"{"version": 1, "backends": {"a": {"class": 3}}}"#,
            "is not a record",
        ),
        (
            r#"{"version": 2, "backends": {}}"#,
            "version 2, which this release does not read",
        ),
        (r#"{"backends": {}}"#, "is not a record"),
    ] {
        write(dir.path(), text);
        match read(dir.path()) {
            Results::Unreadable(why) => assert!(why.contains(reason), "{text}: {why}"),
            other => panic!("{text}: {other:?}"),
        }
        assert_eq!(read(dir.path()).of("a"), None);
        std::fs::remove_file(dir.path().join(FILE)).unwrap();
    }
    // A directory in its place: it cannot be read as a file.
    std::fs::create_dir(dir.path().join(FILE)).unwrap();
    assert!(matches!(read(dir.path()), Results::Unreadable(_)));
}
