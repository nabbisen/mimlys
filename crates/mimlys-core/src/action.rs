//! Actions, executions, termination, failure causes and dispositions
//! (external design §7; REQ-SBX-003, REQ-TOOL-009, REQ-TOOL-010,
//! REQ-PERS-008). Action content is immutable from `proposed`.

use crate::clock::Instant;
use crate::hash::Hash;
use crate::ids::{ActionId, ApprovalId, BlobId, ExecutionId, SnapshotId, StepId, TaskId};
use crate::rejection::RejectionCode;
use crate::step::Budgets;
use crate::task::HaltReason;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The action, execution and observation schema version this code writes
/// (REQ-MAIN-002), recorded on every task at creation (`task.versions`).
///
/// The first values describe the shapes by release: **1** is the shape of
/// 0.7.0 and 0.7.1; **2** is 0.8.0's, which added the termination
/// `process_limit` (the macOS count watchdog), and 0.9.0 has the same shape.
/// A task recorded before this version existed reads with the value of its
/// release (see `TaskVersions`). From here on: an additive field, or a value
/// added where a reader may ignore what it does not know, raises nothing; a
/// renamed or removed field, or a changed meaning, raises it by one, and
/// the change is announced in the changelog (REQ-MAIN-001; external design
/// §22).
pub const ACTION_SCHEMA_VERSION: u32 = 2;

/// The side-effect class of a tool and of its actions (external design §7.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectKind {
    /// No side effect and no data read.
    None,
    /// Reads inside the workspace.
    WorkspaceRead,
    /// Writes inside the workspace; always reversible.
    WorkspaceWrite,
    /// Reads a granted host path.
    HostRead,
    /// Writes a granted host path.
    HostWrite,
    /// Sends arguments to an external service, mutates nothing.
    ExternalRead,
    /// Mutates something outside the host.
    ExternalWrite,
}

impl EffectKind {
    /// Whether the effect writes anywhere.
    pub fn is_write(self) -> bool {
        matches!(
            self,
            EffectKind::WorkspaceWrite | EffectKind::HostWrite | EffectKind::ExternalWrite
        )
    }
}

/// The effect of an action as the disposition and recovery rules see it.
/// Handoff 003's `EffectDecl` converts into this.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffectView {
    /// The effect class.
    pub kind: EffectKind,
    /// Whether the effect can be undone (snapshot or tool backup).
    pub reversible: bool,
    /// Whether a second execution has the same result as one.
    pub idempotent: bool,
}

/// The lifecycle status of an action (external design §7.1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ActionStatus {
    /// Recorded; validation pending or done.
    Proposed,
    /// Refused before execution.
    Rejected {
        /// Why.
        code: RejectionCode,
    },
    /// Waiting for the user's approval.
    WaitingApproval,
    /// Validated and approved where required; budget reserved.
    Authorized,
    /// Executing.
    Running,
    /// Finished successfully.
    Succeeded,
    /// Finished with a failure.
    Failed {
        /// Why.
        cause: FailureCause,
    },
    /// Cancelled before or during execution.
    Cancelled,
    /// The runtime restarted while the action was running; its effect is unknown.
    Unknown,
}

impl ActionStatus {
    /// Whether the status is terminal for the action.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            ActionStatus::Rejected { .. }
                | ActionStatus::Succeeded
                | ActionStatus::Failed { .. }
                | ActionStatus::Cancelled
                | ActionStatus::Unknown
        )
    }

    /// The snake_case name without payload.
    pub fn name(&self) -> &'static str {
        match self {
            ActionStatus::Proposed => "proposed",
            ActionStatus::Rejected { .. } => "rejected",
            ActionStatus::WaitingApproval => "waiting_approval",
            ActionStatus::Authorized => "authorized",
            ActionStatus::Running => "running",
            ActionStatus::Succeeded => "succeeded",
            ActionStatus::Failed { .. } => "failed",
            ActionStatus::Cancelled => "cancelled",
            ActionStatus::Unknown => "unknown",
        }
    }
}

/// An input to the action state machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ActionInput {
    /// Validation or the user refused the action.
    Reject(RejectionCode),
    /// The action needs the user's approval.
    RequireApproval,
    /// Validation passed, approval satisfied where required, budget reserved.
    Authorize,
    /// Execution started (after the running intent is durable).
    Start,
    /// Execution finished successfully.
    Succeed,
    /// Execution finished with a failure.
    Fail(FailureCause),
    /// Cancelled by the user or by a task transition.
    Cancel,
    /// Found running at restart.
    MarkUnknown,
}

