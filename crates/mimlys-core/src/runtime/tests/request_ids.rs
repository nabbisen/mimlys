//! The request id (handoff 014b, item 6; external design §4, §17.1; REQ-ARCH-004): every event
//! that a state-changing request's change writes carries the request's id; a request whose id is
//! recorded returns that it was applied, at which event, and the task now, and applies nothing
//! again; a request that was refused wrote nothing, so a repeat of it is answered afresh. Judged
//! by state: the events of the store, counted and read.

use super::handler::{by_the_verb, create_request, scenario, scenario_harness};
use super::long_lived::{Release, block_on_waiting, rig, until};
use super::*;
use crate::api::{AlreadyApplied, Envelope, Outcome, Reply, Request, Response};
use crate::event::EventRecord;
use crate::ids::{RequestId, SystemIds};
use std::sync::atomic::Ordering;

fn envelope(h: &Harness, id: RequestId, request: Request) -> Envelope {
    Envelope {
        request_id: id,
        session: h.session.clone(),
        request,
    }
}

/// One request, under an id of its own: what the call wrote carries that id and no other, and a
/// refused one wrote nothing.
fn recording(h: &Harness, request: Request) -> Result<Reply, RuntimeError> {
    let id = RequestId::new(&SystemIds);
    let before = h.persistence.events().len();
    let name = request.name();
    let result = h.runtime.handle(envelope(h, id, request)).into_result();
    let written = h.persistence.events()[before..].to_vec();
    match &result {
        Ok(_) => {
            assert!(!written.is_empty(), "{name} wrote nothing");
            for event in &written {
                assert_eq!(
                    event.request_id,
                    Some(id),
                    "{name}: `{}` does not carry the id of the request",
                    event.event.name()
                );
            }
        }
        Err(error) => assert!(written.is_empty(), "{name} was refused ({error}) and wrote"),
    }
    result
}

/// REQ-ARCH-004 (external design §17.1): each of the thirteen verbs records its id on every event
/// its change writes, those of its helpers included (a snapshot, a closed action, a pinned tool);
/// a step's own events carry none.
#[test]
fn req_arch_004_every_verb_records_its_id_on_the_events_it_writes_and_a_step_records_none() {
    let h = scenario_harness();
    scenario(&h, &recording);
    let events = h.persistence.events();
    let with = events.iter().filter(|e| e.request_id.is_some()).count();
    assert!(with >= 20, "the scenario wrote {with} events under an id");
    for step_event in events
        .iter()
        .filter(|e| matches!(e.event.name(), "step.started" | "decision.received"))
    {
        assert_eq!(
            step_event.request_id,
            None,
            "{} was the step's",
            step_event.event.name()
        );
    }
    // And the verbs the scenario met are all thirteen of the creation's and the others'.
    let names: std::collections::BTreeSet<&str> = events.iter().map(|e| e.event.name()).collect();
    for expected in [
        "task.created",
        "workspace.materialized",
        "workspace.snapshot",
        "grant.issued",
        "grant.revoked",
        "approval.decided",
        "question.answered",
        "workspace.rollback",
        "workspace.promoted",
        "task.deleted",
        "task.state_changed",
    ] {
        assert!(names.contains(expected), "{expected} in {names:?}");
    }
}

/// The first event under `id`, with its position.
fn first_with(h: &Harness, id: RequestId) -> (u64, EventRecord) {
    h.persistence
        .events()
        .into_iter()
        .enumerate()
        .find(|(_, e)| e.request_id == Some(id))
        .map(|(i, e)| (i as u64 + 1, e))
        .expect("an event carries the id")
}

