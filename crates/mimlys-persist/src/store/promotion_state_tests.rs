//! Where a promotion is (handoff 006b; REQ-WS-007, REQ-PERS-006, REQ-PERS-007; external design
//! §14.5, §16.3): schema version 4 and its migration from a store written by 0.9.0, the state
//! kept beside the record, and the start that restores a promotion found `applying`.

use super::rejection_detail_tests::copy_fixture;
use super::tests::Fx;
use crate::recovery::{RecoveryContext, recover_promotions};
use crate::sqlite::open;
use mimlys_core::event::Event;
use mimlys_core::hash::hash_bytes;
use mimlys_core::ids::{PromotionId, SnapshotId};
use mimlys_core::policy::Recovery;
use mimlys_core::snapshot::{
    Change, ChangeKind, Manifest, Promotion, PromotionState, SnapshotReason, SnapshotRecord,
};
use mimlys_core::spi::{Persistence, PersistenceRead};
use mimlys_core::task::Task;

fn workspaces() -> mimlys_core::spi::fake::FakeWorkspaces {
    mimlys_core::spi::fake::FakeWorkspaces::new(std::sync::Arc::new(
        mimlys_core::spi::fake::FakeToolProvider::new(vec![]),
    ))
}

/// A task with a backup snapshot and a promotion in `state`, recorded in `store`.
fn promoted(
    store: &dyn Persistence,
    fx: &Fx,
    state: PromotionState,
) -> (Task, SnapshotRecord, Promotion) {
    let task = fx.task();
    store
        .create_task(&task, &[], &fx.event(&task, Event::TaskCreated))
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
        state,
        after: vec![],
        reason: None,
    };
    store
        .record_promotion(
            &promotion,
            &fx.event(
                &task,
                Event::WorkspacePromotionStarted {
                    files: vec!["a.txt".into()],
                },
            ),
        )
        .unwrap();
    (task, backup, promotion)
}

fn context<'a>(fx: &'a Fx, ids: &'a dyn mimlys_core::ids::IdSource) -> RecoveryContext<'a> {
    RecoveryContext {
        ids,
        now: fx.now(),
        policy_hash: hash_bytes(b"policy"),
        recovery: Recovery { auto_resume: false },
    }
}

/// REQ-PERS-006, REQ-WS-007 (handoff 006b): a store written by 0.9.0 (schema version 2) is
/// migrated to version 4 on open. The promotion it holds keeps its record, reads as `applied`,
/// and is not one the start restores.
#[test]
fn req_pers_006_a_0_9_0_store_migrates_to_schema_4_and_its_promotions_read_as_applied() {
    let (_dir, data) = copy_fixture();
    let raw = rusqlite::Connection::open(data.join("mimlys.db")).unwrap();
    let version: i64 = raw
        .query_row("SELECT version FROM schema_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, 2, "the fixture is a schema version 2 store");
    let task_id: String = raw
        .query_row("SELECT task_id FROM tasks LIMIT 1", [], |r| r.get(0))
        .unwrap();
    // A promotion as 0.9.0 wrote it: no state, no entries, no reason.
    let promotion_id = "00000000-0000-7000-8000-00000000f00d";
    let backup = "00000000-0000-7000-8000-00000000beef";
    let record = format!(
        r#"{{"promotion_id":"{promotion_id}","task_id":"{task_id}","backup_snapshot":"{backup}","project_root":"/project","files":[{{"path":"a.txt","kind":"added","bytes":1}}],"applied_at":"2025-01-01T00:00:00Z","reversed_at":null}}"#
    );
    raw.execute(
        "INSERT INTO snapshots (snapshot_id, task_id, reason, manifest_hash, bytes, files, created_at, action_id, record, record_version) VALUES (?1, ?2, 'backup', 'h', 0, 0, '2025-01-01T00:00:00Z', NULL, '{}', 1)",
        rusqlite::params![backup, task_id],
    )
    .unwrap();
    raw.execute(
        "INSERT INTO promotions (promotion_id, task_id, backup_snapshot_id, applied_at, record, record_version) VALUES (?1, ?2, ?3, '2025-01-01T00:00:00Z', ?4, 1)",
        rusqlite::params![promotion_id, task_id, backup, record],
    )
    .unwrap();
    drop(raw);

    let store = open(&data).unwrap();
    assert_eq!(store.schema_version().unwrap(), 4);
    assert_eq!(crate::SCHEMA_VERSION, 4);
    let state: String = store
        .with_conn(|c| {
            c.query_row(
                "SELECT state FROM promotions WHERE promotion_id = ?1",
                [promotion_id],
                |r| r.get(0),
            )
            .unwrap()
        })
        .unwrap();
    assert_eq!(
        state, "applied",
        "a promotion from before the state was applied"
    );
    let tid = store.list_tasks().unwrap()[0].task_id;
    let loaded = store.load_promotions(tid).unwrap();
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].state, PromotionState::Applied);
    assert!(loaded[0].after.is_empty() && loaded[0].reason.is_none());
    assert!(store.promotions_applying().unwrap().is_empty());
    let kept: String = store
        .with_conn(|c| {
            c.query_row(
                "SELECT record FROM promotions WHERE promotion_id = ?1",
                [promotion_id],
                |r| r.get(0),
            )
            .unwrap()
        })
        .unwrap();
    assert_eq!(kept, record, "no record was rewritten");
}

