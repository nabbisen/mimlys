//! The request id (handoff 014b, item 6; external design §4, §17.1; REQ-ARCH-004): every event
//! that a state-changing request's change writes carries the request's id; a request whose id is
//! recorded returns that it was applied, at which event, and the task now, and applies nothing
//! again; a request that was refused wrote nothing, so a repeat of it is answered afresh. Judged
//! by state: the events of the store, counted and read.

use super::handler::{by_the_verb, create_request, scenario, scenario_harness};
use super::long_lived::{Release, block_on_waiting, rig, until};
use super::*;
use crate::api::{AlreadyApplied, Did, Envelope, Outcome, Reply, Request, Response};
use crate::event::{Event, EventRecord};
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
        ..
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

/// The same request twice under one id: the first answer, and the second, with the store as the
/// first left it.
fn sent_twice(h: &Harness, request: Request) -> (Outcome, AlreadyApplied) {
    let id = RequestId::new(&SystemIds);
    let first = h.runtime.handle(envelope(h, id, request.clone()));
    let events = h.persistence.events();
    let second = h.runtime.handle(envelope(h, id, request));
    assert_eq!(h.persistence.events(), events, "the repeat wrote an event");
    let Outcome::AlreadyApplied(already) = second.outcome else {
        panic!("a repeat of a request that wrote: {second:?}")
    };
    (first.outcome, already)
}

/// REQ-ARCH-004 (external design §4, "The recorded outcome"): a repeat of an applied request that
/// returns no task is a success, with what the first answer held, read back from the store: the
/// grant, the snapshot, the promotion, as they are now; nothing is applied again.
#[test]
fn req_arch_004_a_repeat_of_an_applied_request_is_answered_with_what_it_did() {
    let h = Harness::new(vec![]);
    let id = h.create_with(|c| c.hold = true);
    let each = |request: Request, expected: &str| {
        let (first, again) = sent_twice(&h, request);
        let Outcome::Done(reply) = first else {
            panic!("{expected}: {first:?}")
        };
        assert_eq!(
            again.did,
            Did::Applied {
                reply: Some(reply.clone())
            },
            "{expected}"
        );
        assert_eq!(again.event, expected);
        reply
    };
    let Reply::Grant(grant) = each(
        Request::GrantCapability {
            task_id: id,
            kind: GrantKind::Tool {
                name: "write_file".into(),
            },
        },
        "grant.issued",
    ) else {
        panic!("a grant")
    };
    each(
        Request::RevokeCapability {
            task_id: id,
            grant_id: grant.grant_id,
        },
        "grant.revoked",
    );
    each(
        Request::RollbackWorkspace {
            task_id: id,
            snapshot_id: None,
        },
        "workspace.rollback",
    );
    let snapshots = h.persistence.load_snapshots(id).unwrap().len();
    let Reply::Promotion(promotion) = each(
        Request::PromoteWorkspace {
            task_id: id,
            plan: one_file_plan(&h, id),
        },
        "workspace.snapshot",
    ) else {
        panic!("a promotion")
    };
    assert_eq!(
        h.persistence.load_snapshots(id).unwrap().len(),
        snapshots + 1,
        "one backup, not two"
    );
    each(
        Request::RevertPromotion {
            task_id: id,
            promotion_id: promotion.promotion_id,
            overwrite_edited: false,
        },
        "workspace.rollback",
    );
}

