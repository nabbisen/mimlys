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

fn last(class: &str, at: &str) -> LastProbe {
    LastProbe {
        class: class.to_owned(),
        detail: None,
        steps: 3,
        model_ms_per_step: Some(900),
        transport: Some("tool_calls".into()),
        model: Some("m".into()),
        at: at.to_owned(),
    }
}

/// REQ-MDL-011: the record written is the record read: one entry per backend, the latest; another
/// backend's entry is kept as it is; the file is the shape the reader documents; and no temporary
/// file stays beside it.
#[test]
fn req_mdl_011_the_record_written_is_the_record_read_one_entry_per_backend() {
    let dir = tempfile::tempdir().unwrap();
    record(
        dir.path(),
        "default",
        last("acts", "2026-11-20T10:00:00.000Z"),
    )
    .unwrap();
    record(
        dir.path(),
        "small",
        last("plans_only", "2026-11-21T10:00:00.000Z"),
    )
    .unwrap();
    // The latest replaces the one before it, and only for its backend.
    record(
        dir.path(),
        "default",
        last("incomplete", "2026-11-22T10:00:00.000Z"),
    )
    .unwrap();
    let results = read(dir.path());
    assert_eq!(results.of("default").unwrap().class, "incomplete");
    assert_eq!(results.of("default").unwrap().date(), "2026-11-22");
    assert_eq!(results.of("small").unwrap().class, "plans_only");
    let value: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.path().join(FILE)).unwrap()).unwrap();
    assert_eq!(value["version"], 1);
    assert_eq!(value["backends"].as_object().unwrap().len(), 2);
    assert_eq!(value["backends"]["small"]["steps"], 3);
    let names: Vec<String> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, [FILE], "no temporary file is left: {names:?}");
}

/// REQ-MDL-011: an entry of a class this release does not know, written by a later one under the
/// same version, survives the rewrite of another backend's entry.
#[test]
fn req_mdl_011_an_entry_the_release_does_not_know_survives_another_backends_record() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        r#"{"version": 1, "backends": {"old": {"class": "reasons_aloud", "steps": 2, "at": "2026-01-01T00:00:00Z"}}}"#,
    );
    record(
        dir.path(),
        "default",
        last("acts", "2026-11-22T10:00:00.000Z"),
    )
    .unwrap();
    let results = read(dir.path());
    assert_eq!(results.of("old").unwrap().class, "reasons_aloud");
    assert_eq!(results.of("default").unwrap().class, "acts");
}

/// REQ-MDL-011: a record that cannot be read is never replaced: a writer does not repair what it
/// cannot read (a later release's file, or one that is not a record), and says what to do.
#[test]
fn req_mdl_011_a_record_that_cannot_be_read_is_not_replaced() {
    for text in [
        r#"{"version": 2, "backends": {}}"#,
        "not json at all",
        r#"["a", "list"]"#,
    ] {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), text);
        let error = record(
            dir.path(),
            "default",
            last("acts", "2026-11-22T10:00:00.000Z"),
        )
        .unwrap_err();
        assert!(matches!(error, RecordError::Unreadable(_)), "{error:?}");
        assert!(error.to_string().contains("move the file aside"), "{error}");
        assert_eq!(
            std::fs::read_to_string(dir.path().join(FILE)).unwrap(),
            text,
            "the file is as it was"
        );
        assert!(read_for_update(dir.path()).is_err());
    }
}

/// REQ-MDL-011: a temporary file a crash left is not in the way, and it goes.
#[test]
fn req_mdl_011_a_temporary_file_a_crash_left_goes() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(format!("{FILE}.tmp")), "half a rec").unwrap();
    record(
        dir.path(),
        "default",
        last("acts", "2026-11-22T10:00:00.000Z"),
    )
    .unwrap();
    assert!(!dir.path().join(format!("{FILE}.tmp")).exists());
    assert_eq!(read(dir.path()).of("default").unwrap().class, "acts");
}

/// REQ-SEC-008, REQ-MDL-011: the record is owner-only from its creation (Unix; on Windows it has
/// the data directory's protected entries, which CI checks for the data directory itself).
#[cfg(unix)]
#[test]
fn req_sec_008_the_record_is_owner_only() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    record(
        dir.path(),
        "default",
        last("acts", "2026-11-22T10:00:00.000Z"),
    )
    .unwrap();
    let mode = std::fs::metadata(dir.path().join(FILE))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600, "{mode:o}");
    // A second write keeps it so (the new file is renamed over the old).
    record(
        dir.path(),
        "small",
        last("acts", "2026-11-22T10:00:00.000Z"),
    )
    .unwrap();
    let mode = std::fs::metadata(dir.path().join(FILE))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600, "{mode:o}");
}
