//! The client of the control API (external design §4; RFC 014, Amendment 1, item 1): every
//! command sends its requests through one client, and no command knows which transport carries
//! them.
//!
//! The client makes the request id (UUID version 7, the ids' own kind) and carries the session,
//! sends the envelope through its transport, and gives the reply back typed. Today the one
//! transport is in-process, which hands the envelope to the runtime of the command's own
//! process (embedded mode); 014c adds the daemon's socket behind the same [`Transport`], so
//! that REQ-ARCH-009 holds by construction: a command is written once.

use mimlys_core::api::{Envelope, Notice, Reply, Request, Response};
use mimlys_core::grant::{Grant, GrantKind};
use mimlys_core::ids::{GrantId, PromotionId, RequestId, SnapshotId, SystemIds, TaskId};
use mimlys_core::runtime::{
    CreateTask, EffortPlan, ModelChange, Runtime, RuntimeError, Session, StartRefusal,
};
use mimlys_core::snapshot::{Footprint, Promotion, PromotionPlan, SnapshotRecord};
use mimlys_core::spi::PersistenceError;
use mimlys_core::task::{Extension, Task};
use std::sync::Arc;
use tokio::sync::Notify;

/// What carries an envelope to the runtime that handles it and brings the response back. It
/// blocks while the request is handled, as the verb does (a cancel waits for the step in
/// flight), so a caller that must not block sends from a thread of its own.
pub trait Transport: Send + Sync {
    /// Sends the envelope and waits for its response; `notice` is told what the user is to be
    /// told while the request waits (a grant waits for the step in flight).
    fn send(&self, envelope: Envelope, notice: &dyn Fn(Notice)) -> Response;

    /// A follower of the task begins: the driver takes the task if it is running. A transport to
    /// a daemon has nothing to do, the daemon owning the tasks it serves.
    fn attach(&self, _task_id: TaskId) {}

    /// Where a follower waits for something to look at: a step ended, or a request was handled.
    /// A transport with nothing to push returns one that is never notified, and the follower's
    /// own poll is what wakes it.
    fn changes(&self) -> Arc<Notify> {
        Arc::new(Notify::new())
    }

    /// The driver's loop, when it has ended: it only ends by a panic of a step, and a follower
    /// that finds it ends in the same way, never as a hang.
    fn ended_driver(&self) -> Option<tokio::task::JoinHandle<()>> {
        None
    }

    /// The error of the step that failed for this task, once.
    fn failure(&self, _task_id: TaskId) -> Option<RuntimeError> {
        None
    }
}

/// The one client of a command.
#[derive(Clone)]
pub struct Client {
    transport: Arc<dyn Transport>,
    session: Session,
}

impl Client {
    /// A client of this process's own runtime, under `session`: embedded mode, whose driver
    /// lives as long as the command.
    pub fn in_process(runtime: Arc<Runtime>, session: Session) -> Self {
        Self::over(crate::embedded::InProcess::new(runtime), session)
    }

    /// A client over any transport.
    pub fn over(transport: Arc<dyn Transport>, session: Session) -> Self {
        Self { transport, session }
    }

    /// The session every request carries.
    pub fn session(&self) -> &Session {
        &self.session
    }

    /// Sends `request` under a new id, and returns its reply, or the error the runtime gave.
    pub fn send(&self, request: Request) -> Result<Reply, RuntimeError> {
        self.send_as(RequestId::new(&SystemIds), request)
    }

    /// Sends `request` under `request_id`: a request sent once more, after a connection broke,
    /// goes under the id it first had (014c), so that it is applied once.
    pub fn send_as(&self, request_id: RequestId, request: Request) -> Result<Reply, RuntimeError> {
        let response = self.transport.send(
            Envelope {
                request_id,
                session: self.session.clone(),
                request,
            },
            &say,
        );
        if response.request_id != request_id {
            return Err(unexpected(
                "an answer to another request than the one that was sent",
            ));
        }
        response.into_result()
    }

    /// A follower of the task begins: the driver takes it if it is running.
    pub fn attach(&self, task_id: TaskId) {
        self.transport.attach(task_id);
    }

