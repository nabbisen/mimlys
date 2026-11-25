//! The files of a task deletion are journaled before its rows go (handoff 006b; REQ-WS-012,
//! RFC 006 §6): whatever happens to the removals, the next start finishes them, so no manifest and
//! no object of a deleted task is left for good.

use super::tests::Fx;
use crate::recovery::recover_deletions;
use crate::snapshots::SnapshotFiles;
use crate::sqlite::open;
use mimlys_core::event::Event;
use mimlys_core::hash::hash_bytes;
use mimlys_core::ids::SnapshotId;
use mimlys_core::snapshot::{Entry, Manifest, SnapshotReason, SnapshotRecord};
use mimlys_core::spi::{Persistence, PersistenceRead};
use mimlys_core::task::Task;

/// A task with one snapshot of one file, its object and manifest written, its row recorded.
fn task_with_a_snapshot(
    store: &dyn Persistence,
    files: &SnapshotFiles,
    fx: &Fx,
    content: &[u8],
) -> (Task, SnapshotRecord) {
    let task = fx.task();
    store
        .create_task(
            &task,
            &[],
            &fx.event(
                &task,
                Event::TaskCreated {
                    plan_first: false,
                    effort: None,
                },
            ),
        )
        .unwrap();
    let hash = hash_bytes(content);
    files.put_object(&hash, content).unwrap();
    let manifest = Manifest::new(vec![Entry::File {
        path: "f".into(),
        executable: false,
        mode: None,
        size: content.len() as u64,
        hash: hash.clone(),
    }]);
    let id = SnapshotId::new(&fx.ids);
    files.begin(id, std::slice::from_ref(&hash)).unwrap();
    let manifest_hash = files.put_manifest(id, &manifest).unwrap();
    let record = SnapshotRecord {
        snapshot_id: id,
        task_id: task.task_id,
        reason: SnapshotReason::Base,
        manifest_hash,
        bytes: content.len() as u64,
        files: 1,
        created_at: fx.now(),
        action_id: None,
        project_root: None,
    };
    store
        .record_snapshot(
            &record,
            &[(hash, content.len() as u64)],
            &fx.event(
                &task,
                Event::WorkspaceSnapshot {
                    snapshot_id: id,
                    reason: "base".into(),
                },
            ),
        )
        .unwrap();
    files.finish(id).unwrap();
    (task, record)
}

fn workspaces() -> mimlys_core::spi::fake::FakeWorkspaces {
    mimlys_core::spi::fake::FakeWorkspaces::new(std::sync::Arc::new(
        mimlys_core::spi::fake::FakeToolProvider::new(vec![]),
    ))
}

/// REQ-WS-012: the journal lists exactly what the deletion released, is on disk when the rows are
/// gone, and a start that finds it after a crash (the task's rows deleted, the files not) takes the
/// files and closes it; the next start finds nothing.
#[test]
fn req_ws_012_a_committed_deletion_is_finished_by_the_next_start() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let fx = Fx::new();
    let store = open(&data).unwrap();
    let files = SnapshotFiles::open(&data).unwrap();
    let (task, record) = task_with_a_snapshot(&store, &files, &fx, b"only this task holds it");
    let released = store
        .delete_task_data(
            task.task_id,
            false,
            &fx.event(&task, Event::TaskDeleted { audit: false }),
        )
        .unwrap();
    assert_eq!(released.snapshots, [record.snapshot_id]);
    assert_eq!(released.objects_at_zero.len(), 1);
    // The process dies here: the rows are gone, the files are not removed.
    let pending = files.pending_deletions().unwrap();
    assert_eq!(pending, [(task.task_id, released.clone())]);
    assert!(files.has_object(&released.objects_at_zero[0]));
    let ws = workspaces();
    let report = recover_deletions(&store, &files, &ws).unwrap();
    assert_eq!((report.finished, report.discarded), (1, 0));
    assert!(report.failed.is_empty());
}

/// REQ-WS-012: a journal whose deletion never committed (the task still holds its rows) takes
/// nothing: the files stay, and the journal is dropped.
#[test]
fn req_ws_012_a_deletion_that_never_committed_removes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let fx = Fx::new();
    let store = open(&data).unwrap();
    let files = SnapshotFiles::open(&data).unwrap();
    let (task, record) = task_with_a_snapshot(&store, &files, &fx, b"kept");
    // The journal was written, and the process died before the transaction.
    let rows = mimlys_core::spi::DeletedRows {
        objects_at_zero: vec![hash_bytes(b"kept")],
        snapshots: vec![record.snapshot_id],
        blobs: vec![],
    };
    files.begin_deletion(task.task_id, &rows).unwrap();
    let ws = workspaces();
    let report = recover_deletions(&store, &files, &ws).unwrap();
    assert_eq!((report.finished, report.discarded), (0, 1));
    assert!(files.pending_deletions().unwrap().is_empty());
    assert!(files.has_object(&hash_bytes(b"kept")));
    assert!(store.load_snapshot(record.snapshot_id).unwrap().is_some());
}

/// REQ-WS-012: an object another task also references is not in the journal.
#[test]
fn req_ws_012_the_journal_lists_only_what_the_task_alone_referenced() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let fx = Fx::new();
    let store = open(&data).unwrap();
    let files = SnapshotFiles::open(&data).unwrap();
    let (a, _) = task_with_a_snapshot(&store, &files, &fx, b"shared");
    let (_b, _) = task_with_a_snapshot(&store, &files, &fx, b"shared");
    let released = store
        .delete_task_data(
            a.task_id,
            false,
            &fx.event(&a, Event::TaskDeleted { audit: false }),
        )
        .unwrap();
    assert!(
        released.objects_at_zero.is_empty(),
        "the other task holds it"
    );
    assert_eq!(files.pending_deletions().unwrap()[0].1, released);
}