/// External design §4: a promotion that failed after it wrote (`workspace.promotion_failed`) is
/// answered, when sent again, with the recorded failure and its reason, never as applied; the
/// first answer was the same failure.
#[test]
fn req_arch_004_a_repeat_of_a_promotion_that_failed_after_it_wrote_is_answered_with_the_failure() {
    let h = Harness::new(vec![]);
    let id = h.create_with(|c| c.hold = true);
    h.workspaces.on_promote_apply(Box::new(|_| {
        Err(crate::spi::WorkspaceError::Storage {
            detail: "injected".into(),
        })
    }));
    let (first, again) = sent_twice(
        &h,
        Request::PromoteWorkspace {
            task_id: id,
            plan: one_file_plan(&h, id),
        },
    );
    let Outcome::Failed(error) = first else {
        panic!("{first:?}")
    };
    assert!(matches!(error, RuntimeError::PromotionFailed { .. }));
    // The first event it wrote is the backup, which is not the failure: the answer is.
    assert_eq!(again.event, "workspace.snapshot");
    assert_eq!(
        again.did,
        Did::Failed {
            event: "workspace.promotion_failed".into(),
            reason: error.to_string(),
            // What the first answer was: `internal`, as a failed apply is not a conflict.
            error_kind: Some(error.kind().into()),
            error_detail: error.detail(),
        }
    );
}

/// A promotion whose request wrote the backup and stopped (a crash, a failed write) before
/// recording anything of the apply is not applied either: the answer says where it stopped.
#[test]
fn req_arch_004_a_repeat_of_a_promotion_that_stopped_before_it_was_recorded_is_not_applied() {
    let h = Harness::new(vec![]);
    let id = h.create_with(|c| c.hold = true);
    let request = RequestId::new(&SystemIds);
    let backup = h.persistence.load_snapshots(id).unwrap()[0].snapshot_id;
    let mut written = h.runtime.event(
        Actor::Runtime,
        Some(id),
        None,
        None,
        Event::WorkspaceSnapshot {
            snapshot_id: backup,
            reason: "backup".into(),
        },
    );
    written.request_id = Some(request);
    h.persistence.append_event(&written).unwrap();
    let again = h.runtime.handle(envelope(
        &h,
        request,
        Request::PromoteWorkspace {
            task_id: id,
            plan: one_file_plan(&h, id),
        },
    ));
    let Outcome::AlreadyApplied(already) = again.outcome else {
        panic!("{again:?}")
    };
    let Did::Failed {
        event,
        reason,
        error_kind,
        error_detail,
    } = already.did
    else {
        panic!("not applied: {:?}", already.did)
    };
    assert_eq!(event, "workspace.snapshot");
    assert!(reason.contains("did not complete"), "{reason}");
    assert_eq!(error_kind, None, "nothing recorded what stopped it");
    assert_eq!(error_detail, None);
    assert_eq!(h.persistence.load_promotions(id).unwrap().len(), 0);
}

/// External design §4: a deletion repeated after it took effect has no footprint to answer with
/// (it was not kept): the answer is that it was applied, and nothing more. The audit stays unless
/// asked, so the request is found by its event, and the task's record with it.
#[test]
fn req_arch_004_a_repeat_of_a_deletion_finds_the_task_gone() {
    let h = Harness::new(vec![]);
    let id = h.create_with(|c| c.hold = true);
    h.runtime.cancel_task(&h.session, id).unwrap();
    let (first, again) = sent_twice(
        &h,
        Request::DeleteTask {
            task_id: id,
            audit: false,
        },
    );
    assert!(matches!(first, Outcome::Done(Reply::Footprint(_))));
    assert_eq!(again.event, "task.deleted");
    assert_eq!(again.did, Did::Applied { reply: None });
    // With the audit gone too, nothing is found: the id is not recorded any more.
    let other = h.create_with(|c| {
        c.hold = true;
        c.scope.root = "/home/u/other".into();
    });
    h.runtime.cancel_task(&h.session, other).unwrap();
    let request = RequestId::new(&SystemIds);
    let delete = Request::DeleteTask {
        task_id: other,
        audit: true,
    };
    let first = h.runtime.handle(envelope(&h, request, delete.clone()));
    assert!(matches!(first.outcome, Outcome::Done(Reply::Footprint(_))));
    let second = h.runtime.handle(envelope(&h, request, delete));
    let Outcome::Failed(error) = second.outcome else {
        panic!("{second:?}")
    };
    assert_eq!(error.kind(), "not_found", "{error}");
}

