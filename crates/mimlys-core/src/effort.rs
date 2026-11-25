//! Effort levels (REQ-TASK-013; RFC 015 with Amendment 1; external design §19.4, from 0.13).
//!
//! A task is created at one of three levels, `quick` < `standard` < `deep`. A level is a budget,
//! never a permission: it is an overlay of five keys (the backend a task runs on, its steps, its
//! deadline, its context budget and the steps kept whole under compression), and it grants no
//! tool, path, destination or effect.
//!
//! This module is the one place the level's rules are written, as pure functions of the policy and
//! of the configuration's keys: which level a task gets ([`choose`]), whether that level has
//! anything to run on ([`Levels::keys`]), and what the task runs with ([`resolve`]), every key
//! resolved. The result is recorded on the task ([`Effort`]) and never changes during it.

use crate::limits::{
    LIM_C3_RECENT_STEPS, LIM_E4_QUICK_STEPS, LIM_E5_QUICK_DEADLINE_MS, LIM_T5_DEADLINE_MS,
};
use serde::{Deserialize, Serialize};

/// The name of the backend a level that names none runs on, and the one a task recorded before
/// 0.13 ran on (RFC 015 Amendment 1, item 4).
pub const DEFAULT_BACKEND: &str = "default";

/// A level: fixed identifiers, ordered `quick` < `standard` < `deep`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffortLevel {
    /// Stops a task that wanders sooner (LIM-E4, LIM-E5); does not make a completing task faster.
    Quick,
    /// Today's behaviour exactly: no keys of its own.
    Standard,
    /// The larger model the user names: no keys of its own until configured.
    Deep,
}

impl EffortLevel {
    /// Every level, in order.
    pub const ALL: [EffortLevel; 3] =
        [EffortLevel::Quick, EffortLevel::Standard, EffortLevel::Deep];

    /// The name on the wire, in the configuration and on the command line.
    pub fn name(self) -> &'static str {
        match self {
            EffortLevel::Quick => "quick",
            EffortLevel::Standard => "standard",
            EffortLevel::Deep => "deep",
        }
    }

    /// The level named `text`, exactly as written: lower case.
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|l| l.name() == text)
    }
}

impl std::fmt::Display for EffortLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

/// Who chose the level of a task.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChosenBy {
    /// `--effort` was given.
    User,
    /// No flag: `standard`, or the lower `effort_max`.
    Default,
    /// A fork, without `--effort`: its source's level.
    Fork,
}

impl ChosenBy {
    /// The name on the wire.
    pub fn name(self) -> &'static str {
        match self {
            ChosenBy::User => "user",
            ChosenBy::Default => "default",
            ChosenBy::Fork => "fork",
        }
    }
}

/// What a task runs with, every key resolved: what was resolved at creation, not what the file
/// said (§19.4). It never changes during the task; `resume --extend-steps` and
/// `--extend-deadline` extend the task's budgets and deadline, and not this.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Envelope {
    /// The backend, by its name in `[backends]`; resolved again at every start.
    pub backend: String,
    /// LIM-T1.
    pub steps: u32,
    /// LIM-T5, in milliseconds from the creation.
    pub deadline_ms: i64,
    /// LIM-C1: the level's, else its backend's. Absent only in the reading of a record written
    /// before 0.13, whose budget was its backend's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_chars: Option<u32>,
    /// LIM-C3: steps kept whole under compression.
    pub recent_steps: u32,
}

/// A task's level, who chose it and the envelope it runs with: `task.created{effort}` and the
/// task record carry it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Effort {
    /// The level.
    pub level: EffortLevel,
    /// Who chose it.
    pub chosen_by: ChosenBy,
    /// What the task runs with.
    pub envelope: Envelope,
}

/// The keys one `[effort.<level>]` section holds: each is absent unless the level sets it, and
/// an absent key is the profile's (or, for the backend and the context budget, the default
/// backend's).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LevelKeys {
    /// A name from `[backends]`.
    pub backend: Option<String>,
    /// LIM-T1.
    pub steps: Option<u32>,
    /// LIM-T5, in milliseconds.
    pub deadline_ms: Option<i64>,
    /// LIM-C1.
    pub context_chars: Option<u32>,
    /// LIM-C3.
    pub recent_steps: Option<u32>,
}

impl LevelKeys {
    /// Whether the level sets no key at all.
    pub fn is_empty(&self) -> bool {
        *self == LevelKeys::default()
    }
}

/// The keys of the three levels, as the configuration leaves them: the shipped ones replaced by
/// the file's, key by key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Levels {
    /// `[effort.quick]`.
    pub quick: LevelKeys,
    /// `[effort.standard]`.
    pub standard: LevelKeys,
    /// `[effort.deep]`.
    pub deep: LevelKeys,
}

impl Levels {
    /// What ships (REQ-TASK-013): `quick` has `steps` (LIM-E4) and `deadline` (LIM-E5), and
    /// nothing else; `standard` and `deep` have no key.
    pub fn shipped() -> Self {
        Self {
            quick: LevelKeys {
                steps: Some(LIM_E4_QUICK_STEPS),
                deadline_ms: Some(LIM_E5_QUICK_DEADLINE_MS),
                ..LevelKeys::default()
            },
            standard: LevelKeys::default(),
            deep: LevelKeys::default(),
        }
    }

    /// The keys of `level`.
    pub fn keys(&self, level: EffortLevel) -> &LevelKeys {
        match level {
            EffortLevel::Quick => &self.quick,
            EffortLevel::Standard => &self.standard,
            EffortLevel::Deep => &self.deep,
        }
    }

