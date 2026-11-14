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

/// An action in flight that the first signal does not interrupt, or the
/// store's inability to say whether there is one.
#[derive(Clone, Debug, PartialEq, Eq)]
enum InFlight {
    /// The action and its effect, which tells the user why it is waited for.
    Action {
        tool: String,
        effect: mimlys_core::action::EffectKind,
    },
    /// The store could not say. Not being able to tell is not a reason to
    /// interrupt: the safe answer is that an action that cannot be undone may
    /// be in flight.
    Unknown,
}

impl InFlight {
    /// The waiting line for this action, in words for its effect.
    fn waiting_line(&self) -> String {
        match self {
            InFlight::Action { tool, effect } => messages::signal_waiting(tool, *effect),
            InFlight::Unknown => messages::signal_waiting_unknown(),
        }
    }

    /// What the second signal's line calls it.
    fn name(&self) -> String {
        match self {
            InFlight::Action { tool, .. } => tool.clone(),
            InFlight::Unknown => messages::SIGNAL_UNKNOWN_ACTION.to_owned(),
        }
    }
}

/// What the stop signals ask of the task's step in flight (REQ-CLI-005): the
/// runtime in the process, a fake in the unit tests.
trait Interrupt: Send + Sync {
    /// Asks the step to start no new action (the weakest request). The first
    /// signal makes it before it looks at what is in flight, so that an action
    /// that starts after the look is closed without starting and one that
    /// started before is seen by the look at the grace.
    fn start_no_new_action(&self);
    /// Interrupts what can be undone: the model request, or an action whose
    /// rule after `unknown` (external design §7.2) is re-execution or rollback.
    fn interrupt(&self);
    /// Interrupts the step whatever it is doing. Returns the action that cannot
    /// be undone and was in flight, whose outcome is now unknown.
    fn interrupt_anyway(&self) -> Option<InFlight>;
    /// The action in flight that cannot be undone, if any.
    fn irreversible(&self) -> Option<InFlight>;
}

/// The runtime as what the signals act on.
struct StepOf {
    runtime: Arc<Runtime>,
    task_id: TaskId,
}

impl Interrupt for StepOf {
    fn start_no_new_action(&self) {
        self.runtime.start_no_new_action(self.task_id);
    }

    fn interrupt(&self) {
        self.runtime.interrupt_step(self.task_id);
    }

    fn interrupt_anyway(&self) -> Option<InFlight> {
        let in_flight = self.irreversible();
        self.runtime.interrupt_step_anyway(self.task_id);
        in_flight
    }

    fn irreversible(&self) -> Option<InFlight> {
        match self.runtime.irreversible_in_flight(self.task_id) {
            Ok(action) => action.map(|a| InFlight::Action {
                tool: a.tool,
                effect: a.effect,
            }),
            Err(_) => Some(InFlight::Unknown),
        }
    }
}

/// The stop-signal state shared with the signal task.
pub struct Signals {
    stage: AtomicU8,
    /// Whether the wait for an action that cannot be undone was said.
    waiting_said: AtomicBool,
    /// Whether the watchdog interrupted the step at the grace: the step's end
    /// then says so, if it ended `cancelled` (never before).
    interrupted: AtomicBool,
    /// The lines said, for the unit tests; each was also written to standard error.
    said: std::sync::Mutex<Vec<String>>,
}

impl Signals {
    /// Installs the signal handler for `task_id`: the first signal asks the
    /// step to start no new action and the task to pause and, when the step in
    /// flight cannot be interrupted without leaving an outcome unknown, says
    /// that it waits; the second interrupts anyway. A step that does not reach
    /// its end within LIM-O1 has what can be undone in it interrupted.
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
            interrupted: AtomicBool::new(false),
            said: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// Writes one line to standard error and keeps it.
    fn say(&self, line: String) {
        eprintln!("{line}");
        self.said.lock().expect("said lock").push(line);
    }

    #[cfg(test)]
    fn lines(&self) -> Vec<String> {
        self.said.lock().expect("said lock").clone()
    }

