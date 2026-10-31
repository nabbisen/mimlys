//! The task state machine (external design §5; REQ-TASK-002 … 012). A task
//! is in exactly one of nine states; `transition` implements the table of
//! external design §5.1 and refuses everything else. Terminal states are
//! immutable (REQ-TASK-003).

use crate::clock::Instant;
use crate::ids::{ActionId, QuestionId, TaskId};
use crate::step::{Budgets, Counters};
use serde::{Deserialize, Serialize};

/// What a task in `waiting_user` waits for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WaitKind {
    /// An approval request is pending.
    Approval,
    /// A question to the user is pending.
    Question,
}

/// Why the runtime stopped a task (external design §5.2, REQ-TASK-007).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HaltReason {
    /// LIM-T1 reached.
    BudgetSteps,
    /// LIM-T2 reached.
    BudgetRejections,
    /// LIM-T6 reached.
    BudgetFailures,
    /// The deadline passed.
    BudgetDeadline,
    /// LIM-T4 reached.
    Convergence,
    /// The user rejected an action with the halt flag, or the profile halts on rejection.
    ApprovalRejected,
    /// An approval request expired.
    ApprovalExpired,
    /// A question expired.
    QuestionExpired,
    /// Recovered after a restart; the user decides whether to continue.
    Recovered,
    /// An in-flight action with a non-reversible effect has an unknown outcome.
    UnknownOutcome,
    /// LIM-W1 or LIM-W2 breached.
    WorkspaceQuota,
    /// The model backend could not be reached after retries.
    BackendUnavailable,
    /// A tool could not be reached after retries.
    ToolUnavailable,
    /// Policy stopped the task.
    Policy,
}

impl HaltReason {
    /// Every reason, in the order of external design §5.2.
    pub const ALL: [HaltReason; 14] = [
        HaltReason::BudgetSteps,
        HaltReason::BudgetRejections,
        HaltReason::BudgetFailures,
        HaltReason::BudgetDeadline,
        HaltReason::Convergence,
        HaltReason::ApprovalRejected,
        HaltReason::ApprovalExpired,
        HaltReason::QuestionExpired,
        HaltReason::Recovered,
        HaltReason::UnknownOutcome,
        HaltReason::WorkspaceQuota,
        HaltReason::BackendUnavailable,
        HaltReason::ToolUnavailable,
        HaltReason::Policy,
    ];
}

/// How a task completed (external design §5.3, REQ-TASK-010).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Completion {
    /// The configured completion check passed.
    Verified,
    /// The model declared completion and no check was configured.
    Asserted,
}

/// Why a task failed (external design §5.3, REQ-TASK-011).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "failure_reason", rename_all = "snake_case")]
pub enum FailureReason {
    /// The model gave up; `reason` is its bounded text.
    ModelGaveUp {
        /// The model's stated reason (bounded by LIM-D2 at validation).
        reason: String,
    },
    /// The runtime met an unrecoverable condition.
    Unrecoverable {
        /// A fixed-vocabulary detail from the runtime.
        detail: String,
    },
    /// The workspace no longer exists.
    WorkspaceLost,
}

/// The state of a task. Serialized flat: a `state` field plus the payload's fields.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum TaskState {
    /// Persisted, not started.
    Created,
    /// The loop is active.
    Running,
    /// An approval or a question is pending; no resources are held.
    WaitingUser {
        /// What the task waits for.
        wait_kind: WaitKind,
    },
    /// The user paused it.
    Paused,
    /// The runtime stopped it; the user resumes or cancels.
    Halted {
        /// Why.
        halt_reason: HaltReason,
    },
    /// Found running at startup; resolves to `halted` or `running`.
    Recovering,
    /// Terminal: the goal is reached.
    Completed {
        /// Verified or asserted.
        completion: Completion,
    },
    /// Terminal: the task could not be completed.
    Failed {
        /// Why.
        reason: FailureReason,
    },
    /// Terminal: cancelled by the user.
    Cancelled,
}

