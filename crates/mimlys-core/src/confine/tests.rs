use super::*;

/// REQ-SEC-003, REQ-CONF-003: a report is enough to run a host tool only when
/// every mandatory clause is enforced (or best effort and accepted), apart
/// from the clauses exempt for the process it is for.
#[test]
fn req_sec_003_a_short_report_names_the_mandatory_clauses_it_lacks() {
    let mut probe = Probe::unavailable("not applied");
    assert_eq!(
        probe.short_of_mandatory(&[], &[]),
        [
            "F: unavailable: not applied",
            "N: unavailable: not applied",
            "E: unavailable: not applied",
            "K: unavailable: not applied",
            "P: unavailable: not applied"
        ]
    );
    for c in Clause::ALL {
        probe.clauses.insert(c, ClauseState::ENFORCED);
    }
    assert!(probe.short_of_mandatory(&[], &[]).is_empty());
    // M and C are not mandatory.
    probe
        .clauses
        .insert(Clause::Memory, ClauseState::unavailable("no cgroup"));
    assert!(probe.short_of_mandatory(&[], &[]).is_empty());
    // A best-effort mandatory clause is short unless accepted.
    probe
        .clauses
        .insert(Clause::Termination, ClauseState::best_effort("subreaper"));
    assert_eq!(
        probe.short_of_mandatory(&[], &[]),
        ["K: best effort (subreaper)"]
    );
    assert!(
        probe
            .short_of_mandatory(&[Clause::Termination], &[])
            .is_empty()
    );
    // An unavailable one is never accepted; one not reported is short.
    probe
        .clauses
        .insert(Clause::Network, ClauseState::unavailable("network allowed"));
    probe.clauses.remove(&Clause::Filesystem);
    assert_eq!(
        probe.short_of_mandatory(&[Clause::Network, Clause::Termination], &[]),
        ["F: not reported", "N: unavailable: network allowed"]
    );
    // The dependency step's worker has the network by design.
    assert_eq!(
        probe.short_of_mandatory(&[Clause::Termination], &[Clause::Network]),
        ["F: not reported"]
    );
}

/// REQ-CONF-003: the tier is `full` only when every mandatory clause is enforced.
#[test]
fn req_conf_003_tier_rule() {
    let mut probe = Probe::unavailable("backend arrives in M6a");
    assert_eq!(probe.tier(), Tier::Core);
    assert_eq!(probe.unavailable_lines().len(), 7);
    for c in Clause::ALL {
        probe.clauses.insert(c, ClauseState::ENFORCED);
    }
    assert_eq!(probe.tier(), Tier::Full);
    probe
        .clauses
        .insert(Clause::Memory, ClauseState::BestEffort { primitive: None });
    probe
        .clauses
        .insert(Clause::Count, ClauseState::BestEffort { primitive: None });
    assert_eq!(probe.tier(), Tier::Full, "M and C may be best effort");
    probe
        .clauses
        .insert(Clause::Network, ClauseState::BestEffort { primitive: None });
    assert_eq!(
        probe.tier(),
        Tier::Core,
        "a mandatory clause must be enforced"
    );
    assert_eq!(
        probe.unavailable_lines(),
        ["N: best effort", "M: best effort", "C: best effort"]
    );
    let json = serde_json::to_string(&probe).unwrap();
    let back: Probe = serde_json::from_str(&json).unwrap();
    assert_eq!(back, probe);
}
