//! The typed detail of a rejection is stored with its step (handoff 002c;
//! REQ-DEC-008, REQ-DEC-009, REQ-PERS-006, REQ-PERS-013): schema version 3, its
//! migration from a store written by 0.9.0, and the readers that must refuse
//! a newer schema and say `migration pending` for an older one.

use super::tests::Fx;
use crate::integrity;
use crate::reader::open_read_only;
use crate::sqlite::open;
use mimlys_core::event::Event;
use mimlys_core::native::DecisionTransport;
use mimlys_core::rejection::{
    Denial, DeniedReason, Detail, Rejection, RejectionCode, StateReason, feedback_body,
};
use mimlys_core::spi::{Persistence, PersistenceError, PersistenceRead};
use mimlys_core::step::{Step, StepOutcome};
use mimlys_core::task::Task;
use serde_json::{Value, json};

/// A task, created in `store`.
fn create(store: &dyn Persistence, fx: &Fx) -> Task {
    let task = fx.task();
    store
        .create_task(&task, &[], &fx.event(&task, Event::TaskCreated))
        .unwrap();
    task
}

/// A step that ended rejected, written the way the loop writes it: the
/// validation line with the code and the detail, then the outcome.
fn reject(
    store: &dyn Persistence,
    fx: &Fx,
    task: &Task,
    index: u32,
    rejection: &Rejection,
) -> Step {
    use mimlys_core::ids::StepId;
    let mut step = Step::start(StepId::new(&fx.ids), task.task_id, index, fx.now());
    store
        .record_step_started(&step, &fx.event(task, Event::StepStarted))
        .unwrap();
    step.rejection = Some(rejection.code);
    step.rejection_detail = Some(rejection.detail.clone());
    store
        .record_validation(
            &step,
            None,
            &fx.event(
                task,
                Event::DecisionRejected {
                    code: rejection.code,
                    reason: rejection.detail.reason_name().map(str::to_owned),
                    grant_kind: rejection.detail.grant_kind().map(str::to_owned),
                },
            ),
        )
        .unwrap();
    step.finish(StepOutcome::Rejected, fx.now());
    store
        .record_step_outcome(
            &step,
            task,
            &fx.event(
                task,
                Event::StepFinished {
                    outcome: StepOutcome::Rejected,
                },
            ),
        )
        .unwrap();
    step
}

fn denial(reason: DeniedReason) -> Rejection {
    Rejection::denied(Denial::new(reason))
}

