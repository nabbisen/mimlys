use super::*;
use mimlys_core::grant::GrantKind;

/// A version-2 configuration: `default` and `small` local, `cloud` remote on a host of its own
/// (with the permission), and the url of `cloud` carrying a password that must never be shown.
fn config() -> Config {
    let text = r#"config_version = 2
[policy]
remote_backend_allowed = true
[backends.small]
kind = "openai_compatible"
url = "http://127.0.0.1:11434/v1"
model = "s"
[backends.default]
kind = "openai_compatible"
url = "http://127.0.0.1:8080/v1"
model = "d"
[backends.cloud]
kind = "openai_compatible"
url = "https://user:hunter2hunter2@api.example.org/v1"
model = "c"
locality = "remote"
"#;
    let config = Config::parse(text).unwrap();
    config.validate().unwrap();
    config
}

/// REQ-MDL-011: without a name the local backends are probed, `default` first and then by name,
/// and the remote one is left out, named with its host and never its password.
#[test]
fn req_mdl_011_without_a_name_only_the_local_backends_are_probed() {
    let plan = plan(&config(), None).unwrap();
    assert_eq!(plan.probe, ["default", "small"]);
    assert_eq!(plan.remote_named, None);
    assert_eq!(
        plan.left_out,
        [LeftOut {
            backend: "cloud".into(),
            host: Some("api.example.org".into()),
        }]
    );
    let lines = crate::messages::probe_left_out(&plan.left_out);
    assert_eq!(lines.len(), 1);
    assert!(
        lines[0].contains("`cloud`")
            && lines[0].contains("api.example.org")
            && lines[0].contains("mimlys model probe cloud"),
        "{}",
        lines[0]
    );
    assert!(!lines[0].contains("hunter2"), "{}", lines[0]);
}

/// REQ-MDL-011, THR-40: a remote backend is probed when it is named and the administrator allowed
/// remote backends, and then it is the one to be said first to send a fixed mini-task and nothing
/// of the user's; a local one named is probed without that.
#[test]
fn req_mdl_011_a_remote_backend_is_probed_only_when_named() {
    let named = plan(&config(), Some("cloud")).unwrap();
    assert_eq!(named.probe, ["cloud"]);
    assert_eq!(
        named.remote_named.as_ref().map(|r| r.backend.as_str()),
        Some("cloud")
    );
    assert!(named.left_out.is_empty());
    let notice = crate::messages::probe_remote_notice(
        "cloud",
        named.remote_named.as_ref().and_then(|r| r.host.as_deref()),
    );
    assert!(
        notice.contains("fixed mini-task") && notice.contains("nothing of yours"),
        "{notice}"
    );
    assert!(!notice.contains("hunter2"), "{notice}");
    let local = plan(&config(), Some("small")).unwrap();
    assert_eq!(
        (local.probe, local.remote_named),
        (vec!["small".into()], None)
    );
}

/// REQ-MDL-011, REQ-SEC-010: a remote backend named without `policy.remote_backend_allowed` is
/// refused, whatever else the configuration says (the load refuses such a file already; this is
/// the probe asking again, because it is the probe that sends).
#[test]
fn req_mdl_011_a_remote_backend_named_without_the_permission_is_refused() {
    let mut config = config();
    config.policy.remote_backend_allowed = false;
    let error = plan(&config, Some("cloud")).unwrap_err().to_string();
    assert!(
        error.contains("`cloud`") && error.contains("remote_backend_allowed"),
        "{error}"
    );
    // Without a name nothing is refused, and nothing is sent to it: it is left out.
    assert_eq!(plan(&config, None).unwrap().probe, ["default", "small"]);
}

/// REQ-CFG-007: a name the configuration does not have is the error that lists the ones it has.
#[test]
fn req_mdl_011_an_unknown_backend_names_the_configured_ones() {
    let error = plan(&config(), Some("nope")).unwrap_err().to_string();
    assert!(
        error.contains("`nope`") && error.contains("`default`") && error.contains("`cloud`"),
        "{error}"
    );
}

/// REQ-MDL-011: no local backend, nothing to probe without a name.
#[test]
fn req_mdl_011_no_local_backend_is_nothing_to_probe() {
    let mut config = config();
    config.backends.retain(|name, _| name == "cloud");
    let plan = plan(&config, None).unwrap();
    assert!(plan.probe.is_empty());
    assert_eq!(plan.left_out.len(), 1);
}

/// THR-40, REQ-MDL-011: the probe's profile grants three built-in tools and nothing else: no
/// promotion, no host tool, no approval to ask for, the steps of LIM-E2; the policy of its runtime
/// has that one profile and a ceiling that permits those tools, no host root, no network domain
/// and no host tool.
#[test]
fn thr_40_the_probe_profile_grants_three_builtin_tools_and_nothing_else() {
    let granted = profile(false);
    assert_eq!(
        granted.tools,
        ["read_file", "write_file", "replace_in_file"]
    );
    assert!(granted.host_tools.is_empty());
    assert!(!granted.promote, "no promotion");
    assert!(
        !granted.external,
        "a local backend: no external communication"
    );
    assert!(granted.approval_tools.is_empty());
    assert_eq!(
        granted.budgets.steps,
        mimlys_core::limits::LIM_E2_PROBE_STEPS
    );
    assert!(
        profile(true).external,
        "the one departure: a remote backend, named and allowed"
    );

    let policy = policy(false);
    policy.validate().unwrap();
    assert_eq!(policy.default_profile, PROFILE);
    assert_eq!(policy.profiles.keys().collect::<Vec<_>>(), [PROFILE]);
    assert!(policy.host_tools.is_empty());
    assert!(policy.ceiling.host_read_roots.is_empty());
    assert!(policy.ceiling.host_write_roots.is_empty());
    assert!(policy.ceiling.network_domains.is_empty());
    assert!(policy.ceiling.host_tools.is_empty());
    for tool in TOOLS {
        assert!(
            policy
                .ceiling
                .permits(&GrantKind::Tool { name: tool.into() })
                .is_ok(),
            "{tool}"
        );
    }
    for tool in [
        "run_script",
        "run_wasm",
        "delete_path",
        "list_dir",
        "search_files",
    ] {
        assert!(
            policy
                .ceiling
                .permits(&GrantKind::Tool { name: tool.into() })
                .is_err(),
            "{tool} is not permitted"
        );
    }
    assert!(
        policy
            .ceiling
            .permits(&GrantKind::Network {
                domains: vec!["example.org".into()],
                ports: vec![],
            })
            .is_err()
    );
}

/// REQ-MDL-011: the probe's wall time is LIM-E3, and it is a deadline the runtime accepts (within
/// LIM-T5's range); the steps are LIM-E2.
#[test]
fn req_mdl_011_the_bounds_are_lim_e2_and_lim_e3() {
    assert_eq!(WALL_MS, mimlys_core::limits::LIM_E3_PROBE_MS);
    assert_eq!(WALL_MS, 120_000);
    assert_eq!(mimlys_core::limits::LIM_E2_PROBE_STEPS, 8);
    let (low, high) = mimlys_core::limits::LIM_T5_DEADLINE_MS_RANGE;
    assert!((low..=high).contains(&WALL_MS));
}

/// REQ-MDL-011: the fixed mini-task is one edit of one file, and the file as it starts is not the
/// file as it must end; the goal names the file.
#[test]
fn req_mdl_011_the_mini_task_is_one_small_edit() {
    assert_ne!(FILE_START.trim_end(), FILE_WANTED);
    assert!(GOAL.contains(FILE));
    assert!(FILE_START.len() < 64 && GOAL.len() < 128);
}
