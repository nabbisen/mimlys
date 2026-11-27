//! The control API as requests and responses (external design §4; REQ-ARCH-004, REQ-ARCH-009;
//! RFC 014, Amendment 1, item 1).
//!
//! Every client of the runtime sends an [`Envelope`] and gets a [`Response`]: the command line
//! through an in-process transport today, through the daemon's socket from 014c. The types are
//! plain data, serializable with `serde` and versioned with the runtime: the socket's hello
//! carries the version, so a client and a runtime of different releases never exchange them.
//!
//! One handler turns a request into the runtime's work (`Runtime::handle`).
//! A request is one of:
//!
//! - **a verb**: it changes a task, its grants, its workspace or its record, and is answered
//!   with the result or with the error as the runtime reports it ([`RuntimeError`], the kinds
//!   and details of external design §18), so that one error means the same through any
//!   transport;
//! - **an ask**: it changes nothing, and is a request only because the answer depends on what
//!   the runtime was started with (its policy, its levels, its backends), which a client never
//!   sends and in served mode does not share.
//!
//! Readers are not requests: they open the store read-only and never need a runtime
//! (REQ-PERS-013).
//!
//! # No way around the handler
//!
//! The verbs are methods of the runtime that no other crate can call: each of these blocks fails to
//! compile, because the method is private (E0624), so that a command that calls a verb directly is
//! not a program (handoff 014b, item 2). The one way in is `Runtime::handle`.
//!
//! ```compile_fail,E0624
//! # use mimlys_core::{ids::TaskId, runtime::{Runtime, Session}};
//! # fn create_task(r: &Runtime, s: &Session, t: TaskId) {
//! let _ = r.create_task(s, todo!());
//! # }
//! ```
//!
//! ```compile_fail,E0624
//! # use mimlys_core::{ids::TaskId, runtime::{Runtime, Session}};
//! # fn resume_task(r: &Runtime, s: &Session, t: TaskId) {
//! let _ = r.resume_task(s, t, None);
//! # }
//! ```
//!
//! ```compile_fail,E0624
//! # use mimlys_core::{ids::TaskId, runtime::{Runtime, Session}};
//! # fn pause_task(r: &Runtime, s: &Session, t: TaskId) {
//! let _ = r.pause_task(s, t);
//! # }
//! ```
//!
//! ```compile_fail,E0624
//! # use mimlys_core::{ids::TaskId, runtime::{Runtime, Session}};
//! # fn cancel_task(r: &Runtime, s: &Session, t: TaskId) {
//! let _ = r.cancel_task(s, t);
//! # }
//! ```
//!
//! ```compile_fail,E0624
//! # use mimlys_core::{ids::TaskId, runtime::{Runtime, Session}};
//! # fn approve_action(r: &Runtime, s: &Session, t: TaskId) {
//! let _ = r.approve_action(s, t, None);
//! # }
//! ```
//!
//! ```compile_fail,E0624
//! # use mimlys_core::{ids::TaskId, runtime::{Runtime, Session}};
//! # fn reject_action(r: &Runtime, s: &Session, t: TaskId) {
//! let _ = r.reject_action(s, t, None, None);
//! # }
//! ```
//!
//! ```compile_fail,E0624
//! # use mimlys_core::{ids::TaskId, runtime::{Runtime, Session}};
//! # fn answer_question(r: &Runtime, s: &Session, t: TaskId) {
//! let _ = r.answer_question(s, t, "a");
//! # }
//! ```
//!
//! ```compile_fail,E0624
//! # use mimlys_core::{ids::TaskId, runtime::{Runtime, Session}};
//! # fn grant_capability(r: &Runtime, s: &Session, t: TaskId) {
//! let _ = r.grant_capability(s, t, todo!());
//! # }
//! ```
//!
//! ```compile_fail,E0624
//! # use mimlys_core::{ids::TaskId, runtime::{Runtime, Session}};
//! # fn revoke_capability(r: &Runtime, s: &Session, t: TaskId) {
//! let _ = r.revoke_capability(s, t, todo!());
//! # }
//! ```
//!
//! ```compile_fail,E0624
//! # use mimlys_core::{ids::TaskId, runtime::{Runtime, Session}};
//! # fn rollback_workspace(r: &Runtime, s: &Session, t: TaskId) {
//! let _ = r.rollback_workspace(s, t, None);
//! # }
//! ```
//!
//! ```compile_fail,E0624
//! # use mimlys_core::{ids::TaskId, runtime::{Runtime, Session}};
//! # fn promote_workspace(r: &Runtime, s: &Session, t: TaskId) {
//! let _ = r.promote_workspace(s, t, todo!());
//! # }
//! ```
//!
//! ```compile_fail,E0624
//! # use mimlys_core::{ids::TaskId, runtime::{Runtime, Session}};
//! # fn revert_promotion(r: &Runtime, s: &Session, t: TaskId) {
//! let _ = r.revert_promotion(s, t, todo!(), false);
//! # }
//! ```
//!
//! ```compile_fail,E0624
//! # use mimlys_core::{ids::TaskId, runtime::{Runtime, Session}};
//! # fn delete_task(r: &Runtime, s: &Session, t: TaskId) {
//! let _ = r.delete_task(s, t, false);
//! # }
//! ```
//!
//! ```compile_fail,E0624
//! # use mimlys_core::{ids::TaskId, runtime::{Runtime, Session}};
//! # fn plan_effort(r: &Runtime, s: &Session, t: TaskId) {
//! let _ = r.plan_effort(todo!(), None, None);
//! # }
//! ```
//!
//! ```compile_fail,E0624
//! # use mimlys_core::{ids::TaskId, runtime::{Runtime, Session}};
//! # fn model_change(r: &Runtime, s: &Session, t: TaskId) {
//! let _ = r.model_change(todo!());
//! # }
//! ```
//!
//! ```compile_fail,E0624
//! # use mimlys_core::{ids::TaskId, runtime::{Runtime, Session}};
//! # fn start_refusal(r: &Runtime, s: &Session, t: TaskId) {
//! let _ = r.start_refusal(todo!());
//! # }
//! ```
//!
//! ```compile_fail,E0624
//! # use mimlys_core::{ids::TaskId, runtime::{Runtime, Session}};
//! # fn creation_forbidden(r: &Runtime, s: &Session, t: TaskId) {
//! let _ = r.creation_forbidden(None, "default");
//! # }
//! ```
//!
//! And the way in does compile:
//!
//! ```
//! # use mimlys_core::{api::{Envelope, Request}, ids::{RequestId, SystemIds, TaskId}, runtime::{Runtime, Session}};
//! # fn handled(r: &Runtime, s: &Session, t: TaskId) {
//! let _ = r.handle(Envelope {
//!     request_id: RequestId::new(&SystemIds),
//!     session: s.clone(),
//!     request: Request::CancelTask { task_id: t },
//! });
//! # }
//! ```