impl ActionInput {
    fn name(&self) -> &'static str {
        match self {
            ActionInput::Reject(_) => "reject",
            ActionInput::RequireApproval => "require_approval",
            ActionInput::Authorize => "authorize",
            ActionInput::Start => "start",
            ActionInput::Succeed => "succeed",
            ActionInput::Fail(_) => "fail",
            ActionInput::Cancel => "cancel",
            ActionInput::MarkUnknown => "mark_unknown",
        }
    }
}

/// An action transition was refused.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("input {input} is not valid for an action in status {from}")]
pub struct ActionStateError {
    /// The status the action was in.
    pub from: &'static str,
    /// The input's name.
    pub input: &'static str,
}

/// The immutable content of an action, fixed at proposal (external design §7.3).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionContent {
    tool: String,
    arguments: Value,
    action_hash: Hash,
    grant_set_hash: Hash,
    effect: EffectView,
    approval_required: bool,
    deadline_ms: i64,
}

impl ActionContent {
    /// The tool's name.
    pub fn tool(&self) -> &str {
        &self.tool
    }

    /// The canonical arguments.
    pub fn arguments(&self) -> &Value {
        &self.arguments
    }

    /// Hash of the canonical content (tool and arguments).
    pub fn action_hash(&self) -> &Hash {
        &self.action_hash
    }

    /// Hash of the grant set in force at proposal.
    pub fn grant_set_hash(&self) -> &Hash {
        &self.grant_set_hash
    }

    /// The effect.
    pub fn effect(&self) -> EffectView {
        self.effect
    }

    /// Whether approval is required.
    pub fn approval_required(&self) -> bool {
        self.approval_required
    }

    /// The effective deadline in milliseconds (REQ-TOOL-006).
    pub fn deadline_ms(&self) -> i64 {
        self.deadline_ms
    }
}

/// A validated request to run one tool (external design §7.3).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Action {
    /// Identifier.
    pub action_id: ActionId,
    /// Owning task.
    pub task_id: TaskId,
    /// Owning step.
    pub step_id: StepId,
    /// Immutable content.
    #[serde(flatten)]
    content: ActionContent,
    /// Lifecycle status.
    #[serde(flatten)]
    pub status: ActionStatus,
    /// The approval request, when one was made.
    pub approval_id: Option<ApprovalId>,
    /// The pre-execution snapshot of a write-effect action (line 4, REQ-WS-004).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_before: Option<SnapshotId>,
    /// Number of executions so far.
    pub executions: u32,
    /// When proposed.
    pub created_at: Instant,
    /// When a terminal status was reached.
    pub finished_at: Option<Instant>,
}

/// Everything needed to propose an action; the validator builds it.
#[derive(Clone, Debug)]
pub struct Proposal {
    /// Identifier.
    pub action_id: ActionId,
    /// Owning task.
    pub task_id: TaskId,
    /// Owning step.
    pub step_id: StepId,
    /// The tool's name.
    pub tool: String,
    /// Canonical arguments.
    pub arguments: Value,
    /// Hash of the grant set in force.
    pub grant_set_hash: Hash,
    /// The effect.
    pub effect: EffectView,
    /// Whether approval is required.
    pub approval_required: bool,
    /// The effective deadline in milliseconds.
    pub deadline_ms: i64,
}

impl Action {
    /// Records a proposal in status `proposed`; the content hash is
    /// computed here over `{"tool": …, "arguments": …}` (REQ-DEC-012).
    pub fn propose(proposal: Proposal, now: Instant) -> Self {
        let action_hash = crate::hash::hash_canonical(&serde_json::json!({
            "tool": proposal.tool,
            "arguments": proposal.arguments,
        }));
        Self {
            action_id: proposal.action_id,
            task_id: proposal.task_id,
            step_id: proposal.step_id,
            content: ActionContent {
                tool: proposal.tool,
                arguments: proposal.arguments,
                action_hash,
                grant_set_hash: proposal.grant_set_hash,
                effect: proposal.effect,
                approval_required: proposal.approval_required,
                deadline_ms: proposal.deadline_ms,
            },
            status: ActionStatus::Proposed,
            approval_id: None,
            snapshot_before: None,
            executions: 0,
            created_at: now,
            finished_at: None,
        }
    }

