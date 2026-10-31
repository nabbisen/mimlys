//! Steps, counters and budgets (external design §6.1, §6.5; REQ-DEC-001,
//! REQ-DEC-010, REQ-TASK-008). A step holds one decision and at most one
//! action; counters are durable data that the persistence layer stores and
//! that survive restarts.

use crate::clock::Instant;
use crate::hash::Hash;
use crate::ids::{ActionId, ObservationId, SnapshotId, StepId, TaskId};
use crate::limits::{
    LIM_T1_STEPS, LIM_T2_CONSECUTIVE_REJECTIONS, LIM_T3_EXECUTIONS_PER_ACTION,
    LIM_T4_IDENTICAL_DECISIONS, LIM_T6_CONSECUTIVE_FAILURES,
};
use crate::rejection::RejectionCode;
use crate::task::HaltReason;
use serde::{Deserialize, Serialize};

/// The phase a step is in (external design §6.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepPhase {
    /// Assembling the context.
    Assemble,
    /// Waiting for the model's decision.
    Decide,
    /// Validating the decision.
    Validate,
    /// Waiting for the user.
    Wait,
    /// Executing the action.
    Execute,
    /// Normalizing the result.
    Observe,
    /// Applying counters and dispositions.
    Evaluate,
}

/// How a step ended (external design §6.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepOutcome {
    /// The loop continues with the next step.
    Continued,
    /// The decision was rejected; feedback follows.
    Rejected,
    /// The task waits for the user.
    Waited,
    /// The task completed.
    Completed,
    /// The task failed.
    Failed,
    /// The task halted.
    Halted,
    /// The task paused at this boundary.
    Paused,
    /// The task was cancelled during this step.
    Cancelled,
}

/// The outcome of the step's action, if it had one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionOutcome {
    /// The action succeeded.
    Succeeded,
    /// The action failed (after any runtime retries).
    Failed,
}

/// One iteration of the control loop (REQ-DEC-001).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Step {
    /// Identifier.
    pub step_id: StepId,
    /// Owning task.
    pub task_id: TaskId,
    /// Zero-based index within the task.
    pub index: u32,
    /// Current phase.
    pub phase: StepPhase,
    /// Hash of the decision once received.
    pub decision_hash: Option<Hash>,
    /// Rejection code when the decision was rejected.
    pub rejection: Option<RejectionCode>,
    /// The action, when the decision produced one.
    pub action_id: Option<ActionId>,
    /// The observation, when the action produced one.
    pub observation_id: Option<ObservationId>,
    /// Outcome once the step finished.
    pub outcome: Option<StepOutcome>,
    /// When the step started.
    pub started_at: Instant,
    /// When the step finished.
    pub finished_at: Option<Instant>,
    /// Context and model measures of the decision request (REQ-CTX-010);
    /// absent on records written before they were measured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub measures: Option<StepMeasures>,
}

/// What the assembler's rules did to one request (REQ-CTX-010).
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ContextMeasures {
    /// Characters of every message content after the rules.
    pub context_chars: u64,
    /// Characters before any rule applied.
    pub chars_before_fit: u64,
    /// Steps reduced to summaries.
    pub compressed_steps: u32,
    /// Summaries dropped.
    pub dropped_steps: u32,
    /// The retained set alone exceeded the budget; the request went out anyway.
    pub over_budget: bool,
    /// Characters shared with the task's previous request (0 for the first
    /// request of a process).
    pub prefix_chars: u64,
}

/// The measures of one decision request: the context's and the model's.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct StepMeasures {
    /// The context's measures.
    #[serde(flatten)]
    pub context: ContextMeasures,
    /// Wall time of the model call, in milliseconds, when it returned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_ms: Option<i64>,
    /// The backend's reported usage, when any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<crate::spi::Usage>,
}

impl Step {
    /// A new step in the `assemble` phase.
    pub fn start(step_id: StepId, task_id: TaskId, index: u32, now: Instant) -> Self {
        Self {
            step_id,
            task_id,
            index,
            phase: StepPhase::Assemble,
            decision_hash: None,
            rejection: None,
            action_id: None,
            observation_id: None,
            outcome: None,
            started_at: now,
            finished_at: None,
            measures: None,
        }
    }

