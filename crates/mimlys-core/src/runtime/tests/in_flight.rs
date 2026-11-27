//! Requests on a task whose step is in flight (handoff 014b, item 5; external design §4;
//! REQ-DEC-001, REQ-TASK-005, REQ-TASK-006, REQ-CAP-004, REQ-CLI-005): **no write is made to a
//! task while its step holds it, except by the step.** A pause and a stop are noted and taken in
//! the step's own turn, at its end; a grant waits for the step to end and applies from a fresh
//! read; the others need a state that has no step and are refused as the state they would be
//! refused in. Judged by state: what the store holds when the step was held, and after it ended.
//! A request is held in flight by a backend that waits for the test, and the calls that block run
//! on threads of their own, as a second client's would.

use super::long_lived::{Release, Rig, block_on_waiting, counts, rig, state_changes, until};
use super::*;
use crate::api::{Envelope, Notice, Reply, Request};
use crate::grant::GrantSet;
use crate::ids::RequestId;
use crate::runtime::steps::{Held, Note, Noted, OnStep, Steps};
use std::sync::atomic::{AtomicBool, Ordering};

fn send(h: &Harness, request: Request) -> Result<Reply, RuntimeError> {
    crate::api::testing::send(&h.runtime, &h.session, request)
}

/// A rig whose model proposes `read_file` twice, the first request held.
fn reading() -> Rig {
    let r = rig(vec![act("read_file", "a"), act("read_file", "b")], false);
    for _ in 0..2 {
        r.h.tools
            .script("read_file", ExecutionOutcome::succeeded(json!({}), 1));
    }
    r
}

fn noted(reply: Result<Reply, RuntimeError>) -> crate::task::Task {
    match reply {
        Ok(Reply::Noted(task)) => *task,
        other => panic!("not noted: {other:?}"),
    }
}

/// REQ-TASK-005 (external design §4): a pause requested while the step is in flight is noted at
/// once, with the reply that says so, and nothing is written to the task; the step ends and
/// writes its own end; the pause is requested of the task as the step left it, and taken at the
/// next boundary. The step's end and the pause stand together: neither wrote over the other.
#[test]
fn req_task_005_a_pause_during_a_step_is_noted_at_once_and_taken_at_the_boundary() {
    let r = reading();
    let id = r.h.create();
    std::thread::scope(|scope| {
        let _release = Release(&r.front);
        let stepping = scope.spawn(|| block_on_waiting(r.h.runtime.run_step(id)));
        until("the model request is held", || {
            r.front.entered.load(Ordering::SeqCst)
        });
        let before = counts(&r.h, id);
        let task = noted(send(&r.h, Request::PauseTask { task_id: id }));
        assert_eq!(task.state, TaskState::Running);
        assert!(!task.pause_requested, "noted, not written");
        assert_eq!(counts(&r.h, id), before, "nothing was written meanwhile");
        assert!(!r.h.runtime.get_task(id).unwrap().task.pause_requested);
        r.front.released.store(true, Ordering::SeqCst);
        let step = stepping.join().unwrap().unwrap();
        assert_eq!(step.outcome, StepOutcome::Continued);
    });
    let task = r.h.runtime.get_task(id).unwrap().task;
    assert!(
        task.pause_requested,
        "the pause was requested at the step's end"
    );
    assert_eq!(task.state, TaskState::Running);
    assert_eq!(task.counters.steps, 1, "the step's own end stands");
    let steps = r.h.persistence.load_steps(id).unwrap();
    assert_eq!(steps[0].outcome, Some(StepOutcome::Continued));
    assert_eq!(
        r.h.event_names(id)
            .iter()
            .filter(|n| *n == "task.pause_requested")
            .count(),
        1
    );
    assert_eq!(r.h.runtime.steps.held(), 0, "no claim is left");
    // The boundary: the next step takes the pause, and runs nothing.
    let next = r.h.step(id);
    assert_eq!(next.outcome, StepOutcome::Paused);
    assert_eq!(r.h.state(id), TaskState::Paused);
    assert_eq!(r.h.persistence.load_steps(id).unwrap().len(), 1);
}

/// With no step in flight a pause is requested at once, and the reply is the task.
#[test]
fn req_task_005_a_pause_with_no_step_in_flight_is_requested_at_once() {
    let r = reading();
    r.front.hold_next.store(false, Ordering::SeqCst);
    let id = r.h.create();
    let Ok(Reply::Task(task)) = send(&r.h, Request::PauseTask { task_id: id }) else {
        panic!("not the task")
    };
    assert!(task.pause_requested);
    assert_eq!(r.h.step(id).outcome, StepOutcome::Paused);
}

