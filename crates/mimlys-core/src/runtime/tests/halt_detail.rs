//! A halt `policy` names its rule (external design §5.2, REQ-SEC-010; handoff 015a-5, item 2):
//! `halt_detail` on the `halted` state and in the halt's `task.state_changed` event, for each of
//! the two causes the halt has.

use super::*;
use crate::event::Event;
use crate::task::HaltDetail;

/// The `halt_detail` of the last `task.state_changed` event of the task.
fn last_detail(h: &Harness, id: TaskId) -> Option<HaltDetail> {
    h.persistence
        .events()
        .into_iter()
        .filter(|e| e.task_id == Some(id))
        .filter_map(|e| match e.event {
            Event::TaskStateChanged { halt_detail, .. } => Some(halt_detail),
            _ => None,
        })
        .next_back()
        .expect("a state change")
}

/// REQ-TASK-007: a completion check that the validator refuses (a tool that is not offered) halts
/// the task `policy` with the rule `completion_check_refused`, in the task's state and in the
/// event; the check is never run and the model is asked once.
#[test]
fn req_task_007_a_refused_completion_check_halts_policy_naming_its_rule() {
    let h = Harness::new(vec![json!({"decision": "complete", "summary": "done"})]);
    let id = h.create_with(|r| {
        r.completion_check = Some(CompletionCheck {
            tool: "no_such_tool".into(),
            arguments: serde_json::Map::new(),
        })
    });
    let result = h.step(id);
    assert_eq!(result.outcome, StepOutcome::Halted);
    let expected = TaskState::Halted {
        halt_reason: HaltReason::Policy,
        halt_detail: Some(HaltDetail::CompletionCheckRefused),
    };
    assert_eq!(result.state, expected);
    assert_eq!(h.state(id), expected);
    assert_eq!(
        last_detail(&h, id),
        Some(HaltDetail::CompletionCheckRefused)
    );
    assert!(h.tools.calls().is_empty(), "the check never ran");
    // The same through the task as the control API shows it: a script reads the rule from there.
    let json = serde_json::to_value(h.runtime.get_task(id).unwrap().task).unwrap();
    assert_eq!(json["halt_reason"], "policy");
    assert_eq!(
        json["halt_detail"],
        json!({"rule": "completion_check_refused"})
    );
}

/// REQ-SEC-010: the halt of a start the profile forbids carries the rule `remote_backend_forbidden`
/// with the backend's name and the profile's, in the state and in the event.
#[test]
fn req_sec_010_a_forbidden_start_halts_policy_naming_backend_and_profile() {
    let h = Harness::new(vec![act("read_file", "a")]);
    let id = h.create_with(|r| r.hold = true);
    h.backend.set_remote(true);
    h.runtime.resume_task(&h.session, id, None).unwrap();
    assert_eq!(h.step(id).outcome, StepOutcome::Halted);
    let detail = HaltDetail::RemoteBackendForbidden {
        backend: "default".into(),
        profile: "workspace".into(),
    };
    assert_eq!(
        h.state(id),
        TaskState::Halted {
            halt_reason: HaltReason::Policy,
            halt_detail: Some(detail.clone()),
        }
    );
    assert_eq!(last_detail(&h, id), Some(detail));
    // A halt that is not `policy` has none.
    h.backend.set_remote(false);
    h.runtime.resume_task(&h.session, id, None).unwrap();
    h.clock.advance_millis(LIM_T5_DEADLINE_MS + 1);
    assert_eq!(h.step(id).outcome, StepOutcome::Halted);
    assert_eq!(
        h.state(id),
        TaskState::Halted {
            halt_reason: HaltReason::BudgetDeadline,
            halt_detail: None,
        }
    );
    assert_eq!(last_detail(&h, id), None);
}
