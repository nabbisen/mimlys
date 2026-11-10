//! The event vocabulary (external design §17.1; REQ-OBS-001). Events are
//! values; the persistence layer appends them in the transaction of the
//! transition they describe. Names are serialized in their dotted form.

use crate::clock::Instant;
use crate::hash::Hash;
use crate::ids::{ActionId, ApprovalId, EventId, GrantId, QuestionId, SnapshotId, StepId, TaskId};
use crate::native::DecisionTransport;
use crate::rejection::RejectionCode;
use crate::step::StepOutcome;
use crate::task::HaltReason;
use serde::{Deserialize, Serialize};

/// Who caused an event.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "actor", rename_all = "snake_case")]
pub enum Actor {
    /// A user through a session.
    User {
        /// The session (v1: the CLI process).
        session: String,
        /// The operating-system user.
        os_user: String,
    },
    /// The runtime itself.
    Runtime,
    /// The model (only for proposals it made).
    Model,
    /// A tool.
    Tool {
        /// The tool's name.
        name: String,
    },
}

/// What the user decided on an approval request.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecision {
    /// Approved.
    Approved,
    /// Rejected.
    Rejected,
    /// Expired.
    Expired,
}

/// The disposition recorded by recovery.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryDisposition {
    /// No action was in flight.
    Clean,
    /// A reversible action was rolled back.
    RolledBack,
    /// A non-reversible action has an unknown outcome.
    UnknownOutcome,
}

/// The installation's tier.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    /// Sandbox only.
    Core,
    /// Confined host tools available.
    Full,
}

/// An event (external design §17.1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event")]
pub enum Event {
    /// A task was created.
    #[serde(rename = "task.created")]
    TaskCreated,
    /// A task changed state.
    #[serde(rename = "task.state_changed")]
    TaskStateChanged {
        /// Previous state name.
        from: String,
        /// New state name.
        to: String,
        /// Halt reason when the new state is `halted`.
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<HaltReason>,
    },
    /// The user requested a pause; it takes effect at the next step boundary.
    #[serde(rename = "task.pause_requested")]
    TaskPauseRequested,
    /// The user deleted the task's workspace, snapshots and blobs (RFC 006 §6).
    #[serde(rename = "task.deleted")]
    TaskDeleted {
        /// Whether the audit (the task's events and records) went too.
        audit: bool,
    },
    /// A step started.
    #[serde(rename = "step.started")]
    StepStarted,
    /// A step finished.
    #[serde(rename = "step.finished")]
    StepFinished {
        /// The outcome.
        outcome: StepOutcome,
    },
    /// A decision was received.
    #[serde(rename = "decision.received")]
    DecisionReceived {
        /// Its hash: the canonical hash of the decision envelope, which in the
        /// `tool_calls` transport is the envelope the call was mapped onto.
        hash: Hash,
        /// How the decision arrived (from 0.9; `envelope` in records without it).
        #[serde(default)]
        transport: DecisionTransport,
        /// Characters of text beside a structured call: not part of the
        /// decision, not parsed, not put into the context (from 0.9).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text_chars: Option<u64>,
    },
    /// A decision was rejected.
    #[serde(rename = "decision.rejected")]
    DecisionRejected {
        /// The code.
        code: RejectionCode,
    },
    /// An action was proposed.
    #[serde(rename = "action.proposed")]
    ActionProposed,
    /// An action was authorized.
    #[serde(rename = "action.authorized")]
    ActionAuthorized,
    /// An action started executing.
    #[serde(rename = "action.started")]
    ActionStarted,
    /// One execution of an action ended (line 6 of external design §16.2);
    /// an action with retries has several.
    #[serde(rename = "execution.finished")]
    ExecutionFinished {
        /// The attempt, from 1.
        attempt: u32,
        /// The termination's name.
        termination: String,
    },
    /// An action reached a terminal status (line 7).
    #[serde(rename = "action.finished")]
    ActionFinished {
        /// The status name.
        status: String,
        /// The failure cause name, when failed.
        #[serde(skip_serializing_if = "Option::is_none")]
        cause: Option<String>,
    },
    /// An approval was requested.
    #[serde(rename = "approval.requested")]
    ApprovalRequested {
        /// The request.
        approval_id: ApprovalId,
    },
    /// An approval was decided.
    #[serde(rename = "approval.decided")]
    ApprovalDecided {
        /// The request.
        approval_id: ApprovalId,
        /// The decision.
        decision: ApprovalDecision,
    },
    /// An approval was invalidated (hash mismatch or revoked grant).
    #[serde(rename = "approval.invalidated")]
    ApprovalInvalidated {
        /// The request.
        approval_id: ApprovalId,
    },
    /// A question was asked.
    #[serde(rename = "question.asked")]
    QuestionAsked {
        /// The question.
        question_id: QuestionId,
    },
    /// A question was answered.
    #[serde(rename = "question.answered")]
    QuestionAnswered {
        /// The question.
        question_id: QuestionId,
    },
    /// A grant was issued.
    #[serde(rename = "grant.issued")]
    GrantIssued {
        /// The grant.
        grant_id: GrantId,
    },
    /// A grant was revoked.
    #[serde(rename = "grant.revoked")]
    GrantRevoked {
        /// The grant.
        grant_id: GrantId,
    },
    /// The workspace was materialized.
    #[serde(rename = "workspace.materialized")]
    WorkspaceMaterialized,
    /// A snapshot was taken.
    #[serde(rename = "workspace.snapshot")]
    WorkspaceSnapshot {
        /// The snapshot.
        snapshot_id: SnapshotId,
        /// Why.
        reason: String,
    },
    /// A snapshot was restored.
    #[serde(rename = "workspace.rollback")]
    WorkspaceRollback {
        /// The snapshot.
        snapshot_id: SnapshotId,
    },
    /// Changes were promoted to the project scope.
    #[serde(rename = "workspace.promoted")]
    WorkspacePromoted {
        /// The files written.
        files: Vec<String>,
    },
    /// A task was recovered at startup.
    #[serde(rename = "task.recovered")]
    TaskRecovered {
        /// What happened.
        disposition: RecoveryDisposition,
    },
    /// A tool definition was pinned.
    #[serde(rename = "tool.pinned")]
    ToolPinned {
        /// The tool.
        tool: String,
        /// The definition's hash.
        hash: Hash,
    },
    /// A tool definition changed and was unpinned.
    #[serde(rename = "tool.unpinned")]
    ToolUnpinned {
        /// The tool.
        tool: String,
    },
    /// An MCP server process started (RFC 009).
    #[serde(rename = "server.started")]
    ServerStarted {
        /// The configured server name.
        server: String,
        /// The server's reported version, when any.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        version: Option<String>,
    },
    /// An MCP server process stopped.
    #[serde(rename = "server.stopped")]
    ServerStopped {
        /// The configured server name.
        server: String,
        /// `shutdown`, `died`, `killed`.
        reason: String,
    },
    /// The runtime started.
    #[serde(rename = "runtime.started")]
    RuntimeStarted {
        /// The tier.
        tier: Tier,
        /// The backend in force (REQ-SEC-010).
        backend: crate::spi::BackendDescriptor,
    },
    /// The runtime stopped.
    #[serde(rename = "runtime.stopped")]
    RuntimeStopped,
    /// Processes left by a previous runtime were terminated at startup (REQ-CONF-009).
    #[serde(rename = "runtime.orphans_killed")]
    RuntimeOrphansKilled {
        /// How many.
        count: u32,
    },
}

