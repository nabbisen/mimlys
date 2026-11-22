//! REQ-MAIN-006 in the store: the readers keep a record that holds a word a newer release wrote,
//! the typed loads that the runtime and the writers use refuse it naming the word, a damaged
//! record is none of these, and recovery at the start leaves such a task as it is.

use super::tests::{Fx, sequence};
use crate::sqlite::{Store, open};
use mimlys_core::event::Event;
use mimlys_core::ids::{PromotionId, QuestionId, SnapshotId, StepId};
use mimlys_core::newer::{RecordKind, UnknownWord};
use mimlys_core::snapshot::{
    Change, ChangeKind, Manifest, Promotion, PromotionState, SnapshotReason, SnapshotRecord,
};
use mimlys_core::spi::{Persistence, PersistenceError, PersistenceRead};
use mimlys_core::task::{PlanVersion, Task};
use serde_json::{Value, json};

/// A store holding one task with a record of every kind: the task, its step, action, approval,
/// execution, observation, grant, plan, question, snapshot, promotion and events.
fn store_of_every_kind(dir: &std::path::Path) -> (Store, Task) {
    let store = open(dir).unwrap();
    let fx = Fx::new();
    let (task, _) = sequence(&store, &fx, 8, true);
    store
        .record_plan(
            &PlanVersion {
                task_id: task.task_id,
                version: 1,
                steps: vec!["look".into()],
                created_at: fx.now(),
            },
            &fx.event(&task, Event::TaskCreated),
        )
        .unwrap();
    let question = mimlys_core::question::Question::ask(
        QuestionId::new(&fx.ids),
        task.task_id,
        StepId::new(&fx.ids),
        "which one?",
        fx.now(),
        None,
    );
    store
        .record_question(&task, &question, &fx.event(&task, Event::TaskCreated))
        .unwrap();
    let backup = SnapshotRecord {
        snapshot_id: SnapshotId::new(&fx.ids),
        task_id: task.task_id,
        reason: SnapshotReason::Backup,
        manifest_hash: Manifest::default().hash(),
        bytes: 0,
        files: 0,
        created_at: fx.now(),
        action_id: None,
        project_root: Some("/project".into()),
    };
    store
        .record_snapshot(
            &backup,
            &[],
            &fx.event(
                &task,
                Event::WorkspaceSnapshot {
                    snapshot_id: backup.snapshot_id,
                    reason: "backup".into(),
                },
            ),
        )
        .unwrap();
    let promotion = Promotion {
        promotion_id: PromotionId::new(&fx.ids),
        task_id: task.task_id,
        backup_snapshot: backup.snapshot_id,
        project_root: "/project".into(),
        files: vec![Change {
            path: "a.txt".into(),
            kind: ChangeKind::Added,
            bytes: 1,
        }],
        applied_at: fx.now(),
        reversed_at: None,
        state: PromotionState::Applied,
        after: vec![],
        restored: true,
        reason: None,
    };
    store
        .record_promotion(&promotion, &fx.event(&task, Event::TaskCreated))
        .unwrap();
    (store, task)
}

fn sample(store: &Store, task: &Task, kind: RecordKind) -> Value {
    let raw = store.raw_records(kind, task.task_id).unwrap();
    let first = raw
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("no {kind:?} record"));
    serde_json::from_str(&first.text).unwrap()
}

/// `base` with the keys of `patch` set (a key set to `null` is removed).
fn with(base: &Value, patch: Value) -> Value {
    let mut out = base.clone();
    for (key, value) in patch.as_object().unwrap() {
        if value.is_null() {
            out.as_object_mut().unwrap().remove(key);
        } else {
            out[key] = value.clone();
        }
    }
    out
}