use crate::grant::{Grant, GrantKind};
use crate::ids::{GrantId, PromotionId, RequestId, SnapshotId, TaskId};
use crate::runtime::{CreateTask, EffortPlan, ModelChange, RuntimeError, Session, StartRefusal};
use crate::snapshot::{Footprint, Promotion, PromotionPlan, SnapshotRecord};
use crate::task::{Extension, Task};
use serde::{Deserialize, Serialize};

/// What a client sends: who asks, under which id, and what.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Envelope {
    /// The id the client made for this request (UUID version 7). The runtime records it with the
    /// change the request caused, so that a request sent twice is applied once.
    pub request_id: RequestId,
    /// The client process and its operating-system user.
    pub session: Session,
    /// What is asked.
    pub request: Request,
}

/// What is asked. One variant per verb of external design §4 and one per ask; the arguments are
/// the ones the command line passes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "request", rename_all = "snake_case")]
pub enum Request {
    /// `create_task` (REQ-TASK-001).
    CreateTask(CreateTask),
    /// `resume_task` (REQ-TASK-005).
    ResumeTask {
        /// The task.
        task_id: TaskId,
        /// A budget extension, from a budget halt.
        extend: Option<Extension>,
    },
    /// `pause_task` (REQ-TASK-005).
    PauseTask {
        /// The task.
        task_id: TaskId,
    },
    /// `cancel_task` (REQ-TASK-006).
    CancelTask {
        /// The task.
        task_id: TaskId,
    },
    /// `approve_action`, or a plan-first task's plan (REQ-APR-007, REQ-TASK-015).
    ApproveAction {
        /// The task.
        task_id: TaskId,
        /// The user's own words for the model.
        note: Option<String>,
    },
    /// `reject_action`, or a plan-first task's plan (REQ-APR-007, REQ-TASK-015).
    RejectAction {
        /// The task.
        task_id: TaskId,
        /// The user's own words for the model.
        note: Option<String>,
        /// Halt the task; the profile's `on_reject` when absent.
        halt: Option<bool>,
    },
    /// `answer_question` (REQ-APR-010).
    AnswerQuestion {
        /// The task.
        task_id: TaskId,
        /// The answer.
        answer: String,
    },
    /// `grant_capability` (REQ-CAP-004).
    GrantCapability {
        /// The task.
        task_id: TaskId,
        /// What is granted.
        kind: GrantKind,
    },
    /// `revoke_capability` (REQ-CAP-005).
    RevokeCapability {
        /// The task.
        task_id: TaskId,
        /// The grant.
        grant_id: GrantId,
    },
    /// `rollback_workspace` (REQ-WS-006).
    RollbackWorkspace {
        /// The task.
        task_id: TaskId,
        /// The snapshot; the latest when absent.
        snapshot_id: Option<SnapshotId>,
    },
    /// `promote_workspace`, phase 2 (REQ-WS-007): after the user's confirmation of `plan`.
    PromoteWorkspace {
        /// The task.
        task_id: TaskId,
        /// The plan the user confirmed.
        plan: PromotionPlan,
    },
    /// `revert_promotion` (REQ-WS-007).
    RevertPromotion {
        /// The task.
        task_id: TaskId,
        /// The promotion.
        promotion_id: PromotionId,
        /// The user confirmed overwriting the paths edited since.
        overwrite_edited: bool,
    },
    /// `delete_task` (REQ-WS-012).
    DeleteTask {
        /// The task.
        task_id: TaskId,
        /// Remove the audit too.
        audit: bool,
    },
    /// The first stop signal (REQ-CLI-005): the step in flight starts no new action, and the task
    /// pauses at the next boundary. The pause is noted at once and taken at the step's end
    /// ([`Reply::Noted`]), or made now when no step is in flight.
    StopAtBoundary {
        /// The task.
        task_id: TaskId,
    },
    /// What the first signal's watchdog does when the pause has not come within LIM-O1 less a
    /// second: the step in flight is asked to stop what can be undone (the model request, an
    /// action that can be undone), and starts no new action. An action that cannot be undone is
    /// not interrupted.
    InterruptStep {
        /// The task.
        task_id: TaskId,
    },
    /// The second stop signal (REQ-CLI-005): the step in flight is interrupted whatever it is
    /// doing, an action that cannot be undone included (its outcome is then unknown, and the
    /// task halts), and the task is cancelled when the step leaves it `running`.
    StopAnyway {
        /// The task.
        task_id: TaskId,
    },
    /// Ask: the level a task would be given and the backend its envelope names, before anything
    /// is created (`Runtime::plan_effort`).
    PlanEffort {
        /// The profile; the policy's default when absent.
        profile: Option<String>,
        /// `--effort`.
        effort: Option<crate::effort::EffortLevel>,
        /// A fork's source.
        from_task: Option<TaskId>,
    },
    /// Ask: the model the task's backend names now, when it is not the one the task last ran
    /// with (`Runtime::model_change`).
    ModelChange {
        /// The task.
        task_id: TaskId,
    },
    /// Ask: whether the task may not start, its backend being remote under a profile without
    /// `external` (`Runtime::start_refusal`).
    StartRefusal {
        /// The task.
        task_id: TaskId,
    },
    /// Ask: whether a task created under `profile` on `backend` would be refused
    /// (`Runtime::creation_forbidden`).
    CreationForbidden {
        /// The profile; the policy's default when absent.
        profile: Option<String>,
        /// The backend's name.
        backend: String,
    },
}

