use super::*;

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
