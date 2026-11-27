//! The requests and responses as data (REQ-ARCH-004; external design §4; handoff 014b, item 1):
//! what a client sends and what it gets back is exactly what was made, after any number of
//! round trips through JSON, and every error means the same after one.

use super::*;
use crate::effort::EffortLevel;
use crate::grant::GrantKind;
use crate::ids::{IdSource, ScriptedIds};
use crate::spi::{BackendError, PersistenceError, WorkspaceError};
use crate::step::Budgets;
use crate::task::{CompletionCheck, ScopeSpec, StateError};

fn ids() -> ScriptedIds {
    ScriptedIds::default()
}

fn create_everything() -> CreateTask {
    let ids = ids();
    CreateTask {
        goal: "fix the failing test".into(),
        scope: ScopeSpec {
            root: "/project".into(),
            include: vec!["src/**".into()],
        },
        profile: Some("workspace".into()),
        grants: vec![GrantKind::Tool {
            name: "read_file".into(),
        }],
        budgets: Some(Budgets {
            steps: 7,
            consecutive_rejections: 2,
            consecutive_failures: 3,
            identical_decisions: 4,
            executions_per_action: 5,
        }),
        deadline_ms: Some(60_000),
        completion_check: Some(CompletionCheck {
            tool: "run_check".into(),
            arguments: serde_json::json!({"name": "unit"})
                .as_object()
                .unwrap()
                .clone(),
        }),
        hold: true,
        from_task: Some(TaskId::new(&ids)),
        from_snapshot: Some(SnapshotId::new(&ids)),
        plan_first: true,
        effort: Some(EffortLevel::Deep),
    }
}

use crate::ids::{SnapshotId, TaskId};

/// One request of every kind, with every optional argument present.
fn every_request() -> Vec<Request> {
    let ids = ids();
    let task_id = TaskId::new(&ids);
    let plan = crate::snapshot::PromotionPlan {
        task_id,
        base_snapshot: SnapshotId::new(&ids),
        current_hash: crate::hash::hash_bytes(b"tree"),
        changes: vec![crate::snapshot::Change {
            path: "a.txt".into(),
            kind: crate::snapshot::ChangeKind::Added,
            bytes: 1,
        }],
        excluded: vec!["target".into()],
        text_diffs: vec![("a.txt".into(), "+x".into())],
    };
    vec![
        Request::CreateTask(create_everything()),
        Request::ResumeTask {
            task_id,
            extend: Some(Extension {
                steps: Some(90),
                deadline: None,
            }),
        },
        Request::PauseTask { task_id },
        Request::CancelTask { task_id },
        Request::ApproveAction {
            task_id,
            note: Some("fine".into()),
        },
        Request::RejectAction {
            task_id,
            note: Some("no".into()),
            halt: Some(true),
        },
        Request::AnswerQuestion {
            task_id,
            answer: "main".into(),
        },
        Request::GrantCapability {
            task_id,
            kind: GrantKind::Tool {
                name: "write_file".into(),
            },
        },
        Request::RevokeCapability {
            task_id,
            grant_id: crate::ids::GrantId::new(&ids),
        },
        Request::RollbackWorkspace {
            task_id,
            snapshot_id: Some(SnapshotId::new(&ids)),
        },
        Request::PromoteWorkspace { task_id, plan },
        Request::RevertPromotion {
            task_id,
            promotion_id: crate::ids::PromotionId::new(&ids),
            overwrite_edited: true,
        },
        Request::DeleteTask {
            task_id,
            audit: true,
        },
        Request::StopAtBoundary { task_id },
        Request::InterruptStep { task_id },
        Request::StopAnyway { task_id },
        Request::PlanEffort {
            profile: Some("workspace".into()),
            effort: Some(EffortLevel::Quick),
            from_task: Some(task_id),
        },
        Request::ModelChange { task_id },
        Request::StartRefusal { task_id },
        Request::CreationForbidden {
            profile: None,
            backend: "default".into(),
        },
    ]
}