impl Request {
    /// The request's name, as the verb of external design §4 is called.
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            Request::CreateTask(_) => "create_task",
            Request::ResumeTask { .. } => "resume_task",
            Request::PauseTask { .. } => "pause_task",
            Request::CancelTask { .. } => "cancel_task",
            Request::ApproveAction { .. } => "approve_action",
            Request::RejectAction { .. } => "reject_action",
            Request::AnswerQuestion { .. } => "answer_question",
            Request::GrantCapability { .. } => "grant_capability",
            Request::RevokeCapability { .. } => "revoke_capability",
            Request::RollbackWorkspace { .. } => "rollback_workspace",
            Request::PromoteWorkspace { .. } => "promote_workspace",
            Request::RevertPromotion { .. } => "revert_promotion",
            Request::DeleteTask { .. } => "delete_task",
            Request::StopAtBoundary { .. } => "stop_at_boundary",
            Request::InterruptStep { .. } => "interrupt_step",
            Request::StopAnyway { .. } => "stop_anyway",
            Request::PlanEffort { .. } => "plan_effort",
            Request::ModelChange { .. } => "model_change",
            Request::StartRefusal { .. } => "start_refusal",
            Request::CreationForbidden { .. } => "creation_forbidden",
        }
    }

    /// Whether the request may change the store. An ask changes nothing, and neither does the
    /// request to interrupt the step, which only sets what the step reads.
    #[must_use]
    pub fn changes_state(&self) -> bool {
        !matches!(
            self,
            Request::InterruptStep { .. }
                | Request::PlanEffort { .. }
                | Request::ModelChange { .. }
                | Request::StartRefusal { .. }
                | Request::CreationForbidden { .. }
        )
    }

    /// The task the request is about, when it names one (a creation has none yet).
    #[must_use]
    pub fn task_id(&self) -> Option<TaskId> {
        match self {
            Request::CreateTask(_)
            | Request::PlanEffort { .. }
            | Request::CreationForbidden { .. } => None,
            Request::ResumeTask { task_id, .. }
            | Request::PauseTask { task_id }
            | Request::CancelTask { task_id }
            | Request::ApproveAction { task_id, .. }
            | Request::RejectAction { task_id, .. }
            | Request::AnswerQuestion { task_id, .. }
            | Request::GrantCapability { task_id, .. }
            | Request::RevokeCapability { task_id, .. }
            | Request::RollbackWorkspace { task_id, .. }
            | Request::PromoteWorkspace { task_id, .. }
            | Request::RevertPromotion { task_id, .. }
            | Request::DeleteTask { task_id, .. }
            | Request::StopAtBoundary { task_id }
            | Request::InterruptStep { task_id }
            | Request::StopAnyway { task_id }
            | Request::ModelChange { task_id }
            | Request::StartRefusal { task_id } => Some(*task_id),
        }
    }
}