/// REQ-WS-007, REQ-PERS-004: the state is kept with the record, survives a reopen, and the ones
/// found `applying` are those a start must restore.
#[test]
fn req_ws_007_the_state_of_a_promotion_is_stored_and_the_applying_ones_are_found() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let fx = Fx::new();
    let (task, mut promotion) = {
        let store = open(&data).unwrap();
        let (task, _, promotion) = promoted(&store, &fx, PromotionState::Applying);
        assert_eq!(
            store.promotions_applying().unwrap(),
            std::slice::from_ref(&promotion)
        );
        (task, promotion)
    };
    let store = open(&data).unwrap();
    assert_eq!(store.promotions_applying().unwrap(), [promotion.clone()]);
    promotion.state = PromotionState::Applied;
    store
        .record_promotion(
            &promotion,
            &fx.event(&task, Event::WorkspacePromoted { files: vec![] }),
        )
        .unwrap();
    assert!(store.promotions_applying().unwrap().is_empty());
    assert_eq!(
        store.load_promotions(task.task_id).unwrap()[0].state,
        PromotionState::Applied
    );
    promotion.state = PromotionState::Failed;
    promotion.reason = Some("restored".into());
    store
        .record_promotion(
            &promotion,
            &fx.event(
                &task,
                Event::WorkspacePromotionFailed {
                    reason: "restored".into(),
                },
            ),
        )
        .unwrap();
    let loaded = store.load_promotions(task.task_id).unwrap();
    assert_eq!(loaded.len(), 1, "one record, updated");
    assert_eq!(loaded[0], promotion);
}

/// REQ-WS-007, REQ-PERS-007 (external design §16.3): a promotion found `applying` is restored from
/// its backup, recorded `failed` with the reason, and the event is written; a second start finds
/// nothing to do.
#[test]
fn req_ws_007_a_promotion_found_applying_is_restored_recorded_failed_and_listed() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let fx = Fx::new();
    let store = open(&data).unwrap();
    let (task, _, applying) = promoted(&store, &fx, PromotionState::Applying);
    let (_, _, applied) = promoted(&store, &fx, PromotionState::Applied);
    let files = workspaces();
    let ids = mimlys_core::ids::ScriptedIds::starting_at(1_000_000);
    let recovered = recover_promotions(&store, &context(&fx, &ids), &files).unwrap();
    assert_eq!(recovered.len(), 1, "only the one found applying");
    assert_eq!(recovered[0].promotion_id, applying.promotion_id);
    assert_eq!(recovered[0].task_id, task.task_id);
    assert!(recovered[0].kept.is_empty() && recovered[0].restore_error.is_none());
    assert_eq!(
        files.reverted(),
        [(applying.promotion_id, mimlys_core::spi::EditedPaths::Keep)]
    );
    let loaded = store.load_promotions(task.task_id).unwrap();
    assert_eq!(loaded[0].state, PromotionState::Failed);
    assert!(
        loaded[0]
            .reason
            .as_deref()
            .unwrap()
            .contains("restored from its backup")
    );
    assert_eq!(
        store.load_promotions(applied.task_id).unwrap()[0].state,
        PromotionState::Applied,
        "an applied promotion is not touched"
    );
    let events = store.events_since(0, Some(task.task_id), 100).unwrap();
    assert!(
        events
            .iter()
            .any(|e| e.record.event.name() == "workspace.promotion_failed")
    );
    // A second start has nothing to do.
    let again = recover_promotions(&store, &context(&fx, &ids), &files).unwrap();
    assert!(again.is_empty());
    assert_eq!(files.reverted().len(), 1);
}

/// REQ-WS-007: when the restore itself fails, the promotion is still recorded `failed`, with that
/// reason, and the backup is kept.
#[test]
fn req_ws_007_a_failed_restore_at_start_is_recorded_and_the_backup_is_kept() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let fx = Fx::new();
    let store = open(&data).unwrap();
    let (task, backup, applying) = promoted(&store, &fx, PromotionState::Applying);
    let files = workspaces();
    files.fail_reverts();
    let ids = mimlys_core::ids::ScriptedIds::starting_at(1_000_000);
    let recovered = recover_promotions(&store, &context(&fx, &ids), &files).unwrap();
    assert!(recovered[0].restore_error.is_some());
    let loaded = store.load_promotions(task.task_id).unwrap();
    assert_eq!(loaded[0].promotion_id, applying.promotion_id);
    assert_eq!(loaded[0].state, PromotionState::Failed);
    assert!(
        loaded[0]
            .reason
            .as_deref()
            .unwrap()
            .contains("restoring the project from its backup failed")
    );
    assert!(store.load_snapshot(backup.snapshot_id).unwrap().is_some());
}
