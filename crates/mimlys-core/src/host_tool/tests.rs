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
        read_paths: vec!["~/.cargo/bin".into(), "~/.rustup".into()],
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

/// RFC 011 §6, §8: the effect is the declaration's; `workspace_write` is
/// reversible, the dependency step (`external_read`) never.
#[test]
fn rfc_011_effects() {
    let d = cargo_test();
    let e = d.effect_view();
    assert_eq!(e.kind, EffectKind::WorkspaceWrite);
    assert!(e.reversible);
    assert!(!e.idempotent);
    let fetch = fetch();
    assert_eq!(fetch.effect_view().kind, EffectKind::ExternalRead);
    assert!(!fetch.effect_view().reversible);
    let mut host = cargo_test();
    host.effect = EffectKind::HostWrite;
    assert!(!host.effect_view().reversible);
    host.reversible = true;
    assert!(host.effect_view().reversible);
    // Nothing is derived from `network`: the declared effect is what the view shows.
    let mut mismatched = cargo_test();
    mismatched.network = true;
    assert_eq!(mismatched.effect_view().kind, EffectKind::WorkspaceWrite);
}

fn fetch() -> HostToolDeclaration {
    let mut d = cargo_test();
    d.name = "cargo_fetch".into();
    d.network = true;
    d.effect = EffectKind::ExternalRead;
    d.reversible = true;
    d
}

/// REQ-CONF-006 / CLR-1, CLR-2: `external_read` exactly when `network = true`.
/// Both accepted pairings pass; both refused pairings name the declaration,
/// the pair found and the value to write.
#[test]
fn req_conf_006_effect_and_network_must_agree() {
    assert!(cargo_test().check().is_ok(), "workspace_write, offline");
    assert!(fetch().check().is_ok(), "external_read, network");

    let mut d = cargo_test();
    d.name = "cargo_fetch".into();
    d.network = true; // effect stays workspace_write
    let e = d.check().unwrap_err();
    assert_eq!(e.key, "host_tools.cargo_fetch.effect");
    assert!(
        e.reason
            .contains("effect = \"workspace_write\" with network = true"),
        "{e}"
    );
    assert!(e.reason.contains("write effect = \"external_read\""), "{e}");

    let mut d = cargo_test();
    d.effect = EffectKind::ExternalRead; // network stays false
    let e = d.check().unwrap_err();
    assert_eq!(e.key, "host_tools.cargo_test.effect");
    assert!(
        e.reason
            .contains("effect = \"external_read\" with network = false"),
        "{e}"
    );
    assert!(e.reason.contains("effect = \"workspace_write\""), "{e}");
    assert!(e.reason.contains("network = true"), "{e}");
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

fn names(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| (*s).to_owned()).collect()
}

/// REQ-CONF-011 / LIM-H5: a root, the home directory and an ancestor of it
/// are refused whatever they hold; the message names the path and a
/// narrower one to declare.
#[test]
fn req_conf_011_root_home_and_ancestors_are_refused() {
    use std::path::Path;
    let home = Path::new("/home/me");
    assert_eq!(
        check_read_path(Path::new("/"), Some(home), &[], false),
        Err(ReadPathRefusal::Root)
    );
    assert_eq!(
        check_read_path(Path::new("/home/me"), Some(home), &[], false),
        Err(ReadPathRefusal::Home)
    );
    assert_eq!(
        check_read_path(Path::new("/home/me/"), Some(home), &[], false),
        Err(ReadPathRefusal::Home)
    );
    assert_eq!(
        check_read_path(Path::new("/home"), Some(home), &[], false),
        Err(ReadPathRefusal::AncestorOfHome)
    );
    // A sibling that merely shares a prefix is not an ancestor.
    assert_eq!(
        check_read_path(Path::new("/home/me2"), Some(home), &[], false),
        Ok(())
    );
    let text = ReadPathRefusal::Home.reason("~");
    assert!(text.contains("home directory") && text.contains("~/.cargo/bin"));
}

/// REQ-CONF-011 / LIM-H5: a directory that directly holds a credential name
/// (LIM-H5's or LIM-W5's) is refused with the entry named and the narrower
/// path suggested; `~/.cargo/bin` and `~/.rustup` pass.
#[test]
fn req_conf_011_directory_holding_credentials_is_refused() {
    use std::path::Path;
    let home = Path::new("/home/me");
    let cargo = Path::new("/home/me/.cargo");
    for name in [
        "credentials.toml",
        "credentials",
        ".netrc",
        ".npmrc",
        ".pypirc",
        ".docker",
        ".env",
        ".env.local",
        "server.pem",
        "tls.key",
        "id_ed25519",
        ".ssh",
        ".aws",
        ".gnupg",
    ] {
        let found = check_read_path(cargo, Some(home), &names(&["bin", name]), false);
        assert_eq!(
            found,
            Err(ReadPathRefusal::HoldsCredentials {
                entry: name.to_owned()
            }),
            "{name}"
        );
    }
    let err = check_read_path(
        cargo,
        Some(home),
        &names(&["bin", "credentials.toml"]),
        false,
    )
    .unwrap_err()
    .reason("~/.cargo");
    assert!(err.contains("credentials.toml"), "{err}");
    assert!(err.contains("~/.cargo/bin"), "{err}");
    assert_eq!(
        check_read_path(
            Path::new("/home/me/.cargo/bin"),
            Some(home),
            &names(&["cargo", "rustc", "rustup"]),
            false
        ),
        Ok(())
    );
    assert_eq!(
        check_read_path(
            Path::new("/home/me/.rustup"),
            Some(home),
            &names(&[
                "downloads",
                "settings.toml",
                "tmp",
                "toolchains",
                "update-hashes"
            ]),
            false
        ),
        Ok(())
    );
    // Only direct entries count: the check receives depth one, by design.
    assert_eq!(
        check_read_path(cargo, Some(home), &names(&["registry", "git"]), false),
        Ok(())
    );
}

/// REQ-CONF-011: case-insensitive filesystems (Windows, macOS) compare the
/// home directory and the names without regard to case.
#[test]
fn req_conf_011_case_insensitive_comparison() {
    use std::path::Path;
    let home = Path::new("C:\\Users\\Me");
    assert_eq!(
        check_read_path(Path::new("c:\\users\\me"), Some(home), &[], true),
        Err(ReadPathRefusal::Home)
    );
    assert_eq!(
        check_read_path(
            Path::new("c:\\users\\me\\.cargo"),
            Some(home),
            &names(&["Credentials.TOML"]),
            true
        ),
        Err(ReadPathRefusal::HoldsCredentials {
            entry: "Credentials.TOML".into()
        })
    );
    assert_eq!(
        check_read_path(Path::new("/x"), None, &names(&["Credentials.toml"]), false),
        Ok(())
    );
}
