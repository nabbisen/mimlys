use super::*;
use mimlys_core::limits::{LIM_T1_STEPS, LIM_W4_MATERIALIZE_EXCLUSIONS};

/// REQ-CFG-002: no file means the fake backend, the `workspace` profile and
/// every default from the limits register.
#[test]
fn req_cfg_002_defaults_from_limits() {
    let loaded = Config::load(None).unwrap();
    assert_eq!(loaded.path, None);
    let c = loaded.config;
    assert_eq!(c.backend.kind, BackendKind::Fake);
    assert_eq!(c.backend.timeout_ms, LIM_M1_REQUEST_MS);
    assert_eq!(c.backend.max_tokens, LIM_M2_MAX_TOKENS);
    assert_eq!(c.workspace.max_bytes, LIM_W1_WORKSPACE_BYTES);
    let policy = c.policy.resolve().unwrap();
    assert_eq!(policy.default_profile, "workspace");
    assert_eq!(
        policy.profile("workspace").unwrap().budgets.steps,
        LIM_T1_STEPS
    );
    assert_eq!(policy.exclusions.materialize, LIM_W4_MATERIALIZE_EXCLUSIONS);
    assert_eq!(policy, Policy::shipped());
}

/// REQ-WS-001 (external design §14.1): with no `runtime.data_dir` the data
/// directory, and so every workspace under it, is the platform's per-user
/// data directory plus `mimlys`, never a temporary directory.
#[test]
fn req_ws_001_default_data_dir_is_not_temporary() {
    let data_dir = Config::load(None).unwrap().config.data_dir().unwrap();
    let expected = directories::BaseDirs::new()
        .expect("a home directory")
        .data_local_dir()
        .join("mimlys");
    assert_eq!(data_dir, expected);
    assert!(
        !data_dir.starts_with(std::env::temp_dir()),
        "{} is under the temporary directory {}",
        data_dir.display(),
        std::env::temp_dir().display()
    );
}

/// REQ-CFG-001: an unknown key is refused by name, at every level.
#[test]
fn req_cfg_001_unknown_keys_named() {
    let err = Config::parse("config_version = 1\nspeed = 3\n").unwrap_err();
    assert!(err.reason.contains("speed"), "{err}");
    let err =
        Config::parse("config_version = 1\n[backend]\nkind = \"fake\"\ncolour = 1\n").unwrap_err();
    assert!(err.reason.contains("colour"), "{err}");
    let err =
        Config::parse("config_version = 1\n[policy.profiles.workspace]\nstep = 3\n").unwrap_err();
    assert!(err.reason.contains("step"), "{err}");
}

/// REQ-CFG-001: the version must be 1.
#[test]
fn req_cfg_001_version() {
    let c = Config::parse("config_version = 2\n").unwrap();
    let err = c.validate().unwrap_err();
    assert_eq!(err.key, "config_version");
    assert!(
        Config::parse("[backend]\nkind = \"fake\"\n").is_err(),
        "config_version is required"
    );
}

/// REQ-CFG-003: secrets by reference only; the key is named.
#[test]
fn req_cfg_003_secrets_by_reference() {
    let inline = "config_version = 1\n[backend]\nkind = \"openai_compatible\"\nurl = \"http://x\"\nmodel = \"m\"\napi_key = \"sk-abc\"\n";
    let err = Config::parse(inline).unwrap_err();
    assert!(err.reason.contains("env:NAME"), "{err}");
    assert!(
        !err.reason.contains("sk-abc"),
        "the inline value is never echoed"
    );
    let ok = inline.replace("sk-abc", "env:MIMLYS_BACKEND_KEY");
    let c = Config::parse(&ok).unwrap();
    assert_eq!(
        c.backend.api_key,
        Some(SecretRef::Env("MIMLYS_BACKEND_KEY".into()))
    );
    c.validate().unwrap();
    assert!(c.to_toml().contains("api_key = \"env:MIMLYS_BACKEND_KEY\""));
    assert_eq!(
        SecretRef::parse("file:/run/secrets/key").unwrap(),
        SecretRef::File("/run/secrets/key".into())
    );
    assert!(SecretRef::parse("env:").is_err());
}

