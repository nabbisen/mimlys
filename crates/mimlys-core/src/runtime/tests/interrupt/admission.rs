//! The execution's slot among the user's runtimes (handoff 013d; REQ-SCHED-006, REQ-CLI-005; RFC
//! 013 decision 5 with Amendment 1; external design §19.3), through real steps over the fakes and
//! a scripted admission. The slot is taken before the action is recorded `running` and returned
//! when the execution ends; a wait for it is one more wait before the send, ended by the first
//! stop request with the action not begun; a registry that cannot be used is said, once, and the
//! execution goes on. The verdict is always the state the step leaves, never a pause.

use super::second_check::assert_closed_not_begun;
use super::*;
use crate::event::{AdmissionState, Event};
use crate::fake_admission::{Script, ScriptedAdmission};
use std::sync::atomic::{AtomicBool, Ordering};

type Request = fn(&Runtime, TaskId);
const REQUESTS: [(&str, Request); 3] = [
    ("start_no_new_action", Runtime::start_no_new_action),
    ("interrupt_step", Runtime::interrupt_step),
    ("interrupt_step_anyway", Runtime::interrupt_step_anyway),
];

fn rig_with(definition: ToolDefinition, script: Vec<Script>) -> (Rig, Arc<ScriptedAdmission>) {
    let admission = ScriptedAdmission::new(script);
    let rig = Rig::admitted(
        definition,
        ExecutionOutcome::succeeded(json!({}), 1),
        Some(admission.clone()),
    );
    (rig, admission)
}

/// The `task.admission_changed` events of the task, in order.
type Change = (AdmissionState, Option<u32>, Option<String>, Option<u64>);

fn changes(rig: &Rig) -> Vec<Change> {
    rig.persistence
        .events()
        .into_iter()
        .filter(|e| e.task_id == Some(rig.task))
        .filter_map(|e| match e.event {
            Event::TaskAdmissionChanged {
                state,
                slots_taken,
                reason,
                waited_ms,
            } => Some((state, slots_taken, reason, waited_ms)),
            _ => None,
        })
        .collect()
}

