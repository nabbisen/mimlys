//! A wait made before anything is sent observes the first stop request (handoff
//! 010f, item 2; REQ-CLI-005, external design §7.5, §19.2). The action was
//! recorded `running`, the provider (the MCP pin check) stops at the start
//! gate's token and reports "not started": the loop closes the action as one that
//! never began (`stopped_before_start`, no execution), the step ends `cancelled`,
//! and the task pauses. It is not a cancellation of a call (which would leave an
//! action that cannot be undone `unknown` and halt the task) and not a failure of
//! the tool (`tool_unavailable`, which halts a task whose action is not
//! idempotent). The verdicts are states: the task, the action, the records.

use super::*;

/// A first stop request made while the provider waits before anything is sent,
/// for the action of every effect class: the action ends not begun and the task
/// pauses, never halts.
#[test]
fn req_cli_005_a_wait_before_anything_is_sent_ends_at_the_first_request_with_the_action_not_begun()
{
    for (definition, _) in classes() {
        let name = definition.name.clone();
        let rig = Rig::new(definition);
        *rig.provider.stop_at_start_gate.lock().unwrap() = true;
        rig.ask(Ask::NoNewAction);
        let result = rig.execute_the_action();
        // The step ends `cancelled`, the task is still running: it pauses, it does
        // not halt (neither `tool_unavailable` nor `unknown_outcome`).
        assert_eq!(result.outcome, StepOutcome::Cancelled, "{name}");
        assert_eq!(result.state, TaskState::Running, "{name}");
        assert_eq!(
            rig.last_state_change().map(|(to, _)| to),
            Some("running".to_owned()),
            "{name}: the last state change is the task's creation: no halt"
        );
        let action = rig.action();
        assert_eq!(action.status, ActionStatus::Cancelled, "{name}");
        assert_eq!(action.executions, 1, "{name}: begun once, and not again");
        assert!(
            rig.persistence
                .load_executions(action.action_id)
                .unwrap()
                .is_empty(),
            "{name}: no execution record: nothing ran"
        );
        assert_eq!(rig.calls(), 0, "{name}: the tool received no call");
        let events = rig.events();
        assert_eq!(
            events.iter().filter(|e| *e == "action.started").count(),
            1,
            "{name}: recorded begun, as the rule says: {events:?}"
        );
        for never in ["execution.finished", "workspace.rollback"] {
            assert!(
                !events.contains(&never.to_owned()),
                "{name}: no {never}: {events:?}"
            );
        }
        assert!(
            events.contains(&"action.finished".to_owned()),
            "{name}: {events:?}"
        );
        let observations = rig.persistence.load_observations(rig.task).unwrap();
        assert_eq!(observations.len(), 1, "{name}");
        assert_eq!(
            observations[0].status,
            ObservationStatus::Cancelled,
            "{name}"
        );
        assert_eq!(
            observations[0].notices,
            [Notice::StoppedBeforeStart],
            "{name}"
        );
        assert!(!observations[0].rolled_back, "{name}");
        assert!(
            rig.runtime
                .get_task(rig.task)
                .unwrap()
                .unknown_actions
                .is_empty(),
            "{name}: nothing was sent, so nothing is unknown"
        );
        // The caller pauses, as after any cancelled step.
        rig.runtime.pause_task(&rig.session, rig.task).unwrap();
        let paused = futures_block(rig.runtime.run_step(rig.task)).unwrap();
        assert_eq!(paused.outcome, StepOutcome::Paused, "{name}");
        assert_eq!(paused.state, TaskState::Paused, "{name}");
    }
}

/// The token a wait before anything is sent observes is set by every stop
/// request, the weakest included, and is not the token of an action that cannot
/// be undone: a first request leaves that token alone.
#[test]
fn req_cli_005_every_stop_request_sets_the_start_gates_token_and_only_the_second_the_strong_one() {
    for (request, strong) in [
        (Ask::NoNewAction, false),
        (Ask::Undoable, false),
        (Ask::Anyway, true),
    ] {
        for name in ["an_external_write", "a_non_idempotent_read", "a_read"] {
            let (definition, undoable) =
                classes().into_iter().find(|(d, _)| d.name == name).unwrap();
            let rig = Rig::new(definition);
            rig.ask(request);
            rig.execute_the_action();
            assert_eq!(
                *rig.provider.saw_start_gate.lock().unwrap(),
                [true],
                "{name} {request:?}: the start gate's token is set"
            );
            // The token the call observes: set by the second request for every
            // action, and by the first for one that can be undone.
            let call_token_set = strong || (undoable && request == Ask::Undoable);
            assert_eq!(rig.saw(), [call_token_set], "{name} {request:?}");
        }
    }
}

/// A cancelled outcome of a call that was sent is not "not started": only the
/// provider's own report, the cancelled outcome with the notice, is.
#[test]
fn req_cli_005_a_cancelled_call_is_not_an_outcome_that_was_not_started() {
    assert!(ExecutionOutcome::not_started(3).is_not_started());
    assert!(!ExecutionOutcome::cancelled(3).is_not_started());
    assert!(!ExecutionOutcome::timed_out(3).is_not_started());
    // The notice on any other termination does not make it one.
    let mut other = ExecutionOutcome::timed_out(3);
    other.notices.push(Notice::StoppedBeforeStart);
    assert!(!other.is_not_started());
}