/// REQ-MAIN-006, the inventory: every vocabulary of a stored record that a newer release could
/// extend, each in the record that holds it. A record with a word of it that this release does
/// not know is held by that word and the field that holds it; read for a writer it is refused
/// naming the word; and the record the word was put in, unchanged, reads whole.
///
/// The list is the handoff's inventory (`rfcs/handoffs/004e-readers-of-newer-records`, the
/// request's table): a vocabulary that is added to a record type is found by this reading with no
/// change to the code, and is listed here by whoever adds it.
#[test]
fn req_main_006_every_vocabulary_of_every_kind_of_record_is_held_by_its_word() {
    let dir = tempfile::tempdir().unwrap();
    let (store, task) = store_of_every_kind(&dir.path().join("data"));
    let s = |kind| sample(&store, &task, kind);
    let task_ = s(RecordKind::Task);
    let step = s(RecordKind::Step);
    let action = s(RecordKind::Action);
    let execution = s(RecordKind::Execution);
    let observation = s(RecordKind::Observation);
    let approval = s(RecordKind::Approval);
    let question = s(RecordKind::Question);
    let grant = s(RecordKind::Grant);
    let snapshot = s(RecordKind::Snapshot);
    let promotion = s(RecordKind::Promotion);
    let event = |name: &str, extra: Value| {
        let mut e = s(RecordKind::Event);
        e["event"] = json!(name);
        with(&e, extra)
    };
    let hash = task_["versions"]["policy_hash"].clone();
    let w = "from_a_newer_release";
    let backend = json!({"kind": "fake", "model": null, "locality": "local"});

    // (the kind, what it is, the record as a newer release wrote it, the field, the word)
    let table: Vec<(RecordKind, &str, Value, &str, &str)> = vec![
        // The task: its state and what the state carries.
        (
            RecordKind::Task,
            "task state",
            with(&task_, json!({"state": w})),
            "state",
            w,
        ),
        (
            RecordKind::Task,
            "wait kind",
            with(
                &task_,
                json!({"state": "waiting_user", "wait_kind": "plan"}),
            ),
            "wait_kind",
            "plan",
        ),
        (
            RecordKind::Task,
            "pending item",
            with(
                &task_,
                json!({"state": "waiting_user", "wait_kind": "approval", "pending": {"pending": w}}),
            ),
            "pending.pending",
            w,
        ),
        (
            RecordKind::Task,
            "halt reason",
            with(
                &task_,
                json!({"state": "halted", "halt_reason": "plan_required"}),
            ),
            "halt_reason",
            "plan_required",
        ),
        (
            RecordKind::Task,
            "halt detail rule",
            with(
                &task_,
                json!({"state": "halted", "halt_reason": "policy", "halt_detail": {"rule": w}}),
            ),
            "halt_detail.rule",
            w,
        ),
        (
            RecordKind::Task,
            "completion",
            with(&task_, json!({"state": "completed", "completion": w})),
            "completion",
            w,
        ),
        (
            RecordKind::Task,
            "failure reason",
            with(
                &task_,
                json!({"state": "failed", "reason": {"failure_reason": w}}),
            ),
            "reason.failure_reason",
            w,
        ),
        // The step.
        (
            RecordKind::Step,
            "step phase",
            with(&step, json!({"phase": w})),
            "phase",
            w,
        ),
        (
            RecordKind::Step,
            "step outcome",
            with(&step, json!({"outcome": w})),
            "outcome",
            w,
        ),
        (
            RecordKind::Step,
            "rejection code",
            with(&step, json!({"rejection": "plan_required"})),
            "rejection",
            "plan_required",
        ),
        // The action: its status, the code of a rejection, the cause of a failure, the effect.
        (
            RecordKind::Action,
            "action status",
            with(&action, json!({"status": w})),
            "status",
            w,
        ),
        (
            RecordKind::Action,
            "action rejection code",
            with(
                &action,
                json!({"status": "rejected", "code": "plan_required"}),
            ),
            "code",
            "plan_required",
        ),
        (
            RecordKind::Action,
            "action failure cause",
            with(&action, json!({"status": "failed", "cause": {"cause": w}})),
            "cause.cause",
            w,
        ),
        (
            RecordKind::Action,
            "resource limit",
            with(
                &action,
                json!({"status": "failed", "cause": {"cause": "resource_exhausted", "limit": w}}),
            ),
            "cause.limit",
            w,
        ),
        (
            RecordKind::Action,
            "effect kind",
            with(
                &action,
                json!({"effect": {"kind": w, "reversible": true, "idempotent": false}}),
            ),
            "effect.kind",
            w,
        ),
        // The execution: how it ended.
        (
            RecordKind::Execution,
            "termination",
            with(&execution, json!({"termination": w})),
            "termination",
            w,
        ),
        (
            RecordKind::Execution,
            "host fault detail",
            with(
                &execution,
                json!({"termination": "host_fault", "detail": w}),
            ),
            "detail",
            w,
        ),
        // The observation: its status, its cause, its notices.
        (
            RecordKind::Observation,
            "observation status",
            with(&observation, json!({"status": w})),
            "status",
            w,
        ),
        (
            RecordKind::Observation,
            "observation cause",
            with(&observation, json!({"cause": {"cause": w}})),
            "cause.cause",
            w,
        ),
        (
            RecordKind::Observation,
            "notice",
            with(&observation, json!({"notices": [{"code": w}]})),
            "notices[0].code",
            w,
        ),
        // The approval, the question and the grant.
        (
            RecordKind::Approval,
            "approval status",
            with(&approval, json!({"status": w})),
            "status",
            w,
        ),
        (
            RecordKind::Approval,
            "invalidation reason",
            with(&approval, json!({"invalidation": w})),
            "invalidation",
            w,
        ),
        (
            RecordKind::Approval,
            "decided by",
            with(&approval, json!({"decided_by": {"actor": w}})),
            "decided_by.actor",
            w,
        ),
        (
            RecordKind::Question,
            "question status",
            with(&question, json!({"status": w})),
            "status",
            w,
        ),
        (
            RecordKind::Question,
            "answered by",
            with(&question, json!({"answered_by": {"actor": w}})),
            "answered_by.actor",
            w,
        ),
        (
            RecordKind::Grant,
            "grant status",
            with(&grant, json!({"status": w})),
            "status",
            w,
        ),
        (
            RecordKind::Grant,
            "grant kind",
            with(&grant, json!({"kind": {"kind": w}})),
            "kind.kind",
            w,
        ),
        (
            RecordKind::Grant,
            "grant issuer",
            with(&grant, json!({"actor": w})),
            "actor",
            w,
        ),
        // The snapshot and the promotion.
        (
            RecordKind::Snapshot,
            "snapshot reason",
            with(&snapshot, json!({"reason": w})),
            "reason",
            w,
        ),
        (
            RecordKind::Promotion,
            "promotion state",
            with(&promotion, json!({"state": w})),
            "state",
            w,
        ),
        (
            RecordKind::Promotion,
            "change kind",
            with(
                &promotion,
                json!({"files": [{"path": "a", "kind": w, "bytes": 1}]}),
            ),
            "files[0].kind",
            w,
        ),
        // The event: its name, and each vocabulary its payload holds.
        (
            RecordKind::Event,
            "event name",
            event(w, json!({"client": "x"})),
            "event",
            w,
        ),
        (
            RecordKind::Event,
            "event actor",
            event("task.created", json!({"actor": w})),
            "actor",
            w,
        ),
        (
            RecordKind::Event,
            "halt reason of a state change",
            event(
                "task.state_changed",
                json!({"from": "running", "to": "halted", "reason": w}),
            ),
            "reason",
            w,
        ),
        (
            RecordKind::Event,
            "halt detail of a state change",
            event(
                "task.state_changed",
                json!({"from": "running", "to": "halted", "reason": "policy", "halt_detail": {"rule": w}}),
            ),
            "halt_detail.rule",
            w,
        ),
        (
            RecordKind::Event,
            "admission state",
            event("task.admission_changed", json!({"state": w})),
            "state",
            w,
        ),
        (
            RecordKind::Event,
            "decision transport",
            event("decision.received", json!({"hash": hash, "transport": w})),
            "transport",
            w,
        ),
        (
            RecordKind::Event,
            "rejection code of an event",
            event("decision.rejected", json!({"code": w})),
            "code",
            w,
        ),
        (
            RecordKind::Event,
            "step outcome of an event",
            event("step.finished", json!({"outcome": w})),
            "outcome",
            w,
        ),
        (
            RecordKind::Event,
            "approval decision",
            event(
                "approval.decided",
                json!({"approval_id": approval["approval_id"], "decision": w}),
            ),
            "decision",
            w,
        ),
        (
            RecordKind::Event,
            "recovery disposition",
            event("task.recovered", json!({"disposition": w})),
            "disposition",
            w,
        ),
        (
            RecordKind::Event,
            "tier",
            event("runtime.started", json!({"tier": w, "backend": backend})),
            "tier",
            w,
        ),
        (
            RecordKind::Event,
            "locality of the backend",
            event(
                "runtime.started",
                json!({"tier": "core", "backend": {"kind": "fake", "model": null, "locality": w}}),
            ),
            "backend.locality",
            w,
        ),
    ];

    // The kinds with no vocabulary (the plan is text and numbers), and the kinds the table covers.
    let covered: std::collections::BTreeSet<RecordKind> = table.iter().map(|t| t.0).collect();
    for kind in RecordKind::ALL {
        assert!(
            covered.contains(&kind) || kind == RecordKind::Plan,
            "{kind:?} has no entry in the inventory"
        );
    }
    for (kind, what, record, field, word) in table {
        // The record as it is, before the word: reads whole.
        let text = record.to_string();
        let newer = kind
            .check(&text)
            .unwrap_or_else(|e| panic!("{what}: {e:?} for {text}"))
            .unwrap_or_else(|| panic!("{what}: the record reads whole: {text}"));
        let found: Vec<(&str, &str)> = newer
            .words
            .iter()
            .map(|w| (w.field.as_str(), w.word.as_str()))
            .collect();
        // Exactly the one word, at exactly its field: a record that names another place as well
        // is not the record this entry is.
        assert_eq!(
            found,
            vec![(field, word)],
            "{what}: {found:?} is not {field} `{word}`"
        );
        assert_eq!(newer.raw, record, "{what}: the record as written");
    }
    // Every record of the store as it is: none holds a word it does not know.
    assert!(
        mimlys_core::newer::scan(&store, task.task_id)
            .unwrap()
            .is_empty()
    );
}