    /// Whether `level` can be chosen: `standard` always (it is today's behaviour), another level
    /// only when a key, shipped or configured, says what it is. `deep` is refused until it is
    /// configured (REQ-TASK-013).
    pub fn is_configured(&self, level: EffortLevel) -> bool {
        level == EffortLevel::Standard || !self.keys(level).is_empty()
    }
}

impl Default for Levels {
    fn default() -> Self {
        Self::shipped()
    }
}

/// What bounds the levels a task may have: the profile's `effort_max`, or the ceiling's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Bound {
    /// `[policy.profiles.<name>] effort_max`.
    Profile(String),
    /// `[policy.ceiling] effort_max`.
    Ceiling,
    /// Neither is set: `deep`, every level.
    None,
}

/// The highest level a profile allows: the lower of its own `effort_max` and the ceiling's, and
/// what sets it (§19.4).
pub fn bound_of(
    ceiling: Option<EffortLevel>,
    profile_name: &str,
    profile: Option<EffortLevel>,
) -> (EffortLevel, Bound) {
    match (ceiling, profile) {
        (None, None) => (EffortLevel::Deep, Bound::None),
        (Some(c), None) => (c, Bound::Ceiling),
        (None, Some(p)) => (p, Bound::Profile(profile_name.to_owned())),
        (Some(c), Some(p)) if p <= c => (p, Bound::Profile(profile_name.to_owned())),
        (Some(c), Some(_)) => (c, Bound::Ceiling),
    }
}

/// Why a level cannot be given to a task. Each says what to change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The level is above the bound.
    AboveMax {
        /// The level asked for.
        level: EffortLevel,
        /// The bound.
        max: EffortLevel,
        /// What sets it.
        bound: Bound,
    },
    /// The level has no key, shipped or configured.
    NoKeys {
        /// The level.
        level: EffortLevel,
    },
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refusal::AboveMax { level, max, bound } => {
                let at = match bound {
                    Bound::Profile(name) => format!("[policy.profiles.{name}] effort_max"),
                    Bound::Ceiling => "[policy.ceiling] effort_max".to_owned(),
                    Bound::None => "effort_max".to_owned(),
                };
                write!(
                    f,
                    "effort level `{level}` is above `{max}`, the highest level {at} allows; choose a level up to `{max}`, or raise effort_max"
                )
            }
            Refusal::NoKeys { level } => write!(
                f,
                "effort level `{level}` has no configuration: name a backend or a budget in [effort.{level}] (for `deep`, the backend of a larger model: `backend = \"<name>\"`)"
            ),
        }
    }
}

/// The level a task gets and who chose it (§19.4, "Choosing").
///
/// - `requested` (`--effort`) is the user's choice; it must not exceed the bound.
/// - Without it a fork takes `source`'s level, unless that is above the bound now.
/// - Without either: `standard`, or the bound when it is lower.
///
/// A level other than `standard` that has no key is refused, whoever chose it.
pub fn choose(
    requested: Option<EffortLevel>,
    source: Option<EffortLevel>,
    (max, bound): &(EffortLevel, Bound),
    levels: &Levels,
) -> Result<(EffortLevel, ChosenBy), Refusal> {
    let (level, chosen_by) = match (requested, source) {
        (Some(level), _) => (level, ChosenBy::User),
        (None, Some(level)) => (level, ChosenBy::Fork),
        (None, None) => (EffortLevel::Standard.min(*max), ChosenBy::Default),
    };
    if level > *max {
        return Err(Refusal::AboveMax {
            level,
            max: *max,
            bound: bound.clone(),
        });
    }
    if !levels.is_configured(level) {
        return Err(Refusal::NoKeys { level });
    }
    Ok((level, chosen_by))
}

/// What the flags and the profile say, to resolve a level's envelope.
#[derive(Clone, Debug)]
pub struct Inputs<'a> {
    /// The keys of the level.
    pub keys: &'a LevelKeys,
    /// `--steps`, when given: it wins over the level.
    pub flag_steps: Option<u32>,
    /// `--deadline`, when given, in milliseconds: it wins over the level.
    pub flag_deadline_ms: Option<i64>,
    /// The profile's steps (LIM-T1).
    pub profile_steps: u32,
    /// The backend a level that names none runs on.
    pub default_backend: &'a str,
}

/// Resolves the envelope of a level: a flag wins over the level, the level over the profile, and
/// the profile over the register's default (§19.4). `backend_context_chars` is the context budget
/// of the backend the envelope names, which a level without `context_chars` takes.
pub fn resolve(inputs: &Inputs<'_>, backend_context_chars: impl FnOnce(&str) -> u32) -> Envelope {
    let backend = inputs
        .keys
        .backend
        .clone()
        .unwrap_or_else(|| inputs.default_backend.to_owned());
    let context_chars = inputs
        .keys
        .context_chars
        .unwrap_or_else(|| backend_context_chars(&backend));
    Envelope {
        steps: inputs
            .flag_steps
            .or(inputs.keys.steps)
            .unwrap_or(inputs.profile_steps),
        deadline_ms: inputs
            .flag_deadline_ms
            .or(inputs.keys.deadline_ms)
            .unwrap_or(LIM_T5_DEADLINE_MS),
        backend,
        context_chars: Some(context_chars),
        recent_steps: inputs
            .keys
            .recent_steps
            .unwrap_or(LIM_C3_RECENT_STEPS as u32),
    }
}

#[cfg(test)]
mod tests;