    /// The immutable content.
    pub fn content(&self) -> &ActionContent {
        &self.content
    }

    /// Applies `input` at `now` per external design §7.1, returning the
    /// new action or refusing. `Cancel` is also accepted before execution
    /// (from `proposed`, `waiting_approval` and `authorized`) so that a
    /// cancelled task leaves no action without a terminal status.
    pub fn transition(&self, input: ActionInput, now: Instant) -> Result<Action, ActionStateError> {
        let refuse = || ActionStateError {
            from: self.status.name(),
            input: input.name(),
        };
        let mut next = self.clone();
        let status = match (&self.status, &input) {
            (ActionStatus::Proposed, ActionInput::Reject(code)) => {
                ActionStatus::Rejected { code: *code }
            }
            (ActionStatus::Proposed, ActionInput::RequireApproval) => ActionStatus::WaitingApproval,
            (ActionStatus::Proposed, ActionInput::Authorize) => ActionStatus::Authorized,
            (ActionStatus::Proposed, ActionInput::Cancel) => ActionStatus::Cancelled,
            (ActionStatus::WaitingApproval, ActionInput::Reject(code)) => {
                ActionStatus::Rejected { code: *code }
            }
            (ActionStatus::WaitingApproval, ActionInput::Authorize) => ActionStatus::Authorized,
            (ActionStatus::WaitingApproval, ActionInput::Cancel) => ActionStatus::Cancelled,
            (ActionStatus::Authorized, ActionInput::Start) => {
                next.executions = self.executions.saturating_add(1);
                ActionStatus::Running
            }
            (ActionStatus::Authorized, ActionInput::Cancel) => ActionStatus::Cancelled,
            (ActionStatus::Running, ActionInput::Succeed) => ActionStatus::Succeeded,
            (ActionStatus::Running, ActionInput::Fail(cause)) => ActionStatus::Failed {
                cause: cause.clone(),
            },
            (ActionStatus::Running, ActionInput::Cancel) => ActionStatus::Cancelled,
            (ActionStatus::Running, ActionInput::MarkUnknown) => ActionStatus::Unknown,
            _ => return Err(refuse()),
        };
        if status.is_terminal() {
            next.finished_at = Some(now);
        }
        next.status = status;
        Ok(next)
    }

    /// Re-arms a failed action for a runtime retry: `failed` → `authorized`,
    /// allowed only while executions are below the budget (REQ-TOOL-009).
    pub fn rearm_for_retry(&self, budgets: &Budgets) -> Result<Action, ActionStateError> {
        match self.status {
            ActionStatus::Failed { .. } if self.executions < budgets.executions_per_action => {
                let mut next = self.clone();
                next.status = ActionStatus::Authorized;
                next.finished_at = None;
                Ok(next)
            }
            _ => Err(ActionStateError {
                from: self.status.name(),
                input: "rearm_for_retry",
            }),
        }
    }
}

/// The limit that ended an execution.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceLimit {
    /// LIM-S1.
    Fuel,
    /// LIM-S3 or LIM-H2.
    Memory,
    /// LIM-S4 or LIM-H4.
    Output,
    /// LIM-W1 or LIM-W2.
    Workspace,
    /// LIM-H3.
    Processes,
}

/// Why the worker, sandbox or tool infrastructure failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostFaultDetail {
    /// The module failed validation.
    InvalidModule,
    /// The sandbox or worker could not be configured as required (REQ-SBX-012).
    Unavailable,
    /// The worker process died.
    WorkerDied,
}

