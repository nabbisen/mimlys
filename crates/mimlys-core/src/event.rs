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

/// What changed in a task's admission (`task.admission_changed`, external design §19.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdmissionState {
    /// No slot was free: the execution waits for one.
    Waiting,
    /// The execution got its slot after waiting.
    Admitted,
    /// The registry could not be used within its bound: the execution went on as if alone.
    Unadmitted,
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

/// The event schema version this code writes (REQ-MAIN-002), recorded on
/// every task at creation (`task.versions`).
///
/// The first values describe the shapes by release: **1** is the shape of
/// 0.7.0 to 0.8.0; **2** is 0.9.0's, in which `decision.received` gained
/// `transport` and `text_chars`. A task recorded before this version existed
/// reads with the value of its release (see `TaskVersions`). From here on: an
/// additive field raises nothing; a renamed or removed field or event, or a
/// changed meaning, raises it by one, and the change is announced in the
/// changelog (REQ-MAIN-001; external design §22).
pub const EVENT_SCHEMA_VERSION: u32 = 2;

/// An event (external design §17.1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event")]
pub enum Event {
    /// A task was created.
    #[serde(rename = "task.created")]
    TaskCreated {
        /// The task was created with `--plan-first` (from 0.13; REQ-TASK-015): absent, and read
        /// as `false`, in earlier records and for a task without the flag.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        plan_first: bool,
        /// The level the task was created at, who chose it and the envelope it runs with, every
        /// key resolved (from 0.13; REQ-TASK-013): absent in earlier records.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        effort: Option<crate::effort::Effort>,
    },
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
        /// Whether the halt followed a rollback of the workspace that failed: nothing was rolled
        /// back (from 0.11; absent, and read as `false`, in earlier records).
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        rollback_failed: bool,
        /// The rule of a halt `policy` (from 0.13; external design §5.2): absent for any other
        /// halt, for a state that is not `halted` and in earlier records.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        halt_detail: Option<crate::task::HaltDetail>,
    },
    /// The task's execution waited for a slot, got one, or went on without the registry (from
    /// 0.12; external design §19.3, RFC 013 decision 5). The task stays `running` meanwhile.
    #[serde(rename = "task.admission_changed")]
    TaskAdmissionChanged {
        /// What changed.
        state: AdmissionState,
        /// Slots of the user taken at that moment (`waiting`: the limit; `admitted`: this one
        /// included).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        slots_taken: Option<u32>,
        /// Why the registry was not used (`unadmitted` only).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
        /// How long the wait lasted, in milliseconds (`admitted` after a wait only).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        waited_ms: Option<u64>,
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
        /// For `denied`, `state` and `budget`: the reason (from 0.10).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
        /// For `denied`: the grant kind that would allow it, when any (from 0.10).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        grant_kind: Option<String>,
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
    /// The first plan of a plan-first task is put to the user, and the task waits for it (from
    /// 0.13; REQ-TASK-015). The user's note is not in the event, as for an action.
    #[serde(rename = "plan.approval_requested")]
    PlanApprovalRequested {
        /// The plan version that waits.
        version: u32,
    },
    /// The user decided about that plan, or the wait expired (from 0.13).
    #[serde(rename = "plan.approval_decided")]
    PlanApprovalDecided {
        /// The plan version.
        version: u32,
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
    /// A promotion began: its backup is committed and it is recorded `applying`, before the
    /// first write to the project (REQ-WS-007).
    #[serde(rename = "workspace.promotion_started")]
    WorkspacePromotionStarted {
        /// The files it writes or deletes.
        files: Vec<String>,
    },
    /// A promotion did not complete: what it had written was restored from its backup.
    #[serde(rename = "workspace.promotion_failed")]
    WorkspacePromotionFailed {
        /// Why, and what was left as it is.
        reason: String,
        /// The kind of the error the request was answered with the first time
        /// (`conflict`, `internal`, ..., external design §18), so that the same request sent
        /// again is answered with the same kind (external design §4). From 0.14: absent in earlier
        /// records, which answer a repeat as `internal`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        kind: Option<String>,
    },
    /// Part of the task's tree could not be read, and the task halted for it: the path that
    /// could not be read.
    #[serde(rename = "workspace.unreadable")]
    WorkspaceUnreadable {
        /// The path, relative to the task's tree; cleaned and bounded.
        path: String,
        /// The kind of the failure, in words (`entity not found`), when the cause was not a
        /// refused permission (from 0.11; absent for a permission and in earlier records).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        kind: Option<String>,
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
    /// The start finished or undid snapshot journals a crash left (REQ-WS-012).
    #[serde(rename = "snapshot.journals_recovered")]
    SnapshotJournalsRecovered {
        /// Journals whose snapshot row existed: removed.
        finished: u32,
        /// Journals without a row: their manifest and unreferenced objects removed.
        undone: u32,
        /// Objects removed by the undone journals.
        objects_removed: u32,
    },
}

impl Event {
    /// The dotted name on the wire.
    pub fn name(&self) -> &'static str {
        match self {
            Event::TaskCreated { .. } => "task.created",
            Event::TaskStateChanged { .. } => "task.state_changed",
            Event::TaskAdmissionChanged { .. } => "task.admission_changed",
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
            Event::PlanApprovalRequested { .. } => "plan.approval_requested",
            Event::PlanApprovalDecided { .. } => "plan.approval_decided",
            Event::ApprovalInvalidated { .. } => "approval.invalidated",
            Event::QuestionAsked { .. } => "question.asked",
            Event::QuestionAnswered { .. } => "question.answered",
            Event::GrantIssued { .. } => "grant.issued",
            Event::GrantRevoked { .. } => "grant.revoked",
            Event::WorkspaceMaterialized => "workspace.materialized",
            Event::WorkspaceSnapshot { .. } => "workspace.snapshot",
            Event::WorkspaceRollback { .. } => "workspace.rollback",
            Event::WorkspacePromoted { .. } => "workspace.promoted",
            Event::WorkspacePromotionStarted { .. } => "workspace.promotion_started",
            Event::WorkspacePromotionFailed { .. } => "workspace.promotion_failed",
            Event::WorkspaceUnreadable { .. } => "workspace.unreadable",
            Event::TaskRecovered { .. } => "task.recovered",
            Event::ToolPinned { .. } => "tool.pinned",
            Event::ToolUnpinned { .. } => "tool.unpinned",
            Event::ServerStarted { .. } => "server.started",
            Event::ServerStopped { .. } => "server.stopped",
            Event::RuntimeStarted { .. } => "runtime.started",
            Event::RuntimeStopped => "runtime.stopped",
            Event::RuntimeOrphansKilled { .. } => "runtime.orphans_killed",
            Event::SnapshotJournalsRecovered { .. } => "snapshot.journals_recovered",
        }
    }
}

/// The keys of an [`EventRecord`] that are the envelope's, not the event's: what a reader that
/// shows an event it does not know leaves out of the payload it prints (REQ-MAIN-006). A test
/// keeps the list in step with the type.
pub const ENVELOPE_FIELDS: [&str; 10] = [
    "event_id",
    "at",
    "actor",
    "session",
    "os_user",
    "task_id",
    "step_id",
    "action_id",
    "policy_hash",
    "request_id",
];

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
    /// The id of the control-API request whose change wrote the event (from 0.14; external design
    /// §4, §17.1): absent for an event the runtime writes of itself (a step's, the recovery's) and
    /// in records written before. It is how a request that was applied is found again, and not
    /// applied twice: the store's schema does not change for it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<crate::ids::RequestId>,
    /// The event.
    #[serde(flatten)]
    pub event: Event,
}

#[cfg(test)]
mod tests;
