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
        allow_paths: false,
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
}

/// A declaration whose pattern admits anything: the rule on paths is the
/// declaration's `allow_paths`, never what the pattern admits.
fn permissive(allow_paths: bool) -> HostToolDeclaration {
    let mut d = cargo_test();
    d.allowed_args.clear();
    d.arg_pattern = Some(".+".into());
    d.max_args = 1;
    d.allow_paths = allow_paths;
    d
}

/// REQ-CONF-006, REQ-SEC-005 (A-34, scope of 0.11): a host tool's argument
/// names no path outside its workspace, on every platform and whatever the
/// pattern admits. The argument, each part between `=` signs and what follows
/// a short option's letter is refused when it holds a `..` component or
/// begins with `/`, `\`, `~`, a drive or `\\`.
#[test]
fn req_conf_006_a_host_tool_argument_names_no_path_outside_the_workspace() {
    let d = permissive(false);
    for refused in [
        "..",
        "../x",
        "a/../b",
        "a\\..\\b",
        "x/..",
        "/etc/passwd",
        "/",
        "\\x",
        "\\\\server\\share\\f",
        "~",
        "~/x",
        "~root/x",
        "C:\\x",
        "c:/x",
        "C:",
        "d:foo",
        // A part after `=`.
        "--manifest-path=../Cargo.toml",
        "--target-dir=/tmp/out",
        "--dir=~/x",
        "--out=C:\\x",
        "--a=b=/etc",
        "--x=--y=../z",
        "-I=/usr/include",
        // After a short option's letter.
        "-I/usr/include",
        "-I../x",
        "-o~/x",
        "-L\\\\srv\\x",
        "-Ic:\\x",
        "-o..",
    ] {
        let e = d.validate_args(&[refused.to_owned()]).unwrap_err();
        assert!(
            matches!(e, ArgViolation::NotAllowed { index: 0, .. }),
            "{refused:?}: {e}"
        );
    }
    for admitted in [
        "src/lib.rs",
        "a/b/c",
        "foo..bar",
        "1..5",
        "..a",
        "a..",
        "...",
        "my_test",
        "x::y",
        "a::b",
        "--lib",
        "--release",
        "-v",
        "-j4",
        "-Dwarnings",
        "-Itarget/include",
        "--target-dir=target",
        "--features=a,b",
        "--cfg=feature",
        "a~b",
        "--name=~",
    ] {
        // `--name=~` is a part that begins with `~`: refused; the rest pass.
        let expected_ok = admitted != "--name=~";
        assert_eq!(
            d.validate_args(&[admitted.to_owned()]).is_ok(),
            expected_ok,
            "{admitted:?}"
        );
    }
    // The fixed arguments are the administrator's: not looked at.
    let mut fixed = permissive(false);
    fixed.args = vec!["--manifest-path=/srv/project/Cargo.toml".into()];
    assert!(fixed.validate_args(&["x".into()]).is_ok());
    assert!(fixed.check().is_ok());
}

/// REQ-CONF-006, REQ-SEC-005 (A-34): `allow_paths = true` in the declaration
/// admits every refused form, and only what the pattern admits: a path the
/// pattern does not admit is still refused.
#[test]
fn req_conf_006_allow_paths_admits_what_the_pattern_admits() {
    let d = permissive(true);
    for path in [
        "../x",
        "/etc/passwd",
        "~/x",
        "C:\\x",
        "--manifest-path=../Cargo.toml",
        "-I/usr/include",
    ] {
        assert!(d.validate_args(&[path.to_owned()]).is_ok(), "{path}");
    }
    // The override of 0.10 and earlier is gone: a pattern that names `/` or `\.`
    // admits no path by itself (such a declaration fails `check`, below); only
    // `allow_paths` does.
    let mut named = cargo_test();
    named.arg_pattern = Some("[A-Za-z0-9_./-]+".into());
    assert!(named.validate_args(&["src/lib.rs".into()]).is_ok());
    for path in ["../x", "/etc/passwd", "a/../b"] {
        assert!(named.validate_args(&[path.to_owned()]).is_err(), "{path}");
    }
    named.allow_paths = true;
    assert!(named.validate_args(&["../x".into()]).is_ok());
    let mut narrow = cargo_test();
    narrow.allow_paths = true;
    assert!(narrow.validate_args(&["../x".into()]).is_err());
    assert!(narrow.validate_args(&["my_test".into()]).is_ok());
}

