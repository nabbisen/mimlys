//! REQ-MAIN-006: a record that holds a word a newer release wrote is held as written, with the
//! word and the field; a damaged record is not that; a known record is read as it always was.

use super::*;
use crate::clock::{Clock, FakeClock};
use crate::event::{Actor, Event, EventRecord};
use crate::ids::{EventId, QuestionId, ScriptedIds, TaskId};
use crate::limits::LIM_T5_DEADLINE_MS;
use crate::step::Budgets;
use crate::task::{ScopeSpec, Task, TaskSpec, TaskState, WaitKind, test_versions};
use serde_json::{Value, json};

fn task() -> Task {
    let clock = FakeClock::default();
    Task::create(
        TaskSpec {
            task_id: TaskId::new(&ScriptedIds::default()),
            goal: "fix the failing test".into(),
            deadline_ms: LIM_T5_DEADLINE_MS,
            budgets: Budgets::default(),
            profile: "workspace".into(),
            versions: test_versions(),
            scope: ScopeSpec {
                root: "/project".into(),
                include: vec![],
            },
        },
        clock.now(),
    )
    .unwrap()
}

fn event(event: Event) -> EventRecord {
    let ids = ScriptedIds::default();
    EventRecord {
        event_id: EventId::new(&ids),
        at: FakeClock::default().now(),
        actor: Actor::Runtime,
        task_id: Some(TaskId::new(&ids)),
        step_id: None,
        action_id: None,
        policy_hash: crate::hash::hash_bytes(b"policy"),
        event,
    }
}

fn json_of<T: serde::Serialize>(value: &T) -> Value {
    serde_json::to_value(value).unwrap()
}

fn read<T: serde::de::DeserializeOwned>(value: &Value) -> Result<Read<T>, PersistenceError> {
    Read::parse(&value.to_string())
}

fn newer_of<T: serde::de::DeserializeOwned>(value: &Value) -> Newer {
    match read::<T>(value).unwrap_or_else(|e| panic!("{e:?} for {value}")) {
        Read::Newer(newer) => newer,
        Read::Known(_) => panic!("the record reads whole: {value}"),
    }
}

/// A task as a newer release would write it, with `change` applied to the record of this one.
fn written_by_a_newer_release(change: impl FnOnce(&mut Value)) -> Value {
    let mut value = json_of(&task());
    change(&mut value);
    value
}

fn words(newer: &Newer) -> Vec<(&str, &str)> {
    newer
        .words
        .iter()
        .map(|w| (w.field.as_str(), w.word.as_str()))
        .collect()
}

/// REQ-MAIN-006: every known record reads as the typed value it was, and serializes as it did:
/// the marker key is on no record that holds no unknown word.
#[test]
fn req_main_006_a_record_of_known_words_is_the_typed_value() {
    let task = task();
    let text = serde_json::to_string(&task).unwrap();
    match Read::<Task>::parse(&text).unwrap() {
        Read::Known(read) => assert_eq!(read, task),
        Read::Newer(newer) => panic!("a known task is newer: {newer:?}"),
    }
    let known = Read::Known(task.clone());
    assert_eq!(json_of(&known), json_of(&task));
    assert!(json_of(&known).get(MARKER_FIELD).is_none());
}

/// REQ-MAIN-006: a wait kind this release has not got (0.13's `plan`) is held by its name, in
/// the field that holds it, with the record as written.
#[test]
fn req_main_006_an_unknown_wait_kind_is_held_by_its_name() {
    let value = written_by_a_newer_release(|v| {
        v["state"] = json!("waiting_user");
        v["wait_kind"] = json!("plan");
    });
    let newer = newer_of::<Task>(&value);
    assert_eq!(words(&newer), vec![("wait_kind", "plan")]);
    assert_eq!(newer.raw, value, "the record as written");
    assert_eq!(newer.text("goal"), Some("fix the failing test"));
    // The runtime never sees it as a state it knows.
    assert!(matches!(
        parse_strict::<Task>(&value.to_string()),
        Err(PersistenceError::NewerRecord { record, field, word })
            if record == "task" && field == "wait_kind" && word == "plan"
    ));
}