/// REQ-MAIN-006: the field named is the vocabulary's where a free string of the same record, equal
/// to the unknown word, sorts BEFORE it. The maps of a record are sorted by key, so that a
/// record whose vocabulary field comes first is named rightly by the order alone (the review of
/// 004e: a reading that takes the first place that holds the word passed every other test). Each
/// record is read from the store as the typed reads and the readers get it, and is a real one of
/// its kind, with a free string put in the place `free` and the word in both places.
#[test]
fn req_main_006_a_free_string_that_sorts_before_the_vocabulary_is_not_the_field_named() {
    let dir = tempfile::tempdir().unwrap();
    let (store, task) = store_of_every_kind(&dir.path().join("data"));
    let s = |kind| sample(&store, &task, kind);
    let event = |name: &str, extra: Value| {
        let mut e = s(RecordKind::Event);
        e["event"] = json!(name);
        with(&e, extra)
    };
    let w = "from_a_newer_release";
    // (the kind, what it is, the record, the key of the free string met first, the vocabulary's field)
    let table: Vec<(RecordKind, &str, Value, &str, &str)> = vec![
        (
            RecordKind::Task,
            "the goal before the state",
            with(&s(RecordKind::Task), json!({"goal": w, "state": w})),
            "goal",
            "state",
        ),
        (
            RecordKind::Event,
            "the reason of an admission before its state",
            event(
                "task.admission_changed",
                json!({"reason": w, "state": w, "slots_taken": 1}),
            ),
            "reason",
            "state",
        ),
        (
            RecordKind::Event,
            "the `from` of a state change before its halt reason",
            event(
                "task.state_changed",
                json!({"from": w, "to": "halted", "reason": w}),
            ),
            "from",
            "reason",
        ),
        (
            RecordKind::Event,
            "a field of a newer event before its name",
            event(w, json!({"client": w})),
            "client",
            "event",
        ),
        (
            // In a list: the path of the first change before the kind of the second.
            RecordKind::Promotion,
            "the path of one change before the kind of the next",
            with(
                &s(RecordKind::Promotion),
                json!({"files": [
                    {"path": w, "kind": "added", "bytes": 1},
                    {"path": "b", "kind": w, "bytes": 1},
                ]}),
            ),
            "path",
            "files[1].kind",
        ),
    ];
    // Every entry is tried, and the misses are listed together: each entry is a case of its own.
    let mut misses: Vec<String> = Vec::new();
    for (kind, what, record, free, field) in table {
        // The precondition: the free string holds the first place of the word in the record's
        // text (sorted, as the store writes it), or the entry does not tell the places apart.
        let text = record.to_string();
        let free_at = text.find(&format!("\"{w}\"")).unwrap();
        assert!(
            text[..free_at].ends_with(&format!("\"{free}\":")),
            "{what}: `{free}` must hold the first `{w}` of {text}"
        );
        let newer = kind
            .check(&text)
            .unwrap_or_else(|e| panic!("{what}: {e:?} for {text}"))
            .unwrap_or_else(|| panic!("{what}: the record reads whole: {text}"));
        let found: Vec<(&str, &str)> = newer
            .words
            .iter()
            .map(|word| (word.field.as_str(), word.word.as_str()))
            .collect();
        if found != vec![(field, w)] {
            misses.push(format!(
                "{what}: named {found:?}, not the vocabulary's `{field}` alone (the free `{free}` comes first)"
            ));
        }
    }
    assert!(misses.is_empty(), "{misses:#?}");
}