/// The answer to a request sent twice under one id, when the first did not apply: the first
/// answer's error and the repeat's recorded outcome.
fn failed_then_repeated(h: &Harness, request: Request) -> (RuntimeError, AlreadyApplied) {
    let (first, again) = sent_twice(h, request);
    let Outcome::Failed(error) = first else {
        panic!("{first:?}")
    };
    (error, again)
}

/// External design §4 and §17.1 (review of 014b-5): a promotion that failed as a `conflict` is
/// answered as a `conflict` when sent again, with the same words, and not as `internal`: the
/// failure's event records the kind the first answer had. A failure that was `internal` stays so.
#[test]
fn req_arch_004_a_failure_keeps_its_kind_on_a_repeat() {
    for (injected, kind) in [
        (
            crate::spi::WorkspaceError::Conflict {
                paths: vec!["a.txt".into()],
            },
            "conflict",
        ),
        (
            crate::spi::WorkspaceError::Storage {
                detail: "injected".into(),
            },
            "internal",
        ),
    ] {
        let h = Harness::new(vec![]);
        let id = h.create_with(|c| c.hold = true);
        h.workspaces
            .on_promote_apply(Box::new(move |_| Err(injected.clone())));
        let (error, again) = failed_then_repeated(
            &h,
            Request::PromoteWorkspace {
                task_id: id,
                plan: one_file_plan(&h, id),
            },
        );
        assert_eq!(error.kind(), kind, "{error}");
        // The event holds the kind: it is what the repeat is answered from.
        let recorded: Vec<Event> = h
            .persistence
            .events()
            .into_iter()
            .map(|e| e.event)
            .filter(|e| e.name() == "workspace.promotion_failed")
            .collect();
        assert_eq!(
            recorded,
            vec![Event::WorkspacePromotionFailed {
                reason: error.to_string(),
                error_kind: Some(kind.into()),
                error_detail: error.detail(),
            }]
        );
        let Did::Failed {
            reason,
            error_kind: answered,
            ..
        } = again.did.clone()
        else {
            panic!("{:?}", again.did)
        };
        assert_eq!(reason, error.to_string(), "the same words");
        assert_eq!(answered.as_deref(), Some(kind));
        // As an error, it is the kind the first one was.
        let repeated = again.did.into_reply().unwrap_err();
        assert_eq!(repeated.kind(), kind, "{repeated}");
    }
}

/// External design §4: a failure whose event holds no kind (written before 0.14) is answered
/// `internal` on a repeat, with its reason.
#[test]
fn req_arch_004_a_failure_recorded_without_a_kind_is_answered_as_internal() {
    let h = Harness::new(vec![]);
    let id = h.create_with(|c| c.hold = true);
    let request = RequestId::new(&SystemIds);
    let backup = h.persistence.load_snapshots(id).unwrap()[0].snapshot_id;
    for event in [
        Event::WorkspaceSnapshot {
            snapshot_id: backup,
            reason: "backup".into(),
        },
        Event::WorkspacePromotionFailed {
            reason: "the promotion failed (older)".into(),
            error_kind: None,
            error_detail: None,
        },
    ] {
        let mut written = h.runtime.event(Actor::Runtime, Some(id), None, None, event);
        written.request_id = Some(request);
        h.persistence.append_event(&written).unwrap();
    }
    let again = h.runtime.handle(envelope(
        &h,
        request,
        Request::PromoteWorkspace {
            task_id: id,
            plan: one_file_plan(&h, id),
        },
    ));
    let error = again.into_result().unwrap_err();
    assert!(matches!(
        error,
        RuntimeError::Recorded {
            error_kind: None,
            error_detail: None,
            ..
        }
    ));
    assert_eq!(error.detail(), None, "a record without a detail has none");
    assert_eq!(error.kind(), "internal");
    assert_eq!(error.to_string(), "the promotion failed (older)");
}