    /// One stop signal arrived. The first asks the step to start no new action
    /// and the task to pause: if an action that cannot be undone is in flight,
    /// it says that it waits for it; the watchdog then interrupts what can be
    /// undone after `grace` if the pause has not been applied by then. The next
    /// ones interrupt at once, an action that cannot be undone included: its
    /// outcome is then unknown.
    fn stopped(self: &Arc<Self>, step: &Arc<dyn Interrupt>, grace: std::time::Duration) {
        match self.stage.load(Ordering::SeqCst) {
            NONE => {
                self.stage.store(PAUSE, Ordering::SeqCst);
                // The request first, the look after it: an action that
                // slipped past the step's check was started before the look,
                // and the look (here, or at the grace) sees it; one that
                // starts after the look finds the request and is not started.
                step.start_no_new_action();
                match step.irreversible() {
                    Some(in_flight) => {
                        self.say_waiting(&in_flight);
                    }
                    None => self.say(messages::SIGNAL_PAUSING.to_owned()),
                }
                let step = Arc::clone(step);
                let shared = Arc::clone(self);
                tokio::spawn(async move {
                    tokio::time::sleep(grace).await;
                    if shared.stage() != PAUSE {
                        return;
                    }
                    match step.irreversible() {
                        Some(in_flight) => {
                            shared.say_waiting(&in_flight);
                        }
                        None => {
                            // Said by the step's end, once it has ended
                            // `cancelled` (`Signals::report_interruption`): not
                            // before, for the step may be between its model
                            // request and an action that cannot be undone.
                            shared.interrupted.store(true, Ordering::SeqCst);
                            step.interrupt();
                        }
                    }
                });
            }
            _ => {
                self.stage.store(CANCEL, Ordering::SeqCst);
                match step.interrupt_anyway() {
                    Some(in_flight) => {
                        self.say(messages::signal_interrupting_anyway(&in_flight.name()))
                    }
                    None => self.say(messages::SIGNAL_CANCELLING.to_owned()),
                }
            }
        }
    }

    /// Says, once, that the process waits for an action that cannot be undone;
    /// whether this call said it.
    fn say_waiting(&self, in_flight: &InFlight) -> bool {
        let first = !self.waiting_said.swap(true, Ordering::SeqCst);
        if first {
            self.say(in_flight.waiting_line());
        }
        first
    }

    #[cfg(test)]
    fn waiting_said(&self) -> bool {
        self.waiting_said.load(Ordering::SeqCst)
    }