/// Every column of every row the task owns, as text: what "the store unchanged byte for byte in
/// its rows" compares.
fn rows_of(store: &Store, task: &Task) -> String {
    let id = task.task_id.to_string();
    store
        .with_conn(|conn| {
            let tables: Vec<String> = conn
                .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
                .unwrap()
                .query_map([], |r| r.get(0))
                .unwrap()
                .map(Result::unwrap)
                .collect();
            let mut out = String::new();
            for table in tables {
                let columns: Vec<String> = conn
                    .prepare(&format!("SELECT name FROM pragma_table_info('{table}')"))
                    .unwrap()
                    .query_map([], |r| r.get(0))
                    .unwrap()
                    .map(Result::unwrap)
                    .collect();
                if !columns.iter().any(|c| c == "task_id") {
                    continue;
                }
                let quoted = columns
                    .iter()
                    .map(|c| format!("quote({c})"))
                    .collect::<Vec<_>>()
                    .join(" || '|' || ");
                let mut stmt = conn
                    .prepare(&format!(
                        "SELECT {quoted} FROM {table} WHERE task_id = ?1 ORDER BY rowid"
                    ))
                    .unwrap();
                for row in stmt.query_map([&id], |r| r.get::<_, String>(0)).unwrap() {
                    out.push_str(&format!("{table}: {}\n", row.unwrap()));
                }
            }
            out
        })
        .unwrap()
}