/// External design §4 (review of 014b-5): a promotion that stopped before its apply was recorded
/// keeps the kind of what stopped it: its failure is recorded with that kind, so the repeat is
/// answered with it. Here the backup could not be sealed, after it was recorded; the project is
/// not touched and no promotion is recorded.
#[test]
fn req_arch_004_a_promotion_stopped_before_its_apply_keeps_the_kind_of_what_stopped_it() {
    let h = Harness::new(vec![]);
    let id = h.create_with(|c| c.hold = true);
    h.workspaces
        .fail_next_seal(crate::spi::WorkspaceError::Refused {
            reason: "the journal is not writable".into(),
        });
    let (error, again) = failed_then_repeated(
        &h,
        Request::PromoteWorkspace {
            task_id: id,
            plan: one_file_plan(&h, id),
        },
    );
    assert_eq!(error.kind(), "state", "{error}");
    // The backup is the first event it wrote; the failure is the last.
    assert_eq!(again.event, "workspace.snapshot");
    assert_eq!(
        again.did,
        Did::Failed {
            event: "workspace.promotion_failed".into(),
            reason: error.to_string(),
            error_kind: Some("state".into()),
            error_detail: None,
        }
    );
    assert!(h.persistence.load_promotions(id).unwrap().is_empty());
}

/// External design §4 and §17.1 (review of 014b-6): a failure keeps its detail on a repeat, as it
/// keeps its kind: the failure's event records the first answer's `detail` (the paths of a
/// conflict, what a failed restore kept), and the repeat is the first answer, whole: the same
/// kind, the same words, the same detail.
#[test]
fn req_arch_004_a_failure_keeps_its_detail_on_a_repeat() {
    let h = Harness::new(vec![]);
    let id = h.create_with(|c| c.hold = true);
    h.workspaces.on_promote_apply(Box::new(|_| {
        Err(crate::spi::WorkspaceError::Conflict {
            paths: vec!["a.txt".into(), "b/c.txt".into()],
        })
    }));
    let (error, again) = failed_then_repeated(
        &h,
        Request::PromoteWorkspace {
            task_id: id,
            plan: one_file_plan(&h, id),
        },
    );
    // The first answer had a detail: the paths of the conflict.
    let first = error.detail().expect("a conflict names its paths");
    assert_eq!(first["paths"], serde_json::json!(["a.txt", "b/c.txt"]));
    // The event holds it, as the first answer had it.
    let recorded: Vec<Option<serde_json::Value>> = h
        .persistence
        .events()
        .into_iter()
        .filter_map(|e| match e.event {
            Event::WorkspacePromotionFailed { error_detail, .. } => Some(error_detail),
            _ => None,
        })
        .collect();
    assert_eq!(recorded, vec![Some(first.clone())]);
    // The repeat is the first answer, whole.
    let repeated = again.did.into_reply().unwrap_err();
    assert_eq!(repeated.kind(), error.kind());
    assert_eq!(repeated.to_string(), error.to_string());
    assert_eq!(repeated.detail(), Some(first));
}

