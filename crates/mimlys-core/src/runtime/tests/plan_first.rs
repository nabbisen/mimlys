//! Plan first (REQ-TASK-015; RFC 015 decision 6, Amendment 1; handoff 015b): a task created with
//! `--plan-first` rejects an `act` and a `complete` until the user approves its first plan, waits
//! for that plan as for an approval, and keeps the gate in its own record. Every verdict is a
//! state of the records or of a request, never a time.

use super::*;
use crate::event::{Actor, ApprovalDecision, Event};
use crate::ids::StepId;
use crate::rejection::{Detail, RejectionCode, StateReason};
use crate::step::{StepOutcome, StepPhase};
use crate::task::{PendingItem, Task};

fn plan(steps: &[&str]) -> Value {
    json!({"decision": "plan", "steps": steps})
}

fn first(steps: &[&str]) -> Value {
    plan(steps)
}

fn gated(decisions: Vec<Value>) -> (Harness, TaskId) {
    let h = Harness::new(decisions);
    h.tools
        .script("read_file", ExecutionOutcome::succeeded(json!({}), 1));
    let id = h.create_with(|r| {
        r.plan_first = true;
        r.profile = Some("read_only".into());
    });
    (h, id)
}

fn waiting_plan() -> TaskState {
    TaskState::WaitingUser {
        wait_kind: WaitKind::Plan,
    }
}

fn user(h: &Harness) -> Session {
    h.session.clone()
}

/// The contents of the messages of the `n`th request the model got.
fn request(h: &Harness, n: usize) -> Vec<String> {
    h.backend.requests()[n]
        .messages
        .iter()
        .map(|m| m.content.clone())
        .collect()
}

fn has(request: &[String], text: &str) -> bool {
    request.iter().any(|m| m.contains(text))
}

fn steps_of(h: &Harness, id: TaskId) -> Vec<crate::step::Step> {
    h.persistence.load_steps(id).unwrap()
}

/// REQ-TASK-015: a task without the flag has no `plan_first` key in its record and behaves as it
/// did: a first `act` runs, and a `plan` is recorded and the loop goes on.
#[test]
fn req_task_015_a_task_without_the_flag_has_no_key_and_no_gate() {
    let h = Harness::new(vec![act("read_file", "a"), plan(&["x"])]);
    h.tools
        .script("read_file", ExecutionOutcome::succeeded(json!({}), 1));
    let id = h.create_with(|r| r.profile = Some("read_only".into()));
    let record = serde_json::to_value(h.runtime.get_task(id).unwrap().task).unwrap();
    assert!(record.get("plan_first").is_none(), "{record}");
    assert!(record.get("plan_approved").is_none(), "{record}");
    assert_eq!(h.step(id).outcome, StepOutcome::Continued);
    assert_eq!(h.step(id).outcome, StepOutcome::Continued);
    assert_eq!(h.state(id), TaskState::Running);
    let created = h
        .persistence
        .events()
        .into_iter()
        .find(|e| e.event.name() == "task.created")
        .unwrap();
    // The event carries the level since 0.13 (REQ-TASK-013); the flag's own key is what it was.
    let mut event = serde_json::to_value(&created.event).unwrap();
    assert_eq!(event["effort"]["level"], "standard", "{event}");
    event.as_object_mut().unwrap().remove("effort");
    assert_eq!(
        event,
        json!({"event": "task.created"}),
        "the event of a task without the flag is what it was"
    );
}

/// REQ-TASK-015: the flag is in the record and in `task.created`.
#[test]
fn req_task_015_the_flag_is_in_the_record_and_in_the_creation_event() {
    let (h, id) = gated(vec![]);
    let task = h.runtime.get_task(id).unwrap().task;
    assert!(task.plan_first && task.plan_approved.is_none() && task.plan_required());
    let record = serde_json::to_value(&task).unwrap();
    assert_eq!(record["plan_first"], true);
    let created = h
        .persistence
        .events()
        .into_iter()
        .find(|e| e.event.name() == "task.created")
        .unwrap();
    let mut event = serde_json::to_value(&created.event).unwrap();
    event.as_object_mut().unwrap().remove("effort");
    assert_eq!(event, json!({"event": "task.created", "plan_first": true}));
}

