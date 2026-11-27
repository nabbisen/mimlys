//! What a runtime that lives for many tasks needs (handoff 014a; REQ-DEC-001, REQ-TASK-006,
//! REQ-SCHED-004; external design §4): a task's steps never overlap, a request to interrupt
//! creates nothing for a task that is unknown or has ended, and a cancel reaches the step in
//! flight and takes turns with it. Judged by state: what the store holds and what the registries
//! hold, never how the runtime got there. A request is held in flight by a backend that waits
//! for the test (a model request that does not answer), and the calls that block run on threads
//! of their own, as a second client's would.

use super::*;
use crate::event::Event;
use crate::spi::{BackendDescriptor, BoxFuture, Cancellation, GenerateRequest, ModelBackend};
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::Poll;
use std::time::Duration;

/// A backend that holds its first request until the test releases it, or, when it `honours` the
/// token it was given, until the token is requested (the request is then aborted, as a real
/// backend aborts it). Every other request goes straight to the script.
pub(super) struct Holding {
    inner: Arc<ScriptedBackend>,
    pub(super) hold_next: AtomicBool,
    /// The held request has been sent.
    pub(super) entered: AtomicBool,
    pub(super) released: AtomicBool,
    honours: bool,
    /// The held request ended because its token was requested.
    pub(super) aborted: AtomicBool,
}

impl Holding {
    pub(super) fn new(inner: Arc<ScriptedBackend>, honours: bool) -> Arc<Self> {
        Arc::new(Self {
            inner,
            hold_next: AtomicBool::new(true),
            entered: AtomicBool::new(false),
            released: AtomicBool::new(false),
            honours,
            aborted: AtomicBool::new(false),
        })
    }
}

impl ModelBackend for Holding {
    fn describe(&self) -> BackendDescriptor {
        self.inner.describe()
    }

    fn generate<'a>(
        &'a self,
        request: GenerateRequest,
        cancel: Cancellation,
    ) -> BoxFuture<'a, Result<Vec<u8>, BackendError>> {
        Box::pin(async move {
            if self.hold_next.swap(false, Ordering::SeqCst) {
                self.entered.store(true, Ordering::SeqCst);
                let aborted = std::future::poll_fn(|cx| {
                    if self.released.load(Ordering::SeqCst) {
                        Poll::Ready(false)
                    } else if self.honours && cancel.is_cancelled() {
                        Poll::Ready(true)
                    } else {
                        cx.waker().wake_by_ref();
                        Poll::Pending
                    }
                })
                .await;
                if aborted {
                    self.aborted.store(true, Ordering::SeqCst);
                    return Err(BackendError::Cancelled);
                }
            }
            self.inner.generate(request, cancel).await
        })
    }
}