/// Rewrites the task's record and its state column, as a newer release would write it.
fn write_task(store: &Store, task: &Task, state: &str, patch: &str) {
    store
        .with_conn(|c| {
            c.execute(
                "UPDATE tasks SET state = ?2, record = json_patch(record, ?3) WHERE task_id = ?1",
                rusqlite::params![task.task_id.to_string(), state, patch],
            )
            .unwrap();
        })
        .unwrap();
}

/// REQ-MAIN-006: what the runtime and the writers read refuses a record that holds a word it does
/// not know, naming the word; what a reader reads keeps it. The same record, the same word.
#[test]
fn req_main_006_the_typed_loads_refuse_and_the_readers_keep() {
    let dir = tempfile::tempdir().unwrap();
    let (store, task) = store_of_every_kind(&dir.path().join("data"));
    write_task(
        &store,
        &task,
        "waiting_user",
        r#"{"state":"waiting_user","wait_kind":"plan"}"#,
    );
    let refused = store.load_task(task.task_id).unwrap_err();
    assert_eq!(
        refused,
        PersistenceError::NewerRecord {
            record: "task".into(),
            field: "wait_kind".into(),
            word: "plan".into()
        }
    );
    let text = refused.to_string();
    assert!(
        text.contains("`plan`") && text.contains("unknown to this release"),
        "{text}"
    );
    assert!(
        text.contains("the release of mimlys that wrote it is needed"),
        "{text}"
    );

    // The reader's listing keeps the task, with the word.
    let listed = store.list_tasks().unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].state, "waiting_user");
    assert_eq!(
        listed[0].unknown,
        vec![UnknownWord::new("wait_kind", "plan")]
    );
    // Only the task's own record is newer.
    let found = mimlys_core::newer::scan(&store, task.task_id).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].kind, RecordKind::Task);
}