/// REQ-CFG-001 / CLR-3: a value outside its range names the key and the range.
#[test]
fn req_cfg_001_ranges_stated() {
    let c = Config::parse("config_version = 1\n[backend]\ntimeout_ms = 5\n").unwrap();
    let err = c.validate().unwrap_err();
    assert_eq!(err.key, "backend.timeout_ms");
    assert!(err.reason.contains("10000..=3600000"), "{err}");
    let c =
        Config::parse("config_version = 1\n[policy.profiles.workspace]\nsteps = 5000\n").unwrap();
    let err = c.validate().unwrap_err();
    assert!(err.key.starts_with("policy.profiles.workspace"), "{err}");
    let c = Config::parse("config_version = 1\n[workspace]\nmax_bytes = 1\n").unwrap();
    assert_eq!(c.validate().unwrap_err().key, "workspace.max_bytes");
    let c = Config::parse("config_version = 1\n[logging]\nlevel = \"loud\"\n").unwrap();
    assert_eq!(c.validate().unwrap_err().key, "logging.level");
}

/// REQ-SEC-010: a remote backend needs the administrator's permission.
#[test]
fn req_sec_010_remote_needs_permission() {
    let text = "config_version = 1\n[backend]\nkind = \"openai_compatible\"\nurl = \"https://api\"\nmodel = \"m\"\nlocality = \"remote\"\n";
    let err = Config::parse(text).unwrap().validate().unwrap_err();
    assert_eq!(err.key, "backend.locality");
    let allowed = format!("{text}[policy]\nremote_backend_allowed = true\n");
    Config::parse(&allowed).unwrap().validate().unwrap();
}

/// REQ-CFG-004: profile overlays keep the shipped values they do not name;
/// a new profile must list its tools; tools outside the ceiling are refused.
#[test]
fn req_cfg_004_policy_overlay() {
    let text = "config_version = 1\n[policy.profiles.workspace]\napproval_tools = [\"write_file\"]\nsteps = 10\n[policy.profiles.audit]\ntools = [\"read_file\"]\non_reject = \"halt\"\n";
    let policy = Config::parse(text).unwrap().policy.resolve().unwrap();
    let ws = policy.profile("workspace").unwrap();
    assert_eq!(ws.approval_tools, ["write_file"]);
    assert_eq!(ws.budgets.steps, 10);
    assert_eq!(
        ws.tools,
        Policy::shipped().profile("workspace").unwrap().tools
    );
    let audit = policy.profile("audit").unwrap();
    assert_eq!(audit.tools, ["read_file"]);
    assert_eq!(audit.on_reject, OnReject::Halt);
    let err = Config::parse("config_version = 1\n[policy.profiles.audit]\nsteps = 3\n")
        .unwrap()
        .validate()
        .unwrap_err();
    assert_eq!(err.key, "policy.profiles.audit.tools");
    let err =
        Config::parse("config_version = 1\n[policy.profiles.workspace]\ntools = [\"rm_rf\"]\n")
            .unwrap()
            .validate()
            .unwrap_err();
    assert!(
        err.key.starts_with("policy.profiles.workspace.tools"),
        "{err}"
    );
}

/// `mimlys check`: the printed configuration parses back to the same value.
#[test]
fn check_output_round_trips() {
    let c = Config::parse("config_version = 1\n[runtime]\ndata_dir = \"/tmp/m\"\n[policy.profiles.workspace]\nsteps = 7\n").unwrap();
    let again = Config::parse(&c.to_toml()).unwrap();
    assert_eq!(again, c);
}

/// REQ-CFG-001: the file location order.
#[test]
fn req_cfg_001_location_order() {
    let explicit = PathBuf::from("/explicit/config.toml");
    assert_eq!(Config::locate(Some(&explicit)), Some(explicit));
}

