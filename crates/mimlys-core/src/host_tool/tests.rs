use super::*;

fn cargo_test() -> HostToolDeclaration {
    HostToolDeclaration {
        name: "cargo_test".into(),
        command: "cargo".into(),
        args: vec!["test".into(), "--offline".into()],
        allowed_args: vec!["--lib".into(), "--release".into()],
        arg_pattern: Some("[A-Za-z0-9_:]+".into()),
        max_args: 3,
        effect: EffectKind::WorkspaceWrite,
        reversible: false,
        timeout_ms: 600_000,
        read_paths: vec!["~/.cargo".into(), "~/.rustup".into()],
        env: BTreeMap::new(),
        env_allow: vec![],
        network: false,
    }
}

/// REQ-CONF-006 / RFC 011 §6: literals and pattern matches pass; a path,
/// `..`, an unlisted flag and too many arguments are refused with the
/// position.
#[test]
fn req_conf_006_argument_validation() {
    let d = cargo_test();
    assert_eq!(
        d.validate_args(&["--lib".into(), "my_test".into()])
            .unwrap(),
        vec!["--lib".to_owned(), "my_test".to_owned()]
    );
    assert_eq!(
        d.command_line(&["--lib".into()]),
        vec!["cargo", "test", "--offline", "--lib"]
    );
    for bad in [
        "--all-features",
        "../x",
        "/etc/passwd",
        "a b",
        "-- --nocapture",
    ] {
        let e = d.validate_args(&[bad.to_owned()]).unwrap_err();
        assert!(
            matches!(e, ArgViolation::NotAllowed { index: 0, .. }),
            "{bad}: {e}"
        );
    }
    let e = d
        .validate_args(&["a".into(), "b".into(), "c".into(), "d".into()])
        .unwrap_err();
    assert_eq!(e, ArgViolation::TooMany { max: 3 });
    let mut none = cargo_test();
    none.allowed_args.clear();
    none.arg_pattern = None;
    none.max_args = 0;
    assert!(none.validate_args(&[]).is_ok());
    assert!(none.validate_args(&["--lib".into()]).is_err());
    // A pattern that names `/` allows relative paths but still not `..`.
    let mut paths = cargo_test();
    paths.arg_pattern = Some("[A-Za-z0-9_./-]+".into());
    assert!(paths.validate_args(&["src/lib.rs".into()]).is_ok());
    assert!(
        paths.validate_args(&["../x".into()]).is_ok(),
        "the pattern allows dots; the workspace boundary is the worker's"
    );
}

/// RFC 011 §6, §8: the effect is the declaration's, `workspace_write`
/// reversible; the dependency step is `external_read`.
#[test]
fn rfc_011_effects() {
    let d = cargo_test();
    let e = d.effect_view();
    assert_eq!(e.kind, EffectKind::WorkspaceWrite);
    assert!(e.reversible);
    assert!(!e.idempotent);
    let mut fetch = cargo_test();
    fetch.network = true;
    assert_eq!(fetch.effect_view().kind, EffectKind::ExternalRead);
    let mut host = cargo_test();
    host.effect = EffectKind::HostWrite;
    assert!(!host.effect_view().reversible);
    host.reversible = true;
    assert!(host.effect_view().reversible);
}

/// REQ-CFG-001: a declaration's own rules name the key.
#[test]
fn req_cfg_001_declaration_rules() {
    assert!(cargo_test().check().is_ok());
    let mut d = cargo_test();
    d.name = "Cargo".into();
    assert!(d.check().unwrap_err().key.ends_with(".name"));
    let mut d = cargo_test();
    d.command = " ".into();
    assert!(d.check().unwrap_err().key.ends_with(".command"));
    let mut d = cargo_test();
    d.effect = EffectKind::ExternalWrite;
    assert!(d.check().unwrap_err().key.ends_with(".effect"));
    let mut d = cargo_test();
    d.arg_pattern = Some("(".into());
    assert!(d.check().unwrap_err().key.ends_with(".arg_pattern"));
    let mut d = cargo_test();
    d.max_args = 0;
    assert!(d.check().unwrap_err().key.ends_with(".max_args"));
}
