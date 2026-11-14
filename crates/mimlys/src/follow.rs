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
    ///
    /// The listeners are made here, once, before the task that waits on them
    /// starts: a signal that arrives from now on is taken (the default action,
    /// which ends the process, no longer applies to it), and one that arrives
    /// while the first is being handled is not lost to a listener made after
    /// it.
    pub fn install(runtime: Arc<Runtime>, task_id: TaskId) -> Arc<Self> {
        let signals = Arc::new(Self::new());
        let Ok(mut stop) = StopSignals::new() else {
            return signals;
        };
        let shared = Arc::clone(&signals);
        let cancel: Cancel = Arc::new(move || runtime.request_cancel(task_id));
        tokio::spawn(async move {
            while stop.recv().await.is_ok() {
                shared.stopped(&cancel, grace());
            }
        });
        signals
    }

    fn new() -> Self {
        Self {
            stage: AtomicU8::new(NONE),
        }
    }

    /// One stop signal arrived. The first asks for a pause and starts the
    /// watchdog, which cancels the execution after `grace` if the pause has
    /// not been applied by then; the next ones cancel at once. `cancel` asks
    /// the task's step in flight to stop.
    fn stopped(self: &Arc<Self>, cancel: &Cancel, grace: std::time::Duration) {
        match self.stage.load(Ordering::SeqCst) {
            NONE => {
                self.stage.store(PAUSE, Ordering::SeqCst);
                eprintln!("{}", messages::SIGNAL_PAUSING);
                let cancel = Arc::clone(cancel);
                let shared = Arc::clone(self);
                tokio::spawn(async move {
                    tokio::time::sleep(grace).await;
                    if shared.stage.load(Ordering::SeqCst) == PAUSE {
                        eprintln!("{}", messages::SIGNAL_INTERRUPTED);
                        cancel();
                    }
                });
            }
            _ => {
                self.stage.store(CANCEL, Ordering::SeqCst);
                eprintln!("{}", messages::SIGNAL_CANCELLING);
                cancel();
            }
        }
    }

    fn stage(&self) -> u8 {
        self.stage.load(Ordering::SeqCst)
    }

    /// Marks the pause as applied so the watchdog stands down. A second
    /// signal that came meanwhile stays: the pause it found applied is not
    /// the answer to it.
    fn applied(&self) {
        let _ = self
            .stage
            .compare_exchange(PAUSE, NONE, Ordering::SeqCst, Ordering::SeqCst);
    }
}

/// Asks the task's step in flight to stop.
type Cancel = Arc<dyn Fn() + Send + Sync>;

/// The time a pause may take before the execution is cancelled: LIM-O1 less
/// one second, which leaves the second for the pause and the exit.
fn grace() -> std::time::Duration {
    let ms = u64::try_from(LIM_O1_STOP_MS)
        .unwrap_or(5000)
        .saturating_sub(1000);
    std::time::Duration::from_millis(ms)
}

/// The listeners for the stop signals, made once.
#[cfg(unix)]
struct StopSignals {
    interrupt: tokio::signal::unix::Signal,
    terminate: tokio::signal::unix::Signal,
}

#[cfg(unix)]
impl StopSignals {
    fn new() -> std::io::Result<Self> {
        use tokio::signal::unix::{SignalKind, signal};
        Ok(Self {
            interrupt: signal(SignalKind::interrupt())?,
            terminate: signal(SignalKind::terminate())?,
        })
    }

    /// Waits for the next SIGINT or SIGTERM; an error when neither can come.
    async fn recv(&mut self) -> std::io::Result<()> {
        tokio::select! {
            r = self.interrupt.recv() => r.ok_or_else(closed),
            r = self.terminate.recv() => r.ok_or_else(closed),
        }
    }
}

#[cfg(unix)]
fn closed() -> std::io::Error {
    std::io::Error::other("signal stream closed")
}

#[cfg(not(unix))]
struct StopSignals;

