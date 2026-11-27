//! The one handler (handoff 014b, item 2; REQ-ARCH-004): a request sent through
//! `Runtime::handle` does what the verb does, leaves the store as the verb leaves it, and fails
//! with the verb's own error. Judged by state: the events and the answers of two runtimes over
//! the same script, one driven by the verbs and one by the requests.

use super::*;
use crate::api::{Envelope, Outcome, Reply, Request, Response};
use crate::ids::{GrantId, PromotionId, RequestId, SnapshotId};
use crate::task::StateError;

/// One request, made by calling the verb the way the command line did.
pub(super) fn by_the_verb(h: &Harness, request: Request) -> Result<Reply, RuntimeError> {
    let s = &h.session;
    let r = &h.runtime;
    Ok(match request {
        Request::CreateTask(create) => Reply::Task(Box::new(r.create_task(s, create)?)),
        Request::ResumeTask { task_id, extend } => {
            Reply::Task(Box::new(r.resume_task(s, task_id, extend)?))
        }
        Request::PauseTask { task_id } => Reply::Task(Box::new(r.pause_task(s, task_id)?)),
        Request::CancelTask { task_id } => Reply::Task(Box::new(r.cancel_task(s, task_id)?)),
        Request::ApproveAction { task_id, note } => {
            Reply::Task(Box::new(r.approve_action(s, task_id, note)?))
        }
        Request::RejectAction {
            task_id,
            note,
            halt,
        } => Reply::Task(Box::new(r.reject_action(s, task_id, note, halt)?)),
        Request::AnswerQuestion { task_id, answer } => {
            Reply::Task(Box::new(r.answer_question(s, task_id, &answer)?))
        }
        Request::GrantCapability { task_id, kind } => {
            Reply::Grant(r.grant_capability(s, task_id, kind)?)
        }
        Request::RevokeCapability { task_id, grant_id } => {
            Reply::Grant(r.revoke_capability(s, task_id, grant_id)?)
        }
        Request::RollbackWorkspace {
            task_id,
            snapshot_id,
        } => Reply::Snapshot(r.rollback_workspace(s, task_id, snapshot_id)?),
        Request::PromoteWorkspace { task_id, plan } => {
            Reply::Promotion(r.promote_workspace(s, task_id, &plan)?)
        }
        Request::RevertPromotion {
            task_id,
            promotion_id,
            overwrite_edited,
        } => Reply::Promotion(r.revert_promotion(s, task_id, promotion_id, overwrite_edited)?),
        Request::DeleteTask { task_id, audit } => {
            Reply::Footprint(r.delete_task(s, task_id, audit)?)
        }
        Request::StopAtBoundary { .. }
        | Request::InterruptStep { .. }
        | Request::StopAnyway { .. }
        | Request::PlanEffort { .. }
        | Request::ModelChange { .. }
        | Request::StartRefusal { .. }
        | Request::CreationForbidden { .. } => unreachable!("not a verb of the scenario"),
    })
}

/// One request, sent to the handler.
fn by_the_handler(h: &Harness, request: Request) -> Result<Reply, RuntimeError> {
    let request_id = RequestId::new(&crate::ids::SystemIds);
    let response = h.runtime.handle(Envelope {
        request_id,
        session: h.session.clone(),
        request,
    });
    assert_eq!(response.request_id, request_id, "the answer carries the id");
    response.into_result()
}

pub(super) fn create_request(hold: bool) -> Request {
    Request::CreateTask(CreateTask {
        goal: "fix the failing test".into(),
        scope: ScopeSpec {
            root: "/home/u/project".into(),
            include: vec![],
        },
        profile: None,
        grants: vec![],
        budgets: None,
        deadline_ms: None,
        completion_check: None,
        hold,
        from_task: None,
        from_snapshot: None,
        plan_first: false,
        effort: None,
    })
}