/// One request sent twice under one id, as a client does after a broken connection: the second
/// answer says that it was applied, at the first event the first wrote, and the store is as the
/// first left it; a request that was refused is refused again, and wrote nothing either time.
fn twice(h: &Harness, request: Request) -> Result<Reply, RuntimeError> {
    let id = RequestId::new(&SystemIds);
    let task_named = request.task_id();
    let first = h.runtime.handle(envelope(h, id, request.clone()));
    let events = h.persistence.events();
    let writes = h.persistence.writes().len();
    let second = h.runtime.handle(envelope(h, id, request));
    assert_eq!(h.persistence.events(), events, "the repeat wrote an event");
    assert_eq!(h.persistence.writes().len(), writes, "the repeat wrote");
    match &first.outcome {
        Outcome::Done(_) => {
            let Outcome::AlreadyApplied(already) = &second.outcome else {
                panic!("a repeat of an applied request: {second:?}")
            };
            let (position, event) = first_with(h, id);
            assert_eq!(already.position, position);
            assert_eq!(already.event, event.event.name());
            // The task as it is now: the one the request names, or the one it made.
            let now = event
                .task_id
                .or(task_named)
                .and_then(|t| h.persistence.load_task(t).unwrap());
            assert_eq!(already.task.as_deref(), now.as_ref());
        }
        Outcome::Failed(error) => {
            assert_eq!(
                second.outcome,
                Outcome::Failed(error.clone()),
                "a refused request is refused again"
            );
        }
        Outcome::AlreadyApplied(_) => panic!("the first answer: {first:?}"),
    }
    assert_eq!(second.request_id, id);
    first.into_result()
}

/// REQ-ARCH-004 (external design §4): a repeat of every verb, in the life of two tasks, returns
/// that it was applied and applies nothing again: the events are the verbs' own, as if each had
/// been sent once, once the ids are set aside.
#[test]
fn req_arch_004_a_repeat_of_an_applied_request_returns_where_it_was_applied_and_applies_nothing() {
    let repeated = scenario_harness();
    let said_repeated = scenario(&repeated, &twice);
    let once = scenario_harness();
    let said_once = scenario(&once, &by_the_verb);
    assert_eq!(said_repeated, said_once, "the answers are the verbs'");
    let bare = |h: &Harness| -> Vec<EventRecord> {
        h.persistence
            .events()
            .into_iter()
            .map(|mut e| {
                e.request_id = None;
                e
            })
            .collect()
    };
    assert_eq!(bare(&repeated), bare(&once), "nothing was applied twice");
}

/// REQ-ARCH-004: a creation repeated makes no second task; the answer is the task made, as it is
/// now.
#[test]
fn req_arch_004_a_creation_repeated_makes_no_second_task() {
    let h = Harness::new(vec![]);
    let id = RequestId::new(&SystemIds);
    let first = h.runtime.handle(envelope(&h, id, create_request(false)));
    let Outcome::Done(Reply::Task(task)) = &first.outcome else {
        panic!("{first:?}")
    };
    let second = h.runtime.handle(envelope(&h, id, create_request(false)));
    let Outcome::AlreadyApplied(AlreadyApplied {
        position,
        event,
        task: now,
    }) = second.outcome
    else {
        panic!("{second:?}")
    };
    assert_eq!(event, "task.created");
    assert_eq!(position, 1);
    assert_eq!(now.unwrap().task_id, task.task_id);
    assert_eq!(h.runtime.list_tasks().unwrap().len(), 1, "one task");
    // The task moved on meanwhile: the answer is the task as it is now.
    h.runtime.cancel_task(&h.session, task.task_id).unwrap();
    let third = h.runtime.handle(envelope(&h, id, create_request(false)));
    let Outcome::AlreadyApplied(already) = third.outcome else {
        panic!("{third:?}")
    };
    assert_eq!(already.task.unwrap().state.name(), "cancelled");
    assert_eq!(h.runtime.list_tasks().unwrap().len(), 1);
}

/// External design §4: a request that was refused wrote nothing, so a repeat of it is answered
/// afresh: refused again while it would be, applied when what refused it has changed.
#[test]
fn req_arch_004_a_refused_request_is_answered_afresh_when_it_is_sent_again() {
    let h = Harness::with_policy(vec![act("write_file", "src/lib.rs")], approval_policy());
    h.tools.script(
        "write_file",
        ExecutionOutcome::succeeded(json!({"bytes": 3}), 2),
    );
    let task = h.create();
    let id = RequestId::new(&SystemIds);
    let approve = Request::ApproveAction {
        task_id: task,
        note: None,
    };
    let refused = h.runtime.handle(envelope(&h, id, approve.clone()));
    assert!(matches!(refused.outcome, Outcome::Failed(_)), "{refused:?}");
    // Nothing was recorded, so the same request again is refused again...
    let again = h.runtime.handle(envelope(&h, id, approve.clone()));
    assert_eq!(again.outcome, refused.outcome);
    // ...until the task waits for an approval: then it is applied, not "already applied".
    h.step(task);
    let applied = h.runtime.handle(envelope(&h, id, approve.clone()));
    assert!(matches!(applied.outcome, Outcome::Done(_)), "{applied:?}");
    // And now it is recorded.
    let repeat = h.runtime.handle(envelope(&h, id, approve));
    assert!(
        matches!(repeat.outcome, Outcome::AlreadyApplied(_)),
        "{repeat:?}"
    );
}

