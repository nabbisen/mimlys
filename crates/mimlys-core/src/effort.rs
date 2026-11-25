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
    /// LIM-C1: recorded **only when the level sets it** (review of 015c). Absent, the task's budget
    /// is its backend's as it is at each step, so that lowering it reaches a task already made;
    /// present, it is the level's own value, bounded at each step by the backend's as it is then
    /// ([`context_budget`]). A record that holds a value the level did not set (a build of 0.13
    /// before the review) reads as the level's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_chars: Option<u32>,
    /// LIM-C3: steps kept whole under compression.
    pub recent_steps: u32,
}

/// Whose value a task's context budget is at a step.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetSource {
    /// The level's own `context_chars`, which is not above the backend's.
    Level,
    /// The backend's `context_chars`: the level sets none, or sets one that the backend's, as it
    /// is now, is below.
    Backend,
}

/// A task's context budget at one step and whose it is (LIM-C1; external design §19.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ContextBudget {
    /// The budget in characters.
    pub chars: usize,
    /// Whose it is.
    pub source: BudgetSource,
}

/// The context budget of a task at a step: the level's value (`level`, the envelope's
/// `context_chars`) bounded by `backend`'s as it is **then**, or the backend's alone when the
/// level sets none (external design §19.4, review of 015c). It is read at every step, never
/// recorded.
pub fn context_budget(level: Option<u32>, backend: usize) -> ContextBudget {
    match level {
        Some(own) if own as usize <= backend => ContextBudget {
            chars: own as usize,
            source: BudgetSource::Level,
        },
        _ => ContextBudget {
            chars: backend,
            source: BudgetSource::Backend,
        },
    }
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
/// the profile over the register's default (§19.4). The envelope's `context_chars` is the level's
/// alone: a level that sets none leaves it out, and the task's budget is its backend's at each
/// step ([`context_budget`]).
pub fn resolve(inputs: &Inputs<'_>) -> Envelope {
    let backend = inputs
        .keys
        .backend
        .clone()
        .unwrap_or_else(|| inputs.default_backend.to_owned());
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
        context_chars: inputs.keys.context_chars,
        recent_steps: inputs
            .keys
            .recent_steps
            .unwrap_or(LIM_C3_RECENT_STEPS as u32),
    }
}

/// One backend (and model) a task was decided with, and how many of its steps (REQ-PERS-015).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendUsed {
    /// The backend's name.
    pub backend: String,
    /// The model it named at those steps; absent for a backend that names none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Steps decided with this pair.
    pub steps: u32,
}

/// What a task cost (REQ-OBS-010; external design §19.4, "Shown"): the steps used of its envelope,
/// the time against its deadline, the model's time, the tokens where the backend reported them,
/// and every backend and model it was decided with. Read from the task and its steps.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cost {
    /// Steps counted against the budget (the task's counter). The step that ends a task
    /// (`complete`, `give_up`) is decided and not counted, so this is `steps_decided` less one
    /// for a task that ended so.
    pub steps_used: u32,
    /// Steps the task has finished, whether or not they count against the budget (from the review
    /// of 015c): `steps_used` alone is what a task halts on. A task found `running` shows the
    /// steps it has finished, not the one in flight.
    #[serde(default)]
    pub steps_decided: u32,
    /// The envelope's steps: what the level gave.
    pub steps_envelope: u32,
    /// The steps in force: the envelope's, and what `resume --extend-steps` made of it.
    pub steps_limit: u32,
    /// Milliseconds from creation to now, or to the task's last change when it is over.
    pub elapsed_ms: i64,
    /// The envelope's deadline, in milliseconds from creation.
    pub deadline_envelope_ms: i64,
    /// The deadline in force, in milliseconds from creation (after `--extend-deadline`).
    pub deadline_limit_ms: i64,
    /// Milliseconds the model took, summed over the steps that measured it.
    pub model_ms: i64,
    /// The context budget in force as the configuration is now, in characters: what the task's
    /// next step would use (external design §19.4). Set by `task show` only
    /// ([`Cost::set_context`]); absent where it is not asked, and when the level sets none and the
    /// backend is no longer configured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_chars: Option<u64>,
    /// Whose it is: `level` or `backend`. Present with `context_chars`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_source: Option<BudgetSource>,
    /// Prompt tokens, summed over the steps that reported them; absent when none did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_tokens: Option<u64>,
    /// Prompt tokens served from the engine's cache, where reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached_tokens: Option<u64>,
    /// Completion tokens, where reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion_tokens: Option<u64>,
    /// The backends and models the steps were decided with, in the order they were first used.
    /// Empty for a task whose steps recorded none (written before 0.13).
    pub backends: Vec<BackendUsed>,
}

