//! Follow mode (REQ-CLI-003, REQ-CLI-005): drives a task's steps in this
//! process, prints events as they happen, prompts when the task waits for
//! the user, and turns stop signals into a pause (first) or a cancellation
//! (second), leaving within LIM-O1.

use crate::app::{App, AppError};
use crate::messages;
use crate::output::Output;
use crate::prompt::{Decision, Interaction};
use mimlys_core::ids::TaskId;
use mimlys_core::limits::LIM_O1_STOP_MS;
use mimlys_core::runtime::{Runtime, TaskView};
use mimlys_core::spi::PositionedEvent;
use mimlys_core::step::StepOutcome;
use mimlys_core::task::{TaskState, WaitKind};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

/// Nothing requested, a pause requested, a cancellation requested.
const NONE: u8 = 0;
const PAUSE: u8 = 1;
const CANCEL: u8 = 2;

/// The stop-signal state shared with the signal task.
pub struct Signals {
    stage: AtomicU8,
}

impl Signals {
    /// Installs the signal handler for `task_id`: the first signal asks for
    /// a pause, the second for a cancellation; a step that does not reach
    /// its boundary within LIM-O1 has its execution cancelled.
    pub fn install(runtime: Arc<Runtime>, task_id: TaskId) -> Arc<Self> {
        let signals = Arc::new(Self {
            stage: AtomicU8::new(NONE),
        });
        let shared = Arc::clone(&signals);
        tokio::spawn(async move {
            loop {
                if stop_signal().await.is_err() {
                    return;
                }
                match shared.stage.load(Ordering::SeqCst) {
                    NONE => {
                        shared.stage.store(PAUSE, Ordering::SeqCst);
                        eprintln!("{}", messages::SIGNAL_PAUSING);
                        let runtime = Arc::clone(&runtime);
                        let shared = Arc::clone(&shared);
                        tokio::spawn(async move {
                            let grace = u64::try_from(LIM_O1_STOP_MS)
                                .unwrap_or(5000)
                                .saturating_sub(1000);
                            tokio::time::sleep(std::time::Duration::from_millis(grace)).await;
                            if shared.stage.load(Ordering::SeqCst) == PAUSE {
                                eprintln!("{}", messages::SIGNAL_INTERRUPTED);
                                runtime.request_cancel(task_id);
                            }
                        });
                    }
                    _ => {
                        shared.stage.store(CANCEL, Ordering::SeqCst);
                        eprintln!("{}", messages::SIGNAL_CANCELLING);
                        runtime.request_cancel(task_id);
                    }
                }
            }
        });
        signals
    }

    fn stage(&self) -> u8 {
        self.stage.load(Ordering::SeqCst)
    }

    /// Marks the pause as applied so the watchdog stands down.
    fn applied(&self) {
        self.stage.store(NONE, Ordering::SeqCst);
    }
}

#[cfg(unix)]
async fn stop_signal() -> std::io::Result<()> {
    use tokio::signal::unix::{SignalKind, signal};
    let mut term = signal(SignalKind::terminate())?;
    tokio::select! {
        r = tokio::signal::ctrl_c() => r,
        _ = term.recv() => Ok(()),
    }
}

#[cfg(not(unix))]
async fn stop_signal() -> std::io::Result<()> {
    tokio::signal::ctrl_c().await
}