/// How an execution ended (external design §7.4; REQ-SBX-003).
/// `exit_code` exists only for `exited`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "termination", rename_all = "snake_case")]
pub enum Termination {
    /// The program ended by itself.
    Exited {
        /// Its exit code.
        exit_code: i32,
    },
    /// A WASM trap.
    Trapped {
        /// The trap's name, from the engine's fixed vocabulary.
        trap: String,
    },
    /// LIM-S1 reached.
    FuelExhausted,
    /// The execution deadline passed.
    Deadline,
    /// LIM-S3 or LIM-H2 reached.
    MemoryLimit,
    /// LIM-S4 or LIM-H4 reached.
    OutputLimit,
    /// LIM-H3 reached where the tree is ended for it (the macOS count
    /// watchdog); where the kernel refuses the extra process instead (Linux,
    /// Windows) the tool's own ending is reported. Never reported as a tool
    /// that exited by itself (REQ-SBX-003).
    ProcessLimit,
    /// Cancelled.
    Cancelled,
    /// The infrastructure failed.
    HostFault {
        /// What failed.
        detail: HostFaultDetail,
    },
}

impl Termination {
    /// The exit code, present only for `exited`.
    pub fn exit_code(&self) -> Option<i32> {
        match self {
            Termination::Exited { exit_code } => Some(*exit_code),
            _ => None,
        }
    }
}

/// Why an action failed (external design §7.5).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "cause", rename_all = "snake_case")]
pub enum FailureCause {
    /// The tool reported failure, exited non-zero, or trapped.
    ToolError {
        /// A fixed-vocabulary detail (`exit_code:N`, `trap:NAME`, a tool's own code).
        detail: String,
    },
    /// The deadline passed.
    Timeout,
    /// A resource limit ended the execution.
    ResourceExhausted {
        /// Which one.
        limit: ResourceLimit,
    },
    /// The worker, sandbox or tool infrastructure failed.
    SandboxFault {
        /// What failed.
        detail: HostFaultDetail,
    },
    /// A tool server or host tool could not be reached.
    ToolUnavailable,
    /// The result violated the tool's output schema (REQ-TOOL-005).
    OutputInvalid,
}

impl FailureCause {
    /// The snake_case name without payload.
    pub fn name(&self) -> &'static str {
        match self {
            FailureCause::ToolError { .. } => "tool_error",
            FailureCause::Timeout => "timeout",
            FailureCause::ResourceExhausted { .. } => "resource_exhausted",
            FailureCause::SandboxFault { .. } => "sandbox_fault",
            FailureCause::ToolUnavailable => "tool_unavailable",
            FailureCause::OutputInvalid => "output_invalid",
        }
    }

    /// The cause implied by a termination, or `None` when the execution
    /// succeeded (`exited` with code 0) or was cancelled.
    pub fn from_termination(termination: &Termination) -> Option<FailureCause> {
        match termination {
            Termination::Exited { exit_code: 0 } => None,
            Termination::Exited { exit_code } => Some(FailureCause::ToolError {
                detail: format!("exit_code:{exit_code}"),
            }),
            Termination::Trapped { trap } => Some(FailureCause::ToolError {
                detail: format!("trap:{trap}"),
            }),
            Termination::FuelExhausted => Some(FailureCause::ResourceExhausted {
                limit: ResourceLimit::Fuel,
            }),
            Termination::Deadline => Some(FailureCause::Timeout),
            Termination::MemoryLimit => Some(FailureCause::ResourceExhausted {
                limit: ResourceLimit::Memory,
            }),
            Termination::OutputLimit => Some(FailureCause::ResourceExhausted {
                limit: ResourceLimit::Output,
            }),
            Termination::ProcessLimit => Some(FailureCause::ResourceExhausted {
                limit: ResourceLimit::Processes,
            }),
            Termination::Cancelled => None,
            Termination::HostFault { detail } => {
                Some(FailureCause::SandboxFault { detail: *detail })
            }
        }
    }
}

/// What the policy does with a failure (external design §7.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "disposition", rename_all = "snake_case")]
pub enum DispositionKind {
    /// The observation goes to the model.
    Feedback,
    /// The runtime re-executes without consulting the model.
    Retry,
    /// The task halts.
    Halt {
        /// Why.
        reason: HaltReason,
    },
}

/// A disposition with the rollback rule of REQ-TOOL-010 attached.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Disposition {
    /// What happens next.
    pub kind: DispositionKind,
    /// Whether the pre-execution snapshot is restored first (a failed
    /// reversible write effect is always rolled back before anything else).
    pub rollback_first: bool,
}

