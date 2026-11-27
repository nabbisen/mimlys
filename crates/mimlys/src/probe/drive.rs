//! Running the probe's task and reading what it came to: the steps, a stop signal, and the
//! runtime's reading of the file (REQ-MDL-011). Over a `Runtime` it is given, so that the
//! tests of the core's fakes and the binary's tests drive the same code.

use super::classify::{End, Observed, StepFacts};
use crate::client::Client;
use mimlys_core::api::Request;
use mimlys_core::event::Event;
use mimlys_core::ids::TaskId;
use mimlys_core::native::DecisionTransport;
use mimlys_core::rejection::RejectionCode;
use mimlys_core::runtime::{Runtime, RuntimeError};
use mimlys_core::task::{FailureReason, TaskState, WaitKind};
use std::io::Read;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

/// A request to stop the probe (Ctrl-C, or SIGTERM): it ends the step in flight, which includes
/// the wait for the engine's gate (RFC 013 Amendment 1), as a stop request does for a task, and
/// the probe ends and deletes what it made. The class is then not recorded.
///
/// The request is made by [`Stop::signal`], which the command's signal task calls and which the
/// tests call as the handler does, so that the same tests stand for the signals of Unix and of
/// Windows. A request that comes before the task exists is kept and applied when it does
/// ([`Stop::arm`]).
#[derive(Default)]
pub struct Stop {
    requested: AtomicBool,
    signals: AtomicU32,
    armed: Mutex<Option<(Client, TaskId)>>,
}

impl Stop {
    /// A request that has not been made.
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Whether a stop was requested.
    pub fn requested(&self) -> bool {
        self.requested.load(Ordering::SeqCst)
    }

    /// Starts listening for the stop signals of the operating system: each calls
    /// [`Stop::signal`]. Made before anything is created, so that a signal that arrives meanwhile
    /// is taken (the default action, which ends the process at once, no longer applies).
    pub fn listen(self: &Arc<Self>) {
        let Ok(mut signals) = crate::follow::StopSignals::new() else {
            return;
        };
        let stop = Arc::clone(self);
        tokio::spawn(async move {
            while signals.recv().await.is_ok() {
                stop.signal();
            }
        });
    }

    /// A stop signal came. The first asks the step in flight to start nothing new and to end what
    /// can be undone: the model request, the wait for the engine's gate, an action of the
    /// built-in tools. A further one interrupts anyway; no tool of the probe's has an effect that
    /// cannot be undone, so it changes nothing, and it is taken all the same.
    pub fn signal(&self) {
        let first = self.signals.fetch_add(1, Ordering::SeqCst) == 0;
        self.requested.store(true, Ordering::SeqCst);
        let armed = self.armed.lock().expect("stop lock");
        if let Some((client, task_id)) = armed.as_ref() {
            Self::interrupt(client, *task_id, first);
        }
    }

    fn interrupt(client: &Client, task_id: TaskId, first: bool) {
        let send = |request| crate::follow::interrupted(client.send(request).map(|_| ()));
        if first {
            send(Request::StopAtBoundary { task_id });
            send(Request::InterruptStep { task_id });
        } else {
            send(Request::StopAnyway { task_id });
        }
    }

    /// The task now exists: a stop requested before it did is applied to it.
    fn arm(&self, client: Client, task_id: TaskId) {
        let mut armed = self.armed.lock().expect("stop lock");
        if self.requested() {
            Self::interrupt(&client, task_id, true);
        }
        *armed = Some((client, task_id));
    }

    fn disarm(&self) {
        *self.armed.lock().expect("stop lock") = None;
    }
}

/// How the driving ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Driven {
    /// The task left `running`: it completed, failed, halted or waits for the user.
    Ended,
    /// A stop was requested.
    Stopped,
}

/// Follows the task until it leaves `running`, or a stop is requested: its steps are run by the
/// driver of the runtime's client, as the steps of any task are, and never here. The limits are
/// the task's own: LIM-E2 steps (its budget) and LIM-E3 (its deadline), which the runtime
/// enforces at every step and in every request.
pub async fn drive(
    runtime: &Arc<Runtime>,
    task_id: TaskId,
    stop: &Stop,
) -> Result<Driven, RuntimeError> {
    let client = Client::in_process(
        Arc::clone(runtime),
        mimlys_core::runtime::Session {
            id: format!("pid:{}", std::process::id()),
            os_user: crate::app::os_user(),
        },
    );
    stop.arm(client.clone(), task_id);
    client.attach(task_id);
    let driven = async {
        loop {
            // A stop ends the step in flight (it asks the model request to end, and the task to
            // pause at its boundary), and the task leaves `running` when that is done: the driving
            // does not end before the step has, as the command then deletes what it made.
            if runtime.get_task(task_id)?.task.state != TaskState::Running {
                return Ok(if stop.requested() {
                    Driven::Stopped
                } else {
                    Driven::Ended
                });
            }
            client.changed().await;
            if let Some(error) = client.failure(task_id) {
                return Err(error);
            }
        }
    }
    .await;
    stop.disarm();
    driven
}