/// REQ-ARCH-004: the control API is a set of serializable requests: each is read back as it was
/// sent, and the envelope with it.
#[test]
fn req_arch_004_every_request_is_read_back_as_it_was_sent() {
    let ids = ids();
    for request in every_request() {
        let envelope = Envelope {
            request_id: RequestId::new(&ids),
            session: Session {
                id: "pid:4242".into(),
                os_user: "tester".into(),
            },
            request,
        };
        let text = serde_json::to_string(&envelope).unwrap();
        let back: Envelope = serde_json::from_str(&text).unwrap();
        assert_eq!(back, envelope, "{text}");
    }
}

/// The names are the verbs of external design §4, once each, and exactly the asks change nothing.
#[test]
fn req_arch_004_the_requests_are_named_as_the_verbs_and_only_the_asks_change_nothing() {
    let requests = every_request();
    let names: Vec<&str> = requests.iter().map(Request::name).collect();
    assert_eq!(
        names,
        [
            "create_task",
            "resume_task",
            "pause_task",
            "cancel_task",
            "approve_action",
            "reject_action",
            "answer_question",
            "grant_capability",
            "revoke_capability",
            "rollback_workspace",
            "promote_workspace",
            "revert_promotion",
            "delete_task",
            "stop_at_boundary",
            "interrupt_step",
            "stop_anyway",
            "plan_effort",
            "model_change",
            "start_refusal",
            "creation_forbidden",
        ]
    );
    // The tag of the JSON is the same word.
    for request in &requests {
        let value = serde_json::to_value(request).unwrap();
        assert_eq!(value["request"], request.name(), "{value}");
    }
    let changing: Vec<&str> = requests
        .iter()
        .filter(|r| r.changes_state())
        .map(Request::name)
        .collect();
    assert_eq!(
        changing.len(),
        15,
        "the thirteen verbs and the two signals that may write: {changing:?}"
    );
    assert!(!changing.contains(&"plan_effort"));
    assert!(!changing.contains(&"interrupt_step"));
}

/// The task a request is about is the one it names, and a creation, a plan of the level and a
/// check of the profile name none.
#[test]
fn req_arch_004_a_request_names_its_task() {
    for request in every_request() {
        let value = serde_json::to_value(&request).unwrap();
        let named = value.get("task_id").and_then(|v| v.as_str());
        assert_eq!(
            request.task_id().map(|t| t.to_string()).as_deref(),
            named,
            "{}",
            request.name()
        );
    }
}