/// Decides what follows a failed execution (external design §7.5;
/// REQ-TOOL-009, REQ-TOOL-010). `executions` counts the execution that
/// just failed. Dispositions never consult the model.
pub fn disposition(
    cause: &FailureCause,
    effect: EffectView,
    executions: u32,
    budgets: &Budgets,
) -> Disposition {
    let rollback_first = effect.kind.is_write() && effect.reversible;
    let retries_left = executions < budgets.executions_per_action;
    let may_retry =
        effect.idempotent && retries_left && (!effect.kind.is_write() || effect.reversible);
    let kind = match cause {
        FailureCause::Timeout | FailureCause::ToolUnavailable => {
            if may_retry {
                DispositionKind::Retry
            } else if matches!(cause, FailureCause::ToolUnavailable) {
                DispositionKind::Halt {
                    reason: HaltReason::ToolUnavailable,
                }
            } else {
                DispositionKind::Feedback
            }
        }
        FailureCause::ResourceExhausted {
            limit: ResourceLimit::Workspace,
        } => DispositionKind::Halt {
            reason: HaltReason::WorkspaceQuota,
        },
        FailureCause::SandboxFault { detail } => match detail {
            HostFaultDetail::WorkerDied if effect.idempotent && executions == 1 && retries_left => {
                DispositionKind::Retry
            }
            HostFaultDetail::Unavailable => DispositionKind::Halt {
                reason: HaltReason::ToolUnavailable,
            },
            _ => DispositionKind::Feedback,
        },
        FailureCause::ToolError { .. }
        | FailureCause::ResourceExhausted { .. }
        | FailureCause::OutputInvalid => DispositionKind::Feedback,
    };
    Disposition {
        kind,
        rollback_first,
    }
}

/// What a non-reversible or non-idempotent effect means at recovery (external design §7.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AfterUnknown {
    /// Re-execution is permitted (never automatic, REQ-PERS-008).
    RedoPermitted,
    /// Restore the pre-execution snapshot, then re-execution is permitted.
    Rollback,
    /// Undo through the tool's backup, then halt `recovered`.
    UndoThenHalt,
    /// Halt `unknown_outcome` with the action listed.
    Halt,
}

/// The recovery rule for an action whose outcome is unknown (external design §7.2).
pub fn after_unknown(effect: EffectView) -> AfterUnknown {
    match effect.kind {
        EffectKind::None | EffectKind::WorkspaceRead | EffectKind::HostRead => {
            AfterUnknown::RedoPermitted
        }
        EffectKind::WorkspaceWrite => AfterUnknown::Rollback,
        EffectKind::HostWrite if effect.reversible => AfterUnknown::UndoThenHalt,
        EffectKind::HostWrite => AfterUnknown::Halt,
        EffectKind::ExternalRead if effect.idempotent => AfterUnknown::RedoPermitted,
        EffectKind::ExternalRead => AfterUnknown::Halt,
        EffectKind::ExternalWrite => AfterUnknown::Halt,
    }
}

/// Measurements of one execution (external design §7.4).
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ExecutionMetrics {
    /// Wall-clock milliseconds.
    pub wall_ms: i64,
    /// Fuel consumed, when fuel accounting applies.
    pub fuel: Option<u64>,
    /// Peak memory in bytes, when measured.
    pub memory_peak_bytes: Option<u64>,
    /// Bytes of standard output captured.
    pub stdout_bytes: u64,
    /// Bytes of standard error captured.
    pub stderr_bytes: u64,
    /// Sandbox setup milliseconds, when measured (REQ-SBX-009).
    pub setup_ms: Option<i64>,
}

/// One attempt to run an action (external design §7.4).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Execution {
    /// Identifier.
    pub execution_id: ExecutionId,
    /// The action.
    pub action_id: ActionId,
    /// One-based attempt number.
    pub attempt: u32,
    /// Start.
    pub started_at: Instant,
    /// End.
    pub ended_at: Instant,
    /// How it ended.
    #[serde(flatten)]
    pub termination: Termination,
    /// Workspace snapshot before (write effects, M3).
    pub snapshot_before: Option<SnapshotId>,
    /// Workspace snapshot after success (write effects, M3).
    pub snapshot_after: Option<SnapshotId>,
    /// Stored standard output, when any.
    pub stdout_blob: Option<BlobId>,
    /// Stored standard error, when any.
    pub stderr_blob: Option<BlobId>,
    /// Measurements.
    pub metrics: ExecutionMetrics,
}

#[cfg(test)]
mod tests;