/// What a request that was handled returns.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reply", content = "value", rename_all = "snake_case")]
pub enum Reply {
    /// The task as the verb left it: `create_task`, `resume_task`, `pause_task`, `cancel_task`,
    /// `approve_action`, `reject_action`, `answer_question`.
    Task(Box<Task>),
    /// A request that waits for the step in flight to end was noted: the task as it is now. The
    /// request is taken in the step's own turn, at its end (a pause at the next boundary, a stop
    /// that cancels a task the step leaves `running`), and the reply says so at once.
    Noted(Box<Task>),
    /// The step was asked to stop (`interrupt_step`); whether one was in flight to be asked.
    Interrupted {
        /// A step of the task was in flight.
        step_in_flight: bool,
    },
    /// The grant issued or revoked.
    Grant(Grant),
    /// The snapshot the workspace was restored to.
    Snapshot(SnapshotRecord),
    /// The promotion applied or reversed.
    Promotion(Promotion),
    /// What the deleted task held on disk.
    Footprint(Footprint),
    /// The answer to `plan_effort`.
    EffortPlan(EffortPlan),
    /// The answer to `model_change`.
    ModelChange(Option<ModelChange>),
    /// The answer to `start_refusal`.
    StartRefusal(Option<StartRefusal>),
    /// The answer to `creation_forbidden`.
    CreationForbidden(bool),
}

/// What a client says to the user while a request waits (external design §4): the runtime tells
/// it, as a request that waits for the step in flight begins to wait.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "notice", rename_all = "snake_case")]
pub enum Notice {
    /// A grant or a revocation waits for the step in flight of the task to end, and then applies
    /// from a fresh read.
    WaitingForStep {
        /// The task.
        task_id: TaskId,
    },
}

