//! The check, the record that the action runs, and a second check (handoff
//! 010f-4, which replaced the start gate of 010f-1; audit A-33; REQ-CLI-005,
//! external design §19.2). A stop signal's handler makes its request and then
//! looks at the store for the action in flight. The rule: either the handler's
//! look finds the action `running`, or its request came before that record, and
//! the step's look after the record sees it: no call is made. The request never
//! blocks, so the tests raise it from a thread that the store's hook waits for
//! (a request that blocked would hang the hook). The verdict is always the
//! state, never a pause.

use super::*;
use std::sync::atomic::{AtomicBool, Ordering};

/// What a signal's handler found, from the thread that ran it.
type Found = Arc<Mutex<Option<Option<crate::action::Action>>>>;

/// The three stop requests, each stronger than the one before and implying it.
type Request = fn(&Runtime, TaskId);
const REQUESTS: [(&str, Request); 3] = [
    ("start_no_new_action", Runtime::start_no_new_action),
    ("interrupt_step", Runtime::interrupt_step),
    ("interrupt_step_anyway", Runtime::interrupt_step_anyway),
];

/// REQ-CLI-005, A-33: a signal raised inside `record_running` (its handler makes
/// the request, then looks at the store) does not fall between the check and the
/// start. Its look finds nothing, as the action is not yet recorded: the request
/// came before the record, so the second check sees it, no call reaches the
/// tool, and the action is closed `cancelled` from `running`, with
/// `stopped_before_start` and no execution (nothing is unknown, nothing failed).
/// Each of the three requests, for the action of every effect class; none of
/// them blocks (the hook waits for the handler's thread).
#[test]
fn req_cli_005_a_signal_raised_inside_the_record_of_the_start_is_seen_by_the_second_check() {
    for (request_name, request) in REQUESTS {
        for (definition, _) in classes() {
            let name = format!("{request_name} / {}", definition.name);
            let rig = Rig::new(definition);
            let found: Found = Arc::new(Mutex::new(None));
            let fired = Arc::new(AtomicBool::new(false));
            {
                let runtime = rig.runtime.clone();
                let persistence = rig.persistence.clone();
                let task = rig.task;
                let found = found.clone();
                rig.persistence.before_write(
                    "running",
                    Arc::new(move || {
                        if fired.swap(true, Ordering::SeqCst) {
                            return;
                        }
                        let runtime = runtime.clone();
                        let persistence = persistence.clone();
                        let found = found.clone();
                        // The signal's handler: the request first, the look after.
                        std::thread::spawn(move || {
                            request(&runtime, task);
                            *found.lock().unwrap() =
                                Some(persistence.load_running_action(task).unwrap());
                        })
                        .join()
                        .unwrap();
                    }),
                );
            }
            let result = rig.execute_the_action();
            let found = found.lock().unwrap().take().expect("the handler looked");
            // Never begun while the handler's look found nothing.
            assert!(
                rig.calls() == 0 || found.is_some(),
                "{name}: the action began after the handler found nothing"
            );
            // The look came before the record: it found nothing, and the second
            // check saw the request.
            assert!(found.is_none(), "{name}: the look came before the record");
            assert_eq!(result.outcome, StepOutcome::Cancelled, "{name}");
            assert_eq!(result.state, TaskState::Running, "{name}: pauses, no halt");
            assert_closed_not_begun(&rig, &name);
            assert!(
                rig.persistence.writes().contains(&"running"),
                "{name}: it was recorded running before the second check"
            );
            assert!(
                rig.events().contains(&"action.started".to_owned()),
                "{name}: recorded begun, as the rule says"
            );
        }
    }
}

/// REQ-CLI-005, A-33: a first signal that is made as the action's check is
/// reached (its request lands just before the write that authorizes the action,
/// the last write before the check) is seen by the first check: nothing is
/// recorded `running`, no call reaches the tool, and the action is closed
/// `cancelled` with `stopped_before_start`. Without the first check the second
/// would close it, but only after `running` and `action.started` were written.
#[test]
fn req_cli_005_a_first_signal_made_as_the_check_is_reached_is_seen_by_it() {
    for (definition, _) in classes() {
        let name = definition.name.clone();
        let rig = Rig::new(definition);
        let fired = Arc::new(AtomicBool::new(false));
        {
            let runtime = rig.runtime.clone();
            let task = rig.task;
            rig.persistence.before_write(
                "authorized",
                Arc::new(move || {
                    if fired.swap(true, Ordering::SeqCst) {
                        return;
                    }
                    let runtime = runtime.clone();
                    std::thread::spawn(move || runtime.start_no_new_action(task))
                        .join()
                        .unwrap();
                }),
            );
        }
        let result = rig.execute_the_action();
        assert_eq!(result.outcome, StepOutcome::Cancelled, "{name}");
        assert_closed_not_begun(&rig, &name);
        let events = rig.events();
        assert!(
            !events.contains(&"action.started".to_owned()),
            "{name}: {events:?}"
        );
        assert!(
            !rig.persistence.writes().contains(&"running"),
            "{name}: nothing was recorded running"
        );
    }
}

/// What both checks leave when they saw the request: no call reached the tool, it
/// was never executed, the action is `cancelled` and none `unknown`, and the one
/// observation says the stop came before it began, with no execution recorded.
fn assert_closed_not_begun(rig: &Rig, name: &str) {
    assert_eq!(rig.calls(), 0, "{name}: no call reached the tool");
    assert!(rig.saw().is_empty(), "{name}: it was never executed");
    assert_eq!(rig.action().status, ActionStatus::Cancelled, "{name}");
    assert!(
        rig.persistence
            .load_executions(rig.action().action_id)
            .unwrap()
            .is_empty(),
        "{name}: no execution record: nothing ran"
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
