//! The client of the control API (external design §4; RFC 014, Amendment 1, item 1): every
//! command sends its requests through one client, and no command knows which transport carries
//! them.
//!
//! The client makes the request id (UUID version 7, the ids' own kind) and carries the session,
//! sends the envelope through its transport, and gives the reply back typed. Today the one
//! transport is in-process, which hands the envelope to the runtime of the command's own
//! process (embedded mode); 014c adds the daemon's socket behind the same [`Transport`], so
//! that REQ-ARCH-009 holds by construction: a command is written once.

use mimlys_core::api::{AlreadyApplied, Envelope, Notice, Reply, Request, Response};
use mimlys_core::grant::{Grant, GrantKind};
use mimlys_core::ids::{GrantId, PromotionId, RequestId, SnapshotId, SystemIds, TaskId};
use mimlys_core::runtime::{
    CreateTask, EffortPlan, ModelChange, Runtime, RuntimeError, Session, StartRefusal,
};
use mimlys_core::snapshot::{Footprint, Promotion, PromotionPlan, SnapshotRecord};
use mimlys_core::spi::PersistenceError;
use mimlys_core::task::{Extension, Task};
use std::collections::HashSet;
use std::sync::{Arc, Mutex, PoisonError};
use tokio::sync::Notify;

/// What carries an envelope to the runtime that handles it and brings the response back. It
/// blocks while the request is handled, as the verb does (a cancel waits for the step in
/// flight), so a caller that must not block sends from a thread of its own.
pub trait Transport: Send + Sync {
    /// Sends the envelope and waits for its response; `notice` is told what the user is to be
    /// told while the request waits (a grant waits for the step in flight). A transport that
    /// can lose its connection says so ([`ConnectionBroken`]): the request may or may not have
    /// been handled, and the client sends it once more, with the same id (external design §4).
    fn send(
        &self,
        envelope: Envelope,
        notice: &dyn Fn(Notice),
    ) -> Result<Response, ConnectionBroken>;

    /// What the task waits behind in the daemon's line, if it waits (external design §19.5).
    /// Embedded mode has no line.
    fn queued(&self, _task_id: TaskId) -> Option<mimlys_core::api::Line> {
        None
    }

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

/// A connection that broke while a request was in flight, or could not be made: whether the
/// request took effect is not known to the transport, and the recorded outcome tells (external
/// design §4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectionBroken {
    /// What failed.
    pub detail: String,
}

/// The one client of a command.
#[derive(Clone)]
pub struct Client {
    transport: Arc<dyn Transport>,
    session: Session,
    /// The ids of the deletions this client has sent and whose answer was not a refusal: a
    /// deletion sent once more under one of them, and answered that the task is not found, is
    /// answered "the task is gone" (external design §4, "The recorded outcome"). Shared by the
    /// clones of the client.
    deletions: Arc<Mutex<HashSet<RequestId>>>,
}

/// What a deletion came to, as the client says it (external design §4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Deleted {
    /// The task was deleted, and this is what it held.
    Footprint(Footprint),
    /// The task is gone: the request was sent before, under the same id, and the deletion took
    /// effect then. What it held was in the first answer, which is not kept.
    Gone,
}

/// What a request whose reply is not the task came to, as the client says it (external design
/// §4, "The recorded outcome"): the reply, or that it took effect and its task has been deleted
/// since, so that what the reply is made of (a snapshot, a promotion) went with the task's data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Applied<T> {
    /// The reply: the first answer's, or, for a request sent again, read back from the store.
    Done(T),
    /// The request took effect before, and the task is gone since: its events say what was done,
    /// and the reply is not kept. The user was told, on standard error.
    Gone,
}

/// What a request came to, before it is given its type: the reply, or that the task the request
/// names is gone (it was deleted, by this request or since it was applied).
enum Answered {
    Reply(Reply),
    Gone,
}

impl Client {
    /// A client of this process's own runtime, under `session`: embedded mode, whose driver
    /// lives as long as the command.
    pub fn in_process(runtime: Arc<Runtime>, session: Session) -> Self {
        Self::over(crate::embedded::InProcess::new(runtime), session)
    }