/// REQ-MAIN-006: a state, a halt reason, a failure reason and a pending item of a newer release
/// are each held by their name.
#[test]
fn req_main_006_an_unknown_state_or_reason_is_held_by_its_name() {
    let state = written_by_a_newer_release(|v| v["state"] = json!("quarantined"));
    assert_eq!(
        words(&newer_of::<Task>(&state)),
        vec![("state", "quarantined")]
    );

    let halted = written_by_a_newer_release(|v| {
        v["state"] = json!("halted");
        v["halt_reason"] = json!("plan_required");
    });
    assert_eq!(
        words(&newer_of::<Task>(&halted)),
        vec![("halt_reason", "plan_required")]
    );

    let failed = written_by_a_newer_release(|v| {
        v["state"] = json!("failed");
        v["reason"] = json!({ "failure_reason": "daemon_lost" });
    });
    assert_eq!(
        words(&newer_of::<Task>(&failed)),
        vec![("reason.failure_reason", "daemon_lost")]
    );

    let pending = written_by_a_newer_release(|v| {
        v["state"] = json!("waiting_user");
        v["wait_kind"] = json!("approval");
        v["pending"] = json!({ "pending": "plan", "plan_id": "x" });
    });
    assert_eq!(
        words(&newer_of::<Task>(&pending)),
        vec![("pending.pending", "plan")]
    );
}

/// REQ-MAIN-006: two unknown words of one record are both named, when the reading can go on past
/// the first.
#[test]
fn req_main_006_two_unknown_words_of_a_record_are_both_named() {
    let ids = ScriptedIds::default();
    let step = crate::step::Step {
        step_id: crate::ids::StepId::new(&ids),
        task_id: TaskId::new(&ids),
        index: 0,
        phase: crate::step::StepPhase::Decide,
        decision_hash: None,
        rejection: None,
        rejection_detail: None,
        rollback_failed: false,
        action_id: None,
        observation_id: None,
        outcome: None,
        started_at: FakeClock::default().now(),
        finished_at: None,
        measures: None,
    };
    let mut value = json_of(&step);
    value["phase"] = json!("planning_ahead");
    value["outcome"] = json!("deferred");
    let newer = newer_of::<crate::step::Step>(&value);
    assert!(newer.is_unknown("phase"), "{newer:?}");
    assert!(newer.is_unknown("outcome"), "{newer:?}");
    assert!(newer.is_unknown_in("phase"));
    assert!(!newer.is_unknown("index"));
}

/// REQ-MAIN-006: an event that this release has not got is held by its name and its fields; a
/// known event with a word it has not got (a rejection code) is held by that.
#[test]
fn req_main_006_an_unknown_event_is_held_by_its_name_and_fields() {
    let mut value = json_of(&event(Event::TaskCreated));
    value["event"] = json!("session.attached");
    value["client"] = json!("tui");
    let newer = newer_of::<EventRecord>(&value);
    assert_eq!(words(&newer), vec![("event", "session.attached")]);
    assert_eq!(newer.text("client"), Some("tui"));

    let mut rejected = json_of(&event(Event::DecisionRejected {
        code: crate::rejection::RejectionCode::Malformed,
        reason: None,
        grant_kind: None,
    }));
    rejected["code"] = json!("plan_required");
    let newer = newer_of::<EventRecord>(&rejected);
    assert_eq!(words(&newer), vec![("code", "plan_required")]);
    assert_eq!(newer.text("event"), Some("decision.rejected"));
}

/// REQ-MAIN-006: a free string that happens to be the same text as the unknown word is not
/// mistaken for it: the place named is the vocabulary's.
#[test]
fn req_main_006_the_place_named_is_the_vocabularys_not_a_free_string() {
    let mut rejected = json_of(&event(Event::DecisionRejected {
        code: crate::rejection::RejectionCode::Malformed,
        reason: Some("plan_required".into()),
        grant_kind: None,
    }));
    rejected["code"] = json!("plan_required");
    let newer = newer_of::<EventRecord>(&rejected);
    assert_eq!(words(&newer), vec![("code", "plan_required")]);
}