impl TaskState {
    /// Whether the state is terminal (immutable thereafter, REQ-TASK-003).
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            TaskState::Completed { .. } | TaskState::Failed { .. } | TaskState::Cancelled
        )
    }

    /// The snake_case name of the state without its payload.
    pub fn name(&self) -> &'static str {
        match self {
            TaskState::Created => "created",
            TaskState::Running => "running",
            TaskState::WaitingUser { .. } => "waiting_user",
            TaskState::Paused => "paused",
            TaskState::Halted { .. } => "halted",
            TaskState::Recovering => "recovering",
            TaskState::Completed { .. } => "completed",
            TaskState::Failed { .. } => "failed",
            TaskState::Cancelled => "cancelled",
        }
    }
}

/// The pending user item of a task in `waiting_user`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "pending", rename_all = "snake_case")]
pub enum PendingItem {
    /// An approval request for the action.
    Approval {
        /// The action awaiting approval.
        action_id: ActionId,
    },
    /// A question to the user.
    Question {
        /// The question.
        question_id: QuestionId,
    },
}

impl PendingItem {
    /// The wait kind of this item.
    pub fn wait_kind(&self) -> WaitKind {
        match self {
            PendingItem::Approval { .. } => WaitKind::Approval,
            PendingItem::Question { .. } => WaitKind::Question,
        }
    }
}

/// A runtime-verifiable completion check: a tool action that must succeed
/// after the last mutation (external design §5.3; REQ-TASK-010).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompletionCheck {
    /// The tool to run.
    pub tool: String,
    /// Its arguments.
    pub arguments: serde_json::Map<String, serde_json::Value>,
}

/// What the loop does with a `complete` decision (REQ-TASK-010).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompletionStep {
    /// End the task `completed` with this completion kind.
    Finish(Completion),
    /// Run the configured check first.
    RunCheck,
    /// The check failed: the result is an observation, the task continues,
    /// and the failure counts toward LIM-T6.
    CheckFailed,
}

/// The project scope a task is materialized from (external design §14.2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScopeSpec {
    /// Absolute path of the project root on the host.
    pub root: String,
    /// Include patterns relative to the root; empty means everything.
    #[serde(default)]
    pub include: Vec<String>,
}

impl ScopeSpec {
    /// Whether two scopes overlap: one root is the other or lies below it (REQ-TASK-012).
    pub fn overlaps(&self, other: &ScopeSpec) -> bool {
        let a = self.root.trim_end_matches(['/', '\\']).replace('\\', "/");
        let b = other.root.trim_end_matches(['/', '\\']).replace('\\', "/");
        a == b || a.starts_with(&format!("{b}/")) || b.starts_with(&format!("{a}/"))
    }
}

/// The versions in force when a task was created (REQ-PERS-012).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskVersions {
    /// Hash of the policy.
    pub policy_hash: crate::hash::Hash,
    /// The decision schema version.
    pub decision_schema_version: u32,
    /// Hash of each tool definition visible at creation, by tool name.
    pub tool_definition_hashes: std::collections::BTreeMap<String, crate::hash::Hash>,
    /// The runtime's version.
    pub runtime_version: String,
    /// The backend at creation (REQ-SEC-010); absent on records written before it was recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend: Option<crate::spi::BackendDescriptor>,
    /// The external tools imported at creation, by `server.tool` (RFC 009 §2).
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub tool_imports: std::collections::BTreeMap<String, crate::tool::ImportRecord>,
}

/// One immutable version of a task's plan (external design §6.2, REQ-DEC-011).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanVersion {
    /// The task.
    pub task_id: TaskId,
    /// One-based version.
    pub version: u32,
    /// Intended steps.
    pub steps: Vec<String>,
    /// When created.
    pub created_at: Instant,
}