    /// A client over any transport.
    pub fn over(transport: Arc<dyn Transport>, session: Session) -> Self {
        Self {
            transport,
            session,
            deletions: Arc::default(),
        }
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
    /// goes under the id it first had (014c), so that it is applied once. A repeat is answered
    /// with what the request did (external design §4, "The recorded outcome"), and never with an
    /// error for a change that took effect:
    ///
    /// - **applied:** the success it was: the task as it is now for a verb that returns one, the
    ///   grant, the snapshot or the promotion read back from the store for the others; the user
    ///   is told, on standard error, that it was done earlier;
    /// - **failed after it wrote:** the recorded failure, with its reason;
    /// - **refused:** answered afresh, as the runtime does;
    /// - **the task was deleted since,** and its events were kept: as applied, and the user is
    ///   told that the task is gone. After `delete --audit` nothing of the request is known, and
    ///   the answer is `not_found`, as the task is.
    ///
    /// For the requests whose reply is kept with the task's record or is the task itself. The
    /// others (a snapshot, a promotion) have no reply to give once their task's data is gone:
    /// [`Client::applied_as`] says so, and this is a defect (`internal`) for them.
    pub fn send_as(&self, request_id: RequestId, request: Request) -> Result<Reply, RuntimeError> {
        match self.applied_as(request_id, request)? {
            Applied::Done(reply) => Ok(reply),
            Applied::Gone => Err(unexpected(
                "the request took effect and its task is gone, which the reply of this verb cannot say",
            )),
        }
    }

    /// As [`Client::send_as`], and a request that took effect on a task that was deleted since,
    /// with nothing left to reply with, is [`Applied::Gone`], not an error.
    pub fn applied_as(
        &self,
        request_id: RequestId,
        request: Request,
    ) -> Result<Applied<Reply>, RuntimeError> {
        Ok(match self.exchange(request_id, request)? {
            Answered::Reply(reply) => Applied::Done(reply),
            Answered::Gone => Applied::Gone,
        })
    }

    /// A request under a new id, answered with the reply `pick` takes from it.
    fn applied<T>(
        &self,
        what: &str,
        request: Request,
        pick: impl FnOnce(Reply) -> Result<T, Box<Reply>>,
    ) -> Result<Applied<T>, RuntimeError> {
        match self.applied_as(RequestId::new(&SystemIds), request)? {
            Applied::Done(reply) => pick(reply)
                .map(Applied::Done)
                .map_err(|other| unexpected_reply(what, &other)),
            Applied::Gone => Ok(Applied::Gone),
        }
    }

    fn exchange(&self, request_id: RequestId, request: Request) -> Result<Answered, RuntimeError> {
        let asked = (request.name(), returns_task(&request));
        let task_id = request.task_id();
        let deletion = matches!(request, Request::DeleteTask { .. });
        // A deletion sent under this id before: its task may be gone because it worked.
        let sent_before = deletion && self.deletions().contains(&request_id);
        if deletion {
            self.deletions().insert(request_id);
        }
        let envelope = Envelope {
            request_id,
            session: self.session.clone(),
            request,
        };
        // A connection that broke is made again, and the same request is sent once more under the
        // same id: the store answers what the first one did, and nothing is applied twice. It is
        // never said that it is not known whether a request took effect (external design §4).
        let response = match self.transport.send(envelope.clone(), &say) {
            Ok(response) => response,
            Err(_) => self.transport.send(envelope, &say).map_err(|broken| {
                if deletion {
                    self.deletions().remove(&request_id);
                }
                RuntimeError::Unavailable {
                    component: crate::messages::daemon_connection_lost(&broken.detail),
                }
            })?,
        };
        if response.request_id != request_id {
            return Err(unexpected(
                "an answer to another request than the one that was sent",
            ));
        }
        match response.into_outcome() {
            Ok(Ok(reply)) => Ok(Answered::Reply(reply)),
            Ok(Err(already)) => {
                let (answer, said) = applied_before(asked, task_id, already)?;
                for line in said {
                    crate::output::diag(line);
                }
                Ok(answer)
            }
            Err(RuntimeError::NotFound { what }) if sent_before && what == "task" => {
                // The deletion was sent under this id before, and the task is not there now: it
                // took effect then. The id stays known, for the next repeat.
                if let Some(task_id) = task_id {
                    crate::output::diag(crate::messages::task_gone(&task_id.to_string()));
                }
                Ok(Answered::Gone)
            }
            Err(error) => {
                if deletion {
                    // Refused, so nothing was written: the id is not one a deletion took effect
                    // under, and a repeat of it is answered afresh.
                    self.deletions().remove(&request_id);
                }
                Err(error)
            }
        }
    }

    fn deletions(&self) -> std::sync::MutexGuard<'_, HashSet<RequestId>> {
        self.deletions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// A follower of the task begins: the driver takes it if it is running.
    pub fn attach(&self, task_id: TaskId) {
        self.transport.attach(task_id);
    }

    /// What the task waits behind in the daemon's line, if it waits.
    pub fn queued(&self, task_id: TaskId) -> Option<mimlys_core::api::Line> {
        self.transport.queued(task_id)
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
    ) -> Result<Applied<Grant>, RuntimeError> {
        self.applied(
            "grant_capability",
            Request::GrantCapability { task_id, kind },
            |reply| match reply {
                Reply::Grant(grant) => Ok(grant),
                other => Err(Box::new(other)),
            },
        )
    }

    /// `revoke_capability`.
    pub fn revoke_capability(
        &self,
        task_id: TaskId,
        grant_id: GrantId,
    ) -> Result<Applied<Grant>, RuntimeError> {
        self.applied(
            "revoke_capability",
            Request::RevokeCapability { task_id, grant_id },
            |reply| match reply {
                Reply::Grant(grant) => Ok(grant),
                other => Err(Box::new(other)),
            },
        )
    }

    /// `rollback_workspace`.
    pub fn rollback_workspace(
        &self,
        task_id: TaskId,
        snapshot_id: Option<SnapshotId>,
    ) -> Result<Applied<SnapshotRecord>, RuntimeError> {
        self.applied(
            "rollback_workspace",
            Request::RollbackWorkspace {
                task_id,
                snapshot_id,
            },
            |reply| match reply {
                Reply::Snapshot(record) => Ok(record),
                other => Err(Box::new(other)),
            },
        )
    }

    /// `promote_workspace`, after the user confirmed `plan`.
    pub fn promote_workspace(
        &self,
        task_id: TaskId,
        plan: &PromotionPlan,
    ) -> Result<Applied<Promotion>, RuntimeError> {
        self.applied(
            "promote_workspace",
            Request::PromoteWorkspace {
                task_id,
                plan: plan.clone(),
            },
            |reply| match reply {
                Reply::Promotion(promotion) => Ok(promotion),
                other => Err(Box::new(other)),
            },
        )
    }

    /// `revert_promotion`.
    pub fn revert_promotion(
        &self,
        task_id: TaskId,
        promotion_id: PromotionId,
        overwrite_edited: bool,
    ) -> Result<Applied<Promotion>, RuntimeError> {
        self.applied(
            "revert_promotion",
            Request::RevertPromotion {
                task_id,
                promotion_id,
                overwrite_edited,
            },
            |reply| match reply {
                Reply::Promotion(promotion) => Ok(promotion),
                other => Err(Box::new(other)),
            },
        )
    }

    /// `delete_task`. A deletion sent once more, after it took effect, is answered that the
    /// task is gone ([`Deleted::Gone`]), never as an error (external design §4).
    pub fn delete_task(&self, task_id: TaskId, audit: bool) -> Result<Deleted, RuntimeError> {
        self.delete_task_as(RequestId::new(&SystemIds), task_id, audit)
    }

    /// `delete_task` under `request_id`: the id of a deletion sent before (014c's repeat).
    pub fn delete_task_as(
        &self,
        request_id: RequestId,
        task_id: TaskId,
        audit: bool,
    ) -> Result<Deleted, RuntimeError> {
        match self.exchange(request_id, Request::DeleteTask { task_id, audit })? {
            Answered::Reply(Reply::Footprint(footprint)) => Ok(Deleted::Footprint(footprint)),
            Answered::Reply(other) => Err(unexpected_reply("delete_task", &other)),
            Answered::Gone => Ok(Deleted::Gone),
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

/// The answer to a request whose id is recorded (external design §4, "The recorded outcome"):
/// what it did, as its events say, and what the user is told of it, line by line, on standard
/// error. `name` is the request's, and `returns_task` whether its reply is the task.
fn applied_before(
    (name, returns_task): (&str, bool),
    task_id: Option<TaskId>,
    already: AlreadyApplied,
) -> Result<(Answered, Vec<String>), RuntimeError> {
    let AlreadyApplied {
        position,
        event,
        task,
        task_deleted,
        did,
    } = already;
    // It wrote, and failed: the failure, never "applied", with the kind and the detail it had.
    let reply = did.into_reply()?;
    let reply = match (reply, task) {
        (Some(reply), _) => Some(reply),
        (None, Some(task)) if returns_task => Some(Reply::Task(task)),
        (None, _) => None,
    };
    // It took effect before. If the task was deleted since (its events were kept), what was done
    // is the answer and the task is gone; a reply that is not there to give (a deletion's
    // footprint is not kept, and a snapshot or a promotion went with the task's data) says the
    // same: the task is gone.
    let mut said = vec![crate::messages::done_earlier(name, position, &event)];
    if (task_deleted || reply.is_none())
        && let Some(task_id) = task_id
    {
        said.push(crate::messages::task_gone(&task_id.to_string()));
    }
    let answer = match reply {
        Some(reply) => Answered::Reply(reply),
        None => Answered::Gone,
    };
    Ok((answer, said))
}

/// Whether the verb's reply is a task.
fn returns_task(request: &Request) -> bool {
    matches!(
        request,
        Request::CreateTask(_)
            | Request::ResumeTask { .. }
            | Request::PauseTask { .. }
            | Request::CancelTask { .. }
            | Request::ApproveAction { .. }
            | Request::RejectAction { .. }
            | Request::AnswerQuestion { .. }
            | Request::StopAtBoundary { .. }
            | Request::StopAnyway { .. }
    )
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
        // A resume that waits in line (the daemon's, external design §19.5): the task as it is,
        // and the line, once.
        Reply::Queued { task, line } => {
            crate::output::diag(crate::messages::daemon_queued_line(
                &task.task_id.to_string(),
                &line.behind.to_string(),
                &line.running_since.to_rfc3339(),
            ));
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