    /// The step has ended `cancelled` and the pause is taken: if the watchdog
    /// interrupted it at the grace, says so, with the rollback only when the
    /// step wrote one. Said once.
    fn report_interruption(&self, rolled_back: bool) {
        if self.interrupted.swap(false, Ordering::SeqCst) {
            self.say(messages::signal_interrupted(grace().as_secs(), rolled_back));
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
                let printed = print_new(&runtime, task_id, &mut position, out)?;
                if result.outcome == StepOutcome::Cancelled && result.state == TaskState::Running {
                    if signals.stage() == CANCEL {
                        runtime.cancel_task(&app.session, task_id)?;
                    } else {
                        // The step was cancelled (the watchdog's request, or
                        // one that came as the pause was being applied) and
                        // the task is still running: the step has ended here.
                        // The pause is requested and taken by the step that
                        // follows, which returns at once: it does no work, it
                        // calls no model and no tool, and the step that was
                        // cancelled took the request with it (`run_step`), so
                        // this step finds the pause and not the request.
                        runtime.pause_task(&app.session, task_id)?;
                        signals.applied();
                        runtime.run_step(task_id).await?;
                        print_new(&runtime, task_id, &mut position, out)?;
                        // The step has ended cancelled and the pause is taken:
                        // now it is true that the step was interrupted.
                        signals.report_interruption(printed.rolled_back);
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

/// What `print_new` found among the events it printed.
#[derive(Clone, Copy, Debug, Default)]
struct Printed {
    /// A `workspace.rollback` was among them: the step's workspace changes
    /// were rolled back.
    rolled_back: bool,
}

fn print_new(
    runtime: &Runtime,
    task_id: TaskId,
    position: &mut u64,
    out: &Output,
) -> Result<Printed, AppError> {
    let mut printed = Printed::default();
    loop {
        let batch = runtime.get_events(Some(task_id), *position, 1000)?;
        if batch.is_empty() {
            return Ok(printed);
        }
        for e in &batch {
            printed.rolled_back |= e.record.event.name() == "workspace.rollback";
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
    use mimlys_core::action::EffectKind;
    use std::sync::Mutex;
    use std::sync::atomic::AtomicUsize;

    /// A step that counts what was asked of it, and in which order. `in_flight`
    /// is the action that cannot be undone, when one is running.
    #[derive(Default)]
    struct Fake {
        undoable: AtomicUsize,
        anyway: AtomicUsize,
        no_new_action: AtomicUsize,
        in_flight: Mutex<Option<InFlight>>,
        /// `start_no_new_action` and `irreversible`, in the order they were called.
        order: Mutex<Vec<&'static str>>,
    }

    impl Fake {
        fn running(&self, tool: Option<&str>) {
            *self.in_flight.lock().unwrap() = tool.map(|tool| InFlight::Action {
                tool: tool.to_owned(),
                effect: EffectKind::ExternalWrite,
            });
        }

        fn interrupted(&self) -> usize {
            self.undoable.load(Ordering::SeqCst)
        }

        fn interrupted_anyway(&self) -> usize {
            self.anyway.load(Ordering::SeqCst)
        }

        fn asked_no_new_action(&self) -> usize {
            self.no_new_action.load(Ordering::SeqCst)
        }
    }

    impl Interrupt for Fake {
        fn start_no_new_action(&self) {
            self.no_new_action.fetch_add(1, Ordering::SeqCst);
            self.order.lock().unwrap().push("start_no_new_action");
        }

        fn interrupt(&self) {
            self.undoable.fetch_add(1, Ordering::SeqCst);
        }

        fn interrupt_anyway(&self) -> Option<InFlight> {
            self.anyway.fetch_add(1, Ordering::SeqCst);
            self.in_flight.lock().unwrap().clone()
        }

        fn irreversible(&self) -> Option<InFlight> {
            self.order.lock().unwrap().push("irreversible");
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

    /// The waiting line for `rypsok.send`, an external write.
    fn waiting() -> String {
        messages::signal_waiting("rypsok.send", EffectKind::ExternalWrite)
    }

    /// Whether any line said speaks of an interruption of the step.
    fn said_interrupted(signals: &Signals) -> bool {
        signals
            .lines()
            .iter()
            .any(|l| l.contains("so it was interrupted"))
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
        assert!(!said_interrupted(&signals));
    }

    /// REQ-CLI-005, LIM-O1: a pause not applied within the grace has what can be
    /// undone interrupted, once, and only then; nothing is interrupted anyway.
    /// The watchdog does not say so: the step's end does, once the step has ended
    /// `cancelled` (`report_interruption`).
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
        assert!(
            !said_interrupted(&signals),
            "the interruption is not said at the grace: {:?}",
            signals.lines()
        );
        // The step now reaches an action that cannot be undone, as it could
        // before the request was read at the start of an execution: the runtime
        // closes such an action without starting it, so there is nothing to
        // wait for, and nothing looks again (no re-check loop): no line is said,
        // neither a wait nor an interruption, until the step's end reports.
        fake.running(Some("rypsok.send"));
        tokio::time::advance(grace() * 3).await;
        settle().await;
        assert!(!signals.waiting_said());
        assert_eq!(signals.lines(), [messages::SIGNAL_PAUSING]);
        assert_eq!(fake.interrupted(), 1, "interrupted once, at the grace");
    }

    /// REQ-CLI-005, handoff 010e-3: the first signal asks for no new action
    /// first and looks at what is in flight after, so that an action that slips
    /// in is either not started or seen by the look.
    #[tokio::test(start_paused = true)]
    async fn req_cli_005_the_first_signal_makes_its_request_before_it_looks() {
        let signals = Arc::new(Signals::new());
        let (fake, step) = step();
        signals.stopped(&step, grace());
        assert_eq!(fake.asked_no_new_action(), 1);
        assert_eq!(
            fake.order.lock().unwrap()[..2],
            ["start_no_new_action", "irreversible"],
            "the request is made before the look"
        );
        assert_eq!(
            fake.interrupted(),
            0,
            "the first request interrupts nothing"
        );
        signals.stopped(&step, grace());
        assert_eq!(
            fake.asked_no_new_action(),
            1,
            "the second signal asks for the stronger request, which implies it"
        );
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
        assert_eq!(signals.lines(), [waiting()], "said once, nothing else");
        signals.applied();
        assert_eq!(signals.stage(), NONE);
    }

    /// REQ-CLI-005: the waiting is said once, however often it is looked at.
    #[test]
    fn req_cli_005_the_waiting_is_said_once() {
        let signals = Signals::new();
        let in_flight = InFlight::Action {
            tool: "rypsok.send".into(),
            effect: EffectKind::ExternalWrite,
        };
        assert!(!signals.waiting_said());
        assert!(signals.say_waiting(&in_flight), "the first time it is said");
        assert!(!signals.say_waiting(&in_flight), "not again");
        assert!(signals.waiting_said());
        assert_eq!(signals.lines().len(), 1);
    }

    /// REQ-CLI-005 (handoff 010e-3, item 4): the step is between its model
    /// request and an action that cannot be undone when the signal comes (nothing
    /// in flight), and the action starts before the grace. The grace finds it: the
    /// wait is said, nothing is interrupted, and no line speaks of an
    /// interruption, neither then nor at the step's end, for the step was not
    /// interrupted. Before the line moved to the step's end, the watchdog's line
    /// could be printed in this race and the wait said after it.
    #[tokio::test(start_paused = true)]
    async fn req_cli_005_an_action_that_cannot_be_undone_seen_at_the_grace_is_waited_for_and_nothing_is_said_interrupted()
     {
        let signals = Arc::new(Signals::new());
        let (fake, step) = step();
        signals.stopped(&step, grace());
        settle().await;
        assert_eq!(signals.lines(), [messages::SIGNAL_PAUSING]);
        fake.running(Some("rypsok.send"));
        tokio::time::advance(grace() + std::time::Duration::from_millis(50)).await;
        settle().await;
        assert_eq!(fake.interrupted(), 0, "the action is not interrupted");
        assert!(signals.waiting_said(), "said at the grace");
        assert_eq!(
            signals.lines(),
            [messages::SIGNAL_PAUSING.to_owned(), waiting()]
        );
        // The step ends by itself and the pause is taken: nothing was interrupted.
        signals.report_interruption(false);
        signals.report_interruption(true);
        assert!(!said_interrupted(&signals), "{:?}", signals.lines());
    }

    /// REQ-CLI-005 (handoff 010e-3, item 4): when the watchdog did interrupt the
    /// step and it ended `cancelled`, the step's end says so, once, and says that
    /// the workspace changes were rolled back only when the step wrote a rollback.
    #[tokio::test(start_paused = true)]
    async fn req_cli_005_the_interruption_is_said_after_the_step_ended_and_the_rollback_only_when_there_was_one()
     {
        for rolled_back in [false, true] {
            let signals = Arc::new(Signals::new());
            let (fake, step) = step();
            signals.stopped(&step, grace());
            settle().await;
            tokio::time::advance(grace() + std::time::Duration::from_millis(50)).await;
            settle().await;
            assert_eq!(fake.interrupted(), 1);
            assert!(!said_interrupted(&signals), "not at the grace");
            signals.report_interruption(rolled_back);
            signals.report_interruption(rolled_back);
            let said = signals.lines();
            let interrupted: Vec<&String> = said
                .iter()
                .filter(|l| l.contains("so it was interrupted"))
                .collect();
            assert_eq!(interrupted.len(), 1, "said once: {said:?}");
            assert_eq!(
                interrupted[0],
                &messages::signal_interrupted(grace().as_secs(), rolled_back)
            );
            assert_eq!(
                interrupted[0].contains("workspace changes were rolled back"),
                rolled_back,
                "{}",
                interrupted[0]
            );
        }
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
    /// `cancelled` for an action whose outcome is unknown. The line predicts
    /// nothing about the outcome.
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
        assert_eq!(
            signals.lines(),
            [
                waiting(),
                "stop signal: interrupting rypsok.send although it cannot be undone".to_owned()
            ]
        );
    }

    /// REQ-CLI-005 (handoff 010e-3, item 3): the waiting line names the effect, in
    /// the words of external design §19.2, and ends with how to interrupt anyway.
    /// An effect that cannot be in flight as an action that cannot be undone, or
    /// the store's inability to say, gets the line that says mimlys cannot tell.
    #[test]
    fn req_cli_005_the_waiting_line_names_the_effect() {
        let line = |effect| messages::signal_waiting("rypsok.send", effect);
        assert_eq!(
            line(EffectKind::ExternalWrite),
            "waiting for rypsok.send to finish: it changes something outside this machine and cannot be undone; press Ctrl-C again to interrupt it anyway"
        );
        assert_eq!(
            line(EffectKind::HostWrite),
            "waiting for rypsok.send to finish: it changes files outside the task's workspace and cannot be undone; press Ctrl-C again to interrupt it anyway"
        );
        assert_eq!(
            line(EffectKind::ExternalRead),
            "waiting for rypsok.send to finish: it has sent a request outside this machine that cannot be taken back; press Ctrl-C again to interrupt it anyway"
        );
        let cannot_tell = "waiting for the action in flight to finish: mimlys cannot tell whether it can be undone; press Ctrl-C again to interrupt it anyway";
        assert_eq!(messages::signal_waiting_unknown(), cannot_tell);
        assert_eq!(InFlight::Unknown.waiting_line(), cannot_tell);
        for effect in [
            EffectKind::None,
            EffectKind::WorkspaceRead,
            EffectKind::WorkspaceWrite,
            EffectKind::HostRead,
        ] {
            assert_eq!(line(effect), cannot_tell, "{effect:?}");
        }
        // The line a lost look at the store gives, and the name the second signal's line uses.
        assert_eq!(
            InFlight::Unknown.name(),
            messages::SIGNAL_UNKNOWN_ACTION,
            "no tool to name"
        );
    }

    /// REQ-CLI-005, REQ-PERS-008 (handoff 010e-3, item 7): the words say what
    /// happened in the user's words, none of them says "boundary", and the unknown
    /// outcome covers every cause: the line and the halt text do not claim that
    /// the action was interrupted, or that the runtime stopped.
    #[test]
    fn req_cli_005_the_messages_speak_the_users_words() {
        assert_eq!(
            messages::signal_interrupted(grace().as_secs(), false),
            "the step was still running 4 seconds after the stop signal, so it was interrupted; the task is paused"
        );
        assert_eq!(
            messages::signal_interrupted(grace().as_secs(), true),
            "the step was still running 4 seconds after the stop signal, so it was interrupted, and its workspace changes were rolled back; the task is paused"
        );
        assert_eq!(
            messages::signal_interrupting_anyway("rypsok.send"),
            "stop signal: interrupting rypsok.send although it cannot be undone"
        );
        assert_eq!(
            messages::unknown_action_line("a1", "rypsok.send", EffectKind::ExternalWrite),
            "outcome unknown: action a1 (rypsok.send, external_write) did not finish by itself and cannot be undone; check what it did before relying on it"
        );
        assert_eq!(
            messages::halt_text(mimlys_core::task::HaltReason::UnknownOutcome),
            "an action that cannot be undone did not finish by itself (the runtime stopped, it was interrupted, or it reached a limit), so what it did is unknown; check before resuming"
        );
        let all = [
            messages::SIGNAL_PAUSING.to_owned(),
            messages::SIGNAL_CANCELLING.to_owned(),
            messages::signal_interrupted(grace().as_secs(), false),
            messages::signal_interrupted(grace().as_secs(), true),
            waiting(),
            messages::signal_waiting_unknown(),
            messages::signal_interrupting_anyway("t"),
            messages::unknown_action_line("a", "t", EffectKind::ExternalWrite),
            messages::halt_text(mimlys_core::task::HaltReason::UnknownOutcome).to_owned(),
        ];
        for message in &all {
            assert!(
                !message.to_lowercase().contains("boundary"),
                "not the user's word: {message}"
            );
        }
    }
}