    /// Waits until something changed that a follower should look at (a step ended, a request was
    /// handled), and at most the poll of `events --follow` (LIM-O15).
    pub async fn changed(&self) {
        let changes = self.transport.changes();
        let _ = tokio::time::timeout(
            std::time::Duration::from_millis(crate::commands::events::EVENTS_POLL_MS),
            changes.notified(),
        )
        .await;
        if let Some(ended) = self.transport.ended_driver()
            && let Err(error) = ended.await
            && error.is_panic()
        {
            std::panic::resume_unwind(error.into_panic());
        }
    }

    /// The error of the step that failed for this task, once.
    pub fn failure(&self, task_id: TaskId) -> Option<RuntimeError> {
        self.transport.failure(task_id)
    }

    /// `create_task`.
    pub fn create_task(&self, create: CreateTask) -> Result<Task, RuntimeError> {
        task(self.send(Request::CreateTask(create))?)
    }

    /// `resume_task`.
    pub fn resume_task(
        &self,
        task_id: TaskId,
        extend: Option<Extension>,
    ) -> Result<Task, RuntimeError> {
        task(self.send(Request::ResumeTask { task_id, extend })?)
    }

    /// `pause_task`.
    pub fn pause_task(&self, task_id: TaskId) -> Result<Task, RuntimeError> {
        task(self.send(Request::PauseTask { task_id })?)
    }

    /// `cancel_task`.
    pub fn cancel_task(&self, task_id: TaskId) -> Result<Task, RuntimeError> {
        task(self.send(Request::CancelTask { task_id })?)
    }

    /// `approve_action`.
    pub fn approve_action(
        &self,
        task_id: TaskId,
        note: Option<String>,
    ) -> Result<Task, RuntimeError> {
        task(self.send(Request::ApproveAction { task_id, note })?)
    }

    /// `reject_action`.
    pub fn reject_action(
        &self,
        task_id: TaskId,
        note: Option<String>,
        halt: Option<bool>,
    ) -> Result<Task, RuntimeError> {
        task(self.send(Request::RejectAction {
            task_id,
            note,
            halt,
        })?)
    }

    /// `answer_question`.
    pub fn answer_question(&self, task_id: TaskId, answer: &str) -> Result<Task, RuntimeError> {
        task(self.send(Request::AnswerQuestion {
            task_id,
            answer: answer.to_owned(),
        })?)
    }

    /// `grant_capability`.
    pub fn grant_capability(
        &self,
        task_id: TaskId,
        kind: GrantKind,
    ) -> Result<Grant, RuntimeError> {
        match self.send(Request::GrantCapability { task_id, kind })? {
            Reply::Grant(grant) => Ok(grant),
            other => Err(unexpected_reply("grant_capability", &other)),
        }
    }

    /// `revoke_capability`.
    pub fn revoke_capability(
        &self,
        task_id: TaskId,
        grant_id: GrantId,
    ) -> Result<Grant, RuntimeError> {
        match self.send(Request::RevokeCapability { task_id, grant_id })? {
            Reply::Grant(grant) => Ok(grant),
            other => Err(unexpected_reply("revoke_capability", &other)),
        }
    }

    /// `rollback_workspace`.
    pub fn rollback_workspace(
        &self,
        task_id: TaskId,
        snapshot_id: Option<SnapshotId>,
    ) -> Result<SnapshotRecord, RuntimeError> {
        match self.send(Request::RollbackWorkspace {
            task_id,
            snapshot_id,
        })? {
            Reply::Snapshot(record) => Ok(record),
            other => Err(unexpected_reply("rollback_workspace", &other)),
        }
    }

    /// `promote_workspace`, after the user confirmed `plan`.
    pub fn promote_workspace(
        &self,
        task_id: TaskId,
        plan: &PromotionPlan,
    ) -> Result<Promotion, RuntimeError> {
        match self.send(Request::PromoteWorkspace {
            task_id,
            plan: plan.clone(),
        })? {
            Reply::Promotion(promotion) => Ok(promotion),
            other => Err(unexpected_reply("promote_workspace", &other)),
        }
    }

