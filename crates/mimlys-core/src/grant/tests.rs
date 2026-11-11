use super::*;
use crate::clock::{Clock, FakeClock};
use crate::ids::ScriptedIds;
use crate::policy::Policy;

fn set(kinds: Vec<GrantKind>) -> (GrantSet, FakeClock, Vec<GrantId>) {
    let ids = ScriptedIds::default();
    let clock = FakeClock::default();
    let task = TaskId::new(&ids);
    let mut set = GrantSet::new();
    let mut grant_ids = Vec::new();
    for kind in kinds {
        let id = GrantId::new(&ids);
        grant_ids.push(id);
        set.add(Grant::issue(
            id,
            task,
            kind,
            Actor::Runtime,
            clock.now(),
            None,
        ));
    }
    (set, clock, grant_ids)
}

/// REQ-CAP-002: the five kinds serialize with their names; `kind()` is private to mutation.
#[test]
fn req_cap_002_grant_kinds() {
    let kinds = [
        GrantKind::Tool {
            name: "read_file".into(),
        },
        GrantKind::HostRead {
            root: "/p".into(),
            patterns: vec![],
        },
        GrantKind::HostWrite {
            root: "/p".into(),
            patterns: vec!["**/*.rs".into()],
        },
        GrantKind::Network {
            domains: vec!["example.org".into()],
            ports: vec![],
        },
        GrantKind::HostTool {
            name: "cargo_test".into(),
        },
    ];
    let names: Vec<&str> = kinds.iter().map(GrantKind::name).collect();
    assert_eq!(
        names,
        ["tool", "host_read", "host_write", "network", "host_tool"]
    );
    for k in &kinds {
        let json = serde_json::to_value(k).unwrap();
        assert_eq!(json["kind"], k.name());
        let back: GrantKind = serde_json::from_value(json).unwrap();
        assert_eq!(&back, k);
    }
}

/// REQ-CAP-005 / REQ-CAP-006: revocation is immediate and final; no API widens a grant; expiry is honoured.
/// INV-11 (external design §21.2), grants never widen: a revoked grant cannot
/// be acted on again, an expired one leaves the visible set, and the grant's
/// kind has no setter, so nothing changes what an issued grant authorizes.
#[test]
fn req_cap_005_006_revocation_and_expiry() {
    let (mut s, clock, ids) = set(vec![
        GrantKind::Tool { name: "a".into() },
        GrantKind::Tool { name: "b".into() },
    ]);
    assert_eq!(s.visible_tools(clock.now()), ["a", "b"]);
    s.revoke(ids[0]).unwrap();
    assert_eq!(s.visible_tools(clock.now()), ["b"]);
    assert_eq!(s.revoke(ids[0]), Err(GrantError::NotActive));
    assert_eq!(
        s.revoke(GrantId::new(&ScriptedIds::starting_at(99))),
        Err(GrantError::NotFound)
    );
    let until = clock.now().plus_millis(1000).unwrap();
    let id = GrantId::new(&ScriptedIds::starting_at(50));
    s.add(Grant::issue(
        id,
        s.all()[0].task_id,
        GrantKind::Tool { name: "c".into() },
        Actor::Runtime,
        clock.now(),
        Some(until),
    ));
    assert_eq!(s.visible_tools(clock.now()), ["b", "c"]);
    clock.advance_millis(1000);
    assert_eq!(s.visible_tools(clock.now()), ["b"]);
    assert_eq!(s.expire(clock.now()), vec![id]);
    assert_eq!(s.all()[2].status, GrantStatus::Expired);
    // The source file exposes no setter for the kind.
    assert!(!include_str!("../grant.rs").contains("pub fn set_kind"));
}

/// REQ-CAP-007: the grant-set hash is canonical and order-independent, and changes on revocation.
#[test]
fn req_cap_007_grant_set_hash() {
    let (a, clock, _) = set(vec![
        GrantKind::Tool { name: "x".into() },
        GrantKind::Tool { name: "y".into() },
    ]);
    let (b, _, _) = set(vec![
        GrantKind::Tool { name: "y".into() },
        GrantKind::Tool { name: "x".into() },
    ]);
    assert_eq!(a.hash(clock.now()), b.hash(clock.now()));
    let (mut c, _, ids) = set(vec![
        GrantKind::Tool { name: "x".into() },
        GrantKind::Tool { name: "y".into() },
    ]);
    c.revoke(ids[1]).unwrap();
    assert_ne!(a.hash(clock.now()), c.hash(clock.now()));
}