#[cfg(not(unix))]
impl StopSignals {
    fn new() -> std::io::Result<Self> {
        Ok(Self)
    }

    async fn recv(&mut self) -> std::io::Result<()> {
        tokio::signal::ctrl_c().await
    }
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
                        // The step was cancelled (the watchdog's request, or
                        // one that came as the pause was being applied) and
                        // the task is still running: the boundary is here.
                        // The pause is requested and taken by the step that
                        // follows, which returns at once: it does no work, it
                        // calls no model and no tool, and the step that was
                        // cancelled took the request with it (`run_step`), so
                        // this step finds the pause and not the request.
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    /// A `cancel` that counts its calls.
    fn counting() -> (Cancel, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = Arc::clone(&calls);
        let cancel: Cancel = Arc::new(move || {
            seen.fetch_add(1, Ordering::SeqCst);
        });
        (cancel, calls)
    }

    /// Lets the watchdog task run after the clock moved.
    async fn settle() {
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }
    }

    /// REQ-CLI-005, LIM-O1: the grace is LIM-O1 less one second.
    #[test]
    fn req_cli_005_the_grace_is_lim_o1_less_one_second() {
        assert_eq!(
            grace(),
            std::time::Duration::from_millis(u64::try_from(LIM_O1_STOP_MS).unwrap() - 1000)
        );
    }

    /// REQ-CLI-005: one signal asks for a pause and cancels nothing while the
    /// grace runs; a pause applied within it stands the watchdog down for good.
    #[tokio::test(start_paused = true)]
    async fn req_cli_005_a_pause_applied_in_time_stands_the_watchdog_down() {
        let signals = Arc::new(Signals::new());
        let (cancel, calls) = counting();
        signals.stopped(&cancel, grace());
        settle().await; // the watchdog starts its sleep
        assert_eq!(signals.stage(), PAUSE);
        tokio::time::advance(grace() - std::time::Duration::from_millis(1)).await;
        settle().await;
        assert_eq!(calls.load(Ordering::SeqCst), 0, "within the grace");
        signals.applied();
        assert_eq!(signals.stage(), NONE);
        tokio::time::advance(grace() * 3).await;
        settle().await;
        assert_eq!(calls.load(Ordering::SeqCst), 0, "the pause was applied");
    }

    /// REQ-CLI-005, LIM-O1: a pause not applied within the grace has the
    /// execution cancelled, once, and only then.
    #[tokio::test(start_paused = true)]
    async fn req_cli_005_a_pause_not_applied_in_time_cancels_the_execution() {
        let signals = Arc::new(Signals::new());
        let (cancel, calls) = counting();
        signals.stopped(&cancel, grace());
        settle().await; // the watchdog starts its sleep
        tokio::time::advance(grace() - std::time::Duration::from_millis(1)).await;
        settle().await;
        assert_eq!(calls.load(Ordering::SeqCst), 0, "not before the grace");
        tokio::time::advance(std::time::Duration::from_millis(50)).await;
        settle().await;
        assert_eq!(calls.load(Ordering::SeqCst), 1, "at the grace");
        assert_eq!(signals.stage(), PAUSE, "the pause still stands");
    }

    /// REQ-CLI-005: the second signal cancels at once, and a pause applied
    /// meanwhile does not take it back (a signal between `drive` reading the
    /// stage and marking the pause applied).
    #[tokio::test(start_paused = true)]
    async fn req_cli_005_the_second_signal_cancels_and_is_not_taken_back() {
        let signals = Arc::new(Signals::new());
        let (cancel, calls) = counting();
        signals.stopped(&cancel, grace());
        signals.stopped(&cancel, grace());
        assert_eq!(signals.stage(), CANCEL);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        signals.applied();
        assert_eq!(
            signals.stage(),
            CANCEL,
            "marking the pause applied leaves the cancellation"
        );
        // The watchdog of the first signal finds the cancel stage and stays quiet.
        tokio::time::advance(grace() * 2).await;
        settle().await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}