/// REQ-CONF-006 (A-34, the upgrade): a declaration that relied on the override
/// of 0.10 and earlier (an `arg_pattern` that names `/` or `\.`) fails `check`
/// with the key, the reason and the remedy, and passes with
/// `allow_paths = true`; a literal of `allowed_args` that names a path fails
/// the same way; a pattern that never named a path passes unchanged.
#[test]
fn req_conf_006_a_declaration_that_relied_on_the_override_fails_check() {
    for pattern in ["[A-Za-z0-9_./-]+", "src/.*", "[a-z]+\\.rs", "[^ ]*/[^ ]*"] {
        let mut d = cargo_test();
        d.arg_pattern = Some(pattern.into());
        let e = d.check().unwrap_err();
        assert_eq!(e.key, "host_tools.cargo_test.arg_pattern", "{pattern}");
        assert!(e.reason.contains("allow_paths = true"), "{}", e.reason);
        assert!(e.reason.contains("until 0.10"), "{}", e.reason);
        assert!(e.reason.contains(&format!("{pattern:?}")), "{}", e.reason);
        d.allow_paths = true;
        assert!(d.check().is_ok(), "{pattern} with allow_paths");
    }
    let mut literal = cargo_test();
    literal.allowed_args = vec!["--lib".into(), "--manifest-path=../Cargo.toml".into()];
    let e = literal.check().unwrap_err();
    assert_eq!(e.key, "host_tools.cargo_test.allowed_args");
    assert!(e.reason.contains("allow_paths = true"), "{}", e.reason);
    assert!(e.reason.contains("--manifest-path=../Cargo.toml"));
    literal.allow_paths = true;
    assert!(literal.check().is_ok());
    // Patterns that never named a path pass, as does the default declaration.
    assert!(cargo_test().check().is_ok());
    let mut dots = cargo_test();
    dots.arg_pattern = Some("[A-Za-z0-9_.-]+".into());
    assert!(dots.check().is_ok());
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
        check_read_path(Path::new("/"), Some(home), None, &[], false),
        Err(ReadPathRefusal::Root)
    );
    assert_eq!(
        check_read_path(Path::new("/home/me"), Some(home), None, &[], false),
        Err(ReadPathRefusal::Home)
    );
    assert_eq!(
        check_read_path(Path::new("/home/me/"), Some(home), None, &[], false),
        Err(ReadPathRefusal::Home)
    );
    assert_eq!(
        check_read_path(Path::new("/home"), Some(home), None, &[], false),
        Err(ReadPathRefusal::AncestorOfHome)
    );
    // A sibling that merely shares a prefix is not an ancestor.
    assert_eq!(
        check_read_path(Path::new("/home/me2"), Some(home), None, &[], false),
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
        let found = check_read_path(cargo, Some(home), None, &names(&["bin", name]), false);
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
        None,
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
            None,
            &names(&["cargo", "rustc", "rustup"]),
            false
        ),
        Ok(())
    );
    assert_eq!(
        check_read_path(
            Path::new("/home/me/.rustup"),
            Some(home),
            None,
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
        check_read_path(cargo, Some(home), None, &names(&["registry", "git"]), false),
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
        check_read_path(Path::new("c:\\users\\me"), Some(home), None, &[], true),
        Err(ReadPathRefusal::Home)
    );
    assert_eq!(
        check_read_path(
            Path::new("c:\\users\\me\\.cargo"),
            Some(home),
            None,
            &names(&["Credentials.TOML"]),
            true
        ),
        Err(ReadPathRefusal::HoldsCredentials {
            entry: "Credentials.TOML".into()
        })
    );
    assert_eq!(
        check_read_path(
            Path::new("/x"),
            None,
            None,
            &names(&["Credentials.toml"]),
            false
        ),
        Ok(())
    );
}