/// What a request whose id is already recorded returns (external design §4, "The recorded
/// outcome"): what the request did, as its events say, and the task as it is now. Nothing is
/// applied again.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlreadyApplied {
    /// The position of the first event the request's change wrote.
    pub position: u64,
    /// That event's name, which says what the change was (`grant.issued`, `task.created`, …).
    pub event: String,
    /// The task, as it is now; none when the request names none, or when its record is gone
    /// (`delete --audit`, after which a request of it is not found at all).
    pub task: Option<Box<Task>>,
    /// The task was deleted since the request took effect, and its events were kept (a deletion
    /// without `--audit`): what the request did is known from them, the task's data is gone, and
    /// the client says so. A deletion with `--audit` leaves nothing to find the request by.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub task_deleted: bool,
    /// What the change came to.
    pub did: Did,
}

/// What a request that was recorded did (external design §4, "The recorded outcome"). A request
/// that was refused wrote nothing and is neither: it is answered afresh.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "did", rename_all = "snake_case")]
pub enum Did {
    /// The change took effect: the first answer was a success. `reply` is the verb's result for a
    /// verb whose result is not the task (a grant, a snapshot, a promotion), read from the store
    /// as it is now, so that a client prints what the first answer would have said. It is none
    /// for a verb whose result is the task (the task is [`AlreadyApplied::task`]), for one whose
    /// result is not kept (a deletion's footprint), and when the store no longer holds what the
    /// reply is made of (the task was deleted since, [`AlreadyApplied::task_deleted`]: its
    /// snapshots and promotions went with its data).
    Applied {
        /// The verb's result, rebuilt.
        reply: Option<Reply>,
    },
    /// The change wrote, and then failed: what its events recorded, never "applied".
    Failed {
        /// The event that recorded the failure (`workspace.promotion_failed`), or the last event
        /// the request wrote when the change stopped before it could record one.
        event: String,
        /// Why, as it was recorded.
        reason: String,
        /// The kind of the error the first answer was, as the event recorded it (external
        /// design §4, §17.1); none for an event that holds none, which answers `internal`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error_kind: Option<String>,
        /// The `detail` the first answer had, as the event recorded it; none when it had none.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error_detail: Option<serde_json::Value>,
    },
}

impl Did {
    /// The reply the change gave, when it has one to give; or, for a change that failed after it
    /// wrote, the error a repeated request is answered with: the recorded failure, with the kind
    /// and the detail the first answer had (external design §4). A change that took effect is
    /// never an error here.
    pub fn into_reply(self) -> Result<Option<Reply>, RuntimeError> {
        match self {
            Did::Applied { reply } => Ok(reply),
            Did::Failed {
                event,
                reason,
                error_kind,
                error_detail,
            } => Err(RuntimeError::Recorded {
                event,
                reason,
                error_kind,
                error_detail,
            }),
        }
    }
}

/// How a request ended.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", content = "value", rename_all = "snake_case")]
pub enum Outcome {
    /// It was handled.
    Done(Reply),
    /// Its id was recorded already: it was applied before, and is not applied again. A request
    /// that was refused wrote nothing, so it is never answered so.
    AlreadyApplied(AlreadyApplied),
    /// It was refused or failed, with the error of external design §18.
    Failed(RuntimeError),
}

/// What a client gets back, under the id it sent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Response {
    /// The id of the request this answers.
    pub request_id: RequestId,
    /// How it ended.
    pub outcome: Outcome,
}

impl Response {
    /// The reply, or what was recorded of a request that was applied before, or the error.
    ///
    /// There is no method that folds a repeat into a `Result<Reply, RuntimeError>`: a request that
    /// took effect and has no reply to give (a task's verb, a deletion, a snapshot gone with its
    /// task) has neither a reply nor an error to be. A caller says what it was told: the client
    /// does, as `Applied` and `Answered` (handoff 014b-7, item 4).
    pub fn into_outcome(self) -> Result<Result<Reply, AlreadyApplied>, RuntimeError> {
        match self.outcome {
            Outcome::Done(reply) => Ok(Ok(reply)),
            Outcome::AlreadyApplied(already) => Ok(Err(already)),
            Outcome::Failed(error) => Err(error),
        }
    }
}

#[cfg(any(test, feature = "test-support"))]
pub mod testing;

#[cfg(test)]
mod tests;