/// External design §18: every error of the runtime, and every detail, is read back as it was,
/// so that the same error means the same through any transport.
#[test]
fn req_arch_004_every_error_means_the_same_after_a_round_trip() {
    let ids = ids();
    let errors = vec![
        RuntimeError::InvalidRequest {
            detail: "goal must be 1 to 4000 characters".into(),
        },
        RuntimeError::NotFound {
            what: "snapshot".into(),
        },
        RuntimeError::State(StateError::Illegal {
            from: "running".into(),
            input: "resume".into(),
        }),
        RuntimeError::State(StateError::Extension {
            reason: "the step limit can only be raised".into(),
        }),
        RuntimeError::State(StateError::DeadlinePassed),
        RuntimeError::State(StateError::StepBudgetExhausted),
        RuntimeError::Denied {
            detail: "steps 90 exceed the ceiling 50".into(),
        },
        RuntimeError::Conflict {
            detail: "task is active on an overlapping scope".into(),
        },
        RuntimeError::Unavailable {
            component: "tool_provider".into(),
        },
        RuntimeError::Persistence(PersistenceError::Locked),
        RuntimeError::Persistence(PersistenceError::NewerRecord {
            record: "task".into(),
            field: "state".into(),
            word: "sleeping".into(),
        }),
        RuntimeError::Workspace(WorkspaceError::Conflict {
            paths: vec!["a.txt".into(), "b.txt".into()],
        }),
        RuntimeError::Workspace(WorkspaceError::NotRemoved {
            task_id: TaskId::new(&ids),
            paths: vec!["workspaces/x".into()],
        }),
        RuntimeError::Workspace(WorkspaceError::ReadFailed {
            path: "src/x".into(),
            kind: "entity not found".into(),
        }),
        RuntimeError::PromotionFailed {
            cause: WorkspaceError::Unlisted {
                paths: vec!["dir/x".into()],
            },
            kept: vec!["a.txt".into()],
            restore_error: Some("disk full".into()),
            restore_hint: "mimlys rollback t --promotion p".into(),
        },
        RuntimeError::Backend(BackendError::Timeout),
        RuntimeError::Backend(BackendError::Unavailable {
            detail: "connection refused".into(),
        }),
    ];
    for error in errors {
        let response = Response {
            request_id: RequestId::new(&ids),
            outcome: Outcome::Failed(error.clone()),
        };
        let text = serde_json::to_string(&response).unwrap();
        let back: Response = serde_json::from_str(&text).unwrap();
        assert_eq!(back, response, "{text}");
        let read = back.into_result().unwrap_err();
        assert_eq!(read, error);
        // The words the user reads, and the kind a script reads, are the same.
        assert_eq!(read.to_string(), error.to_string());
        assert_eq!(read.kind(), error.kind());
    }
}

/// A response answers the id that was sent.
#[test]
fn req_arch_004_a_response_keeps_the_id_of_its_request() {
    let id = RequestId::new(&ids());
    let response = Response {
        request_id: id,
        outcome: Outcome::Done(Reply::CreationForbidden(true)),
    };
    let back: Response = serde_json::from_str(&serde_json::to_string(&response).unwrap()).unwrap();
    assert_eq!(back.request_id, id);
    assert_eq!(back.into_result().unwrap(), Reply::CreationForbidden(true));
    // Ids are the ids' own kind.
    assert!(id.is_v7());
    assert_eq!(ids().next().get_version_num(), 7);
}

/// REQ-ARCH-004 (external design §4; review of 014b-6, item 4 of 014b-7): a request that took
/// effect is never an error, through any path. What it did is read as it is: a reply when it has
/// one, none when it has none, and the failure of a change that wrote and failed (the only error
/// a repeat can be) with the kind and the detail it had. A caller that wants a `Result` of a repeat
/// with no reply is not given one: the test helper stops, and the outcome stays apart.
#[test]
fn req_arch_004_a_request_that_took_effect_is_never_an_error() {
    let ids = ids();
    for reply in [None, Some(Reply::CreationForbidden(true))] {
        let did = Did::Applied {
            reply: reply.clone(),
        };
        assert_eq!(did.into_reply(), Ok(reply));
    }
    let failed = Did::Failed {
        event: "workspace.promotion_failed".into(),
        reason: "the promotion failed".into(),
        error_kind: Some("conflict".into()),
        error_detail: Some(serde_json::json!({"paths": ["a.txt"]})),
    };
    let error = failed.into_reply().unwrap_err();
    assert_eq!(error.kind(), "conflict");
    assert_eq!(
        error.detail(),
        Some(serde_json::json!({"paths": ["a.txt"]}))
    );

    let already = AlreadyApplied {
        position: 7,
        event: "workspace.snapshot".into(),
        task: None,
        task_deleted: true,
        did: Did::Applied { reply: None },
    };
    let response = Response {
        request_id: RequestId::new(&ids),
        outcome: Outcome::AlreadyApplied(already.clone()),
    };
    // The outcome keeps it apart, whole.
    assert_eq!(response.clone().into_outcome(), Ok(Err(already)));
    // And the helper does not turn it into an error: it stops instead.
    let folded = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| response.into_result()));
    assert!(
        folded.is_err(),
        "an applied request is not an `Err`: {folded:?}"
    );
}
