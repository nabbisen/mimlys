//! The probe's verdict (REQ-MDL-011): what the runtime found when the mini-task ended, put in one
//! of five classes. It is a function of what the store and the workspace hold, never of the
//! model's word that it finished, and it reads no clock.

use mimlys_core::task::HaltReason;

/// The class of a probe (REQ-MDL-011), in the words the record and `status` use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    /// Completed, and the runtime found the file as asked.
    Acts,
    /// It acted, but the task was not completed and verified within the bounds, or it asked the
    /// user or gave up ([`Detail`] says which).
    Incomplete,
    /// No `act` within the budget.
    PlansOnly,
    /// Every reply was malformed: no structured call.
    NoStructuredCall,
    /// No reply came from the backend.
    Unreachable,
}

impl Class {
    /// The word in `probe-results.json`, in `status` and in `--json`.
    pub fn as_str(self) -> &'static str {
        match self {
            Class::Acts => "acts",
            Class::Incomplete => "incomplete",
            Class::PlansOnly => "plans_only",
            Class::NoStructuredCall => "no_structured_call",
            Class::Unreachable => "unreachable",
        }
    }
}

/// Why an `incomplete` probe is incomplete.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Detail {
    /// It did not finish, or what it finished the runtime did not find as asked, within the
    /// bounds (LIM-E2 steps, LIM-E3 time, or the guards of the task).
    NotFinished,
    /// It asked the user, whom nobody answers in a probe.
    AskedUser,
    /// It gave up.
    GaveUp,
}

impl Detail {
    /// The word in the record.
    pub fn as_str(self) -> &'static str {
        match self {
            Detail::NotFinished => "not_finished",
            Detail::AskedUser => "asked_user",
            Detail::GaveUp => "gave_up",
        }
    }
}

/// How the mini-task ended, as the store says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum End {
    /// The task is `completed`: the model said `complete` (and, with no completion check, that was
    /// all it took). Whether the file is as asked is [`Observed::verified`], read by the runtime.
    Completed,
    /// `failed`, the model having given up.
    GaveUp,
    /// `waiting_user`, for a question.
    AskedUser,
    /// `halted`, for a reason of the runtime's: a budget, the guards, the backend.
    Halted(HaltReason),
    /// Any other state: not one a probe reaches; the words name it.
    Other(String),
}

/// What one step of the mini-task came to, from the step's record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StepFacts {
    /// A reply came from the model (a decision was parsed or a reply was rejected). False for a
    /// request that failed.
    pub replied: bool,
    /// The reply was rejected `malformed`.
    pub malformed: bool,
    /// An action was proposed: the model's `act` went through validation.
    pub acted: bool,
}

/// Everything the verdict is made of.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Observed {
    /// How the task ended.
    pub end: End,
    /// The steps, in order.
    pub steps: Vec<StepFacts>,
    /// For a completed task: the runtime read the file in the probe's workspace and found it as
    /// asked. Never set from anything the model said.
    pub verified: bool,
}

/// What the probe's verdict is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// A class to record.
    Class(Class, Option<Detail>),
    /// The probe measured nothing about the model: the task's time ran out before a request was
    /// sent (the engine's gate stayed held by another runtime of the user's). Nothing is recorded.
    NotMeasured,
    /// The mini-task ended in a state a probe does not reach, without any reply. Nothing is
    /// recorded; it is a fault, and the command says what the state was.
    Fault,
}

/// The verdict on what the runtime observed.
///
/// In order: no reply at all is `unreachable` when the backend failed (and not measured when the
/// task's time ran out in the engine's gate); replies that were all `malformed` are
/// `no_structured_call`; a completed task whose file the runtime found as asked is `acts`; no
/// `act` at all is `plans_only` (whatever else the model said: planned, asked, gave up, or claimed
/// to be done); and a model that acted and did not get there is `incomplete`, with how it ended.
pub fn classify(observed: &Observed) -> Verdict {
    let replies = observed.steps.iter().filter(|s| s.replied).count();
    if replies == 0 {
        return match observed.end {
            End::Halted(HaltReason::BackendUnavailable) => Verdict::Class(Class::Unreachable, None),
            End::Halted(HaltReason::BudgetDeadline) => Verdict::NotMeasured,
            _ => Verdict::Fault,
        };
    }
    if observed
        .steps
        .iter()
        .filter(|s| s.replied)
        .all(|s| s.malformed)
    {
        return Verdict::Class(Class::NoStructuredCall, None);
    }
    if observed.end == End::Completed && observed.verified {
        return Verdict::Class(Class::Acts, None);
    }
    if !observed.steps.iter().any(|s| s.acted) {
        return Verdict::Class(Class::PlansOnly, None);
    }
    let detail = match observed.end {
        End::AskedUser => Detail::AskedUser,
        End::GaveUp => Detail::GaveUp,
        End::Completed | End::Halted(_) | End::Other(_) => Detail::NotFinished,
    };
    Verdict::Class(Class::Incomplete, Some(detail))
}

#[cfg(test)]
mod tests;
