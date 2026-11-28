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

use crate::api::{Envelope, Line, Notice, Outcome, Reply, Request, Response};
use crate::clock::Instant;
use crate::ids::{RequestId, SystemIds, TaskId};
use crate::limits::LIM_X1_RUNNING_TASKS;
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
    /// The daemon's line (external design §19.5): a resume that LIM-X1 does not allow yet waits
    /// here and starts when the task ahead leaves `running`. Embedded mode has none: a command
    /// drives one task, and a second `running` task is refused `conflict`, as it always was.
    line: Option<Mutex<LineState>>,
}

/// The line of a driver that has one.
#[derive(Default)]
struct LineState {
    /// The resumes that wait, the next to start first, as their clients sent them.
    waiting: VecDeque<Envelope>,
    /// Since when the driver has run each task it owns.
    since: HashMap<TaskId, Instant>,
}

impl Driver {
    /// The driver of `runtime`'s tasks.
    #[must_use]
    pub fn new(runtime: Arc<Runtime>) -> Self {
        Self {
            runtime,
            owned: Mutex::new(VecDeque::new()),
            failures: Mutex::new(HashMap::new()),
            line: None,
        }
    }

    /// The driver of a daemon: as [`Driver::new`], and a resume that would exceed LIM-X1 waits
    /// in line instead of being refused, and says what it waits behind ([`Reply::Queued`]).
    #[must_use]
    pub fn with_line(runtime: Arc<Runtime>) -> Self {
        Self {
            line: Some(Mutex::new(LineState::default())),
            ..Self::new(runtime)
        }
    }