/// REQ-SCHED-006: the slot is taken before the action is recorded `running`, held while the
/// tool is called, and returned when the execution ends, before its outcome is recorded; for
/// the action of every effect class.
#[test]
fn req_sched_006_the_slot_is_taken_before_running_and_returned_before_the_outcome() {
    for (definition, _) in classes() {
        let name = definition.name.clone();
        let (rig, admission) = rig_with(definition, vec![]);
        let at_admit: Arc<Mutex<Vec<Vec<&'static str>>>> = Arc::default();
        let held_at: Arc<Mutex<Vec<(&'static str, usize)>>> = Arc::default();
        {
            let persistence = rig.persistence.clone();
            let at_admit = at_admit.clone();
            admission.on_admit(move || at_admit.lock().unwrap().push(persistence.writes()));
        }
        for write in ["running", "execution"] {
            let admission = admission.clone();
            let held_at = held_at.clone();
            rig.persistence.before_write(
                write,
                Arc::new(move || held_at.lock().unwrap().push((write, admission.held()))),
            );
        }
        let result = rig.execute_the_action();
        assert_eq!(result.outcome, StepOutcome::Continued, "{name}");
        let at_admit = at_admit.lock().unwrap().clone();
        assert_eq!(at_admit.len(), 1, "{name}: one slot for one execution");
        assert!(
            !at_admit[0].contains(&"running"),
            "{name}: the slot was taken after `running` was recorded: {:?}",
            at_admit[0]
        );
        assert_eq!(
            *held_at.lock().unwrap(),
            [("running", 1), ("execution", 0)],
            "{name}: held from before the call, returned before the outcome is recorded"
        );
        assert_eq!(admission.taken(), 1, "{name}");
        assert_eq!(admission.held(), 0, "{name}: returned");
        assert_eq!(rig.calls(), 1, "{name}");
        assert!(
            changes(&rig).is_empty(),
            "{name}: a slot that was free changes nothing in the events"
        );
    }
}

/// REQ-SCHED-006: a task whose execution waits for a slot stays `running`, and the events
/// record both changes in order, before the action is recorded begun: `waiting` (the slots
/// taken) and `admitted` (with how long it waited); the execution then runs.
#[test]
fn req_sched_006_a_wait_for_a_slot_is_recorded_waiting_then_admitted() {
    let (rig, admission) = rig_with(
        effect_tool("a_read", EffectKind::WorkspaceRead, true, true),
        vec![Script::WaitThenAdmit {
            holders: vec![4242, 4243],
            waited_ms: 1500,
        }],
    );
    let state_during = Arc::new(Mutex::new(None));
    {
        let runtime = rig.runtime.clone();
        let task = rig.task;
        let state_during = state_during.clone();
        admission.on_admit(move || {
            *state_during.lock().unwrap() = Some(runtime.get_task(task).unwrap().task.state);
        });
    }
    let result = rig.execute_the_action();
    assert_eq!(result.outcome, StepOutcome::Continued);
    assert_eq!(
        *state_during.lock().unwrap(),
        Some(TaskState::Running),
        "the task stays running while its execution waits"
    );
    assert_eq!(
        changes(&rig),
        [
            (AdmissionState::Waiting, Some(2), None, None),
            (AdmissionState::Admitted, Some(1), None, Some(1500)),
        ]
    );
    let events = rig.events();
    let position = |name: &str| events.iter().position(|e| e == name).unwrap();
    let waiting = events
        .iter()
        .position(|e| e == "task.admission_changed")
        .unwrap();
    assert!(
        waiting < position("action.started"),
        "the wait is before the action is recorded begun: {events:?}"
    );
    assert_eq!(rig.calls(), 1);
    assert_eq!(admission.held(), 0);
}

/// REQ-SCHED-006, REQ-CLI-005, RFC 013 Amendment 1: the first stop request ends the wait for a
/// slot, whichever of the three requests it is (each implies the first): the action is closed
/// `cancelled` with `stopped_before_start`, nothing was recorded `running` and no
/// `action.started` stands, no call reached the tool, no slot is held, the step ends
/// `cancelled` and the task is still `running` (the caller pauses it).
#[test]
fn req_sched_006_the_first_stop_request_ends_the_wait_with_the_action_not_begun() {
    for (request_name, request) in REQUESTS {
        for (definition, _) in classes() {
            let name = format!("{request_name} / {}", definition.name);
            let (rig, admission) = rig_with(definition, vec![Script::WaitUntilStopped]);
            {
                let runtime = rig.runtime.clone();
                let task = rig.task;
                admission.on_admit(move || request(&runtime, task));
            }
            let result = rig.execute_the_action();
            assert_eq!(result.outcome, StepOutcome::Cancelled, "{name}");
            assert_eq!(result.state, TaskState::Running, "{name}: pauses, no halt");
            assert_closed_not_begun(&rig, &name);
            assert!(
                !rig.persistence.writes().contains(&"running"),
                "{name}: nothing was recorded running"
            );
            assert!(
                !rig.events().contains(&"action.started".to_owned()),
                "{name}: {:?}",
                rig.events()
            );
            assert_eq!(admission.held(), 0, "{name}: no slot is held");
            assert_eq!(
                changes(&rig),
                [(AdmissionState::Waiting, Some(1), None, None)],
                "{name}: the wait began, and was not an admission"
            );
        }
    }
}

/// REQ-SCHED-006: a stop request that lands just as the slot is granted is seen by the look
/// after the slot: the slot goes back, the action never begins, and nothing is recorded
/// `running`. A slot must not be held for an action that is then not begun.
#[test]
fn req_sched_006_a_request_made_as_the_slot_is_granted_returns_it_and_begins_nothing() {
    for (definition, _) in classes() {
        let name = definition.name.clone();
        let (rig, admission) = rig_with(definition, vec![Script::Admit]);
        {
            let runtime = rig.runtime.clone();
            let task = rig.task;
            admission.on_admit(move || runtime.start_no_new_action(task));
        }
        let result = rig.execute_the_action();
        assert_eq!(result.outcome, StepOutcome::Cancelled, "{name}");
        assert_closed_not_begun(&rig, &name);
        assert_eq!(admission.taken(), 1, "{name}: it was granted");
        assert_eq!(admission.held(), 0, "{name}: and returned");
        assert!(
            !rig.persistence.writes().contains(&"running"),
            "{name}: nothing was recorded running"
        );
    }
}

/// REQ-SCHED-006: a first request made before the slot is asked for (just before the action is
/// authorized) never reaches the admission: no slot is asked for, none taken, nothing recorded
/// `running`.
#[test]
fn req_sched_006_a_request_made_before_the_check_asks_for_no_slot() {
    for (definition, _) in classes() {
        let name = definition.name.clone();
        let (rig, admission) = rig_with(definition, vec![]);
        let fired = Arc::new(AtomicBool::new(false));
        {
            let runtime = rig.runtime.clone();
            let task = rig.task;
            rig.persistence.before_write(
                "authorized",
                Arc::new(move || {
                    if !fired.swap(true, Ordering::SeqCst) {
                        runtime.start_no_new_action(task);
                    }
                }),
            );
        }
        let result = rig.execute_the_action();
        assert_eq!(result.outcome, StepOutcome::Cancelled, "{name}");
        assert_closed_not_begun(&rig, &name);
        assert!(
            admission.requests().is_empty(),
            "{name}: no slot was asked for"
        );
        assert_eq!(admission.taken(), 0, "{name}");
    }
}

/// REQ-SCHED-006, A-33: a request made inside the record of the start (after the slot, before
/// the call) is seen by the second look; the slot is returned with the action closed not begun.
#[test]
fn req_sched_006_a_request_made_inside_the_record_of_the_start_returns_the_slot() {
    for (definition, _) in classes() {
        let name = definition.name.clone();
        let (rig, admission) = rig_with(definition, vec![]);
        let fired = Arc::new(AtomicBool::new(false));
        {
            let runtime = rig.runtime.clone();
            let task = rig.task;
            rig.persistence.before_write(
                "running",
                Arc::new(move || {
                    if !fired.swap(true, Ordering::SeqCst) {
                        runtime.start_no_new_action(task);
                    }
                }),
            );
        }
        let result = rig.execute_the_action();
        assert_eq!(result.outcome, StepOutcome::Cancelled, "{name}");
        assert_closed_not_begun(&rig, &name);
        assert_eq!(admission.taken(), 1, "{name}");
        assert_eq!(admission.held(), 0, "{name}: the slot went back");
    }
}

/// REQ-SCHED-006, RFC 013 decision 5: a registry that cannot be used lets the execution go on,
/// and says so: one `unadmitted` event with the reason for the task, however many executions it
/// has while the registry stays unusable; the action runs.
#[test]
fn req_sched_006_a_registry_that_cannot_be_used_is_recorded_once_and_the_execution_goes_on() {
    let (rig, admission) = rig_with(
        effect_tool("a_read", EffectKind::WorkspaceRead, true, true),
        vec![
            Script::Unadmitted("the registry is not writable (permission denied)".into()),
            Script::Unadmitted("the registry is not writable (permission denied)".into()),
        ],
    );
    let first = rig.execute_the_action();
    assert_eq!(first.outcome, StepOutcome::Continued);
    assert_eq!(rig.calls(), 1, "the action ran");
    let second = futures_block(rig.runtime.run_step(rig.task)).unwrap();
    assert_eq!(second.outcome, StepOutcome::Continued);
    assert_eq!(
        changes(&rig),
        [(
            AdmissionState::Unadmitted,
            None,
            Some("the registry is not writable (permission denied)".into()),
            None
        )],
        "said once while it lasts"
    );
    assert_eq!(admission.taken(), 0);
}

/// REQ-SCHED-006: the task's deadline keeps counting through the wait: the request names the
/// task and the time left, and a wait that the deadline ends closes the action not begun and
/// halts the task `budget_deadline`, as the next step boundary would have.
#[test]
fn req_sched_006_the_deadline_ends_the_wait_and_halts_the_task() {
    let (rig, admission) = rig_with(
        effect_tool("a_read", EffectKind::WorkspaceRead, true, true),
        vec![Script::WaitUntilDeadline],
    );
    let result = rig.execute_the_action();
    let request = admission.requests()[0];
    assert_eq!(request.task_id, rig.task);
    assert!(request.remaining_ms > 0, "the deadline had not passed yet");
    assert!(
        matches!(result.state, TaskState::Halted { .. }),
        "{:?}",
        result.state
    );
    assert_eq!(
        rig.last_state_change(),
        Some(("halted".into(), Some(HaltReason::BudgetDeadline)))
    );
    assert_eq!(rig.calls(), 0, "the action never began");
    assert_eq!(rig.action().status, ActionStatus::Cancelled);
    assert!(
        !rig.persistence.writes().contains(&"running"),
        "nothing was recorded running"
    );
    assert_eq!(admission.held(), 0);
}