/// RFC 007: `context_chars` defaults to LIM-C1 and keeps its range; a
/// resolved secret shorter than eight characters is refused by name.
#[test]
fn rfc_007_context_chars_and_short_secret() {
    let c = Config::load(None).unwrap().config;
    assert_eq!(
        c.backend.context_chars,
        mimlys_core::limits::LIM_C1_CONTEXT_CHARS
    );
    let err = Config::parse(
        "config_version = 1\n[backend]\nkind = \"openai_compatible\"\nurl = \"http://127.0.0.1:8080/v1\"\nmodel = \"m\"\ncontext_chars = 10\n",
    )
    .unwrap()
    .validate()
    .unwrap_err();
    assert_eq!(err.key, "backend.context_chars");
    assert!(err.reason.contains("4000"), "{err}");

    let dir = tempfile::tempdir().unwrap();
    let key = dir.path().join("key");
    std::fs::write(&key, "short\n").unwrap();
    // A literal string: on Windows the path holds backslashes.
    let text = format!(
        "config_version = 1\n[backend]\nkind = \"openai_compatible\"\nurl = \"http://127.0.0.1:8080/v1\"\nmodel = \"m\"\napi_key = 'file:{}'\n",
        key.display()
    );
    let err = Config::parse(&text).unwrap().resolve_secrets().unwrap_err();
    assert_eq!(err.key, "backend.api_key");
    assert!(err.reason.contains("8 characters"), "{err}");
    std::fs::write(&key, "long-enough-key\n").unwrap();
    let secrets = Config::parse(&text).unwrap().resolve_secrets().unwrap();
    assert_eq!(
        secrets,
        vec![("backend.api_key".to_owned(), "long-enough-key".to_owned())]
    );
}

/// RFC 008: `temperature` defaults to 0 within 0–2; a profile's `external`
/// overlays and defaults to false; every shipped profile has it false.
#[test]
fn rfc_008_temperature_and_external() {
    let c = Config::load(None).unwrap().config;
    assert_eq!(c.backend.temperature, 0.0);
    let policy = c.policy.resolve().unwrap();
    assert!(policy.profiles.values().all(|p| !p.external));
    let err = Config::parse(
        "config_version = 1\n[backend]\nkind = \"openai_compatible\"\nurl = \"http://127.0.0.1:8080/v1\"\nmodel = \"m\"\ntemperature = 2.5\n",
    )
    .unwrap()
    .validate()
    .unwrap_err();
    assert_eq!(err.key, "backend.temperature");
    assert!(err.reason.contains("0.0–2.0"), "{err}");
    let c = Config::parse(
        "config_version = 1\n[policy]\nremote_backend_allowed = true\n[policy.profiles.workspace]\nexternal = true\n[policy.profiles.open]\ntools = [\"read_file\"]\nexternal = true\n",
    )
    .unwrap();
    c.validate().unwrap();
    let policy = c.policy.resolve().unwrap();
    assert!(policy.profiles["workspace"].external);
    assert!(policy.profiles["open"].external);
    assert!(!policy.profiles["read_only"].external);
}

fn with_host_tool(body: &str) -> Config {
    Config::parse(&format!(
        "config_version = 1\n[host_tools.tool]\ncommand = \"/bin/true\"\n{body}"
    ))
    .unwrap()
}

/// REQ-CONF-006 / CLR-1, CLR-2: `effect` and `network` must agree. The
/// refusal is a configuration error naming the declaration, the pair found
/// and the value to write; both accepted pairings load.
#[test]
fn req_conf_006_effect_and_network_must_agree() {
    // Accepted: offline workspace_write; the dependency step as external_read.
    with_host_tool("effect = \"workspace_write\"\n")
        .validate()
        .unwrap();
    with_host_tool("effect = \"external_read\"\nnetwork = true\n")
        .validate()
        .unwrap();
    // Refused: network with workspace_write.
    let err = with_host_tool("effect = \"workspace_write\"\nnetwork = true\n")
        .validate()
        .unwrap_err();
    assert_eq!(err.key, "host_tools.tool.effect");
    assert!(
        err.reason
            .contains("effect = \"workspace_write\" with network = true"),
        "{err}"
    );
    assert!(err.reason.contains("effect = \"external_read\""), "{err}");
    // Refused: external_read without network.
    let err = with_host_tool("effect = \"external_read\"\n")
        .validate()
        .unwrap_err();
    assert_eq!(err.key, "host_tools.tool.effect");
    assert!(
        err.reason
            .contains("effect = \"external_read\" with network = false"),
        "{err}"
    );
    assert!(err.reason.contains("workspace_write"), "{err}");
    // The effect vocabulary stays closed.
    assert!(
        Config::parse(
            "config_version = 1\n[host_tools.t]\ncommand = \"x\"\neffect = \"host_write\"\n"
        )
        .is_err()
    );
}