/// A persistent unit of work (external design §5; REQ-TASK-001).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    /// Identifier.
    pub task_id: TaskId,
    /// The user's goal (bounded by LIM-D4 at creation).
    pub goal: String,
    /// Current state.
    #[serde(flatten)]
    pub state: TaskState,
    /// Creation instant.
    pub created_at: Instant,
    /// Instant of the last transition.
    pub updated_at: Instant,
    /// Absolute deadline (REQ-TASK-009).
    pub deadline: Instant,
    /// Budgets in force.
    pub budgets: Budgets,
    /// Durable counters (REQ-TASK-008).
    pub counters: Counters,
    /// A pause was requested and takes effect at the next step boundary.
    pub pause_requested: bool,
    /// The pending user item while `waiting_user`.
    pub pending: Option<PendingItem>,
    /// The completion check, when configured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion_check: Option<CompletionCheck>,
    /// The versions in force at creation.
    pub versions: TaskVersions,
    /// The profile name.
    pub profile: String,
    /// The project scope.
    pub scope: ScopeSpec,
    /// The task this one was forked from (REQ-TASK-004).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forked_from: Option<TaskId>,
}

/// A budget extension requested at resumption from a budget halt (REQ-TASK-005).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Extension {
    /// New step limit, if raised.
    pub steps: Option<u32>,
    /// New deadline, if moved later.
    pub deadline: Option<Instant>,
}

/// The ceiling an extension must respect (from the administrator's policy, handoff 003).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExtensionCeiling {
    /// Highest step limit the profile allows.
    pub max_steps: u32,
    /// Latest deadline the profile allows, measured from creation.
    pub max_deadline: Instant,
}

/// An input to the task state machine (one row of external design §5.1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TaskInput {
    /// `resume_task`, valid from `created`, `paused`, `halted`.
    Resume {
        /// Budget extension, valid only from a budget halt.
        extend: Option<Extension>,
        /// The ceiling for the extension.
        ceiling: ExtensionCeiling,
    },
    /// `pause_task`: records the request; the loop pauses at the boundary.
    PauseRequest,
    /// The loop reached a step boundary with a pause requested.
    Pause,
    /// `cancel_task`, valid from every non-terminal state.
    Cancel,
    /// An approval or question is pending.
    NeedsUser(PendingItem),
    /// The user approved, rejected without halt, or answered.
    UserDecided,
    /// The user rejected with the halt flag.
    UserHalted,
    /// The pending item expired.
    Expired,
    /// The runtime halts the task.
    Halt(HaltReason),
    /// The task completed.
    Complete(Completion),
    /// The task failed.
    Fail(FailureReason),
    /// Runtime start found the task running.
    StartRecovery,
    /// Recovery finished: `resume` continues the loop, otherwise the task halts with `reason`.
    Recovered {
        /// Continue the loop (policy `recovery.auto_resume` with a clean disposition).
        resume: bool,
        /// Halt reason when not resuming (`Recovered` or `UnknownOutcome`).
        reason: HaltReason,
    },
}

/// A transition was refused (API error kind `state`).
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum StateError {
    /// The input is not valid in the current state.
    #[error("input {input} is not valid in state {from}")]
    Illegal {
        /// The state the task was in.
        from: &'static str,
        /// The input's name.
        input: &'static str,
    },
    /// An extension was requested where none is allowed, or beyond the ceiling.
    #[error("budget extension refused: {reason}")]
    Extension {
        /// What was wrong.
        reason: &'static str,
    },
    /// A resume would halt again at once because the deadline has passed.
    #[error("the deadline has passed; resume with a later deadline")]
    DeadlinePassed,
    /// A resume would halt again at once because the step budget is used up.
    #[error("the step budget is used up; resume with a higher step limit")]
    StepBudgetExhausted,
}

impl TaskInput {
    fn name(&self) -> &'static str {
        match self {
            TaskInput::Resume { .. } => "resume",
            TaskInput::PauseRequest => "pause_request",
            TaskInput::Pause => "pause",
            TaskInput::Cancel => "cancel",
            TaskInput::NeedsUser(_) => "needs_user",
            TaskInput::UserDecided => "user_decided",
            TaskInput::UserHalted => "user_halted",
            TaskInput::Expired => "expired",
            TaskInput::Halt(_) => "halt",
            TaskInput::Complete(_) => "complete",
            TaskInput::Fail(_) => "fail",
            TaskInput::StartRecovery => "start_recovery",
            TaskInput::Recovered { .. } => "recovered",
        }
    }
}