/// The asks change nothing and are not recorded: repeated, they are answered each time.
#[test]
fn req_arch_004_an_ask_is_not_recorded_and_is_answered_each_time() {
    let h = Harness::new(vec![]);
    let id = RequestId::new(&SystemIds);
    let ask = Request::CreationForbidden {
        profile: None,
        backend: "default".into(),
    };
    for _ in 0..2 {
        let response = h.runtime.handle(envelope(&h, id, ask.clone()));
        assert_eq!(
            response.outcome,
            Outcome::Done(Reply::CreationForbidden(false))
        );
    }
    assert!(h.persistence.events().is_empty());
}

/// Two requests of one id handled together take turns, so the second finds what the first
/// recorded: one task is made, one answer is the creation and every other says it was applied.
#[test]
fn req_arch_004_the_same_request_handled_by_several_threads_is_applied_once() {
    let h = Harness::new(vec![]);
    let id = RequestId::new(&SystemIds);
    let answers: Vec<Response> = std::thread::scope(|scope| {
        let threads: Vec<_> = (0..8)
            .map(|_| scope.spawn(|| h.runtime.handle(envelope(&h, id, create_request(false)))))
            .collect();
        threads.into_iter().map(|t| t.join().unwrap()).collect()
    });
    let done = answers
        .iter()
        .filter(|r| matches!(r.outcome, Outcome::Done(_)))
        .count();
    let already = answers
        .iter()
        .filter(|r| matches!(r.outcome, Outcome::AlreadyApplied(_)))
        .count();
    assert_eq!((done, already), (1, 7), "{answers:?}");
    assert_eq!(h.runtime.list_tasks().unwrap().len(), 1, "one task");
    assert_eq!(h.runtime.requests.held(), 0, "no id is held");
}

/// A pause noted while the step is in flight is written at the step's end under the id of its
/// own request, in the step's turn; the step's own events carry none; and the id is recorded, so
/// the same request sent again is answered as applied.
#[test]
fn req_arch_004_a_pause_taken_at_the_steps_end_records_the_id_of_its_request() {
    let r = rig(vec![act("read_file", "a")], false);
    r.h.tools
        .script("read_file", ExecutionOutcome::succeeded(json!({}), 1));
    let id = r.h.create();
    let pause = RequestId::new(&SystemIds);
    let request = Request::PauseTask { task_id: id };
    std::thread::scope(|scope| {
        let _release = Release(&r.front);
        let stepping = scope.spawn(|| block_on_waiting(r.h.runtime.run_step(id)));
        until("the model request is held", || {
            r.front.entered.load(Ordering::SeqCst)
        });
        let noted = r.h.runtime.handle(envelope(&r.h, pause, request.clone()));
        assert!(
            matches!(noted.outcome, Outcome::Done(Reply::Noted(_))),
            "{noted:?}"
        );
        r.front.released.store(true, Ordering::SeqCst);
        stepping.join().unwrap().unwrap();
    });
    let events = r.h.persistence.events();
    let requested: Vec<&EventRecord> = events
        .iter()
        .filter(|e| e.event.name() == "task.pause_requested")
        .collect();
    assert_eq!(requested.len(), 1);
    assert_eq!(requested[0].request_id, Some(pause));
    for e in events
        .iter()
        .filter(|e| e.task_id == Some(id) && e.event.name().starts_with("step."))
    {
        assert_eq!(e.request_id, None, "{}", e.event.name());
    }
    let again = r.h.runtime.handle(envelope(&r.h, pause, request));
    assert!(
        matches!(again.outcome, Outcome::AlreadyApplied(_)),
        "{again:?}"
    );
}