fn task_of(reply: &Result<Reply, RuntimeError>) -> TaskId {
    match reply {
        Ok(Reply::Task(task)) => task.task_id,
        other => panic!("not a task: {other:?}"),
    }
}

/// Every verb of external design §4, in one life of two tasks: the answers, the errors and the
/// events are the same through the verbs and through the handler, id for id and instant for
/// instant (the ids and the clock are scripted).
pub(super) fn scenario(
    h: &Harness,
    call: &dyn Fn(&Harness, Request) -> Result<Reply, RuntimeError>,
) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut note = |what: &str, reply: &Result<Reply, RuntimeError>| {
        seen.push(format!("{what}: {reply:?}"));
    };
    // A task held, granted, revoked, resumed, run to its approval, approved, run to its
    // question, answered, run to its end.
    let created = call(h, create_request(true));
    note("create (held)", &created);
    let id = task_of(&created);
    let granted = call(
        h,
        Request::GrantCapability {
            task_id: id,
            kind: GrantKind::Tool {
                name: "write_file".into(),
            },
        },
    );
    note("grant", &granted);
    let Ok(Reply::Grant(grant)) = &granted else {
        panic!("{granted:?}")
    };
    let revoked = call(
        h,
        Request::RevokeCapability {
            task_id: id,
            grant_id: grant.grant_id,
        },
    );
    note("revoke", &revoked);
    note(
        "revoke again",
        &call(
            h,
            Request::RevokeCapability {
                task_id: id,
                grant_id: grant.grant_id,
            },
        ),
    );
    note(
        "resume",
        &call(
            h,
            Request::ResumeTask {
                task_id: id,
                extend: None,
            },
        ),
    );
    note(
        "resume a running task",
        &call(
            h,
            Request::ResumeTask {
                task_id: id,
                extend: None,
            },
        ),
    );
    h.step(id);
    note(
        "approve",
        &call(
            h,
            Request::ApproveAction {
                task_id: id,
                note: Some("fine".into()),
            },
        ),
    );
    h.step(id);
    h.step(id);
    note(
        "answer",
        &call(
            h,
            Request::AnswerQuestion {
                task_id: id,
                answer: "main".into(),
            },
        ),
    );
    h.step(id);
    // The end: a snapshot restored, a promotion applied and reversed, the task deleted.
    note(
        "rollback",
        &call(
            h,
            Request::RollbackWorkspace {
                task_id: id,
                snapshot_id: None,
            },
        ),
    );
    let plan = one_file_plan(h, id);
    let promoted = call(
        h,
        Request::PromoteWorkspace {
            task_id: id,
            plan: plan.clone(),
        },
    );
    note("promote", &promoted);
    let Ok(Reply::Promotion(promotion)) = &promoted else {
        panic!("{promoted:?}")
    };
    note(
        "revert",
        &call(
            h,
            Request::RevertPromotion {
                task_id: id,
                promotion_id: promotion.promotion_id,
                overwrite_edited: false,
            },
        ),
    );
    note(
        "revert again",
        &call(
            h,
            Request::RevertPromotion {
                task_id: id,
                promotion_id: promotion.promotion_id,
                overwrite_edited: false,
            },
        ),
    );
    note(
        "delete",
        &call(
            h,
            Request::DeleteTask {
                task_id: id,
                audit: false,
            },
        ),
    );
    // A second task: paused at its boundary, rejected, cancelled.
    let second = call(h, create_request(false));
    note("create (running)", &second);
    let other = task_of(&second);
    note("pause", &call(h, Request::PauseTask { task_id: other }));
    note(
        "reject",
        &call(
            h,
            Request::RejectAction {
                task_id: other,
                note: None,
                halt: Some(true),
            },
        ),
    );
    note("cancel", &call(h, Request::CancelTask { task_id: other }));
    note(
        "cancel again",
        &call(h, Request::CancelTask { task_id: other }),
    );
    // Unknown things.
    note(
        "grant on an unknown task",
        &call(
            h,
            Request::GrantCapability {
                task_id: TaskId::from_uuid(uuid::Uuid::from_u128(7)),
                kind: GrantKind::Tool {
                    name: "read_file".into(),
                },
            },
        ),
    );
    note(
        "revoke an unknown grant",
        &call(
            h,
            Request::RevokeCapability {
                task_id: other,
                grant_id: GrantId::from_uuid(uuid::Uuid::from_u128(8)),
            },
        ),
    );
    note(
        "rollback to an unknown snapshot",
        &call(
            h,
            Request::RollbackWorkspace {
                task_id: other,
                snapshot_id: Some(SnapshotId::from_uuid(uuid::Uuid::from_u128(9))),
            },
        ),
    );
    note(
        "revert an unknown promotion",
        &call(
            h,
            Request::RevertPromotion {
                task_id: other,
                promotion_id: PromotionId::from_uuid(uuid::Uuid::from_u128(10)),
                overwrite_edited: false,
            },
        ),
    );
    seen
}

