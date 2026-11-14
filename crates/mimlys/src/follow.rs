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
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

/// Nothing requested, a pause requested, a cancellation requested.
const NONE: u8 = 0;
const PAUSE: u8 = 1;
const CANCEL: u8 = 2;

/// How often a pause that waits for the step is looked at again, to say that
/// the step has reached an action that cannot be undone.
const RECHECK: std::time::Duration = std::time::Duration::from_millis(250);

/// What the stop signals ask of the task's step in flight (REQ-CLI-005): the
/// runtime in the process, a fake in the unit tests.
trait Interrupt: Send + Sync {
    /// Interrupts what can be undone: the model request, or an action whose
    /// rule after `unknown` (external design §7.2) is not to halt
    /// `unknown_outcome`.
    fn interrupt(&self);
    /// Interrupts the step whatever it is doing. Returns the tool of the
    /// action that cannot be undone and was in flight, whose outcome is now
    /// unknown.
    fn interrupt_anyway(&self) -> Option<String>;
    /// The tool of the action in flight that cannot be undone, if any.
    fn irreversible(&self) -> Option<String>;
}

/// The runtime as what the signals act on.
struct StepOf {
    runtime: Arc<Runtime>,
    task_id: TaskId,
}

impl Interrupt for StepOf {
    fn interrupt(&self) {
        self.runtime.request_cancel(self.task_id);
    }

    fn interrupt_anyway(&self) -> Option<String> {
        let tool = self.irreversible();
        self.runtime.request_cancel_anyway(self.task_id);
        tool
    }

    fn irreversible(&self) -> Option<String> {
        match self.runtime.irreversible_in_flight(self.task_id) {
            Ok(action) => action.map(|a| a.tool),
            // Not being able to tell is not a reason to interrupt: the safe
            // answer is that an action that cannot be undone may be in flight.
            Err(_) => Some(messages::SIGNAL_UNKNOWN_ACTION.to_owned()),
        }
    }
}

/// The stop-signal state shared with the signal task.
pub struct Signals {
    stage: AtomicU8,
    /// Whether the wait for an action that cannot be undone was said.
    waiting_said: AtomicBool,
}