/// As `futures_block`, for futures that pend until another thread acts: it sleeps between polls.
pub(super) fn block_on_waiting<F: std::future::Future>(future: F) -> F::Output {
    use std::task::{Context, Waker};
    let mut cx = Context::from_waker(Waker::noop());
    let mut future = std::pin::pin!(future);
    loop {
        if let Poll::Ready(v) = future.as_mut().poll(&mut cx) {
            return v;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Waits for a state of the world, with a limit that only a hang reaches.
pub(super) fn until(what: &str, mut reached: impl FnMut() -> bool) {
    for _ in 0..20_000 {
        if reached() {
            return;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    panic!("never reached: {what}");
}

/// An id that no task of the test has: the harness draws its own from the counter's start.
fn unknown_task() -> TaskId {
    TaskId::new(&crate::ids::ScriptedIds::starting_at(1_000_000))
}

/// Lets the held request go when a test fails inside its scope, so that the failure is reported
/// and not a hang.
pub(super) struct Release<'a>(pub(super) &'a Holding);

impl Drop for Release<'_> {
    fn drop(&mut self) {
        self.0.released.store(true, Ordering::SeqCst);
    }
}

pub(super) struct Rig {
    pub(super) h: Harness,
    pub(super) front: Arc<Holding>,
}

pub(super) fn rig(decisions: Vec<Value>, honours: bool) -> Rig {
    let scripted = Arc::new(ScriptedBackend::with_decisions(decisions));
    let front = Holding::new(scripted.clone(), honours);
    let h = Harness::with_front(
        front.clone(),
        scripted,
        Policy::shipped(),
        vec![],
        crate::limits::LIM_C1_CONTEXT_CHARS,
        vec![],
    );
    Rig { h, front }
}

fn complete() -> Value {
    json!({"decision": "complete", "summary": "done"})
}

/// What the store holds of a task: its steps, its actions and its events.
pub(super) fn counts(h: &Harness, id: TaskId) -> (usize, usize, usize) {
    (
        h.persistence.load_steps(id).unwrap().len(),
        h.persistence.load_actions(id).unwrap().len(),
        h.persistence
            .events()
            .iter()
            .filter(|e| e.task_id == Some(id))
            .count(),
    )
}

/// The states the task was changed to, in order.
pub(super) fn state_changes(h: &Harness, id: TaskId) -> Vec<String> {
    h.persistence
        .events()
        .into_iter()
        .filter(|e| e.task_id == Some(id))
        .filter_map(|e| match e.event {
            Event::TaskStateChanged { to, .. } => Some(to),
            _ => None,
        })
        .collect()
}

/// A second task that is `running` beside the first. LIM-X1 admits one running task
/// (REQ-SCHED-001), so the task is made running in the store directly: the guard is per task,
/// and a scheduler that admits more than one is what it is for.
fn another_running(h: &Harness) -> TaskId {
    let other = h.create_with(|r| {
        r.scope.root = "/home/u/other".into();
        r.hold = true;
    });
    let mut task = h.runtime.get_task(other).unwrap().task;
    task.state = TaskState::Running;
    h.persistence
        .record_task_transition(
            &task,
            &h.runtime.event(
                Actor::Runtime,
                Some(other),
                None,
                None,
                Event::TaskPauseRequested,
            ),
        )
        .unwrap();
    other
}

/// REQ-DEC-001 (external design §4): a second `run_step` of a task whose step is in flight is
/// refused at once as `conflict`, naming the task, and writes nothing: the steps, the actions and
/// the events of the store are as they were. A step of another task runs meanwhile, and once the
/// first step ends the next one runs.
#[test]
fn req_dec_001_a_second_step_of_a_task_in_flight_is_refused_and_writes_nothing() {
    let r = rig(
        vec![
            act("read_file", "a"),
            act("read_file", "b"),
            act("read_file", "c"),
        ],
        false,
    );
    for _ in 0..3 {
        r.h.tools
            .script("read_file", ExecutionOutcome::succeeded(json!({}), 1));
    }
    let id = r.h.create();
    let other = another_running(&r.h);
    std::thread::scope(|scope| {
        let _release = Release(&r.front);
        let first = scope.spawn(|| block_on_waiting(r.h.runtime.run_step(id)));
        until("the model request is held", || {
            r.front.entered.load(Ordering::SeqCst)
        });
        let before = counts(&r.h, id);
        let refused = block_on_waiting(r.h.runtime.run_step(id)).unwrap_err();
        assert_eq!(refused.kind(), "conflict", "{refused}");
        assert!(
            refused.to_string().contains(&id.to_string()),
            "the refusal names the task: {refused}"
        );
        assert_eq!(counts(&r.h, id), before, "the refused step wrote nothing");
        // Another task's step is not delayed by the first.
        let meanwhile = block_on_waiting(r.h.runtime.run_step(other)).unwrap();
        assert_eq!(meanwhile.outcome, StepOutcome::Continued);
        assert_eq!(
            counts(&r.h, id),
            before,
            "the other task's step wrote nothing of the first task's"
        );
        r.front.released.store(true, Ordering::SeqCst);
        let result = first.join().unwrap().unwrap();
        assert_eq!(result.outcome, StepOutcome::Continued);
    });
    assert_eq!(r.h.runtime.steps.held(), 0, "no claim is left");
    // The claim ended with the step: the next one runs.
    assert!(r.h.persistence.load_steps(id).unwrap().len() == 1);
    assert_eq!(r.h.step(id).outcome, StepOutcome::Continued);
    assert_eq!(r.h.persistence.load_steps(id).unwrap().len(), 2);
}

/// REQ-DEC-001: the claim is released whatever ends the step, an error included; a step refused
/// by the state (the task is not running) leaves none either.
#[test]
fn req_dec_001_the_claim_ends_with_the_step_whatever_ends_it() {
    let r = rig(vec![act("read_file", "a")], false);
    r.front.hold_next.store(false, Ordering::SeqCst);
    let id = r.h.create();
    r.h.runtime.pause_task(&r.h.session, id).unwrap();
    assert_eq!(r.h.step(id).outcome, StepOutcome::Paused);
    // Not running: refused by the state, and no claim stays.
    let refused = block_on_waiting(r.h.runtime.run_step(id)).unwrap_err();
    assert_eq!(refused.kind(), "state");
    assert_eq!(r.h.runtime.steps.held(), 0);
    // An id that is not known: the same.
    let unknown = unknown_task();
    assert!(block_on_waiting(r.h.runtime.run_step(unknown)).is_err());
    assert_eq!(r.h.runtime.steps.held(), 0);
    assert_eq!(r.h.runtime.interruption_entries(), 1, "the paused task's");
}

/// REQ-TASK-006, REQ-DEC-001: while a cancel is in progress no step of the task begins: it is
/// refused as `conflict` and writes nothing; a second cancel of it is refused too. When the
/// cancel ends without having cancelled (it was refused), steps begin again.
#[test]
fn req_task_006_no_step_begins_while_a_cancel_is_in_progress() {
    let r = rig(vec![act("read_file", "a")], false);
    r.front.hold_next.store(false, Ordering::SeqCst);
    r.h.tools
        .script("read_file", ExecutionOutcome::succeeded(json!({}), 1));
    let id = r.h.create();
    let (cancelling, step_in_flight) = r.h.runtime.steps.begin_cancel(id).unwrap();
    assert!(!step_in_flight);
    let before = counts(&r.h, id);
    let refused = block_on_waiting(r.h.runtime.run_step(id)).unwrap_err();
    assert_eq!(refused.kind(), "conflict", "{refused}");
    assert!(refused.to_string().contains(&id.to_string()), "{refused}");
    let second = r.h.runtime.cancel_task(&r.h.session, id).unwrap_err();
    assert_eq!(second.kind(), "conflict", "{second}");
    assert_eq!(counts(&r.h, id), before, "nothing was written");
    drop(cancelling);
    assert!(!r.h.runtime.steps.cancel_in_progress(id));
    assert_eq!(r.h.step(id).outcome, StepOutcome::Continued);
}

/// REQ-TASK-006 (external design §4): a cancel while a model request is in flight aborts the
/// request, ends the task `cancelled`, and no step writes after it: the step ended first and wrote
/// its end, then the cancel wrote its own, and nothing came after. The runtime keeps nothing of
/// the task, and the providers are told that it ended (LIM-X3's counts go with it).
#[test]
fn req_task_006_a_cancel_during_a_model_request_aborts_it_and_nothing_writes_after_it() {
    let r = rig(vec![act("read_file", "a")], true);
    let id = r.h.create();
    std::thread::scope(|scope| {
        let _release = Release(&r.front);
        let stepping = scope.spawn(|| block_on_waiting(r.h.runtime.run_step(id)));
        until("the model request is held", || {
            r.front.entered.load(Ordering::SeqCst)
        });
        let task = r.h.runtime.cancel_task(&r.h.session, id).unwrap();
        assert_eq!(task.state, TaskState::Cancelled);
        let at_return = counts(&r.h, id);
        let step = stepping.join().unwrap().unwrap();
        assert_eq!(step.outcome, StepOutcome::Cancelled);
        assert_eq!(
            counts(&r.h, id),
            at_return,
            "the step wrote nothing after the cancel returned"
        );
    });
    assert!(
        r.front.aborted.load(Ordering::SeqCst),
        "the request was aborted"
    );
    assert_eq!(r.h.state(id), TaskState::Cancelled);
    let steps = r.h.persistence.load_steps(id).unwrap();
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].outcome, Some(StepOutcome::Cancelled));
    assert!(r.h.persistence.load_actions(id).unwrap().is_empty());
    // The step's end first, the cancel's after it, and nothing last but the cancel's.
    let names = r.h.event_names(id);
    let finished = names.iter().position(|n| n == "step.finished").unwrap();
    let last = names
        .iter()
        .rposition(|n| n == "task.state_changed")
        .unwrap();
    assert!(finished < last, "{names:?}");
    assert_eq!(last, names.len() - 1, "{names:?}");
    assert_eq!(r.h.runtime.interruption_entries(), 0);
    assert_eq!(r.h.runtime.steps.held(), 0);
    assert_eq!(r.h.workspaces.ended(), [id]);
    // A step after the cancel is refused by the state.
    let after = block_on_waiting(r.h.runtime.run_step(id)).unwrap_err();
    assert_eq!(after.kind(), "state");
}

/// REQ-TASK-006: a cancel of a task with no step in flight is as it was: the task is `cancelled`,
/// at once, and the runtime holds nothing of it.
#[test]
fn req_task_006_a_cancel_with_no_step_in_flight_is_unchanged() {
    let r = rig(vec![act("read_file", "a")], false);
    r.front.hold_next.store(false, Ordering::SeqCst);
    let id = r.h.create();
    let task = r.h.runtime.cancel_task(&r.h.session, id).unwrap();
    assert_eq!(task.state, TaskState::Cancelled);
    assert!(r.h.persistence.load_steps(id).unwrap().is_empty());
    assert_eq!(r.h.runtime.interruption_entries(), 0);
    assert_eq!(r.h.runtime.steps.held(), 0);
    assert_eq!(r.h.workspaces.ended(), [id]);
    // Cancelled twice: the second is refused by the state, as it always was.
    assert_eq!(
        r.h.runtime
            .cancel_task(&r.h.session, id)
            .unwrap_err()
            .kind(),
        "state"
    );
    assert_eq!(r.h.runtime.interruption_entries(), 0);
    assert_eq!(r.h.runtime.steps.held(), 0);
}

/// REQ-TASK-006 (the step finishing while the cancel runs): a step that does not see the request
/// (its model request is past stopping) finishes the task. The cancel, which waited for it,
/// reads the task after it and finds it `completed`: refused as the state error, nothing written
/// over the step's end, and the request it made is not left behind.
#[test]
fn req_task_006_a_step_that_finishes_the_task_while_the_cancel_waits_is_not_written_over() {
    let r = rig(vec![complete()], false);
    let id = r.h.create();
    std::thread::scope(|scope| {
        let _release = Release(&r.front);
        let stepping = scope.spawn(|| block_on_waiting(r.h.runtime.run_step(id)));
        until("the model request is held", || {
            r.front.entered.load(Ordering::SeqCst)
        });
        let cancelling = scope.spawn(|| r.h.runtime.cancel_task(&r.h.session, id));
        until("the cancel has asked the step to stop", || {
            r.h.runtime.no_new_action_requested(id)
        });
        // The step has not ended: the cancel waits for it, and has written nothing.
        assert_eq!(r.h.state(id), TaskState::Running);
        r.front.released.store(true, Ordering::SeqCst);
        let step = stepping.join().unwrap().unwrap();
        assert_eq!(step.state.name(), "completed");
        let refused = cancelling.join().unwrap().unwrap_err();
        assert_eq!(refused.kind(), "state", "{refused}");
    });
    assert_eq!(r.h.state(id).name(), "completed");
    let changes = state_changes(&r.h, id);
    assert_eq!(
        changes.last().map(String::as_str),
        Some("completed"),
        "the step's change of state is the last: {changes:?}"
    );
    assert!(!changes.iter().any(|to| to == "cancelled"), "{changes:?}");
    assert_eq!(r.h.runtime.interruption_entries(), 0);
    assert_eq!(r.h.runtime.steps.held(), 0);
}

/// REQ-TASK-006, LIM-O19: a step that does not end after the request is not waited for for
/// ever. The cancel is refused as `conflict`, its message names the wait (LIM-O2 at its default
/// and 3000 ms: 5000 ms), the task is as the step leaves it, the request stays, and the step
/// ends `cancelled` as soon as it can: a second cancel then cancels the task. (This test takes
/// LIM-O19.)
#[test]
fn req_task_006_a_step_that_does_not_end_is_waited_for_at_most_lim_o19() {
    let r = rig(vec![act("read_file", "a")], false);
    let id = r.h.create();
    std::thread::scope(|scope| {
        let _release = Release(&r.front);
        let stepping = scope.spawn(|| block_on_waiting(r.h.runtime.run_step(id)));
        until("the model request is held", || {
            r.front.entered.load(Ordering::SeqCst)
        });
        let refused = r.h.runtime.cancel_task(&r.h.session, id).unwrap_err();
        assert_eq!(refused.kind(), "conflict", "{refused}");
        assert!(
            refused.to_string().contains("within 5000 ms"),
            "the message names LIM-O19's value: {refused}"
        );
        assert_eq!(r.h.state(id), TaskState::Running, "the task is untouched");
        assert!(!r.h.runtime.steps.cancel_in_progress(id));
        // The request stays: the step, when it ends, ends `cancelled` with no action begun.
        r.front.released.store(true, Ordering::SeqCst);
        let step = stepping.join().unwrap().unwrap();
        assert_eq!(step.outcome, StepOutcome::Cancelled);
    });
    assert!(r.h.tools.calls().is_empty(), "no action was started");
    let task = r.h.runtime.cancel_task(&r.h.session, id).unwrap();
    assert_eq!(task.state, TaskState::Cancelled);
}

/// REQ-TASK-006, LIM-O19: the wait is LIM-O2 plus 3000 ms, never a fixed time.
#[test]
fn req_task_006_the_wait_of_a_cancel_is_lim_o2_plus_three_seconds() {
    assert_eq!(crate::limits::cancel_wait_ms(2_000), 5_000);
    assert_eq!(crate::limits::cancel_wait_ms(10_000), 13_000);
    assert_eq!(crate::limits::cancel_wait_ms(500), 3_500);
    assert_eq!(crate::limits::cancel_wait_ms(u64::MAX), u64::MAX);
}

/// REQ-TASK-006, LIM-O19: the wait follows LIM-O2 (which has no configuration key; the test
/// raises it through its seam). With LIM-O2 at 6000 ms (a wait of 9000 ms), a step that ends
/// after a time that the default wait would not have given it (here, past 5000 ms: LIM-O1's
/// old value, and the default LIM-O19) is waited for: the cancel ends the task `cancelled`, not
/// refused. The step is held without seeing the request, as an execution still within its
/// bound would be. (This test takes a little over 5 s.)
#[test]
fn req_task_006_with_lim_o2_raised_a_step_that_ends_within_it_is_cancelled_not_refused() {
    let mut r = rig(vec![act("read_file", "a")], false);
    r.h.runtime = r.h.runtime.with_cancel_grace(Duration::from_millis(6_000));
    let id = r.h.create();
    std::thread::scope(|scope| {
        let _release = Release(&r.front);
        let stepping = scope.spawn(|| block_on_waiting(r.h.runtime.run_step(id)));
        until("the model request is held", || {
            r.front.entered.load(Ordering::SeqCst)
        });
        let cancelling = scope.spawn(|| r.h.runtime.cancel_task(&r.h.session, id));
        until("the cancel has asked the step to stop", || {
            r.h.runtime.no_new_action_requested(id)
        });
        // Past the default wait, still within the raised one: nothing has ended or been refused.
        std::thread::sleep(Duration::from_millis(5_200));
        assert!(!cancelling.is_finished(), "the cancel is still waiting");
        assert_eq!(r.h.state(id), TaskState::Running);
        // The step ends (its request, past stopping, is let go) and finds the request to stop:
        // it ends `cancelled` with no action begun; the cancel, which waited, then cancels.
        r.front.released.store(true, Ordering::SeqCst);
        let step = stepping.join().unwrap().unwrap();
        assert_eq!(step.outcome, StepOutcome::Cancelled);
        let task = cancelling.join().unwrap().unwrap();
        assert_eq!(task.state, TaskState::Cancelled);
    });
    assert!(r.h.tools.calls().is_empty(), "no action was started");
    assert_eq!(r.h.state(id), TaskState::Cancelled);
}

type Request = fn(&Runtime, TaskId) -> Result<(), RuntimeError>;
const REQUESTS: [(&str, Request); 3] = [
    ("start_no_new_action", Runtime::start_no_new_action),
    ("interrupt_step", Runtime::interrupt_step),
    ("interrupt_step_anyway", Runtime::interrupt_step_anyway),
];

/// REQ-SCHED-004 (external design §4): a request to interrupt a task that is not known, or that
/// has ended, is refused with an error that says which, and creates nothing: the registry of
/// interruptions is as it was. A task that is known and not terminal is accepted, in any of its
/// states that are not the end.
#[test]
fn req_sched_004_a_request_for_an_unknown_or_an_ended_task_is_refused_and_creates_nothing() {
    for (name, request) in REQUESTS {
        let h = Harness::new(vec![]);
        let unknown = unknown_task();
        let refused = request(&h.runtime, unknown).unwrap_err();
        assert_eq!(refused.kind(), "not_found", "{name}: {refused}");
        assert!(
            refused.to_string().contains(&unknown.to_string()),
            "{name}: names the task: {refused}"
        );
        assert_eq!(h.runtime.interruption_entries(), 0, "{name}");

        let cancelled = h.create();
        h.runtime.cancel_task(&h.session, cancelled).unwrap();
        let refused = request(&h.runtime, cancelled).unwrap_err();
        assert_eq!(refused.kind(), "state", "{name}: {refused}");
        assert!(
            refused.to_string().contains("cancelled"),
            "{name}: says that it ended: {refused}"
        );
        assert_eq!(h.runtime.interruption_entries(), 0, "{name}");

        // Known and not terminal: a request is kept (a task that is paused is one).
        let live = h.create();
        h.runtime.pause_task(&h.session, live).unwrap();
        request(&h.runtime, live).unwrap();
        assert_eq!(h.runtime.interruption_entries(), 1, "{name}");
        // The end removes it.
        h.runtime.cancel_task(&h.session, live).unwrap();
        assert_eq!(h.runtime.interruption_entries(), 0, "{name}");
    }
}

/// REQ-SCHED-004: the readers create nothing either: for a task that has no entry they give a
/// token that nothing will set, and the registry is as it was.
#[test]
fn req_sched_004_the_readers_of_the_interruptions_create_nothing() {
    let h = Harness::new(vec![]);
    let unknown = unknown_task();
    let known = h.create();
    assert!(!h.runtime.step_interruption(unknown).is_cancelled());
    assert!(!h.runtime.no_new_action_requested(unknown));
    assert!(!h.runtime.boundary_interruption(unknown).is_cancelled());
    for effect in [
        EffectView {
            kind: EffectKind::WorkspaceRead,
            reversible: true,
            idempotent: true,
        },
        EffectView {
            kind: EffectKind::ExternalWrite,
            reversible: true,
            idempotent: true,
        },
    ] {
        assert!(
            !h.runtime
                .execution_interruption(unknown, effect)
                .is_cancelled()
        );
        assert!(
            !h.runtime
                .execution_interruption(known, effect)
                .is_cancelled()
        );
    }
    assert!(!h.runtime.step_interruption(known).is_cancelled());
    assert_eq!(h.runtime.interruption_entries(), 0);
}

/// REQ-SCHED-005 (LIM-X3's counts go with the task): the providers are told when a task ends,
/// however it ends: completed, failed (given up), cancelled.
#[test]
fn req_sched_005_the_providers_are_told_that_a_task_ended() {
    let h = Harness::new(vec![
        complete(),
        json!({"decision": "give_up", "reason": "no"}),
    ]);
    let done = h.create();
    assert_eq!(h.step(done).state.name(), "completed");
    assert_eq!(h.workspaces.ended(), [done]);
    let gave_up = h.create();
    let result = h.step(gave_up);
    assert!(result.state.is_terminal(), "{result:?}");
    let cancelled = h.create();
    h.runtime.cancel_task(&h.session, cancelled).unwrap();
    assert_eq!(h.workspaces.ended(), [done, gave_up, cancelled]);
}