pub(super) fn scenario_harness() -> Harness {
    let h = Harness::with_policy(
        vec![
            act("write_file", "src/lib.rs"),
            json!({"decision": "ask_user", "question": "Which branch?"}),
            json!({"decision": "complete", "summary": "done"}),
        ],
        approval_policy(),
    );
    h.tools.script(
        "write_file",
        ExecutionOutcome::succeeded(json!({"bytes": 3}), 2),
    );
    h
}

/// REQ-ARCH-004: the handler is the verbs: a life of two tasks gives the same answers, the same
/// errors and, in the store, the same events whether the verbs or the requests are used.
#[test]
fn req_arch_004_the_handler_does_what_the_verb_does_and_leaves_the_store_as_it_leaves_it() {
    let direct = scenario_harness();
    let handled = scenario_harness();
    let said_direct = scenario(&direct, &by_the_verb);
    let said_handled = scenario(&handled, &by_the_handler);
    assert_eq!(said_handled, said_direct);
    // The same events, but for the id of the request that wrote each (a verb called directly has
    // none; the ids themselves are judged by `request_ids`).
    let bare = |h: &Harness| -> Vec<crate::event::EventRecord> {
        h.persistence
            .events()
            .into_iter()
            .map(|mut e| {
                e.request_id = None;
                e
            })
            .collect()
    };
    assert_eq!(bare(&handled), bare(&direct));
    // The scenario is not empty: it reached the states it names.
    let all = said_handled.join("\n");
    for needle in [
        "create (held): Ok(Task",
        "grant: Ok(Grant",
        "approve: Ok(Task",
        "answer: Ok(Task",
        "promote: Ok(Promotion",
        "delete: Ok(Footprint",
        "cancel: Ok(Task",
    ] {
        assert!(all.contains(needle), "{needle} in\n{all}");
    }
    assert!(
        said_handled
            .iter()
            .any(|l| l.starts_with("cancel again: Err(State(")),
        "a verb's refusal is the response's error: {said_handled:?}"
    );
}

/// External design §18: an error that crossed the response is the error that the verb gave,
/// down to its kind and its words.
#[test]
fn req_arch_004_a_refused_request_fails_with_the_verbs_own_error() {
    let h = Harness::new(vec![]);
    let id = h.create();
    h.runtime.cancel_task(&h.session, id).unwrap();
    let response = h.runtime.handle(Envelope {
        request_id: RequestId::new(&crate::ids::SystemIds),
        session: h.session.clone(),
        request: Request::ResumeTask {
            task_id: id,
            extend: None,
        },
    });
    let Outcome::Failed(error) = &response.outcome else {
        panic!("{response:?}");
    };
    assert_eq!(
        error,
        &h.runtime.resume_task(&h.session, id, None).unwrap_err()
    );
    assert_eq!(error.kind(), "state");
    assert!(matches!(
        error,
        RuntimeError::State(StateError::Illegal { .. })
    ));
    // And the same after the round trip a transport makes.
    let sent: Response = serde_json::from_str(&serde_json::to_string(&response).unwrap()).unwrap();
    assert_eq!(sent, response);
}