    /// Records the end of the step.
    pub fn finish(&mut self, outcome: StepOutcome, now: Instant) {
        self.phase = StepPhase::Evaluate;
        self.outcome = Some(outcome);
        self.finished_at = Some(now);
    }
}

/// The limits of a task (LIM-T1 … LIM-T4, LIM-T6; the deadline lives on the task).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Budgets {
    /// LIM-T1.
    pub steps: u32,
    /// LIM-T2.
    pub consecutive_rejections: u32,
    /// LIM-T6.
    pub consecutive_failures: u32,
    /// LIM-T4.
    pub identical_decisions: u32,
    /// LIM-T3.
    pub executions_per_action: u32,
}

impl Default for Budgets {
    fn default() -> Self {
        Self {
            steps: LIM_T1_STEPS,
            consecutive_rejections: LIM_T2_CONSECUTIVE_REJECTIONS,
            consecutive_failures: LIM_T6_CONSECUTIVE_FAILURES,
            identical_decisions: LIM_T4_IDENTICAL_DECISIONS,
            executions_per_action: LIM_T3_EXECUTIONS_PER_ACTION,
        }
    }
}

/// Durable counters of a task (external design §6.5; REQ-TASK-008, REQ-DEC-010).
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Counters {
    /// Steps taken.
    pub steps: u32,
    /// Rejected steps since the last accepted one.
    pub consecutive_rejections: u32,
    /// Failed actions since the last succeeded one.
    pub consecutive_failures: u32,
    /// Length of the current run of identical decisions (1 for a fresh decision).
    pub identical_decisions: u32,
    /// The previous step's decision hash, for the identical-decision rule.
    pub previous_decision: Option<Hash>,
    /// The workspace snapshot in force at the previous decision.
    pub previous_snapshot: Option<SnapshotId>,
}

/// What a finished step contributes to the counters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StepResult {
    /// How the step ended.
    pub outcome: StepOutcome,
    /// The action's outcome, if the step had an action that finished.
    pub action: Option<ActionOutcome>,
    /// The decision's hash, if one was received.
    pub decision_hash: Option<Hash>,
    /// The workspace snapshot in force when the decision was made.
    pub snapshot: Option<SnapshotId>,
}

impl Counters {
    /// Applies a finished step and returns the halt reason when a budget
    /// is reached (external design §6.5). The first exceeded budget wins,
    /// in the order steps, rejections, failures, convergence.
    pub fn apply(&mut self, result: &StepResult, budgets: &Budgets) -> Option<HaltReason> {
        self.steps = self.steps.saturating_add(1);

        if result.outcome == StepOutcome::Rejected {
            self.consecutive_rejections = self.consecutive_rejections.saturating_add(1);
        } else {
            self.consecutive_rejections = 0;
        }

        match result.action {
            Some(ActionOutcome::Failed) => {
                self.consecutive_failures = self.consecutive_failures.saturating_add(1);
            }
            Some(ActionOutcome::Succeeded) => self.consecutive_failures = 0,
            None => {}
        }

        match &result.decision_hash {
            Some(hash) => {
                let same_decision = self.previous_decision.as_ref() == Some(hash);
                let same_snapshot = self.previous_snapshot == result.snapshot;
                if same_decision && same_snapshot {
                    self.identical_decisions = self.identical_decisions.saturating_add(1);
                } else {
                    self.identical_decisions = 1;
                }
                self.previous_decision = Some(hash.clone());
                self.previous_snapshot = result.snapshot;
            }
            None => {
                self.identical_decisions = 0;
                self.previous_decision = None;
                self.previous_snapshot = result.snapshot;
            }
        }

        if self.steps >= budgets.steps {
            Some(HaltReason::BudgetSteps)
        } else if self.consecutive_rejections >= budgets.consecutive_rejections {
            Some(HaltReason::BudgetRejections)
        } else if self.consecutive_failures >= budgets.consecutive_failures {
            Some(HaltReason::BudgetFailures)
        } else if self.identical_decisions >= budgets.identical_decisions {
            Some(HaltReason::Convergence)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests;
