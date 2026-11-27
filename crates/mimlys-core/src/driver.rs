//! The driver (RFC 014, Amendment 1, item 1; external design §4, "The driver and the follower";
//! REQ-SCHED-001, REQ-SCHED-002): the part of the runtime that runs the steps of the tasks it
//! owns, one at a time (LIM-X1), until a task waits or ends. A step is never run by a client: a
//! client that follows a task reads it, prints its events, prompts the user and sends the stop
//! signals as requests.
//!
//! The driver owns a task from the moment a request makes it `running` (a creation, a resume, an
//! approval, a rejection or an answer: [`Driver::handle`] sees the task in the reply) until a
//! step leaves it in another state. It has no executor and no clock of its own: the host runs
//! [`Driver::step_once`] in a loop, and says when it has nothing to do. In embedded mode the
//! host is the command, and the driver ends with it; the daemon (014c) is the host of the
//! driver that serves every client.

use crate::api::{Envelope, Notice, Outcome, Reply, Response};
use crate::ids::TaskId;
use crate::runtime::{Runtime, RuntimeError};
use crate::step::StepOutcome;
use crate::task::TaskState;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, PoisonError};

/// What one turn of the driver did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Turn {
    /// The driver owns no task that is running: the host waits for [`Driver::hand_over`].
    Idle,
    /// A step of the task was run.
    Ran {
        /// The task.
        task_id: TaskId,
        /// How the step ended.
        outcome: StepOutcome,
        /// The task's state after it.
        state: TaskState,
    },
    /// A request holds the task (a cancel, a grant, a revocation), so no step began: the host
    /// asks again when the request is done.
    Deferred(TaskId),
    /// The task was not `running` (a request moved it since it was handed over) or is gone: the
    /// driver let it go and ran nothing.
    Released(TaskId),
    /// The step failed, and the driver no longer owns the task: its error is kept for
    /// [`Driver::take_failure`].
    Failed(TaskId),
}

/// The driver of the tasks one runtime owns.
pub struct Driver {
    runtime: Arc<Runtime>,
    /// The tasks in line, the next to step first.
    owned: Mutex<VecDeque<TaskId>>,
    /// The errors of steps that failed, until their task's follower takes them.
    failures: Mutex<HashMap<TaskId, RuntimeError>>,
}

impl Driver {
    /// The driver of `runtime`'s tasks.
    #[must_use]
    pub fn new(runtime: Arc<Runtime>) -> Self {
        Self {
            runtime,
            owned: Mutex::new(VecDeque::new()),
            failures: Mutex::new(HashMap::new()),
        }
    }

    /// The runtime it drives.
    #[must_use]
    pub fn runtime(&self) -> &Arc<Runtime> {
        &self.runtime
    }

    /// Handles a request, and takes over the task it made `running`: **a request that makes a
    /// task `running` hands the task to the driver.**
    pub fn handle(&self, envelope: Envelope) -> Response {
        self.handle_noticing(envelope, &|_| {})
    }

    /// As [`Driver::handle`], telling `notice` what the request says while it waits.
    pub fn handle_noticing(&self, envelope: Envelope, notice: &dyn Fn(Notice)) -> Response {
        let response = self.runtime.handle_noticing(envelope, notice);
        if let Outcome::Done(Reply::Task(task) | Reply::Noted(task)) = &response.outcome
            && task.state == TaskState::Running
        {
            self.hand_over(task.task_id);
        }
        response
    }

    /// Takes the task over, if it is not owned already.
    pub fn hand_over(&self, task_id: TaskId) {
        self.failures().remove(&task_id);
        let mut owned = self.owned();
        if !owned.contains(&task_id) {
            owned.push_back(task_id);
        }
    }

    /// Whether the driver owns the task.
    #[must_use]
    pub fn owns(&self, task_id: TaskId) -> bool {
        self.owned().contains(&task_id)
    }

    /// The error of the step that failed for this task, once.
    pub fn take_failure(&self, task_id: TaskId) -> Option<RuntimeError> {
        self.failures().remove(&task_id)
    }

    /// Runs one step of the task at the head of the line. A task that is no longer `running`
    /// (a step left it waiting, paused, halted or ended; another request moved it) is let go.
    pub async fn step_once(&self) -> Turn {
        let Some(task_id) = self.owned().pop_front() else {
            return Turn::Idle;
        };
        match self.runtime.run_step(task_id).await {
            Ok(view) => {
                if view.state == TaskState::Running {
                    // Behind the others, which then have their turn.
                    self.owned().push_back(task_id);
                }
                Turn::Ran {
                    task_id,
                    outcome: view.outcome,
                    state: view.state,
                }
            }
            // A request holds the task for a moment: it keeps its place.
            Err(RuntimeError::Conflict { .. }) => {
                self.owned().push_front(task_id);
                Turn::Deferred(task_id)
            }
            // Not running any more, or gone: nothing to step.
            Err(RuntimeError::State(_) | RuntimeError::NotFound { .. }) => Turn::Released(task_id),
            Err(error) => {
                self.failures().insert(task_id, error);
                Turn::Failed(task_id)
            }
        }
    }

    fn owned(&self) -> std::sync::MutexGuard<'_, VecDeque<TaskId>> {
        self.owned.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn failures(&self) -> std::sync::MutexGuard<'_, HashMap<TaskId, RuntimeError>> {
        self.failures.lock().unwrap_or_else(PoisonError::into_inner)
    }
}