impl Event {
    /// The dotted name on the wire.
    pub fn name(&self) -> &'static str {
        match self {
            Event::TaskCreated => "task.created",
            Event::TaskStateChanged { .. } => "task.state_changed",
            Event::TaskPauseRequested => "task.pause_requested",
            Event::TaskDeleted { .. } => "task.deleted",
            Event::StepStarted => "step.started",
            Event::StepFinished { .. } => "step.finished",
            Event::DecisionReceived { .. } => "decision.received",
            Event::DecisionRejected { .. } => "decision.rejected",
            Event::ActionProposed => "action.proposed",
            Event::ActionAuthorized => "action.authorized",
            Event::ActionStarted => "action.started",
            Event::ExecutionFinished { .. } => "execution.finished",
            Event::ActionFinished { .. } => "action.finished",
            Event::ApprovalRequested { .. } => "approval.requested",
            Event::ApprovalDecided { .. } => "approval.decided",
            Event::ApprovalInvalidated { .. } => "approval.invalidated",
            Event::QuestionAsked { .. } => "question.asked",
            Event::QuestionAnswered { .. } => "question.answered",
            Event::GrantIssued { .. } => "grant.issued",
            Event::GrantRevoked { .. } => "grant.revoked",
            Event::WorkspaceMaterialized => "workspace.materialized",
            Event::WorkspaceSnapshot { .. } => "workspace.snapshot",
            Event::WorkspaceRollback { .. } => "workspace.rollback",
            Event::WorkspacePromoted { .. } => "workspace.promoted",
            Event::TaskRecovered { .. } => "task.recovered",
            Event::ToolPinned { .. } => "tool.pinned",
            Event::ToolUnpinned { .. } => "tool.unpinned",
            Event::ServerStarted { .. } => "server.started",
            Event::ServerStopped { .. } => "server.stopped",
            Event::RuntimeStarted { .. } => "runtime.started",
            Event::RuntimeStopped => "runtime.stopped",
            Event::RuntimeOrphansKilled { .. } => "runtime.orphans_killed",
        }
    }
}

/// The envelope of a persisted event (external design §17.1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventRecord {
    /// Identifier.
    pub event_id: EventId,
    /// When (LIM-U4).
    pub at: Instant,
    /// Who.
    #[serde(flatten)]
    pub actor: Actor,
    /// The task, when applicable.
    pub task_id: Option<TaskId>,
    /// The step, when applicable.
    pub step_id: Option<StepId>,
    /// The action, when applicable.
    pub action_id: Option<ActionId>,
    /// The policy in force.
    pub policy_hash: Hash,
    /// The event.
    #[serde(flatten)]
    pub event: Event,
}

#[cfg(test)]
mod tests;