/// REQ-CONF-006, A-06 (handoff 011o-4): a read path that is the data
/// directory, lies inside it, or contains it is refused, naming the entry and
/// the data directory; a sibling that merely shares a prefix is not. The
/// reason is the same on every platform; only the comparison's case differs.
#[test]
fn req_conf_006_a_read_path_over_the_data_directory_is_refused() {
    use std::path::Path;
    let home = Some(Path::new("/home/me"));
    let data = Some(Path::new("/home/me/.local/share/mimlys"));
    let relation = |path: &str| match check_read_path(Path::new(path), home, data, &[], false) {
        Err(ReadPathRefusal::DataDirectory { relation, .. }) => Some(relation),
        other => {
            assert_eq!(other, Ok(()), "{path}");
            None
        }
    };
    assert_eq!(
        relation("/home/me/.local/share/mimlys"),
        Some(DataDirRelation::Is)
    );
    assert_eq!(
        relation("/home/me/.local/share/mimlys/"),
        Some(DataDirRelation::Is)
    );
    assert_eq!(
        relation("/home/me/.local/share/mimlys/workspaces/t/tree"),
        Some(DataDirRelation::Inside)
    );
    assert_eq!(
        relation("/home/me/.local/share"),
        Some(DataDirRelation::Contains)
    );
    assert_eq!(relation("/home/me/.local"), Some(DataDirRelation::Contains));
    // A sibling, a name that shares a prefix, and a toolchain are not.
    assert_eq!(relation("/home/me/.local/share/mimlys2"), None);
    assert_eq!(relation("/home/me/.local/share/other"), None);
    assert_eq!(relation("/home/me/.cargo/bin"), None);
    // No data directory known: nothing to compare.
    assert_eq!(
        check_read_path(Path::new("/home/me/.local/share"), home, None, &[], false),
        Ok(())
    );
    // The reason names the entry as written, the data directory, and what to declare.
    let text = check_read_path(Path::new("/home/me/.local/share"), home, data, &[], false)
        .unwrap_err()
        .reason("~/.local/share");
    assert!(text.contains("\"~/.local/share\""), "{text}");
    assert!(
        text.contains("contains the data directory \"/home/me/.local/share/mimlys\""),
        "{text}"
    );
    assert!(text.contains("~/.cargo/bin"), "{text}");
    for (path, words) in [
        ("/home/me/.local/share/mimlys", "is the data directory"),
        (
            "/home/me/.local/share/mimlys/tmp",
            "is inside the data directory",
        ),
    ] {
        let text = check_read_path(Path::new(path), home, data, &[], false)
            .unwrap_err()
            .reason(path);
        assert!(text.contains(words), "{text}");
    }
}

/// REQ-CONF-006: the comparison with the data directory is without regard to
/// case on Windows and macOS (`case_insensitive`), as the home directory's is.
#[test]
fn req_conf_006_the_data_directory_comparison_follows_the_case_rule() {
    use std::path::Path;
    let data = Some(Path::new("/Users/Me/Library/Application Support/mimlys"));
    for path in [
        "/users/me/library/application support/mimlys",
        "/USERS/ME/LIBRARY",
        "/users/me/library/application support/MIMLYS/workspaces",
    ] {
        assert!(
            matches!(
                check_read_path(Path::new(path), None, data, &[], true),
                Err(ReadPathRefusal::DataDirectory { .. })
            ),
            "{path}"
        );
    }
    // Case-sensitive: the same names in another case are other directories.
    assert_eq!(
        check_read_path(
            Path::new("/users/me/library/application support/mimlys"),
            None,
            data,
            &[],
            false
        ),
        Ok(())
    );
    // A sibling of the data directory is accepted.
    assert_eq!(
        check_read_path(
            Path::new("/users/me/library/application support/mimlys-tools"),
            None,
            data,
            &[],
            true
        ),
        Ok(())
    );
}

/// REQ-CONF-006 (no shell is involved): a command whose file is a batch
/// script, `.cmd` or `.bat` in any case, is refused by the declaration's own
/// check, on every platform, naming the key, the reason and the remedy.
#[test]
fn req_conf_006_a_batch_script_command_is_refused() {
    for command in [
        "x.cmd",
        "X.CMD",
        "run.Bat",
        "tools/build.bat",
        "C:\\tools\\build.cmd",
        "~/bin/x.cmd",
        "x.cmd.",
        "x.cmd ",
        ".cmd",
    ] {
        let mut d = cargo_test();
        d.command = command.into();
        let e = d.check().expect_err(command);
        assert_eq!(e.key, "host_tools.cargo_test.command", "{command}");
        assert!(
            e.reason.contains("no shell is involved")
                && e.reason.contains("REQ-CONF-006")
                && e.reason.contains("declare the program the script runs"),
            "{command}: {}",
            e.reason
        );
        assert!(is_batch_script(command), "{command}");
    }
    for command in [
        "cargo",
        "x.exe",
        "x.cmdx",
        "x.batch",
        "xcmd",
        "bat",
        "cmd",
        "dir.cmd/tool",
        "dir.bat\\tool",
        "~/.cargo/bin/cargo",
    ] {
        let mut d = cargo_test();
        d.command = command.into();
        d.check().expect(command);
        assert!(!is_batch_script(command), "{command}");
    }
}