impl Signals {
    /// Installs the signal handler for `task_id`: the first signal asks for
    /// a pause and, when the step in flight cannot be interrupted without
    /// leaving an outcome unknown, says that it waits; the second interrupts
    /// anyway. A step that does not reach its end within LIM-O1 has what can
    /// be undone in it interrupted.
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
        let step: Arc<dyn Interrupt> = Arc::new(StepOf { runtime, task_id });
        tokio::spawn(async move {
            while stop.recv().await.is_ok() {
                shared.stopped(&step, grace());
            }
        });
        signals
    }

    fn new() -> Self {
        Self {
            stage: AtomicU8::new(NONE),
            waiting_said: AtomicBool::new(false),
        }
    }

    /// One stop signal arrived. The first asks for a pause: if an action that
    /// cannot be undone is in flight, it says that it waits for it; the
    /// watchdog then interrupts what can be undone after `grace` if the pause
    /// has not been applied by then. The next ones interrupt at once, an
    /// action that cannot be undone included: its outcome is then unknown.
    fn stopped(self: &Arc<Self>, step: &Arc<dyn Interrupt>, grace: std::time::Duration) {
        match self.stage.load(Ordering::SeqCst) {
            NONE => {
                self.stage.store(PAUSE, Ordering::SeqCst);
                match step.irreversible() {
                    Some(tool) => {
                        self.say_waiting(&tool);
                    }
                    None => eprintln!("{}", messages::SIGNAL_PAUSING),
                }
                let step = Arc::clone(step);
                let shared = Arc::clone(self);
                tokio::spawn(async move {
                    tokio::time::sleep(grace).await;
                    if shared.stage() != PAUSE {
                        return;
                    }
                    match step.irreversible() {
                        Some(tool) => {
                            shared.say_waiting(&tool);
                        }
                        None => {
                            eprintln!("{}", messages::SIGNAL_INTERRUPTED);
                            step.interrupt();
                        }
                    }
                    // The step may reach an action that cannot be undone after
                    // this: it is not interrupted, and the user is told why
                    // the process waits.
                    while shared.stage() == PAUSE && !shared.waiting_said() {
                        tokio::time::sleep(RECHECK).await;
                        if let Some(tool) = step.irreversible() {
                            shared.say_waiting(&tool);
                        }
                    }
                });
            }
            _ => {
                self.stage.store(CANCEL, Ordering::SeqCst);
                match step.interrupt_anyway() {
                    Some(tool) => eprintln!("{}", messages::signal_interrupting_anyway(&tool)),
                    None => eprintln!("{}", messages::SIGNAL_CANCELLING),
                }
            }
        }
    }

    /// Says, once, that the process waits for an action that cannot be undone;
    /// whether this call said it.
    fn say_waiting(&self, tool: &str) -> bool {
        let first = !self.waiting_said.swap(true, Ordering::SeqCst);
        if first {
            eprintln!("{}", messages::signal_waiting(tool));
        }
        first
    }

    fn waiting_said(&self) -> bool {
        self.waiting_said.load(Ordering::SeqCst)
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
    use std::sync::Mutex;
    use std::sync::atomic::AtomicUsize;

    /// A step that counts what was asked of it. `in_flight` is the tool of the
    /// action that cannot be undone, when one is running.
    #[derive(Default)]
    struct Fake {
        undoable: AtomicUsize,
        anyway: AtomicUsize,
        in_flight: Mutex<Option<String>>,
    }

    impl Fake {
        fn running(&self, tool: Option<&str>) {
            *self.in_flight.lock().unwrap() = tool.map(str::to_owned);
        }

        fn interrupted(&self) -> usize {
            self.undoable.load(Ordering::SeqCst)
        }

        fn interrupted_anyway(&self) -> usize {
            self.anyway.load(Ordering::SeqCst)
        }
    }

    impl Interrupt for Fake {
        fn interrupt(&self) {
            self.undoable.fetch_add(1, Ordering::SeqCst);
        }

        fn interrupt_anyway(&self) -> Option<String> {
            self.anyway.fetch_add(1, Ordering::SeqCst);
            self.irreversible()
        }

        fn irreversible(&self) -> Option<String> {
            self.in_flight.lock().unwrap().clone()
        }
    }

    fn step() -> (Arc<Fake>, Arc<dyn Interrupt>) {
        let fake = Arc::new(Fake::default());
        let step: Arc<dyn Interrupt> = fake.clone();
        (fake, step)
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

    /// REQ-CLI-005: one signal asks for a pause and interrupts nothing while the
    /// grace runs; a pause applied within it stands the watchdog down for good.
    #[tokio::test(start_paused = true)]
    async fn req_cli_005_a_pause_applied_in_time_stands_the_watchdog_down() {
        let signals = Arc::new(Signals::new());
        let (fake, step) = step();
        signals.stopped(&step, grace());
        settle().await; // the watchdog starts its sleep
        assert_eq!(signals.stage(), PAUSE);
        tokio::time::advance(grace() - std::time::Duration::from_millis(1)).await;
        settle().await;
        assert_eq!(fake.interrupted(), 0, "within the grace");
        signals.applied();
        assert_eq!(signals.stage(), NONE);
        tokio::time::advance(grace() * 3).await;
        settle().await;
        assert_eq!(fake.interrupted(), 0, "the pause was applied");
        assert_eq!(fake.interrupted_anyway(), 0);
    }

    /// REQ-CLI-005, LIM-O1: a pause not applied within the grace has what can be
    /// undone interrupted, once, and only then; nothing is interrupted anyway.
    #[tokio::test(start_paused = true)]
    async fn req_cli_005_a_pause_not_applied_in_time_interrupts_what_can_be_undone() {
        let signals = Arc::new(Signals::new());
        let (fake, step) = step();
        signals.stopped(&step, grace());
        settle().await; // the watchdog starts its sleep
        tokio::time::advance(grace() - std::time::Duration::from_millis(1)).await;
        settle().await;
        assert_eq!(fake.interrupted(), 0, "not before the grace");
        tokio::time::advance(std::time::Duration::from_millis(50)).await;
        settle().await;
        assert_eq!(fake.interrupted(), 1, "at the grace");
        assert_eq!(fake.interrupted_anyway(), 0);
        assert_eq!(signals.stage(), PAUSE, "the pause still stands");
        assert!(!signals.waiting_said(), "nothing was waited for");
    }

    /// REQ-CLI-005, LIM-O1: with an action that cannot be undone in flight, the
    /// first signal says at once that the process waits for it, and the grace
    /// passes without anything being interrupted (the action's own deadline
    /// bounds the wait): neither what can be undone, which is not in flight, nor
    /// the action.
    #[tokio::test(start_paused = true)]
    async fn req_cli_005_the_first_signal_waits_for_an_action_that_cannot_be_undone() {
        let signals = Arc::new(Signals::new());
        let (fake, step) = step();
        fake.running(Some("rypsok.send"));
        signals.stopped(&step, grace());
        assert!(
            signals.waiting_said(),
            "said at the signal, not at the grace"
        );
        settle().await;
        tokio::time::advance(grace() * 3).await;
        settle().await;
        assert_eq!(fake.interrupted(), 0, "not at the grace either");
        assert_eq!(fake.interrupted_anyway(), 0);
        assert_eq!(signals.stage(), PAUSE, "the pause waits for the step's end");
        signals.applied();
        assert_eq!(signals.stage(), NONE);
    }

    /// REQ-CLI-005: the waiting is said once, however often it is looked at.
    #[test]
    fn req_cli_005_the_waiting_is_said_once() {
        let signals = Signals::new();
        assert!(!signals.waiting_said());
        assert!(
            signals.say_waiting("rypsok.send"),
            "the first time it is said"
        );
        assert!(!signals.say_waiting("rypsok.send"), "not again");
        assert!(signals.waiting_said());
    }

    /// REQ-CLI-005: the step reaches an action that cannot be undone after the
    /// grace (the watchdog found only a model request): what could be undone was
    /// interrupted once, the action is not, and the wait is said when it starts.
    #[tokio::test(start_paused = true)]
    async fn req_cli_005_an_action_that_cannot_be_undone_reached_after_the_grace_is_waited_for() {
        let signals = Arc::new(Signals::new());
        let (fake, step) = step();
        signals.stopped(&step, grace());
        settle().await;
        tokio::time::advance(grace() + std::time::Duration::from_millis(50)).await;
        settle().await;
        assert_eq!(fake.interrupted(), 1);
        assert!(!signals.waiting_said());
        fake.running(Some("rypsok.send"));
        tokio::time::advance(RECHECK * 2).await;
        settle().await;
        assert!(signals.waiting_said(), "said when the action began");
        assert_eq!(fake.interrupted(), 1, "nothing more was interrupted");
        assert_eq!(fake.interrupted_anyway(), 0);
    }

    /// REQ-CLI-005: the second signal interrupts anyway at once, and a pause applied
    /// meanwhile does not take it back (a signal between `drive` reading the
    /// stage and marking the pause applied); the first signal's watchdog finds
    /// the cancel stage and stays quiet.
    #[tokio::test(start_paused = true)]
    async fn req_cli_005_the_second_signal_interrupts_anyway_and_is_not_taken_back() {
        let signals = Arc::new(Signals::new());
        let (fake, step) = step();
        signals.stopped(&step, grace());
        signals.stopped(&step, grace());
        assert_eq!(signals.stage(), CANCEL);
        assert_eq!(fake.interrupted_anyway(), 1);
        signals.applied();
        assert_eq!(
            signals.stage(),
            CANCEL,
            "marking the pause applied leaves the cancellation"
        );
        tokio::time::advance(grace() * 2).await;
        settle().await;
        assert_eq!(fake.interrupted(), 0, "the watchdog stayed quiet");
        assert_eq!(fake.interrupted_anyway(), 1);
    }

    /// REQ-CLI-005: the second signal during the wait for an action that cannot
    /// be undone interrupts it (the step is asked to stop anyway), and the
    /// stage is the cancellation, which `drive` does not turn into a plain
    /// `cancelled` for an action whose outcome is unknown.
    #[tokio::test(start_paused = true)]
    async fn req_cli_005_the_second_signal_interrupts_the_action_that_was_waited_for() {
        let signals = Arc::new(Signals::new());
        let (fake, step) = step();
        fake.running(Some("rypsok.send"));
        signals.stopped(&step, grace());
        assert!(signals.waiting_said());
        assert_eq!(fake.interrupted_anyway(), 0, "the first signal waits");
        signals.stopped(&step, grace());
        assert_eq!(fake.interrupted_anyway(), 1);
        assert_eq!(signals.stage(), CANCEL);
    }
}