/// REQ-CAP-004 (external design §4): a grant requested while the step is in flight waits for the
/// step to end, says that it waits, and then applies from a fresh read of the task: the step
/// left an approval pending that did not exist when the grant came, and the grant sees it and
/// asks again under the new set of grants (INV-06). Nothing of the grant is written before.
#[test]
fn req_cap_004_a_grant_during_a_step_waits_for_it_and_applies_from_a_fresh_read() {
    let scripted = Arc::new(ScriptedBackend::with_decisions(vec![act(
        "write_file",
        "src/lib.rs",
    )]));
    let front = super::long_lived::Holding::new(scripted.clone(), false);
    let h = Harness::with_front(
        front.clone(),
        scripted,
        approval_policy(),
        vec![],
        crate::limits::LIM_C1_CONTEXT_CHARS,
        vec![],
    );
    let id = h.create();
    let grants_before = h.persistence.load_grants(id).unwrap().len();
    let noticed = AtomicBool::new(false);
    std::thread::scope(|scope| {
        let _release = Release(&front);
        let stepping = scope.spawn(|| block_on_waiting(h.runtime.run_step(id)));
        until("the model request is held", || {
            front.entered.load(Ordering::SeqCst)
        });
        let granting = scope.spawn(|| {
            h.runtime.handle_noticing(
                Envelope {
                    request_id: RequestId::new(&ScriptedIds::starting_at(9_000)),
                    session: h.session.clone(),
                    request: Request::GrantCapability {
                        task_id: id,
                        kind: GrantKind::Tool {
                            name: "list_dir".into(),
                        },
                    },
                },
                &|notice| {
                    assert_eq!(notice, Notice::WaitingForStep { task_id: id });
                    noticed.store(true, Ordering::SeqCst);
                },
            )
        });
        until("the grant says that it waits", || {
            noticed.load(Ordering::SeqCst)
        });
        assert_eq!(
            h.persistence.load_grants(id).unwrap().len(),
            grants_before,
            "no grant is written while the step holds the task"
        );
        assert!(!h.event_names(id).contains(&"grant.issued".to_owned()));
        front.released.store(true, Ordering::SeqCst);
        let step = stepping.join().unwrap().unwrap();
        assert_eq!(
            step.state,
            TaskState::WaitingUser {
                wait_kind: WaitKind::Approval
            }
        );
        let response = granting.join().unwrap();
        assert!(
            matches!(response.into_result(), Ok(Reply::Grant(_))),
            "the grant was applied"
        );
    });
    // After the step, in the step's own order: its end, then the grant, and the approval asked
    // again under the new set.
    let names = h.event_names(id);
    let at = |name: &str| names.iter().position(|n| n == name).unwrap();
    let last = |name: &str| names.iter().rposition(|n| n == name).unwrap();
    // The step's own write (the approval it asked) comes first, the grant after it, and the
    // approval is withdrawn and asked again after the grant.
    assert!(at("approval.requested") < at("grant.issued"), "{names:?}");
    assert!(at("grant.issued") < at("approval.invalidated"), "{names:?}");
    assert!(
        last("approval.requested") > at("approval.invalidated"),
        "{names:?}"
    );
    let view = h.runtime.get_task(id).unwrap();
    let mut set = GrantSet::new();
    for g in h.persistence.load_grants(id).unwrap() {
        set.add(g);
    }
    assert_eq!(
        view.pending_approval.unwrap().grant_set_hash,
        set.hash(h.runtime.now()),
        "the pending approval is bound to the grants in force now"
    );
    assert_eq!(
        h.persistence.load_grants(id).unwrap().len(),
        grants_before + 1
    );
    assert_eq!(h.runtime.steps.held(), 0);
}