/// REQ-MAIN-006: an event a newer release wrote is read by the readers and refused by the typed
/// read; the events before and after it read as they did.
#[test]
fn req_main_006_an_unknown_event_is_kept_by_the_reader_and_refused_by_the_typed_read() {
    let dir = tempfile::tempdir().unwrap();
    let (store, task) = store_of_every_kind(&dir.path().join("data"));
    let before = store
        .read_events_since(0, Some(task.task_id), 1_000)
        .unwrap()
        .len();
    store
        .with_conn(|c| {
            c.execute(
                "INSERT INTO events (event_id, at, name, task_id, step_id, action_id, record)
                 SELECT '00000000-00ff-7000-8000-0000000000f1', at, 'session.attached', task_id, NULL, NULL,
                        json_set(record, '$.event', 'session.attached', '$.event_id', '00000000-00ff-7000-8000-0000000000f1', '$.client', 'tui')
                 FROM events WHERE task_id = ?1 ORDER BY position LIMIT 1",
                [task.task_id.to_string()],
            )
            .unwrap();
        })
        .unwrap();
    let read = store
        .read_events_since(0, Some(task.task_id), 1_000)
        .unwrap();
    assert_eq!(read.len(), before + 1, "every event is listed");
    let newer: Vec<_> = read
        .iter()
        .filter_map(|e| match &e.record {
            mimlys_core::newer::Read::Newer(n) => Some(n),
            _ => None,
        })
        .collect();
    assert_eq!(newer.len(), 1);
    assert_eq!(newer[0].text("event"), Some("session.attached"));
    assert_eq!(newer[0].text("client"), Some("tui"));
    assert!(matches!(
        store.events_since(0, Some(task.task_id), 1_000),
        Err(PersistenceError::NewerRecord { word, .. }) if word == "session.attached"
    ));
    // The metrics count it by its name, and say it is not known.
    let metrics = store.metrics(Some(task.task_id)).unwrap();
    assert_eq!(metrics.events_by_name.get("session.attached"), Some(&1));
    assert_eq!(
        metrics.unknown_to_this_release,
        vec![UnknownWord::new("events_by_name", "session.attached")]
    );
    assert!(
        metrics
            .events_by_name
            .get("task.created")
            .is_some_and(|n| *n > 1)
    );
}

