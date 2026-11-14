//! The confinement contract's vocabulary (external design §13.2, §13.4;
//! REQ-CONF-001, REQ-CONF-003): the seven clauses, what a probe says about
//! each, and the tier that follows. The backends live in `mimlys-exec`;
//! the runtime only reads and reports this.

use crate::event::Tier;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A clause of the contract.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Clause {
    /// F: explicit read and write sets.
    Filesystem,
    /// N: no network, loopback included.
    Network,
    /// E: explicit environment.
    Environment,
    /// K: whole-tree termination.
    Termination,
    /// P: no privilege gain.
    Privilege,
    /// M: memory ceiling (best effort allowed).
    Memory,
    /// C: process count and wall clock (count best effort allowed).
    Count,
}

impl Clause {
    /// Every clause, in the order of the design table.
    pub const ALL: [Clause; 7] = [
        Clause::Filesystem,
        Clause::Network,
        Clause::Environment,
        Clause::Termination,
        Clause::Privilege,
        Clause::Memory,
        Clause::Count,
    ];

    /// The snake_case word of the JSON output (`filesystem`, `network`, …).
    pub fn name(self) -> &'static str {
        match self {
            Clause::Filesystem => "filesystem",
            Clause::Network => "network",
            Clause::Environment => "environment",
            Clause::Termination => "termination",
            Clause::Privilege => "privilege",
            Clause::Memory => "memory",
            Clause::Count => "count",
        }
    }

    /// The one-letter name of the design.
    pub fn letter(self) -> char {
        match self {
            Clause::Filesystem => 'F',
            Clause::Network => 'N',
            Clause::Environment => 'E',
            Clause::Termination => 'K',
            Clause::Privilege => 'P',
            Clause::Memory => 'M',
            Clause::Count => 'C',
        }
    }

    /// Whether the clause must be `enforced` for the `full` tier.
    pub fn mandatory(self) -> bool {
        !matches!(self, Clause::Memory | Clause::Count)
    }
}

/// What the probe found for one clause.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ClauseState {
    /// The kernel enforces it.
    Enforced {
        /// The primitive that holds it (`landlock`, `seccomp`, `cgroup_kill`, …), when known (RFC 011 §2).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        primitive: Option<String>,
    },
    /// Enforced without a kernel guarantee.
    BestEffort {
        /// The primitive that holds it, when known.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        primitive: Option<String>,
    },
    /// Not enforceable here, with the reason.
    Unavailable {
        /// Why, in fixed words.
        reason: String,
    },
}

impl ClauseState {
    /// `enforced` without a named primitive.
    pub const ENFORCED: ClauseState = ClauseState::Enforced { primitive: None };

    /// `enforced` by a named primitive.
    pub fn enforced(primitive: &str) -> Self {
        ClauseState::Enforced {
            primitive: Some(primitive.to_owned()),
        }
    }

    /// `best_effort` by a named primitive.
    pub fn best_effort(primitive: &str) -> Self {
        ClauseState::BestEffort {
            primitive: Some(primitive.to_owned()),
        }
    }

    /// `unavailable` for a reason.
    pub fn unavailable(reason: &str) -> Self {
        ClauseState::Unavailable {
            reason: reason.to_owned(),
        }
    }

    /// Whether the kernel enforces it.
    pub fn is_enforced(&self) -> bool {
        matches!(self, ClauseState::Enforced { .. })
    }

    /// The state for humans: `enforced (landlock)`, `best effort (rlimit_as)`, `unavailable: reason`.
    pub fn describe(&self) -> String {
        match self {
            ClauseState::Enforced { primitive: Some(p) } => format!("enforced ({p})"),
            ClauseState::Enforced { primitive: None } => "enforced".into(),
            ClauseState::BestEffort { primitive: Some(p) } => format!("best effort ({p})"),
            ClauseState::BestEffort { primitive: None } => "best effort".into(),
            ClauseState::Unavailable { reason } => format!("unavailable: {reason}"),
        }
    }
}

/// The tier with what the administrator accepted (RFC 011 §5).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TierReport {
    /// `core` or `full`.
    pub tier: Tier,
    /// Mandatory clauses that are best-effort and accepted by `policy.accept_best_effort`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub accepted: Vec<Clause>,
}

impl TierReport {
    /// `full`, `full (K best-effort, accepted by policy.accept_best_effort)` or `core`.
    pub fn describe(&self) -> String {
        match (&self.tier, self.accepted.is_empty()) {
            (Tier::Full, true) => "full".into(),
            (Tier::Full, false) => format!(
                "full ({} best-effort, accepted by policy.accept_best_effort)",
                self.accepted
                    .iter()
                    .map(|c| c.letter().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            (Tier::Core, _) => "core".into(),
        }
    }
}

/// The probe's result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Probe {
    /// Per clause.
    pub clauses: BTreeMap<Clause, ClauseState>,
}

impl Probe {
    /// Every clause unavailable for one reason (the state before a probe ran).
    pub fn unavailable(reason: &str) -> Self {
        Self {
            clauses: Clause::ALL
                .iter()
                .map(|c| {
                    (
                        *c,
                        ClauseState::Unavailable {
                            reason: reason.to_owned(),
                        },
                    )
                })
                .collect(),
        }
    }

    /// `full` when every mandatory clause is enforced, else `core` (REQ-CONF-003).
    pub fn tier(&self) -> Tier {
        self.tier_with(&[]).tier
    }

    /// The tier when the administrator accepted best-effort for `accepted`
    /// mandatory clauses (RFC 011 §5): a best-effort mandatory clause in the
    /// list still gives `full`, reported; an unavailable clause never does.
    pub fn tier_with(&self, accepted: &[Clause]) -> TierReport {
        let mut used = Vec::new();
        for c in Clause::ALL.iter().filter(|c| c.mandatory()) {
            match self.clauses.get(c) {
                Some(ClauseState::Enforced { .. }) => {}
                Some(ClauseState::BestEffort { .. }) if accepted.contains(c) => used.push(*c),
                _ => {
                    return TierReport {
                        tier: Tier::Core,
                        accepted: vec![],
                    };
                }
            }
        }
        TierReport {
            tier: Tier::Full,
            accepted: used,
        }
    }

    /// The mandatory clauses this report does not meet, as `F: reason`
    /// lines: a clause that is not `enforced` (a `best_effort` one passes when
    /// the administrator accepted it, as for the tier), except those in
    /// `exempt`, which are not asked of the process the report is for (the
    /// dependency step's worker has the network, so its `N` is not enforced
    /// by design). A clause the report does not mention is short. Empty when
    /// the report is enough to run a host tool (REQ-SEC-003, REQ-CONF-003).
    pub fn short_of_mandatory(&self, accepted: &[Clause], exempt: &[Clause]) -> Vec<String> {
        Clause::ALL
            .iter()
            .filter(|c| c.mandatory() && !exempt.contains(c))
            .filter_map(|c| match self.clauses.get(c) {
                Some(ClauseState::Enforced { .. }) => None,
                Some(ClauseState::BestEffort { .. }) if accepted.contains(c) => None,
                Some(state) => Some(format!("{}: {}", c.letter(), state.describe())),
                None => Some(format!("{}: not reported", c.letter())),
            })
            .collect()
    }

    /// The clauses that are not `enforced`, as `F: reason` lines for the status.
    pub fn unavailable_lines(&self) -> Vec<String> {
        Clause::ALL
            .iter()
            .filter_map(|c| match self.clauses.get(c) {
                Some(ClauseState::Enforced { .. }) | None => None,
                Some(state) => Some(format!("{}: {}", c.letter(), state.describe())),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests;