/// The precondition of the test below, and what it asks: the record holds `word` in the free
/// place `free` and in the vocabulary's place `field`; the free one is met FIRST (the maps are
/// sorted by key, so a free string whose key sorts before the vocabulary's is the place that a
/// reading which takes the first place that holds the word would name); and the place named is
/// the vocabulary's, and only that.
fn names_the_vocabulary_not_the_free_string<T: serde::de::DeserializeOwned>(
    value: &Value,
    free: &str,
    field: &str,
    word: &str,
) {
    let mut paths = Vec::new();
    string_paths(value, word, &mut Vec::new(), &mut paths);
    let places: Vec<String> = paths.iter().map(|p| field_of(p)).collect();
    assert_eq!(
        places.first().map(String::as_str),
        Some(free),
        "the free string must be met first, or the test does not tell the places apart: {places:?}"
    );
    assert!(places.iter().any(|p| p == field), "{places:?}");
    assert_eq!(
        words(&newer_of::<T>(value)),
        vec![(field, word)],
        "the place named is the vocabulary's (not `{free}`) in {value}"
    );
}

/// REQ-MAIN-006: the same, where the free string sorts BEFORE the vocabulary's field (the maps of
/// a record are sorted, so a test whose vocabulary field comes first passes by that order alone:
/// the review of 004e found it). Each record is one a newer release could write, with a free
/// string that happens to be the unknown word; each case is a test of its own, so that each is
/// shown to fail alone. A task: the goal (free) before the state.
#[test]
fn req_main_006_a_free_string_before_the_state_of_a_task_is_not_the_place_named() {
    let task_state = written_by_a_newer_release(|v| {
        v["goal"] = json!("quarantined");
        v["state"] = json!("quarantined");
    });
    names_the_vocabulary_not_the_free_string::<Task>(&task_state, "goal", "state", "quarantined");
}

/// REQ-MAIN-006: the goal of a task (free) before the kind of its wait.
#[test]
fn req_main_006_a_free_string_before_the_wait_kind_of_a_task_is_not_the_place_named() {
    let task_wait = written_by_a_newer_release(|v| {
        v["goal"] = json!("plan");
        v["state"] = json!("waiting_user");
        v["wait_kind"] = json!("plan");
    });
    names_the_vocabulary_not_the_free_string::<Task>(&task_wait, "goal", "wait_kind", "plan");
}

/// REQ-MAIN-006: `task.admission_changed`: the reason (free) before the state.
#[test]
fn req_main_006_a_free_string_before_the_state_of_an_admission_is_not_the_place_named() {
    let mut admission = json_of(&event(Event::TaskAdmissionChanged {
        state: crate::event::AdmissionState::Admitted,
        slots_taken: None,
        reason: Some("queued_by_daemon".into()),
        waited_ms: None,
    }));
    admission["state"] = json!("queued_by_daemon");
    names_the_vocabulary_not_the_free_string::<EventRecord>(
        &admission,
        "reason",
        "state",
        "queued_by_daemon",
    );
}

/// REQ-MAIN-006: `task.state_changed`: `from` (free) before the halt reason.
#[test]
fn req_main_006_a_free_string_before_the_halt_reason_of_a_change_is_not_the_place_named() {
    let mut changed = json_of(&event(Event::TaskStateChanged {
        from: "plan_required".into(),
        to: "halted".into(),
        reason: Some(crate::task::HaltReason::Policy),
        rollback_failed: false,
        halt_detail: None,
    }));
    changed["reason"] = json!("plan_required");
    names_the_vocabulary_not_the_free_string::<EventRecord>(
        &changed,
        "from",
        "reason",
        "plan_required",
    );
}

/// REQ-MAIN-006: an event of a newer release: a field of its own (free) before its name.
#[test]
fn req_main_006_a_free_string_before_the_name_of_an_event_is_not_the_place_named() {
    let mut unknown_event = json_of(&event(Event::TaskCreated));
    unknown_event["event"] = json!("session.attached");
    unknown_event["client"] = json!("session.attached");
    names_the_vocabulary_not_the_free_string::<EventRecord>(
        &unknown_event,
        "client",
        "event",
        "session.attached",
    );
}