/// A response made by the handler is read back as it was made (REQ-ARCH-004).
#[test]
fn req_arch_004_a_reply_of_every_kind_is_read_back_as_it_was_made() {
    let h = scenario_harness();
    let mut replies = Vec::new();
    let mut keep = |reply: Result<Reply, RuntimeError>| replies.push(reply.unwrap());
    keep(by_the_handler(&h, create_request(true)));
    let id = h.runtime.list_tasks().unwrap()[0].task_id;
    keep(by_the_handler(
        &h,
        Request::GrantCapability {
            task_id: id,
            kind: GrantKind::Tool {
                name: "write_file".into(),
            },
        },
    ));
    keep(by_the_handler(
        &h,
        Request::RollbackWorkspace {
            task_id: id,
            snapshot_id: None,
        },
    ));
    h.runtime.cancel_task(&h.session, id).unwrap();
    let plan = one_file_plan(&h, id);
    keep(by_the_handler(
        &h,
        Request::PromoteWorkspace { task_id: id, plan },
    ));
    keep(by_the_handler(
        &h,
        Request::DeleteTask {
            task_id: id,
            audit: false,
        },
    ));
    keep(by_the_handler(
        &h,
        Request::PlanEffort {
            profile: None,
            effort: None,
            from_task: None,
        },
    ));
    keep(by_the_handler(
        &h,
        Request::CreationForbidden {
            profile: None,
            backend: "default".into(),
        },
    ));
    assert_eq!(replies.len(), 7);
    for reply in replies {
        let text = serde_json::to_string(&reply).unwrap();
        assert_eq!(
            serde_json::from_str::<Reply>(&text).unwrap(),
            reply,
            "{text}"
        );
    }
}

/// The asks answer what the methods answer, and change nothing.
#[test]
fn req_arch_004_an_ask_answers_as_the_method_does_and_writes_nothing() {
    let h = Harness::new(vec![]);
    let id = h.create();
    let before = h.persistence.events().len();
    let writes = h.persistence.writes().len();
    let profile = h.runtime.policy().profile("workspace").unwrap().clone();
    let planned = h
        .runtime
        .plan_effort(&profile, Some(crate::effort::EffortLevel::Quick), None)
        .unwrap();
    assert_eq!(
        by_the_handler(
            &h,
            Request::PlanEffort {
                profile: Some("workspace".into()),
                effort: Some(crate::effort::EffortLevel::Quick),
                from_task: None,
            }
        )
        .unwrap(),
        Reply::EffortPlan(planned)
    );
    let task = h.runtime.get_task(id).unwrap().task;
    assert_eq!(
        by_the_handler(&h, Request::ModelChange { task_id: id }).unwrap(),
        Reply::ModelChange(h.runtime.model_change(&task).unwrap())
    );
    assert_eq!(
        by_the_handler(&h, Request::StartRefusal { task_id: id }).unwrap(),
        Reply::StartRefusal(h.runtime.start_refusal(&task).unwrap())
    );
    assert_eq!(
        by_the_handler(
            &h,
            Request::CreationForbidden {
                profile: None,
                backend: "default".into(),
            }
        )
        .unwrap(),
        Reply::CreationForbidden(h.runtime.creation_forbidden(None, "default").unwrap())
    );
    // A profile the policy does not have is an invalid request, as the creation says.
    let refused = by_the_handler(
        &h,
        Request::PlanEffort {
            profile: Some("nonesuch".into()),
            effort: None,
            from_task: None,
        },
    )
    .unwrap_err();
    assert_eq!(refused.kind(), "invalid_request", "{refused}");
    assert_eq!(h.persistence.events().len(), before, "no event");
    assert_eq!(h.persistence.writes().len(), writes, "no write");
}
