//! The check and the start are one step (handoff 010f, item 1; audit A-33;
//! REQ-CLI-005, external design §19.2). A stop signal's handler makes its request
//! and then looks at the store for the action in flight. The rule: either the
//! action is not begun, and then it never begins, or it is begun and recorded
//! `running` before the handler looks. The tests raise the signal from another
//! thread, as the handler is, at the two places where it used to fall between the
//! check and the start; the pauses in them only widen the window so that a
//! mutation is caught, never decide a verdict: the verdict is the state.

use super::*;
use std::sync::atomic::{AtomicBool, Ordering};

/// The fake store's hook for a signal handler on its own thread. Runs once.
struct Handler {
    thread: Arc<Mutex<Option<std::thread::JoinHandle<()>>>>,
}

impl Handler {
    fn new() -> Self {
        Self {
            thread: Arc::new(Mutex::new(None)),
        }
    }

    fn join(&self) {
        let thread = self.thread.lock().unwrap().take();
        thread.expect("the hook ran").join().unwrap();
    }
}

/// How long a hook lets the other thread run before it goes on: far more than a
/// request and a look need, so that a request that does not wait for the gate is
/// done, and has looked, by then.
const WIDEN: std::time::Duration = std::time::Duration::from_millis(150);

/// REQ-CLI-005, A-33: a first signal raised inside `record_running` (its handler
/// makes the request, then looks at the store) does not fall between the check
/// and the start. The check came first here, so the action begins, and the
/// handler's look finds it recorded `running`, whatever its class: it is never
/// begun after the handler found nothing. The request stays for the next step.
#[test]
fn req_cli_005_a_first_signal_raised_inside_the_record_of_the_start_finds_the_action_running() {
    for (definition, _) in classes() {
        let name = definition.name.clone();
        let rig = Rig::new(definition);
        let handler = Handler::new();
        let seen: Arc<Mutex<Option<Option<crate::action::Action>>>> = Arc::new(Mutex::new(None));
        let fired = Arc::new(AtomicBool::new(false));
        {
            let runtime = rig.runtime.clone();
            let persistence = rig.persistence.clone();
            let task = rig.task;
            let thread = handler.thread.clone();
            let seen = seen.clone();
            rig.persistence.before_write(
                "running",
                Arc::new(move || {
                    if fired.swap(true, Ordering::SeqCst) {
                        return;
                    }
                    let (started, running) = std::sync::mpsc::channel();
                    let runtime = runtime.clone();
                    let persistence = persistence.clone();
                    let seen = seen.clone();
                    // The signal's handler: the request first, the look after.
                    let handler = std::thread::spawn(move || {
                        started.send(()).unwrap();
                        runtime.start_no_new_action(task);
                        *seen.lock().unwrap() =
                            Some(persistence.load_running_action(task).unwrap());
                    });
                    running.recv().unwrap();
                    std::thread::sleep(WIDEN);
                    *thread.lock().unwrap() = Some(handler);
                }),
            );
        }
        // The action is held in its execution until the handler has looked: a
        // fake action ends in microseconds, and a look that comes after it has
        // ended finds nothing, rightly. What is decided is what the look finds
        // while the action is in flight.
        {
            let seen = seen.clone();
            rig.effect(move || {
                let begun = std::time::Instant::now();
                while seen.lock().unwrap().is_none() {
                    assert!(
                        begun.elapsed() < std::time::Duration::from_secs(60),
                        "the handler never looked"
                    );
                    std::thread::yield_now();
                }
            });
        }
        let result = rig.execute_the_action();
        handler.join();
        let found = seen.lock().unwrap().take().expect("the handler looked");
        // Either the action is not begun and never began, or it is begun and
        // the look found it: never begun while the look found nothing.
        assert!(
            rig.calls() == 0 || found.is_some(),
            "{name}: the action began after the handler found nothing"
        );
        // Here the check came before the request: the action begins, and was
        // recorded running before the look.
        assert_eq!(rig.calls(), 1, "{name}: the action began");
        let found = found.unwrap_or_else(|| panic!("{name}: the look found nothing"));
        assert_eq!(found.status, ActionStatus::Running, "{name}");
        assert_eq!(found.action_id, rig.action().action_id, "{name}");
        assert_eq!(result.outcome, StepOutcome::Continued, "{name}");
        // The request was not lost: the next step ends at once, without a request.
        assert!(rig.runtime.no_new_action_requested(rig.task), "{name}");
        assert_eq!(
            futures_block(rig.runtime.run_step(rig.task))
                .unwrap()
                .outcome,
            StepOutcome::Cancelled,
            "{name}"
        );
    }
}