/// External design §4: the requests that need a state with no step in flight are refused while
/// one is, as the state they would be refused in (`running`), and write nothing. An approval, a
/// rejection and an answer are never deferred: they would reach a request the user has not seen.
#[test]
fn req_dec_001_a_request_that_needs_no_step_is_refused_while_one_is_in_flight_and_writes_nothing() {
    let r = reading();
    let id = r.h.create();
    let snapshot = r.h.persistence.load_snapshots(id).unwrap()[0].snapshot_id;
    let plan = one_file_plan(&r.h, id);
    std::thread::scope(|scope| {
        let _release = Release(&r.front);
        let stepping = scope.spawn(|| block_on_waiting(r.h.runtime.run_step(id)));
        until("the model request is held", || {
            r.front.entered.load(Ordering::SeqCst)
        });
        let before = counts(&r.h, id);
        for request in [
            Request::ResumeTask {
                task_id: id,
                extend: None,
            },
            Request::ApproveAction {
                task_id: id,
                note: None,
            },
            Request::RejectAction {
                task_id: id,
                note: None,
                halt: None,
            },
            Request::AnswerQuestion {
                task_id: id,
                answer: "x".into(),
            },
            Request::RollbackWorkspace {
                task_id: id,
                snapshot_id: Some(snapshot),
            },
            Request::PromoteWorkspace {
                task_id: id,
                plan: plan.clone(),
            },
            Request::DeleteTask {
                task_id: id,
                audit: false,
            },
        ] {
            let name = request.name();
            let refused = send(&r.h, request).unwrap_err();
            assert_eq!(refused.kind(), "state", "{name}: {refused}");
            assert!(
                refused.to_string().contains("running") && refused.to_string().contains(name),
                "{name}: {refused}"
            );
        }
        assert_eq!(counts(&r.h, id), before, "nothing was written");
        r.front.released.store(true, Ordering::SeqCst);
        stepping.join().unwrap().unwrap();
    });
    assert_eq!(r.h.runtime.steps.held(), 0);
}

/// REQ-CLI-005, REQ-TASK-006: the first signal during a step is a pause noted and an action not
/// started: the step ends `cancelled` with no action run, the pause is requested at its end and
/// taken at the boundary; between steps the same request pauses at once.
#[test]
fn req_cli_005_the_first_signal_during_a_step_starts_no_action_and_pauses_at_the_boundary() {
    let r = reading();
    let id = r.h.create();
    std::thread::scope(|scope| {
        let _release = Release(&r.front);
        let stepping = scope.spawn(|| block_on_waiting(r.h.runtime.run_step(id)));
        until("the model request is held", || {
            r.front.entered.load(Ordering::SeqCst)
        });
        noted(send(&r.h, Request::StopAtBoundary { task_id: id }));
        assert!(!r.h.runtime.get_task(id).unwrap().task.pause_requested);
        r.front.released.store(true, Ordering::SeqCst);
        let step = stepping.join().unwrap().unwrap();
        assert_eq!(
            step.outcome,
            StepOutcome::Cancelled,
            "no action was started"
        );
        assert_eq!(step.state, TaskState::Running);
    });
    assert!(
        r.h.persistence.load_actions(id).unwrap()[0]
            .status
            .is_terminal()
    );
    assert!(r.h.tools.calls().is_empty(), "the tool was never called");
    assert!(r.h.runtime.get_task(id).unwrap().task.pause_requested);
    assert_eq!(r.h.step(id).outcome, StepOutcome::Paused);
    assert_eq!(r.h.state(id), TaskState::Paused);
}

/// REQ-CLI-005: between steps the first signal pauses at once, and the next step takes it.
#[test]
fn req_cli_005_the_first_signal_between_steps_pauses_the_task_at_once() {
    let r = reading();
    r.front.hold_next.store(false, Ordering::SeqCst);
    let id = r.h.create();
    let Ok(Reply::Task(task)) = send(&r.h, Request::StopAtBoundary { task_id: id }) else {
        panic!("not the task")
    };
    assert!(task.pause_requested);
    // The request to start no new action ends the boundary first; the pause is the next.
    assert_eq!(r.h.step(id).outcome, StepOutcome::Cancelled);
    assert_eq!(r.h.step(id).outcome, StepOutcome::Paused);
}

/// REQ-CLI-005, REQ-TASK-006: the second signal during a step interrupts it anyway and, as the
/// step leaves the task `running`, cancels it at the step's end, in the step's own turn: the
/// step's end is written first and the cancel after it.
#[test]
fn req_cli_005_the_second_signal_during_a_step_cancels_the_task_it_leaves_running() {
    let scripted = Arc::new(ScriptedBackend::with_decisions(vec![act("read_file", "a")]));
    let front = super::long_lived::Holding::new(scripted.clone(), true);
    let h = Harness::with_front(
        front.clone(),
        scripted,
        Policy::shipped(),
        vec![],
        crate::limits::LIM_C1_CONTEXT_CHARS,
        vec![],
    );
    let id = h.create();
    std::thread::scope(|scope| {
        let _release = Release(&front);
        let stepping = scope.spawn(|| block_on_waiting(h.runtime.run_step(id)));
        until("the model request is held", || {
            front.entered.load(Ordering::SeqCst)
        });
        noted(send(&h, Request::StopAnyway { task_id: id }));
        let step = stepping.join().unwrap().unwrap();
        assert_eq!(step.outcome, StepOutcome::Cancelled);
    });
    assert!(
        front.aborted.load(Ordering::SeqCst),
        "the request was aborted"
    );
    assert_eq!(h.state(id), TaskState::Cancelled);
    let names = h.event_names(id);
    assert!(
        names.iter().position(|n| n == "step.finished").unwrap()
            < names
                .iter()
                .rposition(|n| n == "task.state_changed")
                .unwrap(),
        "{names:?}"
    );
    assert_eq!(state_changes(&h, id).last().unwrap(), "cancelled");
    assert_eq!(h.runtime.steps.held(), 0);
}

