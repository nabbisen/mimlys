//! REQ-SEC-010 at every start of a task, not only at its creation (RFC 015, Amendment 1; handoff
//! 015a, item 3). The configuration is read when a process starts, so a task made under a local
//! backend can meet a remote one at its next start: it must halt `policy` before the backend is
//! asked for anything. The tests change the fake backend's declaration between two starts, as an
//! edit of the file does, and decide by what the backend received.

use super::*;
use crate::runtime::StartRefusal;

/// A halt `policy` for a remote backend under a profile that forbids it: it names the rule, the
/// backend and the profile (external design §5.2).
fn halted_policy() -> TaskState {
    TaskState::Halted {
        halt_reason: HaltReason::Policy,
        halt_detail: Some(crate::task::HaltDetail::RemoteBackendForbidden {
            backend: "default".into(),
            profile: "workspace".into(),
        }),
    }
}

/// The new state and the halt reason of the task's state changes, in order.
fn changes(h: &Harness, id: TaskId) -> Vec<(String, Option<HaltReason>)> {
    h.persistence
        .events()
        .into_iter()
        .filter(|e| e.task_id == Some(id))
        .filter_map(|e| match e.event {
            crate::event::Event::TaskStateChanged { to, reason, .. } => Some((to, reason)),
            _ => None,
        })
        .collect()
}

/// REQ-SEC-010: a task created under a local backend and resumed under a remote one, whose
/// profile has `external = false`, halts `policy` at its first step; the backend received no
/// request. The refusal names the profile; resuming it again, with the backend still remote,
/// halts it again; with the backend local again it runs.
#[test]
fn req_sec_010_resume_under_a_remote_backend_halts_policy_and_sends_nothing() {
    let h = Harness::new(vec![act("read_file", "a"), act("read_file", "b")]);
    let id = h.create_with(|r| r.hold = true);
    assert_eq!(h.state(id), TaskState::Created);
    // The configuration is edited: the backend is remote now.
    h.backend.set_remote(true);

    h.runtime.resume_task(&h.session, id, None).unwrap();
    let result = h.step(id);
    assert_eq!(result.outcome, StepOutcome::Halted);
    assert_eq!(result.state, halted_policy());
    assert_eq!(h.state(id), halted_policy());
    assert!(
        h.backend.requests().is_empty(),
        "nothing was sent to the remote backend"
    );
    let task = h.runtime.get_task(id).unwrap().task;
    assert_eq!(
        h.runtime.start_refusal(&task).unwrap(),
        Some(StartRefusal {
            backend: "default".into(),
            profile: "workspace".into()
        })
    );
    // The halt is a state change with the reason `policy`, recorded.
    assert_eq!(
        changes(&h, id).last(),
        Some(&("halted".to_owned(), Some(HaltReason::Policy)))
    );

    // Resumed again, still remote: halted again, still nothing sent.
    h.runtime.resume_task(&h.session, id, None).unwrap();
    assert_eq!(h.step(id).state, halted_policy());
    assert!(h.backend.requests().is_empty());

    // The backend is local again (or the profile allows it): the task runs.
    h.backend.set_remote(false);
    h.runtime.resume_task(&h.session, id, None).unwrap();
    assert_eq!(h.step(id).outcome, StepOutcome::Continued);
    assert_eq!(h.backend.requests().len(), 1);
    let task = h.runtime.get_task(id).unwrap().task;
    assert_eq!(h.runtime.start_refusal(&task).unwrap(), None);
}

/// REQ-SEC-010: a profile with `external = true` is not refused a remote backend, at a start
/// either; the rule is the profile's.
#[test]
fn req_sec_010_a_profile_with_external_starts_under_a_remote_backend() {
    let mut policy = Policy::shipped();
    policy.remote_backend_allowed = true;
    let mut open = policy.profiles["workspace"].clone();
    open.name = "open".into();
    open.external = true;
    policy.profiles.insert("open".into(), open);
    let h = Harness::with_backend(
        Arc::new(ScriptedBackend::with_decisions(vec![act("read_file", "a")])),
        policy,
        vec![],
        crate::limits::LIM_C1_CONTEXT_CHARS,
    );
    let id = h.create_with(|r| {
        r.profile = Some("open".into());
        r.hold = true;
    });
    h.backend.set_remote(true);
    h.runtime.resume_task(&h.session, id, None).unwrap();
    assert_eq!(h.step(id).outcome, StepOutcome::Continued);
    assert_eq!(h.backend.requests().len(), 1);
}