    /// `revert_promotion`.
    pub fn revert_promotion(
        &self,
        task_id: TaskId,
        promotion_id: PromotionId,
        overwrite_edited: bool,
    ) -> Result<Promotion, RuntimeError> {
        match self.send(Request::RevertPromotion {
            task_id,
            promotion_id,
            overwrite_edited,
        })? {
            Reply::Promotion(promotion) => Ok(promotion),
            other => Err(unexpected_reply("revert_promotion", &other)),
        }
    }

    /// `delete_task`.
    pub fn delete_task(&self, task_id: TaskId, audit: bool) -> Result<Footprint, RuntimeError> {
        match self.send(Request::DeleteTask { task_id, audit })? {
            Reply::Footprint(footprint) => Ok(footprint),
            other => Err(unexpected_reply("delete_task", &other)),
        }
    }

    /// The level a task would be given and the backend its envelope names, before anything is
    /// created.
    pub fn plan_effort(
        &self,
        profile: Option<&str>,
        effort: Option<mimlys_core::effort::EffortLevel>,
        from_task: Option<TaskId>,
    ) -> Result<EffortPlan, RuntimeError> {
        match self.send(Request::PlanEffort {
            profile: profile.map(str::to_owned),
            effort,
            from_task,
        })? {
            Reply::EffortPlan(plan) => Ok(plan),
            other => Err(unexpected_reply("plan_effort", &other)),
        }
    }

    /// The model the task's backend names now, when it is not the one the task last ran with.
    pub fn model_change(&self, task_id: TaskId) -> Result<Option<ModelChange>, RuntimeError> {
        match self.send(Request::ModelChange { task_id })? {
            Reply::ModelChange(change) => Ok(change),
            other => Err(unexpected_reply("model_change", &other)),
        }
    }

    /// Whether the task may not start under the profile and the backend in force.
    pub fn start_refusal(&self, task_id: TaskId) -> Result<Option<StartRefusal>, RuntimeError> {
        match self.send(Request::StartRefusal { task_id })? {
            Reply::StartRefusal(refusal) => Ok(refusal),
            other => Err(unexpected_reply("start_refusal", &other)),
        }
    }

    /// Whether a task created under `profile` on `backend` would be refused.
    pub fn creation_forbidden(
        &self,
        profile: Option<&str>,
        backend: &str,
    ) -> Result<bool, RuntimeError> {
        match self.send(Request::CreationForbidden {
            profile: profile.map(str::to_owned),
            backend: backend.to_owned(),
        })? {
            Reply::CreationForbidden(forbidden) => Ok(forbidden),
            other => Err(unexpected_reply("creation_forbidden", &other)),
        }
    }
}

/// What the user is told while a request waits (external design §4): on standard error, so that
/// what a command prints on standard output is what it printed before.
fn say(notice: Notice) {
    match notice {
        Notice::WaitingForStep { task_id } => {
            crate::output::diag(crate::messages::waiting_for_step(&task_id.to_string()));
        }
    }
}

/// The task a verb returns. A request that was noted for the end of the step in flight
/// (`Reply::Noted`: a pause, a stop) returns the task as it is, and the user is told that the
/// request is taken when the step has ended.
fn task(reply: Reply) -> Result<Task, RuntimeError> {
    match reply {
        Reply::Task(task) => Ok(*task),
        Reply::Noted(task) => {
            crate::output::diag(crate::messages::request_noted(&task.task_id.to_string()));
            Ok(*task)
        }
        other => Err(unexpected_reply("a verb that returns a task", &other)),
    }
}

/// A reply of another kind than the request's is a defect of the transport or of the runtime
/// (`internal`, with the request and the reply named), never something to guess a result from.
fn unexpected_reply(request: &str, reply: &Reply) -> RuntimeError {
    let reply = serde_json::to_value(reply)
        .ok()
        .and_then(|v| v.get("reply").and_then(|r| r.as_str().map(str::to_owned)))
        .unwrap_or_default();
    unexpected(&format!(
        "the reply to {request} was a `{reply}` reply, which does not answer it"
    ))
}

fn unexpected(detail: &str) -> RuntimeError {
    RuntimeError::Persistence(PersistenceError::Integrity {
        rule: "control_api".to_owned(),
        detail: detail.to_owned(),
    })
}

#[cfg(test)]
mod tests;