/// External design §4 (REQ-MAIN-006; review of 014b-6): a failure recorded with an error kind that
/// this release does not know (a newer release wrote it) is answered `internal` when sent again,
/// and its message names the word; the event keeps the word, as it was written.
#[test]
fn req_arch_004_an_unknown_kind_is_answered_internal_naming_the_word() {
    let h = Harness::new(vec![]);
    let id = h.create_with(|c| c.hold = true);
    let request = RequestId::new(&SystemIds);
    let backup = h.persistence.load_snapshots(id).unwrap()[0].snapshot_id;
    for event in [
        Event::WorkspaceSnapshot {
            snapshot_id: backup,
            reason: "backup".into(),
        },
        // As a newer release wrote it: the field holds a word of its own.
        serde_json::from_value(serde_json::json!({
            "event": "workspace.promotion_failed",
            "reason": "the promotion failed (newer)",
            "error_kind": "quota_exceeded",
            "error_detail": {"limit": 3},
        }))
        .unwrap(),
    ] {
        let mut written = h.runtime.event(Actor::Runtime, Some(id), None, None, event);
        written.request_id = Some(request);
        h.persistence.append_event(&written).unwrap();
    }
    let events = h.persistence.events();
    let again = h.runtime.handle(envelope(
        &h,
        request,
        Request::PromoteWorkspace {
            task_id: id,
            plan: one_file_plan(&h, id),
        },
    ));
    let Outcome::AlreadyApplied(already) = again.outcome else {
        panic!("{again:?}")
    };
    let error = already.did.into_reply().unwrap_err();
    assert_eq!(error.kind(), "internal", "{error}");
    let words = error.to_string();
    assert!(words.contains("the promotion failed (newer)"), "{words}");
    assert!(
        words.contains("quota_exceeded"),
        "the word is named: {words}"
    );
    // The detail it was written with is kept, as the word is.
    assert_eq!(error.detail(), Some(serde_json::json!({"limit": 3})));
    // A word this release knows is not named: its message is the reason, as it was.
    let known = RuntimeError::Recorded {
        event: "workspace.promotion_failed".into(),
        reason: "the promotion failed".into(),
        error_kind: Some("conflict".into()),
        error_detail: None,
    };
    assert_eq!(known.to_string(), "the promotion failed");
    // The event keeps the word: nothing was written, and what is stored is what was written.
    assert_eq!(h.persistence.events(), events);
    assert!(events.iter().any(|e| matches!(
        &e.event,
        Event::WorkspacePromotionFailed { error_kind: Some(k), .. } if k == "quota_exceeded"
    )));
}

/// The task cancelled, then deleted by a request of its own: with the audit or without.
fn deleted(h: &Harness, id: crate::ids::TaskId, audit: bool) {
    h.runtime.cancel_task(&h.session, id).unwrap();
    let response = h.runtime.handle(envelope(
        h,
        RequestId::new(&SystemIds),
        Request::DeleteTask { task_id: id, audit },
    ));
    assert!(matches!(response.outcome, Outcome::Done(_)), "{response:?}");
}