/// The second signal leaves a task the step halted or paused as the step left it (the step's own
/// end says what happened to it), and with no step in flight cancels a `running` task at once.
#[test]
fn req_cli_005_the_second_signal_cancels_a_running_task_with_no_step_and_leaves_a_paused_one() {
    let r = reading();
    r.front.hold_next.store(false, Ordering::SeqCst);
    let id = r.h.create();
    let Ok(Reply::Task(task)) = send(&r.h, Request::StopAnyway { task_id: id }) else {
        panic!("not the task")
    };
    assert_eq!(task.state, TaskState::Cancelled);
    // A paused task is not cancelled by a signal that finds no step.
    let other = r.h.create_with(|c| c.scope.root = "/home/u/other".into());
    r.h.runtime.pause_task(&r.h.session, other).unwrap();
    assert_eq!(r.h.step(other).outcome, StepOutcome::Paused);
    let Ok(Reply::Task(task)) = send(&r.h, Request::StopAnyway { task_id: other }) else {
        panic!("not the task")
    };
    assert_eq!(task.state, TaskState::Paused);
}

/// The request to interrupt says whether a step was in flight to be interrupted.
#[test]
fn req_cli_005_the_request_to_interrupt_says_whether_a_step_was_in_flight() {
    let r = reading();
    let id = r.h.create();
    assert_eq!(
        send(&r.h, Request::InterruptStep { task_id: id }).unwrap(),
        Reply::Interrupted {
            step_in_flight: false
        }
    );
    r.h.runtime.clear_interruption(id);
    std::thread::scope(|scope| {
        let _release = Release(&r.front);
        let stepping = scope.spawn(|| block_on_waiting(r.h.runtime.run_step(id)));
        until("the model request is held", || {
            r.front.entered.load(Ordering::SeqCst)
        });
        assert_eq!(
            send(&r.h, Request::InterruptStep { task_id: id }).unwrap(),
            Reply::Interrupted {
                step_in_flight: true
            }
        );
        r.front.released.store(true, Ordering::SeqCst);
        stepping.join().unwrap().unwrap();
    });
    // An unknown task is refused, and nothing is created.
    let entries = r.h.runtime.interruption_entries();
    let unknown = TaskId::from_uuid(uuid::Uuid::from_u128(77));
    assert!(send(&r.h, Request::InterruptStep { task_id: unknown }).is_err());
    assert_eq!(r.h.runtime.interruption_entries(), entries);
}

fn note() -> Note {
    Note {
        request_id: RequestId::new(&ScriptedIds::starting_at(5_000)),
        session: Session {
            id: "s".into(),
            os_user: "u".into(),
        },
    }
}

/// A request noted as the step ends is never lost: what is noted while the notes of the step are
/// being applied is applied too, and the claim is released only when none is left.
#[test]
fn req_dec_001_a_request_noted_as_the_step_ends_is_applied_before_the_claim_is_released() {
    let steps = Steps::default();
    let id = TaskId::from_uuid(uuid::Uuid::from_u128(3));
    let step = steps.begin_step(id).unwrap();
    assert!(matches!(
        steps
            .begin_write(id, OnStep::Note(Noted::Pause(note())))
            .unwrap(),
        Held::Noted
    ));
    let mut applied = Vec::new();
    step.finish(|pause, stop| {
        applied.push((pause.is_some(), stop.is_some()));
        if applied.len() == 1 {
            // A request that comes while the step applies its notes.
            assert!(matches!(
                steps
                    .begin_write(id, OnStep::Note(Noted::Stop(note())))
                    .unwrap(),
                Held::Noted
            ));
        }
    });
    assert_eq!(applied, [(true, false), (false, true)]);
    assert_eq!(steps.held(), 0, "the claim is released");
    drop(steps.begin_step(id).unwrap());
}