impl Cost {
    /// Sets the context budget in force as the configuration is now: `level` is the envelope's
    /// `context_chars`, `backend` the `context_chars` of the task's backend in the configuration
    /// (`None` when it is no longer configured). The same rule as the step's ([`context_budget`]);
    /// with no backend, the level's own value, and nothing when the level sets none.
    pub fn set_context(&mut self, level: Option<u32>, backend: Option<usize>) {
        let budget = match (level, backend) {
            (_, Some(backend)) => Some(context_budget(level, backend)),
            (Some(own), None) => Some(ContextBudget {
                chars: own as usize,
                source: BudgetSource::Level,
            }),
            (None, None) => None,
        };
        self.context_chars = budget.map(|b| b.chars as u64);
        self.context_source = budget.map(|b| b.source);
    }

    /// What `task` cost, from its steps. `now` closes the time of a task that is not over.
    pub fn of(
        task: &crate::task::Task,
        steps: &[crate::step::Step],
        now: crate::clock::Instant,
    ) -> Self {
        let effort = task.effort_in_force();
        // A task that is over ends at its last change, which is stamped when the step that ended
        // it began: the model's time of that step is after it, so the end is the later of that
        // and the end of the last model request (a step's start and its model time). A task that
        // is not over runs until now.
        let end = if task.state.is_terminal() {
            steps
                .iter()
                .filter_map(|s| {
                    let model_ms = s.measures.as_ref()?.model_ms?;
                    s.started_at.plus_millis(model_ms).ok()
                })
                .fold(task.updated_at, |end, request_end| end.max(request_end))
        } else {
            now
        };
        let mut cost = Cost {
            steps_used: task.counters.steps,
            steps_decided: u32::try_from(steps.iter().filter(|s| s.outcome.is_some()).count())
                .unwrap_or(u32::MAX),
            steps_envelope: effort.envelope.steps,
            steps_limit: task.budgets.steps,
            elapsed_ms: task.created_at.millis_until(end).max(0),
            deadline_envelope_ms: effort.envelope.deadline_ms,
            deadline_limit_ms: task.created_at.millis_until(task.deadline),
            model_ms: 0,
            context_chars: None,
            context_source: None,
            prompt_tokens: None,
            cached_tokens: None,
            completion_tokens: None,
            backends: Vec::new(),
        };
        let add = |into: &mut Option<u64>, from: Option<u64>| {
            if let Some(n) = from {
                *into = Some(into.unwrap_or(0) + n);
            }
        };
        for step in steps {
            if let Some(m) = &step.measures {
                cost.model_ms += m.model_ms.unwrap_or(0);
                if let Some(u) = &m.usage {
                    add(&mut cost.prompt_tokens, u.prompt_tokens);
                    add(&mut cost.cached_tokens, u.cached_tokens);
                    add(&mut cost.completion_tokens, u.completion_tokens);
                }
            }
            if let Some(backend) = &step.backend {
                match cost
                    .backends
                    .iter_mut()
                    .find(|b| &b.backend == backend && b.model == step.model)
                {
                    Some(used) => used.steps += 1,
                    None => cost.backends.push(BackendUsed {
                        backend: backend.clone(),
                        model: step.model.clone(),
                        steps: 1,
                    }),
                }
            }
        }
        cost
    }
}

#[cfg(test)]
mod tests;