/// REQ-CONF-011 / LIM-H5: a declared read path that is a filesystem root,
/// the home directory, an ancestor of it, or a directory directly holding
/// credentials is a configuration error at load, naming the declaration, the
/// path as written and the narrower path; a directory without them loads.
#[test]
fn req_conf_011_declaration_with_credentials_is_refused() {
    let toolchain = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(toolchain.path().join("bin")).unwrap();
    std::fs::write(toolchain.path().join("credentials.toml"), "token = \"x\"\n").unwrap();
    let declare = |path: &str| {
        with_host_tool(&format!(
            "effect = \"workspace_write\"\nread_paths = [{path:?}]\n"
        ))
        .validate()
    };
    // The directory beside the credentials file is refused; its `bin` is not.
    let beside = toolchain.path().to_string_lossy().into_owned();
    let err = declare(&beside).unwrap_err();
    assert_eq!(err.key, "host_tools.tool.read_paths");
    assert!(err.reason.contains("credentials.toml"), "{err}");
    assert!(err.reason.contains(&format!("{beside}/bin")), "{err}");
    declare(&format!("{beside}/bin")).unwrap();
    // The real home directory and its ancestors, and the root.
    for path in ["~", "/"] {
        let err = declare(path).unwrap_err();
        assert!(err.reason.contains("REQ-CONF-011"), "{path}: {err}");
    }
    let home = directories::BaseDirs::new()
        .unwrap()
        .home_dir()
        .to_path_buf();
    if let Some(parent) = home.parent() {
        let err = declare(&parent.to_string_lossy()).unwrap_err();
        assert!(err.reason.contains("contains the home directory"), "{err}");
    }
}

/// REQ-CONF-011: `toolchain grant` and the declarations share one decision,
/// and the home directory it compares against is given (not the machine's).
#[test]
fn req_conf_011_one_function_decides_for_grant_and_declaration() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    let cargo = home.join(".cargo");
    std::fs::create_dir_all(cargo.join("bin")).unwrap();
    std::fs::write(cargo.join("credentials.toml"), "x").unwrap();
    let shown = cargo.to_string_lossy().into_owned();
    let refused = crate::toolchain::read_path_refusal_with(&shown, Some(&home)).unwrap();
    assert!(
        refused.contains("credentials.toml") && refused.contains("/bin"),
        "{refused}"
    );
    assert!(
        crate::toolchain::read_path_refusal_with(&cargo.join("bin").to_string_lossy(), Some(&home))
            .is_none()
    );
    assert!(
        crate::toolchain::read_path_refusal_with(&home.to_string_lossy(), Some(&home))
            .unwrap()
            .contains("home directory")
    );
    assert!(
        crate::toolchain::read_path_refusal_with(&root.path().to_string_lossy(), Some(&home))
            .unwrap()
            .contains("contains the home directory")
    );
}

/// REQ-WS-001: a configured `runtime.data_dir` is inside the temporary
/// directory when it is the directory, a directory beneath it that exists, or
/// one that does not exist yet; a sibling whose name only begins like it, and
/// the default (unconfigured) location, are not. The comparison is between
/// canonical paths, so a spelling through `..` is the same as the plain one.
#[test]
fn req_ws_001_data_dir_in_temp_dir() {
    let with = |dir: &std::path::Path| {
        Config::parse(&format!(
            "config_version = 1\n[runtime]\ndata_dir = {:?}\n",
            dir.to_string_lossy()
        ))
        .unwrap()
    };
    let temp = std::env::temp_dir();
    let made = tempfile::tempdir().unwrap();
    assert!(with(&temp).data_dir_in_temp_dir());
    assert!(with(made.path()).data_dir_in_temp_dir());
    assert!(with(&made.path().join("not/yet/there")).data_dir_in_temp_dir());
    assert!(with(&made.path().join("a/../b")).data_dir_in_temp_dir());
    let canonical_temp = temp.canonicalize().unwrap();
    let lookalike = {
        let mut name = canonical_temp.file_name().unwrap().to_owned();
        name.push("-not-the-temporary-directory");
        canonical_temp.with_file_name(name).join("data")
    };
    assert!(
        !with(&lookalike).data_dir_in_temp_dir(),
        "{}",
        lookalike.display()
    );
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    if !manifest
        .canonicalize()
        .unwrap()
        .starts_with(&canonical_temp)
    {
        assert!(!with(&manifest.join("data")).data_dir_in_temp_dir());
        // A link inside the temporary directory that leads out of it: the
        // directory is not in it (the paths are compared canonical).
        #[cfg(unix)]
        {
            let out = made.path().join("out");
            std::os::unix::fs::symlink(manifest, &out).unwrap();
            assert!(!with(&out.join("data")).data_dir_in_temp_dir());
        }
    }
    assert!(
        !Config::load(None).unwrap().config.data_dir_in_temp_dir(),
        "the default location is not configured, so it is not in question"
    );
}