/// A step that ends without finishing (its future dropped) takes what was noted with it, and
/// releases the claim.
#[test]
fn req_dec_001_a_step_dropped_without_finishing_drops_its_notes_and_its_claim() {
    let steps = Steps::default();
    let id = TaskId::from_uuid(uuid::Uuid::from_u128(4));
    let step = steps.begin_step(id).unwrap();
    assert!(matches!(
        steps
            .begin_write(id, OnStep::Note(Noted::Stop(note())))
            .unwrap(),
        Held::Noted
    ));
    drop(step);
    assert_eq!(steps.held(), 0);
    // The next step meets no note.
    let next = steps.begin_step(id).unwrap();
    next.finish(|pause, stop| panic!("nothing was noted: {pause:?} {stop:?}"));
}

/// A request that holds the task while it writes keeps a step out until it is done, and two
/// requests take turns.
#[test]
fn req_dec_001_a_request_holds_the_task_while_it_writes_and_steps_wait_for_it() {
    let steps = Steps::default();
    let id = TaskId::from_uuid(uuid::Uuid::from_u128(5));
    let claimed = AtomicBool::new(false);
    let proceed = AtomicBool::new(false);
    std::thread::scope(|scope| {
        let writing = scope.spawn(|| {
            let held = steps.begin_write(id, OnStep::Wait(&|| {})).unwrap();
            claimed.store(true, Ordering::SeqCst);
            until("the test lets the write end", || {
                proceed.load(Ordering::SeqCst)
            });
            drop(held);
        });
        until("the task is claimed", || claimed.load(Ordering::SeqCst));
        let refused = steps.begin_step(id).err().expect("refused");
        assert_eq!(refused.kind(), "conflict", "{refused}");
        // A second request waits for the first.
        let second = scope.spawn(|| {
            let held = steps.begin_write(id, OnStep::Wait(&|| {})).unwrap();
            drop(held);
        });
        proceed.store(true, Ordering::SeqCst);
        writing.join().unwrap();
        second.join().unwrap();
    });
    assert_eq!(steps.held(), 0);
    drop(steps.begin_step(id).unwrap());
}

/// A cancel in progress refuses every other request with `conflict`, and a cancel waits for a
/// request that holds the task.
#[test]
fn req_task_006_a_cancel_in_progress_refuses_the_other_requests() {
    let r = reading();
    r.front.hold_next.store(false, Ordering::SeqCst);
    let id = r.h.create();
    let (cancelling, _) = r.h.runtime.steps.begin_cancel(id).unwrap();
    for request in [
        Request::PauseTask { task_id: id },
        Request::StopAtBoundary { task_id: id },
        Request::ApproveAction {
            task_id: id,
            note: None,
        },
    ] {
        let refused = send(&r.h, request).unwrap_err();
        assert_eq!(refused.kind(), "conflict", "{refused}");
    }
    drop(cancelling);
    assert_eq!(r.h.runtime.steps.held(), 0);
}

/// A request that finds a step in flight while the task is already in another state than
/// `running` (the step wrote its end and is returning) waits for the step to end, and applies;
/// one that finds the task `running` is refused.
#[test]
fn req_dec_001_a_request_that_finds_a_step_returning_waits_for_it_and_one_that_finds_it_running_is_refused()
 {
    let steps = Steps::default();
    let id = TaskId::from_uuid(uuid::Uuid::from_u128(6));
    let step = steps.begin_step(id).unwrap();
    let applied = AtomicBool::new(false);
    std::thread::scope(|scope| {
        let request = scope.spawn(|| {
            let held = steps
                .begin_write(id, OnStep::UnlessRunning("approve_action", &|| Ok(false)))
                .unwrap();
            applied.store(true, Ordering::SeqCst);
            drop(held);
        });
        until("the request waits for the step", || steps.waiting(id) == 1);
        assert!(
            !applied.load(Ordering::SeqCst),
            "nothing is applied while the step holds the task"
        );
        // A request that finds the task running meanwhile is refused.
        let refused = steps
            .begin_write(id, OnStep::UnlessRunning("resume_task", &|| Ok(true)))
            .err()
            .expect("refused");
        assert_eq!(refused.kind(), "state", "{refused}");
        step.finish(|_, _| {});
        request.join().unwrap();
    });
    assert!(applied.load(Ordering::SeqCst));
    assert_eq!(steps.held(), 0);
}