/// External design §4, "a request whose task was deleted since": while the task's events are kept
/// (a deletion without `--audit`), the repeat is answered as applied earlier and says that the
/// task is gone since: the grant, which is kept with the task's record, is read back, and what
/// went with the task's data (a snapshot, a promotion) is not there to read back. After
/// `--audit` nothing of the request is known and the answer is `not_found`.
#[test]
fn req_arch_004_a_request_whose_task_was_deleted_since_is_answered_as_applied_while_its_events_stay()
 {
    let h = Harness::new(vec![]);
    let id = h.create_with(|c| c.hold = true);
    let requests = [
        Request::GrantCapability {
            task_id: id,
            kind: GrantKind::Tool {
                name: "write_file".into(),
            },
        },
        Request::RollbackWorkspace {
            task_id: id,
            snapshot_id: None,
        },
        Request::PromoteWorkspace {
            task_id: id,
            plan: one_file_plan(&h, id),
        },
    ];
    let ids = [0; 3].map(|_| RequestId::new(&SystemIds));
    let first: Vec<Outcome> = requests
        .iter()
        .zip(ids)
        .map(|(r, i)| h.runtime.handle(envelope(&h, i, r.clone())).outcome)
        .collect();
    assert!(
        first.iter().all(|o| matches!(o, Outcome::Done(_))),
        "{first:?}"
    );
    // Before the deletion the task is there.
    let Outcome::AlreadyApplied(before) = h
        .runtime
        .handle(envelope(&h, ids[0], requests[0].clone()))
        .outcome
    else {
        panic!("applied")
    };
    assert!(!before.task_deleted);
    deleted(&h, id, false);
    let events = h.persistence.events().len();
    let answers: Vec<AlreadyApplied> = requests
        .iter()
        .zip(ids)
        .map(|(r, i)| {
            let Outcome::AlreadyApplied(already) =
                h.runtime.handle(envelope(&h, i, r.clone())).outcome
            else {
                panic!("a request whose task was deleted since: not `not_found`")
            };
            already
        })
        .collect();
    assert_eq!(h.persistence.events().len(), events, "nothing was written");
    for (answer, name) in
        answers
            .iter()
            .zip(["grant.issued", "workspace.rollback", "workspace.snapshot"])
    {
        assert!(answer.task_deleted, "{name}");
        assert_eq!(answer.event, name);
        assert!(answer.task.is_some(), "the task's record stays");
    }
    // The grant is kept with the record; the snapshot and the promotion went with the data.
    let Outcome::Done(grant) = &first[0] else {
        panic!()
    };
    assert_eq!(
        answers[0].did,
        Did::Applied {
            reply: Some(grant.clone())
        }
    );
    assert_eq!(answers[1].did, Did::Applied { reply: None });
    assert_eq!(answers[2].did, Did::Applied { reply: None });

    // With `--audit` nothing of the task is known: `not_found`, as the task is.
    let other = h.create_with(|c| {
        c.hold = true;
        c.scope.root = "/home/u/other".into();
    });
    let grant = Request::GrantCapability {
        task_id: other,
        kind: GrantKind::Tool {
            name: "write_file".into(),
        },
    };
    let sent = RequestId::new(&SystemIds);
    assert!(matches!(
        h.runtime.handle(envelope(&h, sent, grant.clone())).outcome,
        Outcome::Done(_)
    ));
    deleted(&h, other, true);
    let Outcome::Failed(error) = h.runtime.handle(envelope(&h, sent, grant)).outcome else {
        panic!("after `--audit` nothing of the request is known")
    };
    assert_eq!(error.kind(), "not_found", "{error}");
}

/// External design §4, "a repeated creation": it finds its task while the task's record stays,
/// deleted or not (a default deletion keeps it), and is answered with that task, and makes none.
/// After `delete --audit` the record is gone, and the repeat makes a new task.
#[test]
fn req_arch_004_a_creation_repeated_after_a_deletion_finds_its_task_unless_the_audit_went() {
    let h = Harness::new(vec![]);
    let id = RequestId::new(&SystemIds);
    let Outcome::Done(Reply::Task(made)) = h
        .runtime
        .handle(envelope(&h, id, create_request(true)))
        .outcome
    else {
        panic!("a task")
    };
    deleted(&h, made.task_id, false);
    let tasks = h.runtime.list_tasks().unwrap().len();
    let Outcome::AlreadyApplied(already) = h
        .runtime
        .handle(envelope(&h, id, create_request(true)))
        .outcome
    else {
        panic!("the repeat of a creation whose task was deleted: its task")
    };
    assert_eq!(already.event, "task.created");
    assert_eq!(already.task.unwrap().task_id, made.task_id);
    assert!(already.task_deleted);
    assert_eq!(
        h.runtime.list_tasks().unwrap().len(),
        tasks,
        "none was made"
    );

    // With the audit gone too, the request is not recorded any more: a new task.
    let id = RequestId::new(&SystemIds);
    let Outcome::Done(Reply::Task(first)) = h
        .runtime
        .handle(envelope(&h, id, create_request(true)))
        .outcome
    else {
        panic!("a task")
    };
    deleted(&h, first.task_id, true);
    let Outcome::Done(Reply::Task(second)) = h
        .runtime
        .handle(envelope(&h, id, create_request(true)))
        .outcome
    else {
        panic!("after `--audit` the repeat makes a task")
    };
    assert_ne!(second.task_id, first.task_id);
    assert!(h.runtime.get_task(second.task_id).is_ok());
}