/// REQ-MAIN-006: in a list: the path of the first change (free) before the kind of the second.
#[test]
fn req_main_006_a_free_string_earlier_in_a_list_is_not_the_place_named() {
    let ids = ScriptedIds::default();
    let change = |path: &str| crate::snapshot::Change {
        path: path.into(),
        kind: crate::snapshot::ChangeKind::Added,
        bytes: 1,
    };
    let mut promotion = json_of(&crate::snapshot::Promotion {
        promotion_id: crate::ids::PromotionId::new(&ids),
        task_id: TaskId::new(&ids),
        backup_snapshot: crate::ids::SnapshotId::new(&ids),
        project_root: "/project".into(),
        files: vec![change("renamed"), change("b.txt")],
        applied_at: FakeClock::default().now(),
        reversed_at: None,
        state: crate::snapshot::PromotionState::Applied,
        after: vec![],
        reason: None,
        restored: true,
    });
    promotion["files"][1]["kind"] = json!("renamed");
    names_the_vocabulary_not_the_free_string::<crate::snapshot::Promotion>(
        &promotion,
        "files[0].path",
        "files[1].kind",
        "renamed",
    );
}

/// REQ-MAIN-006: a record that is damaged, rather than newer, is the failure it always was.
#[test]
fn req_main_006_a_damaged_record_is_not_a_newer_one() {
    let mut value = json_of(&task());
    value.as_object_mut().unwrap().remove("goal");
    assert!(matches!(
        read::<Task>(&value),
        Err(PersistenceError::Storage { detail }) if detail == "deserialize"
    ));
    assert!(matches!(
        Read::<Task>::parse("not json"),
        Err(PersistenceError::Storage { .. })
    ));
}

/// REQ-MAIN-006 (the one JSON rule): the record as written, with the key that lists each unknown
/// word and its field; nothing else changes.
#[test]
fn req_main_006_json_keeps_the_record_and_adds_the_marker() {
    let value = written_by_a_newer_release(|v| {
        v["state"] = json!("waiting_user");
        v["wait_kind"] = json!("plan");
    });
    let newer = newer_of::<Task>(&value);
    let shown = json_of(&Read::<Task>::Newer(newer));
    let mut expected = value.clone();
    expected[MARKER_FIELD] = json!([{ "field": "wait_kind", "word": "plan" }]);
    assert_eq!(shown, expected);
}

/// REQ-MAIN-006: the typed value of a known word is unchanged: `waiting_user` is a state with a
/// kind, and a record without one is damaged, never a `waiting_user` of no kind.
#[test]
fn req_main_006_a_state_is_never_read_without_its_kind() {
    let value = written_by_a_newer_release(|v| v["state"] = json!("waiting_user"));
    assert!(matches!(
        read::<Task>(&value),
        Err(PersistenceError::Storage { .. })
    ));
    let known = written_by_a_newer_release(|v| {
        v["state"] = json!("waiting_user");
        v["wait_kind"] = json!("question");
        v["pending"] = json!({
            "pending": "question",
            "question_id": QuestionId::new(&ScriptedIds::default()),
        });
    });
    match read::<Task>(&known) {
        Ok(Read::Known(t)) => assert_eq!(
            t.state,
            TaskState::WaitingUser {
                wait_kind: WaitKind::Question
            }
        ),
        other => panic!("{other:?}"),
    }
}

/// The envelope of an event is the keys that are not the event's: the list a reader leaves out
/// of the payload it prints is the type's.
#[test]
fn req_main_006_the_envelope_fields_are_the_event_records() {
    let mut record = event(Event::TaskCreated);
    record.actor = Actor::User {
        session: "pid:1".into(),
        os_user: "someone".into(),
    };
    let value = json_of(&record);
    let mut keys: Vec<&str> = value
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .filter(|k| *k != "event")
        .collect();
    keys.sort_unstable();
    let mut envelope = crate::event::ENVELOPE_FIELDS.to_vec();
    envelope.sort_unstable();
    assert_eq!(keys, envelope);
}