/// REQ-DEC-009, REQ-PERS-004: the detail is written with the step, in its own column and not in
/// the step's record, survives a reopen, and an update of the step that does not carry it keeps it.
#[test]
fn req_dec_009_the_rejection_detail_is_stored_with_the_step_and_survives_a_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let fx = Fx::new();
    let first = denial(DeniedReason::SchemeNotHttps);
    let second = Rejection::new(
        RejectionCode::State,
        Detail::State {
            reason: StateReason::PendingUserItem,
        },
    );
    let written = {
        let store = open(&data).unwrap();
        let task = create(&store, &fx);
        let a = reject(&store, &fx, &task, 0, &first);
        let b = reject(&store, &fx, &task, 1, &second);
        // A step that was not rejected has none.
        let mut plain = Step::start(
            mimlys_core::ids::StepId::new(&fx.ids),
            task.task_id,
            2,
            fx.now(),
        );
        store
            .record_step_started(&plain, &fx.event(&task, Event::StepStarted))
            .unwrap();
        plain.finish(StepOutcome::Continued, fx.now());
        store
            .record_step_outcome(
                &plain,
                &task,
                &fx.event(
                    &task,
                    Event::StepFinished {
                        outcome: StepOutcome::Continued,
                    },
                ),
            )
            .unwrap();
        // The same step written again without its detail (a copy that never had it) keeps what is stored.
        let mut again = a.clone();
        again.rejection_detail = None;
        store
            .record_step_outcome(
                &again,
                &task,
                &fx.event(
                    &task,
                    Event::StepFinished {
                        outcome: StepOutcome::Rejected,
                    },
                ),
            )
            .unwrap();
        vec![a, b, plain]
    };
    let store = open(&data).unwrap();
    let task_id = written[0].task_id;
    let read = store.load_steps(task_id).unwrap();
    assert_eq!(
        read, written,
        "the steps read as they were written, details included"
    );
    assert_eq!(read[0].rejection_detail, Some(first.detail.clone()));
    assert_eq!(read[2].rejection_detail, None);
    // Where it is: the column holds the fixed keys; the record has no such field.
    store
        .with_conn(|c| {
            let column: Option<String> = c
                .query_row(
                    "SELECT rejection_detail FROM steps WHERE idx = 0",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(
                serde_json::from_str::<Value>(&column.unwrap()).unwrap(),
                json!({"reason": "scheme_not_https"})
            );
            let record: String = c
                .query_row("SELECT record FROM steps WHERE idx = 0", [], |r| r.get(0))
                .unwrap();
            assert!(!record.contains("rejection_detail"), "{record}");
            let plain: Option<String> = c
                .query_row(
                    "SELECT rejection_detail FROM steps WHERE idx = 2",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(plain, None);
            // A damaged column reads as no detail, and so does one that is not a detail of its code.
            c.execute(
                "UPDATE steps SET rejection_detail = 'not json' WHERE idx = 0",
                [],
            )
            .unwrap();
            let foreign = json!({"reason": "never_a_reason", "x": "y".repeat(10_000)}).to_string();
            c.execute(
                "UPDATE steps SET rejection_detail = ?1 WHERE idx = 1",
                [foreign],
            )
            .unwrap();
        })
        .unwrap();
    let read = store.load_steps(task_id).unwrap();
    assert_eq!(read[0].rejection, Some(RejectionCode::Denied));
    assert_eq!(read[0].rejection_detail, None, "a damaged detail is none");
    assert_eq!(
        read[1].rejection_detail, None,
        "a detail with an unknown reason is none"
    );
}

/// REQ-DEC-009: what the model reads from a stored step is what it read live.
#[test]
fn req_dec_009_the_feedback_of_a_stored_step_is_the_live_feedback() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let fx = Fx::new();
    let rejection = Rejection::schema("$.arguments.path", "string", None);
    let live = rejection.feedback(DecisionTransport::Envelope);
    let task_id = {
        let store = open(&data).unwrap();
        let task = create(&store, &fx);
        reject(&store, &fx, &task, 0, &rejection);
        task.task_id
    };
    let store = open(&data).unwrap();
    let step = store.load_steps(task_id).unwrap().remove(0);
    assert_eq!(
        feedback_body(
            step.rejection.unwrap(),
            step.rejection_detail.as_ref(),
            DecisionTransport::Envelope
        ),
        live
    );
}

/// Copies the 0.9.0 fixture (schema version 2; its data in an uncheckpointed `-wal`) into a
/// fresh data directory.
fn copy_fixture() -> (tempfile::TempDir, std::path::PathBuf) {
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/v0_9_0");
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    for sub in ["snapshots/objects", "snapshots/manifests"] {
        std::fs::create_dir_all(data.join(sub)).unwrap();
        for entry in std::fs::read_dir(fixture.join(sub)).unwrap() {
            let entry = entry.unwrap();
            std::fs::copy(entry.path(), data.join(sub).join(entry.file_name())).unwrap();
        }
    }
    for name in ["mimlys.db", "mimlys.db-wal"] {
        std::fs::copy(fixture.join(name), data.join(name)).unwrap();
    }
    (dir, data)
}

