//! The control API as requests and responses (external design §4; REQ-ARCH-004, REQ-ARCH-009;
//! RFC 014, Amendment 1, item 1).
//!
//! Every client of the runtime sends an [`Envelope`] and gets a [`Response`]: the command line
//! through an in-process transport today, through the daemon's socket from 014c. The types are
//! plain data, serializable with `serde` and versioned with the runtime: the socket's hello
//! carries the version, so a client and a runtime of different releases never exchange them.
//!
//! One handler turns a request into the runtime's work ([`crate::runtime::Runtime::handle`]).
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
    /// Ask: the level a task would be given and the backend its envelope names, before anything
    /// is created ([`crate::runtime::Runtime::plan_effort`]).
    PlanEffort {
        /// The profile; the policy's default when absent.
        profile: Option<String>,
        /// `--effort`.
        effort: Option<crate::effort::EffortLevel>,
        /// A fork's source.
        from_task: Option<TaskId>,
    },
    /// Ask: the model the task's backend names now, when it is not the one the task last ran
    /// with ([`crate::runtime::Runtime::model_change`]).
    ModelChange {
        /// The task.
        task_id: TaskId,
    },
    /// Ask: whether the task may not start, its backend being remote under a profile without
    /// `external` ([`crate::runtime::Runtime::start_refusal`]).
    StartRefusal {
        /// The task.
        task_id: TaskId,
    },
    /// Ask: whether a task created under `profile` on `backend` would be refused
    /// ([`crate::runtime::Runtime::creation_forbidden`]).
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
            Request::PlanEffort { .. } => "plan_effort",
            Request::ModelChange { .. } => "model_change",
            Request::StartRefusal { .. } => "start_refusal",
            Request::CreationForbidden { .. } => "creation_forbidden",
        }
    }

    /// Whether the request is a verb: it may change the store. An ask changes nothing.
    #[must_use]
    pub fn changes_state(&self) -> bool {
        !matches!(
            self,
            Request::PlanEffort { .. }
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

/// How a request ended.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", content = "value", rename_all = "snake_case")]
pub enum Outcome {
    /// It was handled.
    Done(Reply),
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
    /// The result a caller works with: the reply, or the error.
    pub fn into_result(self) -> Result<Reply, RuntimeError> {
        match self.outcome {
            Outcome::Done(reply) => Ok(reply),
            Outcome::Failed(error) => Err(error),
        }
    }
}

#[cfg(test)]
mod tests;
