use super::*;
use crate::limits::LIM_T1_STEPS;

/// REQ-CAP-009 / ED §9.2: the shipped profiles, their tools and defaults; the policy validates and hashes.
#[test]
fn req_cap_009_shipped_profiles() {
    let p = Policy::shipped();
    p.validate().unwrap();
    assert_eq!(p.default_profile, "workspace");
    assert!(!p.remote_backend_allowed);
    let ro = p.profile("read_only").unwrap();
    assert_eq!(ro.tools, ["read_file", "list_dir", "search_files"]);
    assert!(!ro.promote);
    let ws = p.profile("workspace").unwrap();
    assert_eq!(ws.tools.len(), BUILTIN_TOOLS.len());
    assert!(
        ws.approval_tools.is_empty(),
        "REQ-SEC-014: no routine approval by default; the M2 stopgap ended with rollback (RFC 006 §9)"
    );
    assert_eq!(ws.budgets.steps, LIM_T1_STEPS);
    assert_eq!(ws.on_reject, OnReject::Continue);
    assert_eq!(ws.on_cancel, OnCancel::Rollback);
    let pj = p.profile("project").unwrap();
    assert!(pj.promote);
    assert_eq!(p.initial_grants(ro).len(), 3);
    assert_eq!(p.hash(), Policy::shipped().hash());
    assert!(p.profile("admin").is_none());
}

/// REQ-CAP-003 / CLR-3: validation names the key and states the range or ceiling.
#[test]
fn req_cap_003_validation_names_key_and_bounds() {
    let mut p = Policy::shipped();
    p.profiles.get_mut("workspace").unwrap().budgets.steps = 9999;
    let e = p.validate().unwrap_err();
    assert_eq!(e.key, "profiles.workspace.budgets.steps");
    assert!(e.reason.contains("..=200"), "{e}");
    let mut p = Policy::shipped();
    p.profiles
        .get_mut("workspace")
        .unwrap()
        .tools
        .push("launch_missiles".into());
    let e = p.validate().unwrap_err();
    assert_eq!(e.key, "profiles.workspace.tools");
    assert!(e.reason.contains("launch_missiles"));
    let mut p = Policy::shipped();
    p.default_profile = "nope".into();
    assert_eq!(p.validate().unwrap_err().key, "default_profile");
    let mut p = Policy::shipped();
    p.config_version = 2;
    assert_eq!(p.validate().unwrap_err().key, "config_version");
    let mut p = Policy::shipped();
    p.exclusions.secrets.push("[".into());
    assert_eq!(p.validate().unwrap_err().key, "exclusions.secrets");
    // Unknown keys are refused at deserialization (REQ-CFG-001).
    let json = serde_json::to_value(Policy::shipped()).unwrap();
    let mut with_extra = json.clone();
    with_extra["surprise"] = serde_json::json!(1);
    assert!(serde_json::from_value::<Policy>(with_extra).is_err());
    let back: Policy = serde_json::from_value(json).unwrap();
    assert_eq!(back, Policy::shipped());
}

/// REQ-CAP-003: the ceiling permits grants only within its lists and roots; the error lists what is permitted.
#[test]
fn req_cap_003_ceiling_permits() {
    let mut c = Policy::shipped().ceiling;
    c.host_read_roots = vec!["/home/u/docs".into()];
    c.network_domains = vec!["*.rust-lang.org".into(), "example.org".into()];
    assert!(
        c.permits(&GrantKind::Tool {
            name: "read_file".into()
        })
        .is_ok()
    );
    let e = c
        .permits(&GrantKind::Tool {
            name: "shell".into(),
        })
        .unwrap_err();
    assert_eq!(e.grant_kind, "tool");
    assert!(e.permitted.contains(&"read_file".to_owned()));
    assert!(
        c.permits(&GrantKind::HostRead {
            root: "/home/u/docs/sub".into(),
            patterns: vec![]
        })
        .is_ok()
    );
    assert!(
        c.permits(&GrantKind::HostRead {
            root: "/home/u/docs".into(),
            patterns: vec![]
        })
        .is_ok()
    );
    assert!(
        c.permits(&GrantKind::HostRead {
            root: "/home/u/docsx".into(),
            patterns: vec![]
        })
        .is_err()
    );
    assert!(
        c.permits(&GrantKind::HostWrite {
            root: "/home/u/docs".into(),
            patterns: vec![]
        })
        .is_err()
    );
    assert!(
        c.permits(&GrantKind::Network {
            domains: vec!["docs.rust-lang.org".into(), "example.org".into()],
            ports: vec![]
        })
        .is_ok()
    );
    assert!(
        c.permits(&GrantKind::Network {
            domains: vec!["evil.net".into()],
            ports: vec![]
        })
        .is_err()
    );
    assert!(
        c.permits(&GrantKind::HostTool {
            name: "cargo_test".into()
        })
        .is_err()
    );
}