/// REQ-SEC-010: every way a task is set running again ends at the same step: a task that waited
/// for an approval and one that waited for an answer, set running after the backend became
/// remote, halt `policy` at their next step and send nothing more; a paused task too.
#[test]
fn req_sec_010_every_start_is_checked_not_only_resume() {
    // An approval: the first request was sent under the local backend.
    let h = Harness::with_policy(vec![act("write_file", "a")], approval_policy());
    let id = h.create();
    assert_eq!(h.step(id).outcome, StepOutcome::Waited);
    let before = h.backend.requests().len();
    assert_eq!(before, 1);
    h.backend.set_remote(true);
    h.runtime.approve_action(&h.session, id).unwrap();
    assert_eq!(h.step(id).state, halted_policy(), "approve");
    assert_eq!(h.backend.requests().len(), before, "approve: nothing sent");
    assert!(
        h.tools.calls().is_empty(),
        "the approved action did not run"
    );

    // A rejection.
    let h = Harness::with_policy(vec![act("write_file", "a")], approval_policy());
    let id = h.create();
    assert_eq!(h.step(id).outcome, StepOutcome::Waited);
    h.backend.set_remote(true);
    h.runtime
        .reject_action(&h.session, id, Some("no".into()), None)
        .unwrap();
    assert_eq!(h.step(id).state, halted_policy(), "reject");
    assert_eq!(h.backend.requests().len(), 1, "reject: nothing sent");

    // An answer.
    let h = Harness::new(vec![json!({"decision": "ask_user", "question": "Which?"})]);
    let id = h.create();
    assert_eq!(h.step(id).outcome, StepOutcome::Waited);
    h.backend.set_remote(true);
    h.runtime.answer_question(&h.session, id, "main").unwrap();
    assert_eq!(h.step(id).state, halted_policy(), "answer");
    assert_eq!(h.backend.requests().len(), 1, "answer: nothing sent");

    // A revocation that invalidates the pending approval sets the task running again.
    let h = Harness::with_policy(vec![act("write_file", "a")], approval_policy());
    let id = h.create();
    assert_eq!(h.step(id).outcome, StepOutcome::Waited);
    let write = h
        .runtime
        .get_task(id)
        .unwrap()
        .grants
        .into_iter()
        .find(
            |g| matches!(g.kind(), crate::grant::GrantKind::Tool { name } if name == "write_file"),
        )
        .unwrap();
    h.backend.set_remote(true);
    h.runtime
        .revoke_capability(&h.session, id, write.grant_id)
        .unwrap();
    assert_eq!(h.state(id), TaskState::Running);
    assert_eq!(h.step(id).state, halted_policy(), "revoke");
    assert_eq!(h.backend.requests().len(), 1, "revoke: nothing sent");

    // A pause.
    let h = Harness::new(vec![act("read_file", "a"), act("read_file", "b")]);
    let id = h.create();
    h.runtime.pause_task(&h.session, id).unwrap();
    assert_eq!(h.step(id).outcome, StepOutcome::Paused);
    h.backend.set_remote(true);
    h.runtime.resume_task(&h.session, id, None).unwrap();
    assert_eq!(h.step(id).state, halted_policy(), "pause then resume");
    assert!(h.backend.requests().is_empty(), "pause: nothing sent");
}

/// REQ-SEC-010: the first step of a task that never left `running` (a recovered task under
/// `recovery.auto_resume` is exactly this: the recovery sets it running, and the next command
/// that drives it runs its step) is checked: the task is `running` when the backend turns remote.
#[test]
fn req_sec_010_a_task_that_is_running_when_the_backend_turns_remote_halts_at_its_step() {
    let h = Harness::new(vec![act("read_file", "a")]);
    let id = h.create();
    assert_eq!(h.state(id), TaskState::Running);
    h.backend.set_remote(true);
    let result = h.step(id);
    assert_eq!(result.outcome, StepOutcome::Halted);
    assert_eq!(result.state, halted_policy());
    assert!(h.backend.requests().is_empty());
}

/// REQ-SEC-010: the creation is refused by the same rule, with the message it always had; and
/// `creation_forbidden` answers the question for the profile named (the policy's default for
/// none; false for a profile the policy does not have).
#[test]
fn req_sec_010_the_creation_and_the_start_ask_one_rule() {
    let mut policy = Policy::shipped();
    policy.remote_backend_allowed = true;
    let mut open = policy.profiles["workspace"].clone();
    open.name = "open".into();
    open.external = true;
    policy.profiles.insert("open".into(), open);
    let h = Harness::with_backend(
        Arc::new(ScriptedBackend::with_decisions(vec![]).remote()),
        policy,
        vec![],
        crate::limits::LIM_C1_CONTEXT_CHARS,
    );
    assert!(h.runtime.creation_forbidden(None).unwrap());
    assert!(h.runtime.creation_forbidden(Some("workspace")).unwrap());
    assert!(!h.runtime.creation_forbidden(Some("open")).unwrap());
    assert!(!h.runtime.creation_forbidden(Some("nope")).unwrap());
    let err = h
        .runtime
        .create_task(&h.session, create_request())
        .unwrap_err();
    assert!(
        matches!(&err, RuntimeError::Denied { detail } if detail.contains("profile workspace forbids external communication (external = false)")),
        "{err}"
    );
    h.backend.set_remote(false);
    assert!(!h.runtime.creation_forbidden(None).unwrap());
}