    fn line(&self) -> Option<std::sync::MutexGuard<'_, LineState>> {
        self.line
            .as_ref()
            .map(|l| l.lock().unwrap_or_else(PoisonError::into_inner))
    }

    /// What the task would wait behind if it were resumed now: the task that runs, when LIM-X1
    /// is full and this task is not that one. `None` when it may start, and without a line.
    fn ahead_of(&self, task_id: TaskId) -> Option<Line> {
        self.line.as_ref()?;
        let running = self.runtime.running_tasks().ok()?;
        if running.len() < LIM_X1_RUNNING_TASKS || running.contains(&task_id) {
            return None;
        }
        let behind = *running.first()?;
        let running_since = self
            .line()?
            .since
            .get(&behind)
            .copied()
            .unwrap_or_else(|| self.runtime.now());
        Some(Line {
            behind,
            running_since,
        })
    }

    /// The line a task waits in, if it waits.
    #[must_use]
    pub fn queued(&self, task_id: TaskId) -> Option<Line> {
        let waits = self
            .line()?
            .waiting
            .iter()
            .any(|e| e.request.task_id() == Some(task_id));
        if waits {
            self.ahead_of(task_id).or_else(|| {
                // The task ahead has gone and the next turn of the driver starts this one.
                self.runtime.running_tasks().ok().and_then(|r| {
                    r.first().map(|behind| Line {
                        behind: *behind,
                        running_since: self.runtime.now(),
                    })
                })
            })
        } else {
            None
        }
    }

    /// Every task in line, the next to start first, with what it waits behind.
    #[must_use]
    pub fn waiting(&self) -> Vec<(TaskId, Line)> {
        let ids: Vec<TaskId> = self
            .line()
            .map(|l| {
                l.waiting
                    .iter()
                    .filter_map(|e| e.request.task_id())
                    .collect()
            })
            .unwrap_or_default();
        ids.into_iter()
            .filter_map(|id| self.queued(id).map(|line| (id, line)))
            .collect()
    }

    /// Empties the line, which then starts nothing (a daemon that stops): the tasks that waited,
    /// in order. They stay as they were, `created`, `paused` or `halted`, for the user to resume.
    pub fn drain_waiting(&self) -> Vec<TaskId> {
        self.line()
            .map(|mut l| {
                l.waiting
                    .drain(..)
                    .filter_map(|e| e.request.task_id())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The tasks the driver owns, running or about to run.
    #[must_use]
    pub fn owned_tasks(&self) -> Vec<TaskId> {
        self.owned().iter().copied().collect()
    }

    /// Starts the resumes in line that LIM-X1 now allows, in order.
    fn start_waiting(&self) {
        loop {
            let next = match self.line().and_then(|l| l.waiting.front().cloned()) {
                Some(next) => next,
                None => return,
            };
            let Some(task_id) = next.request.task_id() else {
                if let Some(mut l) = self.line() {
                    l.waiting.pop_front();
                }
                continue;
            };
            if self.ahead_of(task_id).is_some() {
                return;
            }
            if let Some(mut l) = self.line() {
                l.waiting.pop_front();
            }
            let request_id = next.request_id;
            let response = self.runtime.handle(next);
            match response.outcome {
                Outcome::Done(Reply::Task(task) | Reply::Noted(task))
                    if task.state == TaskState::Running =>
                {
                    self.hand_over(task.task_id);
                }
                Outcome::Failed(error) => {
                    tracing::warn!(%task_id, %request_id, "a resume that waited in line was refused: {error}");
                    self.failures().insert(task_id, error);
                }
                _ => {}
            }
        }
    }

    /// Takes a request that ends a task's wait in line out of the line: the client cancelled,
    /// paused or deleted the task, which then is not started.
    fn leave_line(&self, request: &Request) {
        if matches!(
            request,
            Request::CancelTask { .. } | Request::PauseTask { .. } | Request::DeleteTask { .. }
        ) && let (Some(task_id), Some(mut line)) = (request.task_id(), self.line())
        {
            line.waiting
                .retain(|e| e.request.task_id() != Some(task_id));
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
        self.leave_line(&envelope.request);
        if let Some(response) = self.queue_resume(&envelope) {
            return response;
        }
        if let Some(response) = self.queue_creation(&envelope, notice) {
            return response;
        }
        let response = self.runtime.handle_noticing(envelope, notice);
        if let Outcome::Done(Reply::Task(task) | Reply::Noted(task)) = &response.outcome
            && task.state == TaskState::Running
        {
            self.hand_over(task.task_id);
        }
        response
    }

    /// What a task started now would wait behind: the task that runs, when LIM-X1 is full.
    fn ahead_of_new(&self) -> Option<Line> {
        self.line.as_ref()?;
        let running = self.runtime.running_tasks().ok()?;
        if running.len() < LIM_X1_RUNNING_TASKS {
            return None;
        }
        let behind = *running.first()?;
        let running_since = self
            .line()?
            .since
            .get(&behind)
            .copied()
            .unwrap_or_else(|| self.runtime.now());
        Some(Line {
            behind,
            running_since,
        })
    }

    /// A creation that starts its task (`hold` false), while LIM-X1 is full: the task is created
    /// held, so that the creation is recorded under the client's id as it always is, and its
    /// start waits in line under an id of its own. The answer says what it waits behind.
    fn queue_creation(&self, envelope: &Envelope, notice: &dyn Fn(Notice)) -> Option<Response> {
        let Request::CreateTask(create) = &envelope.request else {
            return None;
        };
        if create.hold {
            return None;
        }
        let line = self.ahead_of_new()?;
        let mut held = envelope.clone();
        if let Request::CreateTask(create) = &mut held.request {
            create.hold = true;
        }
        let response = self.runtime.handle_noticing(held, notice);
        let task = match &response.outcome {
            Outcome::Done(Reply::Task(task)) => task.clone(),
            _ => return Some(response),
        };
        let start = Envelope {
            request_id: RequestId::new(&SystemIds),
            session: envelope.session.clone(),
            request: Request::ResumeTask {
                task_id: task.task_id,
                extend: None,
            },
        };
        self.line()?.waiting.push_back(start);
        Some(Response {
            request_id: envelope.request_id,
            outcome: Outcome::Done(Reply::Queued { task, line }),
        })
    }

    /// A resume that LIM-X1 does not allow yet, of a task that can be resumed, waits in line: the
    /// answer says so and what it waits behind. Anything else is the runtime's to answer.
    fn queue_resume(&self, envelope: &Envelope) -> Option<Response> {
        let Request::ResumeTask { task_id, .. } = &envelope.request else {
            return None;
        };
        let line = self.ahead_of(*task_id)?;
        let task = self.runtime.persistence().load_task(*task_id).ok()??;
        if !matches!(
            task.state,
            TaskState::Created | TaskState::Paused | TaskState::Halted { .. }
        ) {
            return None;
        }
        let mut waiting = self.line()?;
        // The same request sent again (after a broken connection) is the one that waits.
        if !waiting
            .waiting
            .iter()
            .any(|e| e.request_id == envelope.request_id)
        {
            waiting.waiting.push_back(envelope.clone());
        }
        Some(Response {
            request_id: envelope.request_id,
            outcome: Outcome::Done(Reply::Queued {
                task: Box::new(task),
                line,
            }),
        })
    }

    /// Takes the task over, if it is not owned already.
    pub fn hand_over(&self, task_id: TaskId) {
        if let Some(mut line) = self.line() {
            let now = self.runtime.now();
            line.since.entry(task_id).or_insert(now);
        }
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
        let turn = self.step_owned().await;
        // A task that left `running` makes room for the next in line.
        self.start_waiting();
        turn
    }

    async fn step_owned(&self) -> Turn {
        self.start_waiting();
        let Some(task_id) = self.owned().pop_front() else {
            return Turn::Idle;
        };
        let turn = self.step_task(task_id).await;
        if !matches!(
            turn,
            Turn::Ran {
                state: TaskState::Running,
                ..
            } | Turn::Deferred(_)
        ) && let Some(mut line) = self.line()
        {
            line.since.remove(&task_id);
        }
        turn
    }

    async fn step_task(&self, task_id: TaskId) -> Turn {
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