/// REQ-CONF-011 / external design §13.5: a `command` whose canonical target,
/// after its links, leaves the read paths is a configuration error at load,
/// on every platform, naming the declaration, the resolved path and the
/// directory to declare (Homebrew's `rustup` links into
/// `/opt/homebrew/Cellar/…`). Declaring that directory loads; an absolute
/// command outside every read path is refused the same way.
#[cfg(unix)]
#[test]
fn req_conf_011_command_that_links_out_of_the_read_paths_is_refused() {
    let root = tempfile::tempdir().unwrap();
    let real = root.path().join("cellar/bin");
    let shim = root.path().join("shim/bin");
    std::fs::create_dir_all(&real).unwrap();
    std::fs::create_dir_all(&shim).unwrap();
    std::fs::write(real.join("rustup"), "#!/bin/sh\n").unwrap();
    std::os::unix::fs::symlink(real.join("rustup"), shim.join("cargo")).unwrap();
    let canonical_real = real.canonicalize().unwrap();
    let declare = |command: &str, read_paths: &[&Path]| {
        let paths: Vec<String> = read_paths
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        Config::parse(&format!(
            "config_version = 1\n[host_tools.tool]\ncommand = {command:?}\neffect = \"workspace_write\"\nread_paths = {paths:?}\n"
        ))
        .unwrap()
        .validate()
    };
    // A name resolved against the shim directory: its target is elsewhere.
    let err = declare("cargo", &[&shim]).unwrap_err();
    assert_eq!(err.key, "host_tools.tool.command");
    assert!(
        err.reason
            .contains(&canonical_real.join("rustup").to_string_lossy().into_owned()),
        "names the resolved path: {err}"
    );
    assert!(
        err.reason.contains(&format!(
            "declare {} in read_paths",
            canonical_real.to_string_lossy()
        )),
        "names the directory to declare: {err}"
    );
    // Declaring the directory the target lives in loads.
    declare("cargo", &[&shim, &real]).unwrap();
    // An absolute command is judged the same way.
    let absolute = shim.join("cargo").to_string_lossy().into_owned();
    declare(&absolute, &[&shim]).unwrap_err();
    declare(&absolute, &[&real]).unwrap();
    // A command that cannot be resolved here is not refused at load: the run says so.
    declare("no-such-tool", &[&shim]).unwrap();
}

/// REQ-CONF-011: the decision itself (pure), on spellings of one platform:
/// under a read path or equal to one is inside; a sibling with the same
/// prefix, or the parent, is not.
#[test]
fn req_conf_011_command_decision_is_by_path_components() {
    use crate::toolchain::command_leaves_read_paths;
    let program = Path::new("/home/u/bin/cargo");
    let allowed = vec!["/home/u/.rustup".to_owned(), "/usr".to_owned()];
    assert!(
        command_leaves_read_paths(program, Path::new("/home/u/.rustup/bin/cargo"), &allowed)
            .is_none()
    );
    assert!(command_leaves_read_paths(program, Path::new("/usr"), &allowed).is_none());
    for outside in [
        "/home/u/.rustup-other/bin/cargo",
        "/home/u",
        "/usr2/bin/cargo",
    ] {
        let why = command_leaves_read_paths(program, Path::new(outside), &allowed)
            .unwrap_or_else(|| panic!("{outside} is outside"));
        assert!(why.contains(outside), "{why}");
    }
}
