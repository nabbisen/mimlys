//! The requests as calls, for the tests of other crates that build a runtime of their own over
//! the fakes (REQ-TEST-002; feature `test-support`). The verbs of the runtime are not callable
//! from outside this crate; these send the request of each verb through the handler, under a new
//! id, and give back what the verb gave.

use super::{Envelope, Reply, Request};
use crate::ids::{RequestId, SystemIds, TaskId};
use crate::runtime::{CreateTask, Runtime, RuntimeError, Session};
use crate::task::{Extension, Task};

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
