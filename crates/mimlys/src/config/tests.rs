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