/// What the probe measured besides the class.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Measures {
    /// The steps the mini-task took.
    pub steps: u32,
    /// The model's time per step, in milliseconds: the mean of the steps that have it.
    pub model_ms_per_step: Option<u64>,
    /// The transport the last accepted decision arrived by; none when no decision was accepted.
    pub transport: Option<DecisionTransport>,
}

/// Reads what the ended task came to: its state, its steps and its events, and, for a completed
/// task, the runtime's own reading of the file (`as_asked`, called only then). Nothing here takes
/// anything the model said as the answer: the steps are the runtime's records.
pub fn observe(
    runtime: &Runtime,
    task_id: TaskId,
    as_asked: impl FnOnce() -> bool,
) -> Result<(Observed, Measures), RuntimeError> {
    let state = runtime.get_task(task_id)?.task.state;
    let steps = runtime.persistence().load_steps(task_id)?;
    let end = match &state {
        TaskState::Completed { .. } => End::Completed,
        TaskState::Failed {
            reason: FailureReason::ModelGaveUp { .. },
        } => End::GaveUp,
        TaskState::WaitingUser {
            wait_kind: WaitKind::Question,
        } => End::AskedUser,
        TaskState::Halted { halt_reason, .. } => End::Halted(*halt_reason),
        other => End::Other(other.name().to_owned()),
    };
    let facts: Vec<StepFacts> = steps
        .iter()
        .map(|s| StepFacts {
            replied: s.decision_hash.is_some() || s.rejection.is_some(),
            malformed: s.rejection == Some(RejectionCode::Malformed),
            // An `act` that was accepted has an action; one rejected after parsing (stages 4 to 10
            // of the validator: the tool, the shape of its arguments, the paths, the grants, the
            // bounds, the budget) has the decision's hash and the rejection. A decision that was
            // not an `act` has the hash and no rejection, and one that did not parse has no hash.
            acted: s.action_id.is_some() || (s.rejection.is_some() && s.decision_hash.is_some()),
        })
        .collect();
    let times: Vec<i64> = steps
        .iter()
        .filter_map(|s| s.measures.as_ref().and_then(|m| m.model_ms))
        .collect();
    let model_ms_per_step = (!times.is_empty()).then(|| {
        let total: i64 = times.iter().sum();
        u64::try_from(total / times.len() as i64).unwrap_or(0)
    });
    let mut transport = None;
    let mut position = 0;
    loop {
        let batch = runtime.get_events(Some(task_id), position, 1000)?;
        let Some(last) = batch.last() else { break };
        position = last.position;
        for positioned in &batch {
            if let Event::DecisionReceived { transport: t, .. } = positioned.record.event {
                transport = Some(t);
            }
        }
    }
    let verified = matches!(state, TaskState::Completed { .. }) && as_asked();
    Ok((
        Observed {
            end,
            steps: facts,
            verified,
        },
        Measures {
            steps: u32::try_from(steps.len()).unwrap_or(u32::MAX),
            model_ms_per_step,
            transport,
        },
    ))
}

/// Whether the file of the probe's scope, in the workspace tree at `tree`, is as the goal asks:
/// the file there holds [`super::FILE_WANTED`], a line ending after it or not. It is the runtime
/// that reads it, through a handle on the tree that cannot leave it, at most
/// [`super::READ_AT_MOST`] bytes of a regular file; a file that is missing, is not a regular file
/// or is longer is not as asked.
pub fn file_is_as_asked(tree: &Path) -> bool {
    use cap_std::ambient_authority;
    let Ok(dir) = cap_std::fs::Dir::open_ambient_dir(tree, ambient_authority()) else {
        return false;
    };
    let Ok(file) = dir.open(super::FILE) else {
        return false;
    };
    let Ok(meta) = file.metadata() else {
        return false;
    };
    if !meta.is_file() || meta.len() > super::READ_AT_MOST {
        return false;
    }
    let mut text = String::new();
    if file
        .take(super::READ_AT_MOST)
        .read_to_string(&mut text)
        .is_err()
    {
        return false;
    }
    text.trim_end_matches(['\n', '\r']) == super::FILE_WANTED
}

#[cfg(test)]
mod tests;