/// REQ-MAIN-006: a halt reason and a rejection code of a newer release are counted in their
/// groups by their names, and the event under its own name too.
#[test]
fn req_main_006_metrics_count_the_words_of_a_newer_event_in_their_groups() {
    let dir = tempfile::tempdir().unwrap();
    let (store, task) = store_of_every_kind(&dir.path().join("data"));
    let known = store.metrics(None).unwrap();
    store
        .with_conn(|c| {
            for (i, (name, patch)) in [
                (
                    "task.state_changed",
                    r#"{"from":"running","to":"halted","reason":"plan_required"}"#,
                ),
                ("decision.rejected", r#"{"code":"plan_required"}"#),
            ]
            .into_iter()
            .enumerate()
            {
                c.execute(
                    "INSERT INTO events (event_id, at, name, task_id, step_id, action_id, record)
                     SELECT ?3, at, ?2, task_id, NULL, NULL,
                            json_patch(json_set(record, '$.event', ?2, '$.event_id', ?3), ?4)
                     FROM events WHERE task_id = ?1 ORDER BY position LIMIT 1",
                    rusqlite::params![
                        task.task_id.to_string(),
                        name,
                        format!("00000000-00ff-7000-8000-0000000000f{i}"),
                        patch
                    ],
                )
                .unwrap();
            }
        })
        .unwrap();
    let metrics = store.metrics(None).unwrap();
    assert_eq!(metrics.halts_by_reason.get("plan_required"), Some(&1));
    assert_eq!(metrics.rejections_by_code.get("plan_required"), Some(&1));
    assert_eq!(
        metrics.events_by_name.get("task.state_changed"),
        Some(&(known.events_by_name["task.state_changed"] + 1))
    );
    assert_eq!(metrics.events_by_name.get("decision.rejected"), Some(&1));
    assert_eq!(
        metrics.unknown_to_this_release,
        vec![
            UnknownWord::new("halts_by_reason", "plan_required"),
            UnknownWord::new("rejections_by_code", "plan_required"),
        ]
    );
}

/// REQ-MAIN-006: a task whose state is a word of a newer release is counted by that word and
/// marked, and a store of known words has no marker anywhere.
#[test]
fn req_main_006_metrics_mark_an_unknown_state_and_only_that() {
    let dir = tempfile::tempdir().unwrap();
    let (store, task) = store_of_every_kind(&dir.path().join("data"));
    let known = store.metrics(None).unwrap();
    assert!(known.unknown_to_this_release.is_empty());
    assert!(
        serde_json::to_value(&known)
            .unwrap()
            .get(mimlys_core::newer::MARKER_FIELD)
            .is_none(),
        "no marker in a store of known words"
    );
    write_task(&store, &task, "quarantined", r#"{"state":"quarantined"}"#);
    let metrics = store.metrics(None).unwrap();
    assert_eq!(metrics.tasks_by_state.get("quarantined"), Some(&1));
    assert_eq!(
        metrics.unknown_to_this_release,
        vec![UnknownWord::new("tasks_by_state", "quarantined")]
    );
}

/// REQ-MAIN-006: a record that is damaged is not "newer": the typed read fails as it did, and the
/// listing still survives it.
#[test]
fn req_main_006_a_damaged_record_is_still_a_failure_and_still_listed() {
    let dir = tempfile::tempdir().unwrap();
    let (store, task) = store_of_every_kind(&dir.path().join("data"));
    store
        .with_conn(|c| {
            c.execute(
                "UPDATE tasks SET record = json_remove(record, '$.goal') WHERE task_id = ?1",
                [task.task_id.to_string()],
            )
            .unwrap();
        })
        .unwrap();
    assert_eq!(
        store.load_task(task.task_id).unwrap_err(),
        PersistenceError::Storage {
            detail: "deserialize".into()
        }
    );
    let listed = store.list_tasks().unwrap();
    assert_eq!(listed.len(), 1);
    assert!(listed[0].unknown.is_empty(), "damage is not a word");
}

/// REQ-MAIN-006: the start's integrity check (CONS-06, CONS-07) does not take a word of a newer
/// release for corruption: a state, a status, a grant of a newer kind under an open approval.
#[test]
fn req_main_006_the_integrity_check_lets_a_newer_word_pass() {
    use super::tests::sequence_with;
    let dir = tempfile::tempdir().unwrap();
    let store = open(&dir.path().join("data")).unwrap();
    let fx = Fx::new();
    // A task waiting for an approval, under the grant set the approval was made under.
    let (task, _) = sequence_with(&store, &fx, 3, true, true);
    crate::integrity::check(&store, fx.now()).unwrap();
    // The grant is of a kind a newer release wrote: the set cannot be hashed here, and the
    // approval is not held to it.
    store
        .with_conn(|c| {
            c.execute(
                "UPDATE grants SET record = json_set(record, '$.kind.kind', 'hologram') WHERE task_id = ?1",
                [task.task_id.to_string()],
            )
            .unwrap();
        })
        .unwrap();
    crate::integrity::check(&store, fx.now()).unwrap();
    write_task(&store, &task, "quarantined", r#"{"state":"quarantined"}"#);
    store
        .with_conn(|c| {
            c.execute(
                "UPDATE actions SET status = 'held', record = json_set(record, '$.status', 'held') WHERE task_id = ?1",
                [task.task_id.to_string()],
            )
            .unwrap();
        })
        .unwrap();
    crate::integrity::check(&store, fx.now()).unwrap();
    // A word that is not in the vocabulary, in a record that does not read for another reason,
    // is still a violation.
    store
        .with_conn(|c| {
            c.execute(
                "UPDATE tasks SET state = 'garbled', record = json_remove(record, '$.goal') WHERE task_id = ?1",
                [task.task_id.to_string()],
            )
            .unwrap();
        })
        .unwrap();
    assert!(matches!(
        crate::integrity::check(&store, fx.now()),
        Err(PersistenceError::Integrity { rule, .. }) if rule == "CONS-07"
    ));
}

/// REQ-MAIN-006: a workspace state of a newer release is named, not read as "no workspace".
#[test]
fn req_main_006_a_workspace_state_of_a_newer_release_is_named() {
    let dir = tempfile::tempdir().unwrap();
    let (store, task) = store_of_every_kind(&dir.path().join("data"));
    store
        .with_conn(|c| {
            c.execute(
                "INSERT INTO workspaces (task_id, state, changed_at) VALUES (?1, 'archived', 'x')",
                [task.task_id.to_string()],
            )
            .unwrap();
        })
        .unwrap();
    assert_eq!(
        store.load_workspace_state(task.task_id).unwrap_err(),
        PersistenceError::NewerRecord {
            record: "workspace".into(),
            field: "state".into(),
            word: "archived".into()
        }
    );
}

/// REQ-MAIN-006, recovery at the start: a task found `running` that holds a word of a newer
/// release is left exactly as it is (its rows byte for byte), once, and the pass recovers the
/// others and does not fail. A task waiting for something this release does not know is left
/// too, and so is one whose state it does not know.
#[test]
fn req_main_006_recovery_leaves_a_newer_task_as_it_is_and_recovers_the_others() {
    use crate::recovery::{RecoveryContext, recover_all_reporting};
    use mimlys_core::policy::Recovery;
    let dir = tempfile::tempdir().unwrap();
    let store = open(&dir.path().join("data")).unwrap();
    // Four tasks, each in a store of this release: one running as any (known), one running
    // with an event of a newer release, one waiting for a kind of a newer release, one in a
    // state of a newer release.
    let fx = Fx::new();
    let (known, _) = sequence(&store, &fx, 0, false);
    let (with_event, _) = sequence(&store, &fx, 0, false);
    let (waiting, _) = sequence(&store, &fx, 0, false);
    let (stated, _) = sequence(&store, &fx, 0, false);
    store
        .with_conn(|c| {
            c.execute(
                "INSERT INTO events (event_id, at, name, task_id, step_id, action_id, record)
                 SELECT '00000000-00ff-7000-8000-0000000000f1', at, 'session.attached', task_id, NULL, NULL,
                        json_set(record, '$.event', 'session.attached', '$.event_id', '00000000-00ff-7000-8000-0000000000f1')
                 FROM events WHERE task_id = ?1 ORDER BY position LIMIT 1",
                [with_event.task_id.to_string()],
            )
            .unwrap();
        })
        .unwrap();
    write_task(
        &store,
        &waiting,
        "waiting_user",
        r#"{"state":"waiting_user","wait_kind":"plan"}"#,
    );
    write_task(&store, &stated, "quarantined", r#"{"state":"quarantined"}"#);
    let untouched = [&with_event, &waiting, &stated].map(|t| (t.task_id, rows_of(&store, t)));
    let ctx = RecoveryContext {
        ids: &fx.ids,
        now: fx.now(),
        policy_hash: mimlys_core::hash::hash_bytes(b"policy"),
        recovery: Recovery { auto_resume: false },
    };
    let pass = recover_all_reporting(
        &store,
        &ctx,
        &mimlys_core::spi::fake::FakeWorkspaces::new(std::sync::Arc::new(
            mimlys_core::spi::fake::FakeToolProvider::new(vec![]),
        )),
    )
    .unwrap();
    // The known task was recovered; none of the others was.
    assert_eq!(
        pass.recovered.iter().map(|r| r.task_id).collect::<Vec<_>>(),
        vec![known.task_id]
    );
    // Each of the others is left, named once, with the record that holds the word.
    let mut left: Vec<_> = pass
        .left
        .iter()
        .map(|l| {
            (
                l.task_id,
                l.record.as_str(),
                l.word.field.as_str(),
                l.word.word.as_str(),
            )
        })
        .collect();
    left.sort();
    let mut expected = vec![
        (with_event.task_id, "event", "event", "session.attached"),
        (waiting.task_id, "task", "wait_kind", "plan"),
        (stated.task_id, "task", "state", "quarantined"),
    ];
    expected.sort();
    assert_eq!(left, expected);
    // And what they hold is what it was, byte for byte.
    for (id, before) in untouched {
        let task = [&with_event, &waiting, &stated]
            .into_iter()
            .find(|t| t.task_id == id)
            .unwrap();
        assert_eq!(rows_of(&store, task), before);
    }
}