/// What a new task is made of (REQ-TASK-001).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskSpec {
    /// The task's id.
    pub task_id: TaskId,
    /// The goal.
    pub goal: String,
    /// Milliseconds from creation to the deadline (LIM-T5).
    pub deadline_ms: i64,
    /// Budgets.
    pub budgets: Budgets,
    /// Profile name.
    pub profile: String,
    /// Pinned versions.
    pub versions: TaskVersions,
    /// Project scope.
    pub scope: ScopeSpec,
}

impl Task {
    /// Creates a task in `created` (REQ-TASK-001) with the deadline at
    /// `now + deadline_ms` (REQ-TASK-009).
    pub fn create(spec: TaskSpec, now: Instant) -> Result<Self, crate::clock::InstantError> {
        Ok(Self {
            task_id: spec.task_id,
            goal: spec.goal,
            state: TaskState::Created,
            created_at: now,
            updated_at: now,
            deadline: now.plus_millis(spec.deadline_ms)?,
            budgets: spec.budgets,
            counters: Counters::default(),
            pause_requested: false,
            pending: None,
            completion_check: None,
            versions: spec.versions,
            profile: spec.profile,
            scope: spec.scope,
            forked_from: None,
        })
    }

    /// Sets the completion check.
    pub fn with_completion_check(mut self, check: CompletionCheck) -> Self {
        self.completion_check = Some(check);
        self
    }

    /// The completion rule of external design §5.3: without a check the
    /// task finishes `asserted`; with one, the check runs first, and the
    /// task finishes `verified` only when it passed.
    pub fn completion_step(&self, check_result: Option<bool>) -> CompletionStep {
        match (&self.completion_check, check_result) {
            (None, _) => CompletionStep::Finish(Completion::Asserted),
            (Some(_), None) => CompletionStep::RunCheck,
            (Some(_), Some(true)) => CompletionStep::Finish(Completion::Verified),
            (Some(_), Some(false)) => CompletionStep::CheckFailed,
        }
    }

    /// Milliseconds until the deadline; zero or negative when it has passed.
    pub fn remaining_ms(&self, now: Instant) -> i64 {
        now.millis_until(self.deadline)
    }

    /// Whether the deadline has passed.
    pub fn deadline_passed(&self, now: Instant) -> bool {
        self.remaining_ms(now) <= 0
    }