/// REQ-TASK-015: an `act` first is rejected `state{plan_required}` with the fixed hint, nothing
/// is proposed, and three of them halt `budget_rejections` (LIM-T2).
#[test]
fn req_task_015_an_act_first_is_rejected_plan_required_and_three_halt() {
    let (h, id) = gated(vec![
        act("read_file", "a"),
        act("read_file", "b"),
        act("read_file", "c"),
    ]);
    for _ in 0..2 {
        assert_eq!(h.step(id).outcome, StepOutcome::Rejected);
    }
    let last = h.step(id);
    assert_eq!(
        last.state,
        TaskState::Halted {
            halt_reason: HaltReason::BudgetRejections,
            halt_detail: None
        }
    );
    let steps = steps_of(&h, id);
    assert!(steps.iter().all(|s| {
        s.rejection == Some(RejectionCode::State)
            && s.rejection_detail
                == Some(Detail::State {
                    reason: StateReason::PlanRequired,
                })
    }));
    assert!(h.persistence.load_actions(id).unwrap().is_empty());
    assert!(h.tools.calls().is_empty(), "nothing acted");
    // The model reads the reason and the hint; the event names the reason.
    let second = request(&h, 1);
    assert!(has(&second, "\"plan_required\""), "{second:?}");
    assert!(has(&second, "State your plan first (decide `plan`)."));
    let rejected = h
        .persistence
        .events()
        .into_iter()
        .filter_map(|e| match e.event {
            Event::DecisionRejected { code, reason, .. } => Some((code, reason)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        rejected[0],
        (RejectionCode::State, Some("plan_required".to_owned()))
    );
}

/// REQ-TASK-015: a `complete` first is rejected too, and the task is not completed.
#[test]
fn req_task_015_a_complete_first_is_rejected() {
    let (h, id) = gated(vec![json!({"decision": "complete", "summary": "done"})]);
    let result = h.step(id);
    assert_eq!(result.outcome, StepOutcome::Rejected);
    assert_eq!(result.state, TaskState::Running);
    assert!(h.persistence.load_actions(id).unwrap().is_empty());
}

/// REQ-TASK-015: `ask_user` before the plan acts on nothing: it is asked, answered, and the
/// gate is still closed.
#[test]
fn req_task_015_ask_user_first_is_asked_and_answered() {
    let (h, id) = gated(vec![
        json!({"decision": "ask_user", "question": "which file?"}),
        act("read_file", "a"),
    ]);
    assert_eq!(h.step(id).outcome, StepOutcome::Waited);
    assert_eq!(
        h.state(id),
        TaskState::WaitingUser {
            wait_kind: WaitKind::Question
        }
    );
    h.runtime.answer_question(&user(&h), id, "a.rs").unwrap();
    assert_eq!(h.state(id), TaskState::Running);
    // The answer does not open the gate.
    assert_eq!(h.step(id).outcome, StepOutcome::Rejected);
}

/// REQ-TASK-015: `give_up` before the plan fails the task.
#[test]
fn req_task_015_give_up_first_fails_the_task() {
    let (h, id) = gated(vec![json!({"decision": "give_up", "reason": "cannot"})]);
    let result = h.step(id);
    assert_eq!(result.outcome, StepOutcome::Failed);
    assert!(matches!(result.state, TaskState::Failed { .. }));
}

/// REQ-TASK-015: the first plan is recorded and the task waits for it, with nothing acted: the
/// state, the pending plan, the open step holding the wait, the events, one write.
#[test]
fn req_task_015_the_first_plan_waits_and_nothing_acted() {
    let (h, id) = gated(vec![first(&["read a", "edit a"])]);
    let writes_before = h.persistence.writes().len();
    let result = h.step(id);
    assert_eq!(result.outcome, StepOutcome::Waited);
    assert_eq!(result.state, waiting_plan());
    let view = h.runtime.get_task(id).unwrap();
    assert_eq!(view.task.pending, Some(PendingItem::Plan { version: 1 }));
    let plan = view.plan.expect("the plan is shown");
    assert_eq!((plan.version, plan.steps.len()), (1, 2));
    assert!(h.persistence.load_actions(id).unwrap().is_empty());
    assert!(h.tools.calls().is_empty());
    let writes = h.persistence.writes();
    assert_eq!(
        &writes[writes_before..],
        ["step_started", "plan_step"],
        "the plan, the step, the task and the events are one write"
    );
    let steps = steps_of(&h, id);
    let step = steps.last().unwrap();
    assert_eq!(step.phase, StepPhase::Wait);
    assert!(step.outcome.is_none(), "the step stays open while it waits");
    assert!(step.plan_wait.as_ref().unwrap().is_pending());
    let names = h.event_names(id);
    let tail: Vec<&str> = names.iter().rev().take(3).map(String::as_str).collect();
    assert_eq!(
        tail,
        [
            "task.state_changed",
            "plan.approval_requested",
            "decision.received"
        ]
    );
    // Nothing runs while it waits.
    assert!(
        futures_block(h.runtime.run_step(id)).is_err(),
        "a waiting task takes no step"
    );
}

/// REQ-TASK-015: `approve` ends the gate and records the decision; the task acts at its next
/// step, and the model's next request holds the approved plan.
#[test]
fn req_task_015_approve_ends_the_gate_and_an_act_runs() {
    let (h, id) = gated(vec![first(&["read a"]), act("read_file", "a")]);
    h.step(id);
    let after = h.runtime.approve_action(&user(&h), id, None).unwrap();
    assert_eq!(after.state, TaskState::Running);
    assert_eq!(after.plan_approved, Some(1));
    assert!(after.pending.is_none());
    assert!(!after.plan_required());
    let steps = steps_of(&h, id);
    let plan_step = &steps[0];
    assert_eq!(plan_step.outcome, Some(StepOutcome::Continued));
    let decided = plan_step
        .plan_wait
        .as_ref()
        .unwrap()
        .decided
        .clone()
        .unwrap();
    assert_eq!(decided.decision, ApprovalDecision::Approved);
    assert!(matches!(decided.by, Actor::User { .. }));
    assert!(
        h.event_names(id)
            .contains(&"plan.approval_decided".to_owned())
    );
    assert_eq!(h.step(id).outcome, StepOutcome::Continued);
    assert_eq!(h.persistence.load_actions(id).unwrap().len(), 1);
    assert_eq!(h.tools.calls().len(), 1);
    let second = request(&h, 1);
    assert!(
        has(&second, "<<plan version=\"1\">>"),
        "the approved plan is in force: {second:?}"
    );
}

/// REQ-TASK-015: `approve --note` puts the note in the next request as `approval_note`, beside
/// the plan, as the user's own words.
#[test]
fn req_task_015_an_approval_note_reaches_the_model_beside_the_plan() {
    let (h, id) = gated(vec![first(&["read a"]), act("read_file", "a")]);
    h.step(id);
    h.runtime
        .approve_action(&user(&h), id, Some("do not touch b.rs".into()))
        .unwrap();
    h.step(id);
    let second = request(&h, 1);
    let at = second
        .iter()
        .position(|m| m.contains("<<user kind=\"approval_note\">>"))
        .expect("the note is in the request");
    assert!(second[at].contains("\"do not touch b.rs\""));
    assert!(
        second[at - 1].contains("<<plan version=\"1\">>"),
        "beside the plan: {second:?}"
    );
    assert!(
        !has(&request(&h, 0), "approval_note"),
        "nothing before the approval"
    );
}

/// REQ-TASK-015: a note above LIM-D2 is refused as a rejection's is, and the task still waits.
#[test]
fn req_task_015_a_note_over_the_bound_is_refused_and_the_task_still_waits() {
    let (h, id) = gated(vec![first(&["read a"])]);
    h.step(id);
    let long = "x".repeat(crate::limits::LIM_D2_REASON_CHARS + 1);
    assert!(
        h.runtime
            .approve_action(&user(&h), id, Some(long.clone()))
            .is_err()
    );
    assert!(
        h.runtime
            .reject_action(&user(&h), id, Some(long), None)
            .is_err()
    );
    assert_eq!(h.state(id), waiting_plan());
    assert!(steps_of(&h, id)[0].plan_wait.as_ref().unwrap().is_pending());
}

/// REQ-TASK-015: only the first plan is gated: a second plan after the approval is recorded and
/// the task goes on.
#[test]
fn req_task_015_a_second_plan_after_approval_does_not_wait() {
    let (h, id) = gated(vec![first(&["read a"]), plan(&["read a", "read b"])]);
    h.step(id);
    h.runtime.approve_action(&user(&h), id, None).unwrap();
    let result = h.step(id);
    assert_eq!(result.outcome, StepOutcome::Continued);
    assert_eq!(result.state, TaskState::Running);
    assert_eq!(h.runtime.get_task(id).unwrap().plan.unwrap().version, 2);
    assert_eq!(
        h.runtime.get_task(id).unwrap().task.plan_approved,
        Some(1),
        "the approved version is the first"
    );
}

/// REQ-TASK-015: `reject --note` ends the plan's step `approval_rejected`, the model's next
/// request holds the plan it made, the rejection and the note as `rejection_note`, an `act` is
/// still `plan_required`, and the next plan waits again. No plan is in force meanwhile.
#[test]
fn req_task_015_reject_with_a_note_keeps_the_gate() {
    let (h, id) = gated(vec![
        first(&["delete everything"]),
        act("read_file", "a"),
        first(&["read a"]),
    ]);
    h.step(id);
    let after = h
        .runtime
        .reject_action(&user(&h), id, Some("too much".into()), Some(false))
        .unwrap();
    assert_eq!(after.state, TaskState::Running);
    assert!(after.plan_required(), "rejecting does not end the gate");
    let steps = steps_of(&h, id);
    assert_eq!(steps[0].outcome, Some(StepOutcome::Rejected));
    assert_eq!(steps[0].rejection, Some(RejectionCode::ApprovalRejected));
    assert_eq!(
        steps[0]
            .plan_wait
            .as_ref()
            .unwrap()
            .decided
            .as_ref()
            .unwrap()
            .decision,
        ApprovalDecision::Rejected
    );
    // The model is asked again.
    assert_eq!(h.step(id).outcome, StepOutcome::Rejected, "still gated");
    let second = request(&h, 1);
    assert!(
        has(&second, "delete everything"),
        "the rejected plan: {second:?}"
    );
    assert!(has(&second, "\"approval_rejected\""));
    assert!(has(
        &second,
        "<<user kind=\"rejection_note\">>\n\"too much\""
    ));
    assert!(
        !has(&second, "<<plan version"),
        "a plan the user rejected is not the plan in force: {second:?}"
    );
    // The next plan waits again.
    let again = h.step(id);
    assert_eq!(again.state, waiting_plan());
    assert_eq!(
        h.runtime.get_task(id).unwrap().task.pending,
        Some(PendingItem::Plan { version: 2 })
    );
    assert!(h.persistence.load_actions(id).unwrap().is_empty());
}

/// REQ-TASK-015: the same in the `tool_calls` transport: the rejected plan is the model's own
/// call, answered by the rejection, and the note follows as the user's words.
#[test]
fn req_task_015_a_rejected_plan_in_the_tool_calls_transport() {
    let backend =
        ScriptedBackend::with_decisions(vec![first(&["delete everything"]), act("read_file", "a")])
            .tool_calls();
    let h = Harness::with_backend(
        Arc::new(backend),
        Policy::shipped(),
        vec![],
        crate::limits::LIM_C1_CONTEXT_CHARS,
    );
    let id = h.create_with(|r| {
        r.plan_first = true;
        r.profile = Some("read_only".into());
    });
    h.step(id);
    h.runtime
        .reject_action(&user(&h), id, Some("too much".into()), Some(false))
        .unwrap();
    h.step(id);
    let messages = &h.backend.requests()[1].messages;
    let call = messages
        .iter()
        .position(|m| m.call.as_ref().is_some_and(|c| c.name == "plan"))
        .expect("the plan is the model's call");
    assert!(messages[call + 1].content.contains("approval_rejected"));
    assert!(
        messages[call + 2]
            .content
            .contains("<<user kind=\"rejection_note\">>")
    );
}

/// REQ-TASK-015: `reject --halt` halts `approval_rejected`; the plan's step ends halted.
#[test]
fn req_task_015_reject_halt_halts_approval_rejected() {
    let (h, id) = gated(vec![first(&["read a"])]);
    h.step(id);
    let after = h
        .runtime
        .reject_action(&user(&h), id, None, Some(true))
        .unwrap();
    assert_eq!(
        after.state,
        TaskState::Halted {
            halt_reason: HaltReason::ApprovalRejected,
            halt_detail: None
        }
    );
    let steps = steps_of(&h, id);
    assert_eq!(steps[0].outcome, Some(StepOutcome::Halted));
    assert!(after.plan_required(), "a halted task is still gated");
}

/// REQ-TASK-015: the profile's `on_reject` applies to a plan as to an action.
#[test]
fn req_task_015_the_profiles_on_reject_halts_a_rejected_plan() {
    let mut policy = Policy::shipped();
    policy.profiles.get_mut("read_only").unwrap().on_reject = OnReject::Halt;
    let h = Harness::with_policy(vec![first(&["read a"])], policy);
    let id = h.create_with(|r| {
        r.plan_first = true;
        r.profile = Some("read_only".into());
    });
    h.step(id);
    let after = h.runtime.reject_action(&user(&h), id, None, None).unwrap();
    assert!(matches!(
        after.state,
        TaskState::Halted {
            halt_reason: HaltReason::ApprovalRejected,
            ..
        }
    ));
}

/// REQ-TASK-015: `task cancel` from the plan wait cancels the task and ends the step.
#[test]
fn req_task_015_cancel_from_the_plan_wait() {
    let (h, id) = gated(vec![first(&["read a"])]);
    h.step(id);
    let after = h.runtime.cancel_task(&user(&h), id).unwrap();
    assert_eq!(after.state, TaskState::Cancelled);
    assert!(after.pending.is_none());
    let steps = steps_of(&h, id);
    assert_eq!(steps[0].outcome, Some(StepOutcome::Cancelled));
    assert!(
        steps[0].plan_wait.as_ref().unwrap().is_pending(),
        "no decision was made"
    );
}

/// REQ-TASK-015: a rejected plan counts toward LIM-T2 as a rejected action does: three
/// rejections in a row halt `budget_rejections`.
#[test]
fn req_task_015_rejected_plans_count_toward_the_rejection_budget() {
    let (h, id) = gated(vec![first(&["a"]), first(&["b"]), first(&["c"])]);
    for n in 0..3 {
        h.step(id);
        let after = h
            .runtime
            .reject_action(&user(&h), id, None, Some(false))
            .unwrap();
        if n < 2 {
            assert_eq!(after.state, TaskState::Running);
        } else {
            assert_eq!(
                after.state,
                TaskState::Halted {
                    halt_reason: HaltReason::BudgetRejections,
                    halt_detail: None
                }
            );
        }
    }
}

/// REQ-TASK-015: the gate is in the task's own record: a task read back from its JSON is gated
/// as it was, whatever the configuration, and the approved version is kept the same way.
#[test]
fn req_task_015_the_gate_is_read_from_the_record() {
    let (h, id) = gated(vec![first(&["read a"])]);
    h.step(id);
    let record = serde_json::to_string(&h.runtime.get_task(id).unwrap().task).unwrap();
    let back: Task = serde_json::from_str(&record).unwrap();
    assert!(back.plan_first && back.plan_required());
    assert_eq!(back.pending, Some(PendingItem::Plan { version: 1 }));
    let approved = h.runtime.approve_action(&user(&h), id, None).unwrap();
    let back: Task = serde_json::from_str(&serde_json::to_string(&approved).unwrap()).unwrap();
    assert_eq!(back.plan_approved, Some(1));
    assert!(!back.plan_required());
}

/// REQ-TASK-015: a fork is plan-first only when it is created with the flag.
#[test]
fn req_task_015_a_fork_is_gated_only_when_created_with_the_flag() {
    let (h, id) = gated(vec![first(&["read a"])]);
    h.step(id);
    h.runtime.cancel_task(&user(&h), id).unwrap();
    let fork = |plan_first: bool, root: &str| {
        h.runtime
            .create_task(
                &h.session,
                CreateTask {
                    from_task: Some(id),
                    hold: true,
                    plan_first,
                    profile: Some("read_only".into()),
                    scope: ScopeSpec {
                        root: root.into(),
                        include: vec![],
                    },
                    ..create_request()
                },
            )
            .unwrap()
    };
    assert!(
        !fork(false, "/home/u/one").plan_first,
        "a fork does not inherit the gate"
    );
    assert!(fork(true, "/home/u/two").plan_first);
}

/// REQ-TASK-015: a plan wait answers `answer`, `resume` and `pause` with a refusal, the task
/// still waiting; `grant` is allowed (it acts on no plan); the verbs are listed from the control
/// API, one result each.
#[test]
fn req_task_015_the_verbs_that_meet_a_plan_wait() {
    let (h, id) = gated(vec![first(&["read a"])]);
    h.step(id);
    let s = user(&h);
    let ids = crate::ids::ScriptedIds::starting_at(900);
    let results: Vec<(&str, bool)> = vec![
        (
            "answer_question",
            h.runtime.answer_question(&s, id, "x").is_ok(),
        ),
        ("resume_task", h.runtime.resume_task(&s, id, None).is_ok()),
        ("pause_task", h.runtime.pause_task(&s, id).is_ok()),
        (
            "grant_capability",
            h.runtime
                .grant_capability(
                    &s,
                    id,
                    crate::grant::GrantKind::Tool {
                        name: "read_file".into(),
                    },
                )
                .is_ok(),
        ),
        (
            "revoke_capability",
            h.runtime
                .revoke_capability(&s, id, crate::ids::GrantId::new(&ids))
                .is_ok(),
        ),
        ("delete_task", h.runtime.delete_task(&s, id, false).is_ok()),
    ];
    assert_eq!(
        results,
        [
            ("answer_question", false),
            ("resume_task", false),
            ("pause_task", false),
            ("grant_capability", true),
            ("revoke_capability", false),
            ("delete_task", false),
        ]
    );
    assert_eq!(h.state(id), waiting_plan(), "none of them ended the wait");
    let _ = StepId::new(&ids);
}

/// REQ-TASK-015: `approve --note` on an action's approval carries the note as `approval_note`
/// with the action's observation, as the user's own words; a rejection's note stays
/// `rejection_note`.
#[test]
fn req_task_015_an_actions_approval_note_reaches_the_model() {
    let mut policy = Policy::shipped();
    policy
        .profiles
        .get_mut("read_only")
        .unwrap()
        .approval_tools
        .push("read_file".into());
    let h = Harness::with_policy(vec![act("read_file", "a"), act("read_file", "b")], policy);
    h.tools.script(
        "read_file",
        ExecutionOutcome::succeeded(json!({"content": "x"}), 1),
    );
    let id = h.create_with(|r| r.profile = Some("read_only".into()));
    assert_eq!(h.step(id).outcome, StepOutcome::Waited);
    h.runtime
        .approve_action(&user(&h), id, Some("only this once".into()))
        .unwrap();
    h.step(id);
    // The second request is made after the first action's observation.
    assert_eq!(h.step(id).outcome, StepOutcome::Waited);
    let second = request(&h, 1);
    let at = second
        .iter()
        .position(|m| m.contains("<<user kind=\"approval_note\">>"))
        .expect("the note is in the request");
    assert!(second[at].contains("\"only this once\""));
    assert!(
        second[at - 1].starts_with("<<observation"),
        "after the observation of the action it was given with: {second:?}"
    );
    assert!(!has(&second, "rejection_note"));
}

// ---- What the model is told: the first request of a plan-first task (handoff 015b-3) ----

use crate::runtime::context::{
    PLAN_FIRST_INSTRUCTION, SYSTEM_INSTRUCTIONS, SYSTEM_INSTRUCTIONS_TOOL_CALLS,
};

#[derive(Clone, Copy, Debug)]
enum Wire {
    Envelope,
    ToolCalls,
}

impl Wire {
    fn base(self) -> &'static str {
        match self {
            Wire::Envelope => SYSTEM_INSTRUCTIONS,
            Wire::ToolCalls => SYSTEM_INSTRUCTIONS_TOOL_CALLS,
        }
    }
}

/// A task over `wire` with the decisions given, plan-first or not.
fn over(wire: Wire, plan_first: bool, decisions: Vec<Value>) -> (Harness, TaskId) {
    let backend = ScriptedBackend::with_decisions(decisions);
    let backend = match wire {
        Wire::Envelope => backend,
        Wire::ToolCalls => backend.tool_calls(),
    };
    let h = Harness::with_backend(
        Arc::new(backend),
        Policy::shipped(),
        vec![],
        crate::limits::LIM_C1_CONTEXT_CHARS,
    );
    h.tools
        .script("read_file", ExecutionOutcome::succeeded(json!({}), 1));
    let id = h.create_with(|r| {
        r.plan_first = plan_first;
        r.profile = Some("read_only".into());
    });
    (h, id)
}

fn system_of(h: &Harness, n: usize) -> String {
    let first = &h.backend.requests()[n].messages[0];
    assert_eq!(first.role, crate::spi::Role::System);
    first.content.clone()
}

/// A plan-first task's run: the plan, its approval, an act, a completion.
fn plan_first_run(wire: Wire) -> Harness {
    let (h, id) = over(
        wire,
        true,
        vec![
            first(&["read a"]),
            act("read_file", "a"),
            json!({"decision": "complete", "summary": "done"}),
        ],
    );
    h.step(id);
    h.runtime.approve_action(&user(&h), id, None).unwrap();
    h.step(id);
    h.step(id);
    assert_eq!(h.backend.requests().len(), 3);
    h
}

/// REQ-TASK-015, REQ-CTX-002: the first request of a plan-first task says, in the fixed
/// instructions and in both transports, that the model must plan before it acts; and every later
/// request, the approval's included, says it in the same bytes.
#[test]
fn req_task_015_the_first_request_of_a_plan_first_task_says_it_must_plan_before_it_acts() {
    for wire in [Wire::Envelope, Wire::ToolCalls] {
        let h = plan_first_run(wire);
        for n in 0..3 {
            let system = system_of(&h, n);
            assert!(
                system.starts_with(&format!("{} {PLAN_FIRST_INSTRUCTION}\n", wire.base())),
                "{wire:?}, request {n}: the sentence ends the fixed instructions: {system}"
            );
            assert_eq!(
                system.matches(PLAN_FIRST_INSTRUCTION).count(),
                1,
                "{wire:?}, request {n}"
            );
        }
    }
}

/// REQ-TASK-015 (handoff 015b-4): the words are what external design §15.1 gives, and they say
/// nothing untrue: the model may ask first, so the sentence does not claim a first decision.
#[test]
fn req_task_015_the_sentence_has_the_words_of_the_design() {
    assert_eq!(
        PLAN_FIRST_INSTRUCTION,
        "This task is plan first: decide `plan` before you act or complete; the user approves \
         the plan before you act. If you cannot plan without an answer, you may ask the user first."
    );
}

/// REQ-CTX-002: no request of another task has the sentence, and its fixed instructions are
/// byte for byte the plan-first task's without it: nothing else of the first segment moved.
#[test]
fn req_task_015_no_request_of_another_task_has_the_sentence() {
    for wire in [Wire::Envelope, Wire::ToolCalls] {
        let (h, id) = over(
            wire,
            false,
            vec![
                first(&["read a"]),
                act("read_file", "a"),
                json!({"decision": "complete", "summary": "done"}),
            ],
        );
        h.step(id);
        h.step(id);
        h.step(id);
        let requests = h.backend.requests();
        assert_eq!(requests.len(), 3);
        for (n, request) in requests.iter().enumerate() {
            for message in &request.messages {
                assert!(
                    !message.content.contains("plan first"),
                    "{wire:?}, request {n}: {}",
                    message.content
                );
            }
        }
        assert!(
            system_of(&h, 0).starts_with(&format!("{}\n", wire.base())),
            "{wire:?}"
        );
        let gated = plan_first_run(wire);
        assert_eq!(
            system_of(&gated, 0).replacen(&format!(" {PLAN_FIRST_INSTRUCTION}"), "", 1),
            system_of(&h, 0),
            "{wire:?}: the instructions differ by the sentence alone"
        );
    }
}

/// REQ-CTX-010: the first segment and the goal are byte-stable from step to step of a plan-first
/// task, the end of the gate included; what the approval adds follows them. The prefix measured
/// at the approval is the whole request before it.
#[test]
fn req_task_015_the_prefix_is_byte_stable_through_the_gate_and_its_end() {
    for wire in [Wire::Envelope, Wire::ToolCalls] {
        let (h, id) = over(
            wire,
            true,
            vec![
                first(&["read a"]),
                act("read_file", "a"),
                json!({"decision": "complete", "summary": "done"}),
            ],
        );
        h.step(id);
        h.runtime
            .approve_action(&user(&h), id, Some("only a".into()))
            .unwrap();
        h.step(id);
        h.step(id);
        let requests = h.backend.requests();
        for (n, request) in requests.iter().enumerate() {
            assert_eq!(
                request.messages[0], requests[0].messages[0],
                "{wire:?}, request {n}: the instructions"
            );
            assert_eq!(
                request.messages[1], requests[0].messages[1],
                "{wire:?}, request {n}: the goal"
            );
        }
        // The first request is the instructions, the goal and the runtime's own request for a
        // decision. The approval's request keeps the first two and puts the plan (and the note)
        // after them, so the prefix at the approval is those two, plus the bytes the old tail
        // and the plan have in common: never less than the two, and the sentence is in them.
        assert_eq!(
            requests[1].messages[..2],
            requests[0].messages[..2],
            "{wire:?}"
        );
        let steps = steps_of(&h, id);
        let measured = |n: usize| steps[n].measures.as_ref().unwrap().context.prefix_chars as usize;
        let view =
            |n: usize| crate::context::prefix_view(&requests[n].messages, &requests[n].functions);
        let head = &requests[0].messages[..2];
        let head_chars = crate::context::common_prefix_chars(head, head);
        assert_eq!(measured(0), 0, "{wire:?}: nothing before the first request");
        assert_eq!(
            measured(1),
            crate::context::common_prefix_chars(&view(0), &view(1)),
            "{wire:?}"
        );
        assert!(
            measured(1) >= head_chars,
            "{wire:?}: at the approval the instructions and the goal are reused ({} < {head_chars})",
            measured(1)
        );
        assert_eq!(
            measured(2),
            crate::context::common_prefix_chars(&view(1), &view(2)),
            "{wire:?}"
        );
        assert!(
            measured(2) > measured(1),
            "{wire:?}: after the approval the plan is in the prefix too"
        );
    }
}

/// REQ-CTX-010: when a plan is rejected before one is approved, the request after the approval
/// has the plan segment where the earlier ones had the rejected plan's step: the prefix is then
/// the instructions and the goal, as for any plan that changes, and never less than those.
#[test]
fn req_task_015_the_prefix_at_an_approval_after_a_rejection_is_the_instructions_and_the_goal() {
    for wire in [Wire::Envelope, Wire::ToolCalls] {
        let (h, id) = over(
            wire,
            true,
            vec![
                first(&["delete everything"]),
                first(&["read a"]),
                act("read_file", "a"),
            ],
        );
        h.step(id);
        h.runtime
            .reject_action(&user(&h), id, Some("too much".into()), Some(false))
            .unwrap();
        h.step(id);
        h.runtime.approve_action(&user(&h), id, None).unwrap();
        h.step(id);
        let requests = h.backend.requests();
        assert_eq!(requests.len(), 3);
        let view =
            |n: usize| crate::context::prefix_view(&requests[n].messages, &requests[n].functions);
        let head = &requests[0].messages[..2];
        let head_chars = crate::context::common_prefix_chars(head, head);
        let steps = steps_of(&h, id);
        let at_approval = steps[2].measures.as_ref().unwrap().context.prefix_chars as usize;
        assert_eq!(
            at_approval,
            crate::context::common_prefix_chars(&view(1), &view(2))
        );
        assert!(
            at_approval >= head_chars,
            "{wire:?}: the instructions and the goal are always reused ({at_approval} < {head_chars})"
        );
        assert!(
            requests[2].messages[0] == requests[0].messages[0],
            "{wire:?}"
        );
    }
}
