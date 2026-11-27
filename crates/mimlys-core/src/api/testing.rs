//! The requests as calls, for the tests of other crates that build a runtime of their own over
//! the fakes (REQ-TEST-002; feature `test-support`). The verbs of the runtime are not callable
//! from outside this crate; these send the request of each verb through the handler, under a new
//! id, and give back what the verb gave.

use super::{Envelope, Outcome, Reply, Request, Response};
use crate::ids::{RequestId, SystemIds, TaskId};
use crate::runtime::{CreateTask, Runtime, RuntimeError, Session};
use crate::task::{Extension, Task};

impl Response {
    /// For a test: the answer of a request sent once, as the reply or the error.
    ///
    /// A recorded failure is its error, and a repeat that has its reply gives it, as the client
    /// does. A repeat of a request that took effect and has no reply to give is neither a reply nor
    /// an error, and **panics**: it is read with [`Response::into_outcome`], as the client reads it
    /// (handoff 014b-7, item 4). A success is never an error here.
    pub fn into_result(self) -> Result<Reply, RuntimeError> {
        match self.outcome {
            Outcome::Done(reply) => Ok(reply),
            Outcome::Failed(error) => Err(error),
            Outcome::AlreadyApplied(already) => match already.did.into_reply() {
                Ok(Some(reply)) => Ok(reply),
                Ok(None) => panic!(
                    "a request that took effect, with no reply to give (at event {}, `{}`): \
                     read it with `into_outcome`",
                    already.position, already.event
                ),
                Err(error) => Err(error),
            },
        }
    }
}

/// Sends `request` to the runtime's handler.
pub fn send(runtime: &Runtime, session: &Session, request: Request) -> Result<Reply, RuntimeError> {
    runtime
        .handle(Envelope {
            request_id: RequestId::new(&SystemIds),
            session: session.clone(),
            request,
        })
        .into_result()
}

fn task(reply: Reply) -> Result<Task, RuntimeError> {
    match reply {
        Reply::Task(task) => Ok(*task),
        other => panic!("a verb that returns a task answered {other:?}"),
    }
}

/// `create_task`.
pub fn create_task(
    runtime: &Runtime,
    session: &Session,
    create: CreateTask,
) -> Result<Task, RuntimeError> {
    task(send(runtime, session, Request::CreateTask(create))?)
}

/// `resume_task`.
pub fn resume_task(
    runtime: &Runtime,
    session: &Session,
    task_id: TaskId,
    extend: Option<Extension>,
) -> Result<Task, RuntimeError> {
    task(send(
        runtime,
        session,
        Request::ResumeTask { task_id, extend },
    )?)
}

/// `approve_action`.
pub fn approve_action(
    runtime: &Runtime,
    session: &Session,
    task_id: TaskId,
    note: Option<String>,
) -> Result<Task, RuntimeError> {
    task(send(
        runtime,
        session,
        Request::ApproveAction { task_id, note },
    )?)
}

/// `reject_action`.
pub fn reject_action(
    runtime: &Runtime,
    session: &Session,
    task_id: TaskId,
    note: Option<String>,
    halt: Option<bool>,
) -> Result<Task, RuntimeError> {
    task(send(
        runtime,
        session,
        Request::RejectAction {
            task_id,
            note,
            halt,
        },
    )?)
}