    /// Applies `input` at `now`, returning the new task or refusing
    /// (external design §5.1). The task itself is not modified.
    pub fn transition(&self, input: TaskInput, now: Instant) -> Result<Task, StateError> {
        let illegal = || StateError::Illegal {
            from: self.state.name(),
            input: input.name(),
        };
        let mut next = self.clone();
        next.updated_at = now;
        let state = match (&self.state, &input) {
            // created
            (TaskState::Created, TaskInput::Resume { extend, .. }) => {
                if extend.is_some() {
                    return Err(StateError::Extension {
                        reason: "an extension is allowed only from a budget halt",
                    });
                }
                TaskState::Running
            }
            (TaskState::Created, TaskInput::Cancel) => TaskState::Cancelled,

            // running
            (TaskState::Running, TaskInput::PauseRequest) => {
                next.pause_requested = true;
                TaskState::Running
            }
            (TaskState::Running, TaskInput::Pause) => {
                next.pause_requested = false;
                TaskState::Paused
            }
            (TaskState::Running, TaskInput::NeedsUser(item)) => {
                next.pending = Some(*item);
                TaskState::WaitingUser {
                    wait_kind: item.wait_kind(),
                }
            }
            (TaskState::Running, TaskInput::Halt(reason)) => TaskState::Halted {
                halt_reason: *reason,
            },
            (TaskState::Running, TaskInput::Complete(completion)) => TaskState::Completed {
                completion: *completion,
            },
            (TaskState::Running, TaskInput::Fail(reason)) => TaskState::Failed {
                reason: reason.clone(),
            },
            (TaskState::Running, TaskInput::Cancel) => TaskState::Cancelled,
            (TaskState::Running, TaskInput::StartRecovery) => TaskState::Recovering,

            // waiting_user
            (TaskState::WaitingUser { .. }, TaskInput::UserDecided) => {
                next.pending = None;
                TaskState::Running
            }
            (TaskState::WaitingUser { .. }, TaskInput::UserHalted) => {
                next.pending = None;
                TaskState::Halted {
                    halt_reason: HaltReason::ApprovalRejected,
                }
            }
            (TaskState::WaitingUser { wait_kind }, TaskInput::Expired) => {
                next.pending = None;
                TaskState::Halted {
                    halt_reason: match wait_kind {
                        WaitKind::Approval => HaltReason::ApprovalExpired,
                        WaitKind::Question => HaltReason::QuestionExpired,
                    },
                }
            }
            (TaskState::WaitingUser { .. }, TaskInput::Cancel) => {
                next.pending = None;
                TaskState::Cancelled
            }

            // paused
            (TaskState::Paused, TaskInput::Resume { extend, .. }) => {
                if extend.is_some() {
                    return Err(StateError::Extension {
                        reason: "an extension is allowed only from a budget halt",
                    });
                }
                TaskState::Running
            }
            (TaskState::Paused, TaskInput::Cancel) => TaskState::Cancelled,

            // halted
            (TaskState::Halted { halt_reason }, TaskInput::Resume { extend, ceiling }) => {
                if let Some(extension) = extend {
                    apply_extension(&mut next, *halt_reason, *extension, *ceiling)?;
                }
                // A resume that would halt again at the first boundary is refused (REQ-TASK-005).
                if next.deadline_passed(now) {
                    return Err(StateError::DeadlinePassed);
                }
                if next.counters.steps >= next.budgets.steps {
                    return Err(StateError::StepBudgetExhausted);
                }
                next.pause_requested = false;
                TaskState::Running
            }
            (TaskState::Halted { .. }, TaskInput::Cancel) => TaskState::Cancelled,

            // recovering
            (TaskState::Recovering, TaskInput::Recovered { resume: true, .. }) => {
                TaskState::Running
            }
            (
                TaskState::Recovering,
                TaskInput::Recovered {
                    resume: false,
                    reason,
                },
            ) => TaskState::Halted {
                halt_reason: *reason,
            },
            (TaskState::Recovering, TaskInput::Cancel) => TaskState::Cancelled,

            _ => return Err(illegal()),
        };
        next.state = state;
        if next.state.is_terminal() {
            next.pause_requested = false;
            next.pending = None;
        }
        Ok(next)
    }
}

fn apply_extension(
    task: &mut Task,
    halt_reason: HaltReason,
    extension: Extension,
    ceiling: ExtensionCeiling,
) -> Result<(), StateError> {
    match halt_reason {
        HaltReason::BudgetSteps | HaltReason::BudgetDeadline => {}
        _ => {
            return Err(StateError::Extension {
                reason: "an extension is allowed only from a budget halt",
            });
        }
    }
    if let Some(steps) = extension.steps {
        if steps <= task.budgets.steps {
            return Err(StateError::Extension {
                reason: "the step limit can only be raised",
            });
        }
        if steps > ceiling.max_steps {
            return Err(StateError::Extension {
                reason: "the step limit exceeds the ceiling",
            });
        }
        task.budgets.steps = steps;
    }
    if let Some(deadline) = extension.deadline {
        if deadline <= task.deadline {
            return Err(StateError::Extension {
                reason: "the deadline can only be moved later",
            });
        }
        if deadline > ceiling.max_deadline {
            return Err(StateError::Extension {
                reason: "the deadline exceeds the ceiling",
            });
        }
        task.deadline = deadline;
    }
    Ok(())
}

/// Versions for tests and replay fixtures (REQ-PERS-012).
#[cfg(any(test, feature = "test-support"))]
pub fn test_versions() -> TaskVersions {
    TaskVersions {
        policy_hash: crate::hash::hash_bytes(b"policy"),
        decision_schema_version: 1,
        tool_definition_hashes: std::collections::BTreeMap::new(),
        runtime_version: "0.0.0-test".into(),
        backend: None,
        tool_imports: Default::default(),
    }
}

#[cfg(test)]
mod tests;
