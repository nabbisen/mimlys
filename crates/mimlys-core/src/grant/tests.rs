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
    assert!(s.permits_destination("https://example.org", now));
    assert!(s.permits_destination("https://example.org/a?b=c#d", now));
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

/// REQ-MCP-005 (handoff 009h, item 1): a destination is judged as it is sent. Its
/// parse must serialize back to the same text (an empty path's `/` aside) and hold
/// no user information; else the form to write instead is given, and the text is
/// covered by no grant, whatever the grant says of the host the parse reads.
#[test]
fn req_mcp_005_a_destination_is_admitted_only_in_its_canonical_form() {
    use DestinationForm::{Canonical, NotCanonical, Unparsable};
    // Written as the parse writes it, or with the empty path left out.
    for text in [
        "https://example.org/",
        "https://example.org",
        "https://example.org?q=1",
        "https://example.org#top",
        "https://example.org:8443/x/y?z=1#f",
        "https://docs.rust-lang.org/std/",
        "https://[2001:db8::1]/a",
        "https://127.0.0.1/",
        "https://example.org/a%20b",
        "https://xn--mnchen-3ya.de/",
    ] {
        assert_eq!(destination_form(text), Canonical, "{text}");
    }
    // The kinds of text the grants and a server may read differently, with the
    // form to write instead.
    for (text, canonical) in [
        // A backslash is a slash to the parse.
        ("https://example.org\\x", "https://example.org/x"),
        ("https:\\\\example.org/x", "https://example.org/x"),
        // White space around, in the path, and control characters in it.
        (" https://example.org/", "https://example.org/"),
        ("https://example.org/ ", "https://example.org/"),
        ("https://example.org/a b", "https://example.org/a%20b"),
        ("https://example.org/\tx", "https://example.org/x"),
        ("https://exa\nmple.org/", "https://example.org/"),
        ("https://example.org/\u{1}x", "https://example.org/%01x"),
        // User information: the host is the one after the `@`.
        ("https://example.org@evil.test/", "https://evil.test/"),
        ("https://user:secret@example.org/x", "https://example.org/x"),
        ("https://@example.org/", "https://example.org/"),
        ("https://:@example.org/", "https://example.org/"),
        // A numeric host written short.
        ("https://127.1/", "https://127.0.0.1/"),
        ("https://0x7f.1/", "https://127.0.0.1/"),
        ("https://2130706433/", "https://127.0.0.1/"),
        ("https://0177.0.0.1/", "https://127.0.0.1/"),
        ("https://[0:0:0:0:0:0:0:1]/", "https://[::1]/"),
        // The default port written out.
        ("https://example.org:443/", "https://example.org/"),
        ("https://example.org:443", "https://example.org/"),
        // Other spellings of the same address.
        ("https://EXAMPLE.org/x", "https://example.org/x"),
        ("HTTPS://example.org/x", "https://example.org/x"),
        ("https://example.org/a/../b", "https://example.org/b"),
        ("https://example.org/./a", "https://example.org/a"),
        ("https://m\u{fc}nchen.de/", "https://xn--mnchen-3ya.de/"),
    ] {
        let form = destination_form(text);
        assert_eq!(form, NotCanonical(canonical.to_owned()), "{text:?}");
        // The form given is itself canonical: following the hint ends it.
        assert_eq!(destination_form(canonical), Canonical, "{canonical}");
    }
    for text in ["", "not a url", "https://", "https://exa mple.org/", "/x"] {
        assert_eq!(destination_form(text), Unparsable, "{text:?}");
    }
}

/// REQ-MCP-005 (handoff 009h, item 1): no grant covers a destination that is not
/// in its canonical form, though the parse of it is a granted host: the text a
/// server reads is not the text the grant judged.
#[test]
fn req_mcp_005_no_grant_covers_a_destination_written_otherwise_than_its_parse() {
    let (s, clock, _) = set(vec![GrantKind::Network {
        domains: vec!["example.org".into()],
        ports: vec![],
    }]);
    let now = clock.now();
    for text in [
        "https://example.org\\x",
        " https://example.org/",
        "https://example.org/a b",
        "https://example.org/\tx",
        "https://evil.test@example.org/",
        "https://user:secret@example.org/",
        "https://example.org:443/",
        "https://EXAMPLE.org/",
        "https://example.org/a/../b",
    ] {
        assert!(!s.permits_destination(text, now), "{text:?}");
    }
    // The canonical forms of the same are covered.
    for text in [
        "https://example.org/x",
        "https://example.org",
        "https://example.org/a%20b",
        "https://example.org/b",
    ] {
        assert!(s.permits_destination(text, now), "{text:?}");
    }
    // A granted host the numbers of which are written short is not the granted one.
    let (n, clock, _) = set(vec![GrantKind::Network {
        domains: vec!["127.0.0.1".into()],
        ports: vec![],
    }]);
    assert!(n.permits_destination("https://127.0.0.1/", clock.now()));
    for text in ["https://127.1/", "https://0x7f.1/", "https://2130706433/"] {
        assert!(!n.permits_destination(text, clock.now()), "{text}");
    }
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