/// REQ-CLI-005, A-33: a first signal that is being made as the action's check
/// is reached (its request holds the gate when the step arrives) is seen by the
/// check: no call reaches the tool, the action is closed `cancelled` with
/// `stopped_before_start`, nothing is recorded `running`. A check made before
/// the gate is taken would pass the request by.
#[test]
fn req_cli_005_a_first_signal_made_as_the_check_is_reached_is_seen_by_it() {
    for (definition, _) in classes() {
        let name = definition.name.clone();
        let rig = Rig::new(definition);
        let handler = Handler::new();
        let fired = Arc::new(AtomicBool::new(false));
        {
            let runtime = rig.runtime.clone();
            let task = rig.task;
            let thread = handler.thread.clone();
            // Just before the step that executes the action: the write that
            // authorizes it is the last one before its check.
            rig.persistence.before_write(
                "authorized",
                Arc::new(move || {
                    if fired.swap(true, Ordering::SeqCst) {
                        return;
                    }
                    let (holds, held) = std::sync::mpsc::channel();
                    let runtime = runtime.clone();
                    let handler = std::thread::spawn(move || {
                        // A handler that has the gate and is making its request.
                        runtime.at_the_start_gate(task, || {
                            holds.send(()).unwrap();
                            std::thread::sleep(WIDEN);
                            runtime.interruption(task).no_new_action.cancel();
                        });
                    });
                    held.recv().unwrap();
                    *thread.lock().unwrap() = Some(handler);
                }),
            );
        }
        let result = rig.execute_the_action();
        handler.join();
        assert_eq!(result.outcome, StepOutcome::Cancelled, "{name}");
        assert_eq!(rig.calls(), 0, "{name}: no call reached the tool");
        assert!(rig.saw().is_empty(), "{name}: it was never executed");
        assert_eq!(rig.action().status, ActionStatus::Cancelled, "{name}");
        let events = rig.events();
        assert!(
            !events.contains(&"action.started".to_owned()),
            "{name}: {events:?}"
        );
        assert!(
            !rig.persistence.writes().contains(&"running"),
            "{name}: nothing was recorded running"
        );
        let observations = rig.persistence.load_observations(rig.task).unwrap();
        assert_eq!(observations.len(), 1, "{name}");
        assert_eq!(
            observations[0].notices,
            [Notice::StoppedBeforeStart],
            "{name}"
        );
        for action in rig.persistence.load_actions(rig.task).unwrap() {
            assert_ne!(action.status, ActionStatus::Unknown, "{name}");
        }
    }
}

/// REQ-CLI-005, A-33: every stop request is made at the task's start gate, so
/// it waits for a step that is recording its action as running: it takes effect
/// when the holder lets go, not before. Each of the three requests (the two
/// stronger ones imply the weakest and are made at the same gate), and a request
/// made after the requests were cleared by a transition meets the same gate.
#[test]
fn req_cli_005_a_stop_request_waits_for_the_start_gate() {
    type Ask = fn(&Runtime, TaskId);
    let asks: [(&str, Ask); 3] = [
        ("start_no_new_action", Runtime::start_no_new_action),
        ("interrupt_step", Runtime::interrupt_step),
        ("interrupt_step_anyway", Runtime::interrupt_step_anyway),
    ];
    for cleared in [false, true] {
        for (name, ask) in asks {
            let h = Harness::new(vec![]);
            let id = h.create();
            let (done, finished) = std::sync::mpsc::channel();
            std::thread::scope(|scope| {
                h.runtime.at_the_start_gate(id, || {
                    if cleared {
                        // A transition cleared the tokens meanwhile: not the gate.
                        h.runtime.clear_interruption(id);
                    }
                    scope.spawn(|| {
                        ask(&h.runtime, id);
                        done.send(()).unwrap();
                    });
                    assert!(
                        finished.recv_timeout(WIDEN).is_err(),
                        "{name} cleared={cleared}: the request waits for the gate"
                    );
                    assert!(!h.runtime.no_new_action_requested(id), "{name}");
                });
                finished.recv().unwrap();
                assert!(
                    h.runtime.no_new_action_requested(id),
                    "{name} cleared={cleared}: made when the gate was let go"
                );
            });
        }
    }
}

/// REQ-CLI-005, LIM-O1: a start that panics (here inside the durable record)
/// leaves the gate usable: the signal's handler is not wedged, nor panics, and
/// the next start passes. The gate guards no data, so a poisoned gate is used.
#[test]
fn req_cli_005_a_start_that_panicked_leaves_the_gate_usable() {
    let h = Harness::new(vec![]);
    let id = h.create();
    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        h.runtime
            .at_the_start_gate(id, || panic!("a start that panicked"))
    }));
    assert!(panicked.is_err());
    h.runtime.start_no_new_action(id);
    assert!(h.runtime.no_new_action_requested(id));
    h.runtime.interrupt_step_anyway(id);
    assert_eq!(h.runtime.at_the_start_gate(id, || 7), 7);

    // The same through a step: a store that panics as the action is recorded.
    let rig = Rig::new(
        classes()
            .into_iter()
            .find(|(d, _)| d.name == "an_external_write")
            .unwrap()
            .0,
    );
    rig.persistence.before_write(
        "running",
        Arc::new(|| panic!("a store that panicked as the action was recorded")),
    );
    let panicked =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| rig.execute_the_action()));
    assert!(panicked.is_err());
    assert_eq!(rig.calls(), 0);
    // The signal's handler goes through, and is not left waiting.
    rig.runtime.start_no_new_action(rig.task);
    assert!(rig.runtime.no_new_action_requested(rig.task));
}