/// Runs the task until it waits for the user without an interactive
/// channel, pauses, halts or ends; prompts and continues otherwise.
pub async fn drive(
    app: &App,
    task_id: TaskId,
    interaction: &mut dyn Interaction,
    out: &Output,
) -> Result<TaskView, AppError> {
    let runtime = Arc::clone(&app.runtime);
    let signals = Signals::install(Arc::clone(&runtime), task_id);
    let mut position = last_position(&runtime, task_id)?;
    loop {
        let view = runtime.get_task(task_id)?;
        match view.task.state {
            TaskState::Running => {
                match signals.stage() {
                    CANCEL => {
                        runtime.cancel_task(&app.session, task_id)?;
                        print_new(&runtime, task_id, &mut position, out)?;
                        continue;
                    }
                    PAUSE => {
                        runtime.pause_task(&app.session, task_id)?;
                        signals.applied();
                    }
                    _ => {}
                }
                let result = runtime.run_step(task_id).await?;
                print_new(&runtime, task_id, &mut position, out)?;
                if result.outcome == StepOutcome::Cancelled && result.state == TaskState::Running {
                    if signals.stage() == CANCEL {
                        runtime.cancel_task(&app.session, task_id)?;
                    } else {
                        runtime.pause_task(&app.session, task_id)?;
                        signals.applied();
                        runtime.run_step(task_id).await?;
                    }
                    print_new(&runtime, task_id, &mut position, out)?;
                }
            }
            TaskState::WaitingUser { wait_kind } => {
                if !interaction.interactive() {
                    out.line(messages::pending_non_interactive(
                        &task_id.to_string(),
                        wait_kind,
                    ));
                    return Ok(view);
                }
                match wait_kind {
                    WaitKind::Approval => {
                        let Some(approval) = view.pending_approval.clone() else {
                            return Ok(view);
                        };
                        let step_index = runtime
                            .persistence()
                            .load_actions(task_id)?
                            .iter()
                            .find(|a| a.action_id == approval.action_id)
                            .and_then(|a| {
                                runtime
                                    .persistence()
                                    .load_steps(task_id)
                                    .ok()?
                                    .iter()
                                    .find(|s| s.step_id == a.step_id)
                                    .map(|s| s.index)
                            });
                        match interaction.approve(&approval, step_index) {
                            Decision::Approve => {
                                runtime.approve_action(&app.session, task_id)?;
                            }
                            Decision::Reject { note, halt } => {
                                runtime.reject_action(&app.session, task_id, note, Some(halt))?;
                            }
                            Decision::Grant(kind) => {
                                let grant =
                                    runtime.grant_capability(&app.session, task_id, kind)?;
                                out.line(messages::grant_issued(
                                    &grant.grant_id.to_string(),
                                    &serde_json::to_string(grant.kind()).unwrap_or_default(),
                                ));
                            }
                            Decision::Leave => return Ok(view),
                        }
                    }
                    WaitKind::Question => {
                        let Some(question) = view.pending_question.clone() else {
                            return Ok(view);
                        };
                        match interaction.answer(&question) {
                            Some(text) => {
                                runtime.answer_question(&app.session, task_id, &text)?;
                            }
                            None => return Ok(view),
                        }
                    }
                }
                print_new(&runtime, task_id, &mut position, out)?;
            }
            _ => return Ok(view),
        }
    }
}

fn last_position(runtime: &Runtime, task_id: TaskId) -> Result<u64, AppError> {
    let mut position = 0;
    loop {
        let batch = runtime.get_events(Some(task_id), position, 1000)?;
        match batch.last() {
            Some(last) => position = last.position,
            None => return Ok(position),
        }
    }
}

fn print_new(
    runtime: &Runtime,
    task_id: TaskId,
    position: &mut u64,
    out: &Output,
) -> Result<(), AppError> {
    loop {
        let batch = runtime.get_events(Some(task_id), *position, 1000)?;
        if batch.is_empty() {
            return Ok(());
        }
        for e in &batch {
            out.line(event_text(e));
            *position = e.position;
        }
    }
}

/// One event as a human line: time, name, and the payload as `key=value`.
pub fn event_text(e: &PositionedEvent) -> String {
    let at = e.record.at.to_rfc3339();
    let time = at.get(11..19).unwrap_or(&at).to_owned();
    let value = serde_json::to_value(&e.record.event).unwrap_or_default();
    let detail = match value.as_object() {
        Some(map) => map
            .iter()
            .filter(|(k, _)| k.as_str() != "event")
            .map(|(k, v)| match v {
                serde_json::Value::String(s) => format!("{k}={s}"),
                other => format!("{k}={other}"),
            })
            .collect::<Vec<_>>()
            .join(" "),
        None => String::new(),
    };
    messages::event_line(&time, e.record.event.name(), &detail)
}