/// REQ-CAP-001 / ED §9.1: host paths are permitted only under a granted root, matching its patterns, per case mode.
#[test]
fn req_cap_001_host_path_matching() {
    let (s, clock, _) = set(vec![
        GrantKind::HostRead {
            root: "/home/u/proj".into(),
            patterns: vec![],
        },
        GrantKind::HostWrite {
            root: "/home/u/proj".into(),
            patterns: vec!["src/**/*.rs".into()],
        },
    ]);
    let now = clock.now();
    assert!(s.permits_host_path(
        "/home/u/proj/README.md",
        HostAccess::Read,
        CaseMode::Sensitive,
        now
    ));
    assert!(s.permits_host_path("/home/u/proj", HostAccess::Read, CaseMode::Sensitive, now));
    assert!(!s.permits_host_path(
        "/home/u/project/x",
        HostAccess::Read,
        CaseMode::Sensitive,
        now
    ));
    assert!(!s.permits_host_path(
        "/home/u/proj/../other",
        HostAccess::Read,
        CaseMode::Sensitive,
        now
    ));
    assert!(s.permits_host_path(
        "/home/u/proj/src/a/b.rs",
        HostAccess::Write,
        CaseMode::Sensitive,
        now
    ));
    assert!(!s.permits_host_path(
        "/home/u/proj/README.md",
        HostAccess::Write,
        CaseMode::Sensitive,
        now
    ));
    assert!(!s.permits_host_path(
        "/HOME/u/proj/src/a.rs",
        HostAccess::Write,
        CaseMode::Sensitive,
        now
    ));
    assert!(s.permits_host_path(
        "/HOME/u/proj/src/A.RS",
        HostAccess::Write,
        CaseMode::Insensitive,
        now
    ));
    assert!(
        s.permits_host_path(
            "/home/u/proj\\src\\a.rs",
            HostAccess::Write,
            CaseMode::Sensitive,
            now
        ),
        "backslashes are separators"
    );
    assert!(
        GrantSet::new()
            .reconcile(&Policy::shipped().ceiling, now)
            .is_empty()
    );
}

/// REQ-CAP-001 / K-02: destinations need `https`, a listed domain (or a `*.` subdomain) and an allowed port.
#[test]
fn req_cap_001_destination_matching() {
    let (s, clock, _) = set(vec![GrantKind::Network {
        domains: vec!["example.org".into(), "*.rust-lang.org".into()],
        ports: vec![],
    }]);
    let now = clock.now();
    assert!(s.permits_destination("https://example.org/page", now));
    assert!(s.permits_destination("https://EXAMPLE.org:443/", now));
    assert!(s.permits_destination("https://docs.rust-lang.org/std/", now));
    assert!(s.permits_destination("https://rust-lang.org/", now));
    assert!(!s.permits_destination("https://example.org:8443/", now));
    assert!(!s.permits_destination("http://example.org/", now));
    assert!(!s.permits_destination("https://evil-example.org/", now));
    assert!(!s.permits_destination("https://example.org.evil.net/", now));
    assert!(!s.permits_destination("not a url", now));
    let (p, _, _) = set(vec![GrantKind::Network {
        domains: vec!["api.test".into()],
        ports: vec![8443],
    }]);
    assert!(p.permits_destination("https://api.test:8443/", now));
    assert!(!p.permits_destination("https://api.test/", now));
}

/// REQ-MCP-005 (K-02): an `http` destination is never admitted, whatever the
/// ports granted: its other form, `https` on 443, is what a server may read
/// (rypsok does), and the grant must cover that form too, so a grant for port
/// 80 alone cannot admit it; and no grant admits the plain-text form itself.
#[test]
fn req_mcp_005_an_http_destination_is_not_admitted_by_any_ports() {
    for ports in [vec![], vec![80], vec![443], vec![80, 443]] {
        let (s, clock, _) = set(vec![GrantKind::Network {
            domains: vec!["d.test".into()],
            ports: ports.clone(),
        }]);
        let now = clock.now();
        for url in [
            "http://d.test/",
            "http://d.test:80/",
            "http://d.test:443/",
            "HTTP://D.TEST/a?b=c",
        ] {
            assert!(
                !s.permits_destination(url, now),
                "{url} with ports {ports:?}"
            );
        }
    }
    // Only the `https` form of a granted port is admitted, as before.
    let (s, clock, _) = set(vec![GrantKind::Network {
        domains: vec!["d.test".into()],
        ports: vec![80],
    }]);
    let now = clock.now();
    assert!(s.permits_destination("https://d.test:80/", now));
    assert!(!s.permits_destination("https://d.test/", now));
}

/// REQ-CAP-010: reconciliation names the grants a reduced ceiling no longer permits, and never widens anything.
/// INV-11 (external design §21.2), grants never exceed the ceiling: a grant
/// the ceiling does not permit (an unlisted tool, a host tool) is named for
/// revocation, and one it permits is not.
#[test]
fn req_cap_010_reconcile_against_ceiling() {
    let (s, clock, ids) = set(vec![
        GrantKind::Tool {
            name: "read_file".into(),
        },
        GrantKind::Tool {
            name: "launch_missiles".into(),
        },
        GrantKind::HostTool {
            name: "cargo_test".into(),
        },
    ]);
    let excess = s.reconcile(&Policy::shipped().ceiling, clock.now());
    assert_eq!(excess, vec![ids[1], ids[2]]);
}