/// The step records of a database, in order of index.
fn step_records(data: &std::path::Path) -> Vec<String> {
    let raw = rusqlite::Connection::open(data.join("mimlys.db")).unwrap();
    let mut stmt = raw
        .prepare("SELECT record FROM steps ORDER BY idx")
        .unwrap();
    stmt.query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

/// REQ-PERS-006, REQ-DEC-009 (handoff 002c): a store written by 0.9.0 (schema version 2) is
/// migrated to version 3 on open: one nullable column, no record rewritten, a step rejected
/// before the migration reads with its code and no detail (its feedback is the code's hint),
/// and a rejection after it keeps its detail across a reopen.
#[test]
fn req_pers_006_a_0_9_0_store_migrates_to_schema_3_and_old_rejections_read_with_the_code() {
    let (_dir, data) = copy_fixture();
    // As 0.9.0 wrote a rejected step: the code in the record, nothing else.
    let raw = rusqlite::Connection::open(data.join("mimlys.db")).unwrap();
    let version: i64 = raw
        .query_row("SELECT version FROM schema_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, 2, "the fixture is a schema version 2 store");
    let had_column: i64 = raw
        .query_row(
            "SELECT count(*) FROM pragma_table_info('steps') WHERE name = 'rejection_detail'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(had_column, 0);
    raw.execute(
        "UPDATE steps SET record = json_set(record, '$.rejection', 'denied') WHERE idx = 0",
        [],
    )
    .unwrap();
    drop(raw);
    let before = step_records(&data);
    assert!(!before.is_empty());

    let fx = Fx::starting_at(1_000_000);
    let store = open(&data).unwrap();
    assert_eq!(store.schema_version().unwrap(), 3);
    assert_eq!(crate::SCHEMA_VERSION, 3);
    let column_ok: i64 = store
        .with_conn(|c| {
            c.query_row(
                "SELECT count(*) FROM pragma_table_info('steps') WHERE name = 'rejection_detail' AND \"notnull\" = 0",
                [],
                |r| r.get(0),
            )
            .unwrap()
        })
        .unwrap();
    assert_eq!(column_ok, 1, "one nullable column");
    integrity::check(&store, fx.now()).unwrap();
    let tasks = store.list_tasks().unwrap();
    assert_eq!(tasks.len(), 1);
    let task_id = tasks[0].task_id;
    let steps = store.load_steps(task_id).unwrap();
    assert_eq!(steps.len(), before.len());
    assert_eq!(steps[0].rejection, Some(RejectionCode::Denied));
    assert_eq!(steps[0].rejection_detail, None, "written before the column");
    assert_eq!(
        feedback_body(
            steps[0].rejection.unwrap(),
            steps[0].rejection_detail.as_ref(),
            DecisionTransport::Envelope
        ),
        json!({"code": "denied", "hint": RejectionCode::Denied.hint()}),
        "an older step's feedback is the code's hint"
    );
    assert!(steps[1..].iter().all(|s| s.rejection_detail.is_none()));
    assert_eq!(step_records(&data), before, "no step record was rewritten");

    // A rejection after the migration keeps its detail, across a reopen; the migration does not run twice.
    let task = store.load_task(task_id).unwrap().unwrap();
    let rejection = denial(DeniedReason::PathOutsideWorkspace);
    let next = before.len() as u32;
    let fresh = reject(&store, &fx, &task, next, &rejection);
    drop(store);
    let store = open(&data).unwrap();
    assert_eq!(store.schema_version().unwrap(), 3);
    let steps = store.load_steps(task_id).unwrap();
    assert_eq!(steps.len(), before.len() + 1);
    assert_eq!(steps[before.len()], fresh);
    assert_eq!(steps[before.len()].rejection_detail, Some(rejection.detail));
    assert_eq!(steps[0].rejection_detail, None);
    integrity::check(&store, fx.now()).unwrap();
}

/// REQ-PERS-013 (handoff 002c): with schema version 3, a reader still refuses a newer schema
/// with the reason and touches nothing, says `migration pending` for the 0.9.0 store (version 2)
/// without migrating it, and reads the detail of a version 3 store.
#[test]
fn req_pers_013_readers_refuse_version_4_and_say_pending_for_a_0_9_0_store() {
    let (_dir, data) = copy_fixture();
    let bytes = |data: &std::path::Path| -> Vec<Vec<u8>> {
        ["mimlys.db", "mimlys.db-wal"]
            .iter()
            .map(|n| std::fs::read(data.join(n)).unwrap_or_default())
            .collect()
    };
    let before = bytes(&data);
    let err = open_read_only(&data).expect_err("version 2 is older than 3");
    assert_eq!(
        err,
        PersistenceError::MigrationPending {
            found: 2,
            supported: 3
        }
    );
    assert!(err.to_string().starts_with("migration pending"), "{err}");
    assert_eq!(bytes(&data), before, "a reader migrates nothing");

    // The writer migrates it; a reader then reads the stored detail.
    let fx = Fx::starting_at(1_000_000);
    let task_id = {
        let store = open(&data).unwrap();
        let task = store
            .load_task(store.list_tasks().unwrap()[0].task_id)
            .unwrap()
            .unwrap();
        reject(
            &store,
            &fx,
            &task,
            1,
            &denial(DeniedReason::PathOutsideWorkspace),
        );
        task.task_id
    };
    let reader = open_read_only(&data).unwrap();
    let steps = reader.load_steps(task_id).unwrap();
    assert_eq!(
        steps[1].rejection_detail,
        Some(denial(DeniedReason::PathOutsideWorkspace).detail)
    );
    drop(reader);

    // A store from a newer release: version 4.
    {
        let raw = rusqlite::Connection::open(data.join("mimlys.db")).unwrap();
        raw.execute("UPDATE schema_version SET version = 4", [])
            .unwrap();
    }
    let before = bytes(&data);
    let err = open_read_only(&data).expect_err("version 4 is newer than 3");
    assert_eq!(
        err,
        PersistenceError::NewerSchema {
            found: 4,
            supported: 3
        }
    );
    assert!(err.to_string().contains('4'), "the reason is stated: {err}");
    assert_eq!(bytes(&data), before, "a newer store is left as it was");
    assert_eq!(
        open(&data).map(|_| ()),
        Err(PersistenceError::NewerSchema {
            found: 4,
            supported: 3
        }),
        "the writer refuses it too"
    );
}

/// REQ-OBS-008 (handoff 002c): rejections are counted by code, and denials by reason and by the
/// grant kind that would allow them, from the events; an event of 0.9.0, which carries neither
/// field, still counts by code.
#[test]
fn req_obs_008_denials_are_counted_by_reason_and_grant_kind() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let fx = Fx::new();
    let store = open(&data).unwrap();
    let task = create(&store, &fx);
    let network = Rejection::denied(Denial::grant(
        DeniedReason::DestinationNotGranted,
        "network",
    ));
    let host_tool = Rejection::denied(Denial::grant(DeniedReason::GrantMissing, "host_tool"));
    for (i, rejection) in [
        denial(DeniedReason::SchemeNotHttps),
        denial(DeniedReason::SchemeNotHttps),
        network.clone(),
        network,
        host_tool,
        Rejection::schema("$.x", "string", None),
    ]
    .iter()
    .enumerate()
    {
        reject(&store, &fx, &task, i as u32, rejection);
    }
    // 0.9.0 wrote `decision.rejected{code}` alone.
    store
        .append_event(&fx.event(
            &task,
            Event::DecisionRejected {
                code: RejectionCode::Denied,
                reason: None,
                grant_kind: None,
            },
        ))
        .unwrap();
    let m = store.metrics(Some(task.task_id)).unwrap();
    assert_eq!(m.rejections_by_code["denied"], 6);
    assert_eq!(m.rejections_by_code["schema"], 1);
    let by = |map: &std::collections::BTreeMap<String, u64>| -> Vec<(String, u64)> {
        map.iter().map(|(k, v)| (k.clone(), *v)).collect()
    };
    assert_eq!(
        by(&m.denials_by_reason),
        [
            ("destination_not_granted".to_owned(), 2),
            ("grant_missing".to_owned(), 1),
            ("scheme_not_https".to_owned(), 2)
        ]
    );
    assert_eq!(
        by(&m.denials_by_grant_kind),
        [("host_tool".to_owned(), 1), ("network".to_owned(), 2)]
    );
    // All tasks: the same counts.
    assert_eq!(
        store.metrics(None).unwrap().denials_by_reason,
        m.denials_by_reason
    );
}
