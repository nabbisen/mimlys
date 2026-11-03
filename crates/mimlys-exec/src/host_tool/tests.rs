//! Linux and macOS: Windows keeps its own `PATH` rule (external design §13.3).
#![cfg(unix)]

use super::*;
use std::collections::BTreeMap;

fn declaration(command: &str, read_paths: &[&Path]) -> HostToolDeclaration {
    HostToolDeclaration {
        name: "tool".into(),
        command: command.into(),
        args: vec![],
        allowed_args: vec![],
        arg_pattern: None,
        max_args: 0,
        effect: mimlys_core::action::EffectKind::WorkspaceWrite,
        reversible: true,
        timeout_ms: 10_000,
        read_paths: read_paths
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect(),
        env: BTreeMap::new(),
        env_allow: vec![],
        network: false,
    }
}

fn path_of(env: &[(String, String)]) -> String {
    env.iter()
        .find(|(k, _)| k == "PATH")
        .map(|(_, v)| v.clone())
        .expect("PATH is always set")
}

/// REQ-CONF-006 / external design §13.3, §13.5: the tool's `PATH` is built
/// from the declared read paths that are themselves a `bin` directory
/// (`~/.cargo/bin`) or hold one, then `/usr/bin:/bin`; a declaration's `env`
/// that sets `PATH` wins; a command given as a name resolves against the
/// same directories. (Linux and macOS: Windows keeps its own rule.)
#[test]
fn req_conf_006_path_is_built_from_declared_bin_paths() {
    let root = tempfile::tempdir().unwrap();
    // A declared `…/bin` itself, a declared parent that holds `bin`, and a
    // directory that is neither (a toolchain home with no `bin`).
    let cargo_bin = root.path().join("cargo/bin");
    let parent = root.path().join("sdk");
    let neither = root.path().join("rustup");
    std::fs::create_dir_all(&cargo_bin).unwrap();
    std::fs::create_dir_all(parent.join("bin")).unwrap();
    std::fs::create_dir_all(neither.join("toolchains")).unwrap();
    let tmp = root.path().join("tmp");

    let d = declaration("tool", &[&cargo_bin, &parent, &neither]);
    let expected = format!(
        "{}:{}:/usr/bin:/bin",
        cargo_bin.display(),
        parent.join("bin").display()
    );
    assert_eq!(path_of(&env_for(&d, &tmp)), expected);

    // Each case alone.
    assert_eq!(
        path_of(&env_for(&declaration("tool", &[&cargo_bin]), &tmp)),
        format!("{}:/usr/bin:/bin", cargo_bin.display()),
        "a declared bin directory is itself on PATH, not `bin/bin`"
    );
    assert_eq!(
        path_of(&env_for(&declaration("tool", &[&parent]), &tmp)),
        format!("{}:/usr/bin:/bin", parent.join("bin").display()),
        "a declared parent contributes its `bin`"
    );
    assert_eq!(
        path_of(&env_for(&declaration("tool", &[&neither]), &tmp)),
        "/usr/bin:/bin",
        "a directory that is neither adds nothing"
    );

    // The declaration's own PATH wins over the built one.
    let mut own = declaration("tool", &[&cargo_bin]);
    own.env.insert("PATH".into(), "/custom/bin:/usr/bin".into());
    let env = env_for(&own, &tmp);
    assert_eq!(path_of(&env), "/custom/bin:/usr/bin");
    assert_eq!(
        env.iter().filter(|(k, _)| k == "PATH").count(),
        1,
        "set once"
    );

    // A command given as a name resolves against the same directories.
    std::fs::write(cargo_bin.join("cargo"), "#!/bin/sh\n").unwrap();
    std::fs::write(parent.join("bin/helper"), "#!/bin/sh\n").unwrap();
    let both = declaration("cargo", &[&cargo_bin, &parent, &neither]);
    assert_eq!(resolve_command(&both).unwrap(), cargo_bin.join("cargo"));
    let helper = declaration("helper", &[&cargo_bin, &parent, &neither]);
    assert_eq!(resolve_command(&helper).unwrap(), parent.join("bin/helper"));
    let missing = declaration("absent", &[&cargo_bin, &parent, &neither]);
    assert!(
        resolve_command(&missing)
            .unwrap_err()
            .contains("no declared bin directory")
    );
}

/// The rule itself, without a filesystem: only a directory that is named
/// `bin`, or that the test says holds one, contributes.
#[test]
fn req_conf_006_bin_dirs_rule_without_a_filesystem() {
    let held = Path::new("/h/sdk/bin");
    let dirs = bin_dirs_with(
        &["/h/.cargo/bin".into(), "/h/sdk".into(), "/h/.rustup".into()],
        |d| d == held,
    );
    assert_eq!(
        dirs,
        vec![PathBuf::from("/h/.cargo/bin"), PathBuf::from("/h/sdk/bin")]
    );
}