/// REQ-WS-003 / LIM-W4 / LIM-W5: exclusion patterns match directories at any depth and files by name.
#[test]
fn req_ws_003_exclusion_matching() {
    let p = Policy::shipped();
    let all: Vec<String> = p
        .exclusions
        .materialize
        .iter()
        .chain(p.exclusions.secrets.iter())
        .cloned()
        .collect();
    let m = ExclusionMatcher::new(&all, CaseMode::Sensitive).unwrap();
    let path =
        |s: &str| WorkspacePath::canonicalize_with(s, crate::path::PlatformRules::Posix).unwrap();
    for excluded in [
        ".git",
        ".git/config",
        "sub/.git/HEAD",
        "target/debug/x",
        "node_modules/a/b.js",
        ".env",
        ".env.local",
        "config/.env",
        "keys/server.pem",
        "id_rsa",
        ".ssh/known_hosts",
        "sub/.aws/credentials",
    ] {
        assert!(m.is_excluded(&path(excluded)), "{excluded}");
    }
    for allowed in [
        "src/main.rs",
        "environment.md",
        "gitignore",
        "targets.txt",
        "node_modules.md",
        "pemfile.txt",
    ] {
        assert!(!m.is_excluded(&path(allowed)), "{allowed}");
    }
    let ci = ExclusionMatcher::new(&[".ENV".into()], CaseMode::Insensitive).unwrap();
    assert!(ci.is_excluded(&path(".env")));
    let cs = ExclusionMatcher::new(&[".ENV".into()], CaseMode::Sensitive).unwrap();
    assert!(!cs.is_excluded(&path(".env")));
}

/// ED §9.1 / RFC 011 §6: `run_host_tool` is visible through `host_tool`
/// grants alone: the shipped ceiling does not list it, a profile may not,
/// `approval_tools` may, and a profile's `host_tools` must be declared.
#[test]
fn rfc_011_run_host_tool_is_not_a_tool_grant() {
    let shipped = Policy::shipped();
    assert!(!shipped.ceiling.tools.iter().any(|t| t == "run_host_tool"));
    let mut p = shipped.clone();
    p.profiles
        .get_mut("workspace")
        .unwrap()
        .tools
        .push("run_host_tool".into());
    let err = p.validate().unwrap_err();
    assert_eq!(err.key, "profiles.workspace.tools");
    assert!(err.reason.contains("host_tool:<name>"), "{}", err.reason);
    let mut p = shipped.clone();
    p.profiles
        .get_mut("workspace")
        .unwrap()
        .approval_tools
        .push("run_host_tool".into());
    p.validate().unwrap();
    let mut p = shipped.clone();
    p.profiles
        .get_mut("workspace")
        .unwrap()
        .host_tools
        .push("cargo_test".into());
    assert_eq!(
        p.validate().unwrap_err().key,
        "profiles.workspace.host_tools"
    );
    p.ceiling.host_tools.push("cargo_test".into());
    p.validate().unwrap();
    let grants = p.initial_grants(p.profile("workspace").unwrap());
    assert!(grants.contains(&GrantKind::HostTool {
        name: "cargo_test".into()
    }));
}
