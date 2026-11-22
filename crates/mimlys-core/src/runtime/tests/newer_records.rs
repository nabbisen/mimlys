//! Every verb of the control API that changes or runs a task refuses a task that holds a word a
//! newer release wrote (REQ-MAIN-006; handoff 004e, item 2), naming the word, and writes nothing.
//! The records are held as text, as a store holds what a release this one does not know wrote.

use super::*;
use crate::grant::GrantKind;
use crate::ids::{GrantId, PromotionId, ScriptedIds, SnapshotId};
use crate::newer::RecordKind;
use crate::snapshot::PromotionPlan;
use serde_json::json;

/// A task held in `created`, and an event of it as a newer release wrote it.
fn held_task(h: &Harness) -> TaskId {
    let task = h
        .runtime
        .create_task(
            &h.session,
            CreateTask {
                hold: true,
                ..create_request()
            },
        )
        .unwrap();
    let first = h
        .persistence
        .events_since(0, Some(task.task_id), 1)
        .unwrap()
        .remove(0);
    let mut record = serde_json::to_value(&first.record).unwrap();
    record["event"] = json!("session.attached");
    record["client"] = json!("tui");
    h.persistence
        .hold_raw(task.task_id, RecordKind::Event, &record.to_string());
    task.task_id
}

/// REQ-MAIN-006: each verb refuses, with the word, and the store was not written to.
#[test]
fn req_main_006_every_verb_that_changes_a_task_refuses_a_newer_word_and_writes_nothing() {
    let h = Harness::new(vec![]);
    let id = held_task(&h);
    let ids = ScriptedIds::starting_at(900);
    let writes = h.persistence.writes();
    let plan = PromotionPlan {
        task_id: id,
        base_snapshot: SnapshotId::new(&ids),
        current_hash: crate::hash::hash_bytes(b"x"),
        changes: vec![],
        excluded: vec![],
        text_diffs: vec![],
    };
    let s = &h.session;
    let refused: Vec<(&str, Result<(), RuntimeError>)> = vec![
        ("resume_task", h.runtime.resume_task(s, id, None).map(drop)),
        ("pause_task", h.runtime.pause_task(s, id).map(drop)),
        ("cancel_task", h.runtime.cancel_task(s, id).map(drop)),
        ("approve_action", h.runtime.approve_action(s, id).map(drop)),
        (
            "reject_action",
            h.runtime.reject_action(s, id, None, None).map(drop),
        ),
        (
            "answer_question",
            h.runtime.answer_question(s, id, "yes").map(drop),
        ),
        (
            "grant_capability",
            h.runtime
                .grant_capability(
                    s,
                    id,
                    GrantKind::Tool {
                        name: "read_file".into(),
                    },
                )
                .map(drop),
        ),
        (
            "revoke_capability",
            h.runtime
                .revoke_capability(s, id, GrantId::new(&ids))
                .map(drop),
        ),
        (
            "rollback_workspace",
            h.runtime.rollback_workspace(s, id, None).map(drop),
        ),
        (
            "promote_workspace",
            h.runtime.promote_workspace(s, id, &plan).map(drop),
        ),
        (
            "revert_promotion",
            h.runtime
                .revert_promotion(s, id, PromotionId::new(&ids), false)
                .map(drop),
        ),
        ("delete_task", h.runtime.delete_task(s, id, false).map(drop)),
        (
            "create_task from it",
            h.runtime
                .create_task(
                    s,
                    CreateTask {
                        hold: true,
                        from_task: Some(id),
                        scope: ScopeSpec {
                            root: "/home/u/another".into(),
                            include: vec![],
                        },
                        ..create_request()
                    },
                )
                .map(drop),
        ),
    ];
    for (verb, result) in refused {
        match result {
            Err(RuntimeError::Persistence(PersistenceError::NewerRecord {
                record, word, ..
            })) => {
                assert_eq!(
                    (record.as_str(), word.as_str()),
                    ("event", "session.attached"),
                    "{verb}"
                );
            }
            other => panic!("{verb} did not refuse: {other:?}"),
        }
    }
    assert_eq!(
        h.persistence.writes(),
        writes,
        "a refused verb writes nothing"
    );
}

/// REQ-MAIN-006: a task that holds nothing of the kind is not refused by it.
#[test]
fn req_main_006_a_task_of_known_words_is_not_refused() {
    let h = Harness::new(vec![]);
    let task = h
        .runtime
        .create_task(
            &h.session,
            CreateTask {
                hold: true,
                ..create_request()
            },
        )
        .unwrap();
    h.runtime.cancel_task(&h.session, task.task_id).unwrap();
}
