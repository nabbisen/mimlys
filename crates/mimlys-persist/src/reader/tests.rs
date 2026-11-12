use super::*;
use crate::integrity;
use crate::recovery::{RecoveryContext, recover_all};
use crate::sqlite::{HOLDER_FILE, open};
use crate::store::tests::{Fx, sequence};
use mimlys_core::event::Event;
use mimlys_core::hash::hash_bytes;
use mimlys_core::policy::Recovery;
use mimlys_core::spi::{Persistence, PersistenceError, PersistenceRead};
use std::marker::PhantomData;

/// Whether `T` implements the writer's trait, decided at compile time: the
/// inherent constant wins where `T: Persistence` holds, the fallback otherwise.
struct Check<T: ?Sized>(PhantomData<T>);
trait Fallback {
    const WRITES: bool = false;
}
impl<T: ?Sized> Fallback for Check<T> {}
impl<T: ?Sized + Persistence> Check<T> {
    const WRITES: bool = true;
}

fn fake_workspaces() -> mimlys_core::spi::fake::FakeWorkspaces {
    mimlys_core::spi::fake::FakeWorkspaces::new(std::sync::Arc::new(
        mimlys_core::spi::fake::FakeToolProvider::new(vec![]),
    ))
}

/// The bytes of the database files, to show that a read changed none.
fn db_files(data: &std::path::Path) -> Vec<(String, Vec<u8>)> {
    ["mimlys.db", "mimlys.db-wal"]
        .iter()
        .map(|name| {
            (
                (*name).to_owned(),
                std::fs::read(data.join(name)).unwrap_or_default(),
            )
        })
        .collect()
}

/// REQ-PERS-013: the reader's handle has no writing method. The type does not
/// implement the writer's trait (while the writer's handle does), and a call
/// of a writing method on it does not compile (the doctest of `ReadStore`).
#[test]
fn req_pers_013_the_readers_handle_has_no_writing_method() {
    const {
        assert!(Check::<crate::Store>::WRITES, "the check sees a writer");
        assert!(!Check::<ReadStore>::WRITES, "a reader must not write");
        assert!(!Check::<dyn PersistenceRead>::WRITES);
    }
}

/// REQ-PERS-013: the connection is SQLite's read-only mode as well, so that
/// a statement that writes is refused by the database, not only by the type.
#[test]
fn req_pers_013_the_readers_connection_is_read_only() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    drop(open(&data).unwrap());
    let reader = open_read_only(&data).unwrap();
    let conn = reader.conn.lock().unwrap();
    let err = conn
        .execute("UPDATE schema_version SET version = 7", [])
        .expect_err("a read-only connection refuses a write");
    assert!(err.to_string().contains("readonly"), "{err}");
}

/// REQ-PERS-013: a reader opens while a writer holds the lock, reads the
/// running task's state and events, and names the holder.
#[test]
fn req_pers_013_a_reader_reads_while_a_writer_holds_the_lock() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let fx = Fx::new();
    let writer = open(&data).unwrap();
    let (task, _) = sequence(&writer, &fx, 1, false);

    let reader = open_read_only(&data).expect("a reader is never refused as locked");
    match reader.lock_state() {
        LockState::Held(Some(holder)) => {
            assert_eq!(holder.pid, std::process::id());
            assert!(holder.since.ends_with('Z'), "{}", holder.since);
        }
        other => panic!("the lock is held by this process: {other:?}"),
    }
    let loaded = reader.load_task(task.task_id).unwrap().unwrap();
    assert_eq!(loaded.state.name(), "running");
    assert_eq!(reader.tasks_in_state("running").unwrap(), [task.task_id]);
    let events = reader.events_since(0, Some(task.task_id), 100).unwrap();
    assert!(events.len() >= 2, "{events:?}");
    assert!(
        !reader.recovery_pending().unwrap(),
        "a running task of a live runtime is not a store that needs recovery"
    );

    // What the writer commits next, the reader sees on its next read.
    let before = reader.events_since(0, None, 1000).unwrap().len();
    writer
        .append_event(&fx.event(&task, Event::RuntimeStopped))
        .unwrap();
    assert_eq!(
        reader.events_since(0, None, 1000).unwrap().len(),
        before + 1
    );
}

/// REQ-PERS-013: a reader does not take the lock. With a reader open, a
/// writer opens at once; and the reader, with nothing held, reports `Free`.
#[test]
fn req_pers_013_a_reader_does_not_hold_the_lock() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    drop(open(&data).unwrap());
    let reader = open_read_only(&data).unwrap();
    assert_eq!(reader.lock_state(), &LockState::Free);
    // Decided by state, not by elapsed time: a lock the reader held would keep
    // the writer out for the whole of LIM-O3 and then refuse it (`Locked`), so
    // that the open succeeding is the proof that there was nothing to wait for.
    let writer = open(&data).expect("an open reader does not keep a writer out");
    drop(writer);
    // And the other way round: a second reader beside a writer.
    let writer = open(&data).unwrap();
    let second = open_read_only(&data).unwrap();
    assert!(matches!(second.lock_state(), LockState::Held(_)));
    drop((reader, second, writer));
}

/// REQ-PERS-013: the writer records itself beside the lock and removes the
/// record when it ends cleanly; a reader trusts the record only while the
/// lock is held.
#[test]
fn req_pers_013_the_holder_is_recorded_while_the_lock_is_held() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let writer = open(&data).unwrap();
    let text = std::fs::read_to_string(data.join(HOLDER_FILE)).unwrap();
    let holder: Holder = serde_json::from_str(&text).unwrap();
    assert_eq!(holder.pid, std::process::id());
    drop(writer);
    assert!(!data.join(HOLDER_FILE).exists());

    // A record left by a runtime that ended uncleanly names no holder.
    std::fs::write(
        data.join(HOLDER_FILE),
        r#"{"pid":1,"since":"2026-01-01T00:00:00Z"}"#,
    )
    .unwrap();
    let reader = open_read_only(&data).unwrap();
    assert_eq!(reader.lock_state(), &LockState::Free);
}

/// REQ-PERS-013: the reader runs no recovery. A store whose runtime ended
/// with a task `running` is read as committed, says recovery is pending, and
/// is byte for byte what it was; the writer's recovery then does its work.
#[test]
fn req_pers_013_the_reader_runs_no_recovery() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let fx = Fx::new();
    let task = {
        let writer = open(&data).unwrap();
        let (task, _) = sequence(&writer, &fx, 2, true);
        task
    };
    // The runtime ended with the task `running`: nothing halted it and nothing
    // recovered it. A crash also leaves its holder record behind.
    std::fs::write(
        data.join(HOLDER_FILE),
        r#"{"pid":4194303,"since":"2026-01-01T00:00:00Z"}"#,
    )
    .unwrap();
    let before = db_files(&data);

    let reader = open_read_only(&data).unwrap();
    assert_eq!(reader.lock_state(), &LockState::Free);
    assert!(reader.recovery_pending().unwrap());
    assert_eq!(
        reader
            .load_task(task.task_id)
            .unwrap()
            .unwrap()
            .state
            .name(),
        "running",
        "read as committed"
    );
    let events = reader.events_since(0, None, 1000).unwrap().len();
    drop(reader);
    assert_eq!(db_files(&data), before, "a reader writes nothing");

    // The same store, opened by a writer: recovery acts on what the reader left.
    let writer = open(&data).unwrap();
    assert_eq!(writer.events_since(0, None, 1000).unwrap().len(), events);
    let ctx = RecoveryContext {
        ids: &fx.ids,
        now: fx.now(),
        policy_hash: hash_bytes(b"policy"),
        recovery: Recovery { auto_resume: false },
    };
    let recovered = recover_all(&writer, &ctx, &fake_workspaces()).unwrap();
    assert_eq!(recovered.len(), 1);
    integrity::check(&writer, fx.now()).unwrap();
    drop(writer);
    let reader = open_read_only(&data).unwrap();
    assert!(!reader.recovery_pending().unwrap());
}

/// REQ-PERS-013: a schema newer than the reader knows is refused with the
/// reason, and the database is left as it was (not migrated, not written).
#[test]
fn req_pers_013_a_newer_schema_is_refused_and_not_touched() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    {
        let writer = open(&data).unwrap();
        writer
            .with_conn(|c| {
                c.execute("UPDATE schema_version SET version = 99", [])
                    .unwrap()
            })
            .unwrap();
    }
    let before = db_files(&data);
    let err = open_read_only(&data).expect_err("refused");
    assert_eq!(
        err,
        PersistenceError::NewerSchema {
            found: 99,
            supported: crate::SCHEMA_VERSION
        }
    );
    assert!(
        err.to_string().contains("99"),
        "the reason is stated: {err}"
    );
    assert_eq!(db_files(&data), before);
}

/// REQ-PERS-013: an older schema is not migrated by a reader; it says
/// `migration pending` (version 1 lacks the snapshot tables every task view reads).
#[test]
fn req_pers_013_an_older_schema_says_migration_pending() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    // A store as the first schema left it (the migration of version 1 alone).
    std::fs::create_dir_all(&data).unwrap();
    {
        let conn = rusqlite::Connection::open(data.join("mimlys.db")).unwrap();
        conn.execute_batch("PRAGMA journal_mode = WAL;").unwrap();
        conn.execute_batch(include_str!("../migrations/001_initial.sql"))
            .unwrap();
    }
    let before = db_files(&data);
    let err = open_read_only(&data).expect_err("not read");
    assert_eq!(
        err,
        PersistenceError::MigrationPending {
            found: 1,
            supported: crate::SCHEMA_VERSION
        }
    );
    assert!(err.to_string().starts_with("migration pending"), "{err}");
    assert_eq!(db_files(&data), before, "nothing migrated");
    // The writer still migrates it, as before.
    let writer = open(&data).unwrap();
    assert_eq!(writer.schema_version().unwrap(), crate::SCHEMA_VERSION);
}

/// REQ-PERS-013: a data directory with no database is read as an empty
/// store, and the read creates nothing.
#[test]
fn req_pers_013_an_absent_store_is_empty_and_nothing_is_created() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("never-created");
    let reader = open_read_only(&data).unwrap();
    assert!(!reader.exists());
    assert!(reader.list_tasks().unwrap().is_empty());
    assert!(reader.tasks_in_state("running").unwrap().is_empty());
    assert!(!reader.recovery_pending().unwrap());
    drop(reader);
    assert!(!data.exists(), "a reader creates no directory");

    // An existing directory gets no lock file, no database and no folders.
    std::fs::create_dir_all(&data).unwrap();
    drop(open_read_only(&data).unwrap());
    assert_eq!(std::fs::read_dir(&data).unwrap().count(), 0);
}

/// A workspace stub whose manifests are those it was given, by snapshot.
struct Manifests(
    std::collections::BTreeMap<mimlys_core::ids::SnapshotId, mimlys_core::snapshot::Manifest>,
);

impl mimlys_core::spi::WorkspaceRead for Manifests {
    fn manifest(
        &self,
        snapshot: &mimlys_core::snapshot::SnapshotRecord,
    ) -> Result<mimlys_core::snapshot::Manifest, mimlys_core::spi::WorkspaceError> {
        self.0
            .get(&snapshot.snapshot_id)
            .cloned()
            .ok_or(mimlys_core::spi::WorkspaceError::Missing)
    }

    fn promotion_plan(
        &self,
        _task_id: mimlys_core::ids::TaskId,
        _base: &mimlys_core::snapshot::SnapshotRecord,
        _project_root: &str,
    ) -> Result<mimlys_core::snapshot::PromotionPlan, mimlys_core::spi::WorkspaceError> {
        Err(mimlys_core::spi::WorkspaceError::Missing)
    }

    fn tree_bytes(
        &self,
        _task_id: mimlys_core::ids::TaskId,
    ) -> Result<u64, mimlys_core::spi::WorkspaceError> {
        panic!("the storage view never walks a working tree");
    }
}

/// REQ-WS-013: the holdings come from the records. Objects only one task's
/// manifests list are exclusive to it, objects two tasks list are shared, a
/// promotion's backup counts as objects but not as the working tree, the tree
/// is the latest snapshot that is not a backup, blobs are per task; no working
/// tree is walked (the stub panics if it is asked).
#[test]
fn req_ws_013_holdings_come_from_the_records() {
    use mimlys_core::snapshot::{Entry, Manifest, SnapshotReason, SnapshotRecord};
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let fx = Fx::new();
    let writer = open(&data).unwrap();
    let (a, _) = sequence(&writer, &fx, 0, false);
    let (b, _) = sequence(&writer, &fx, 0, false);

    let file = |path: &str, content: &[u8]| Entry::File {
        path: path.into(),
        executable: false,
        size: content.len() as u64,
        hash: hash_bytes(content),
    };
    let mut manifests = std::collections::BTreeMap::new();
    let mut snapshot = |task: &mimlys_core::task::Task,
                        reason: SnapshotReason,
                        entries: Vec<Entry>,
                        bytes: u64| {
        let manifest = Manifest::new(entries);
        let id = mimlys_core::ids::SnapshotId::new(&fx.ids);
        let objects: Vec<_> = manifest.objects().into_iter().collect();
        writer
            .record_snapshot(
                &SnapshotRecord {
                    snapshot_id: id,
                    task_id: task.task_id,
                    reason,
                    manifest_hash: manifest.hash(),
                    bytes,
                    files: 0,
                    created_at: fx.now(),
                    action_id: None,
                    project_root: None,
                },
                &objects,
                &fx.event(task, Event::RuntimeStopped),
            )
            .unwrap();
        manifests.insert(id, manifest);
    };
    // A: base {f1 10, f2 20}; after {f1, f3 5} (tree 15); backup {f5 3}.
    snapshot(
        &a,
        SnapshotReason::Base,
        vec![file("f1", &[1; 10]), file("f2", &[2; 20])],
        30,
    );
    snapshot(
        &a,
        SnapshotReason::After,
        vec![file("f1", &[1; 10]), file("f3", &[3; 5])],
        15,
    );
    snapshot(&a, SnapshotReason::Backup, vec![file("f5", &[5; 3])], 0);
    // B: base {f1 (the same content as A's), f4 7}.
    snapshot(
        &b,
        SnapshotReason::Base,
        vec![file("f1", &[1; 10]), file("f4", &[4; 7])],
        17,
    );
    writer.put_blob(a.task_id, &[0; 100]).unwrap();
    writer.put_blob(b.task_id, &[0; 40]).unwrap();
    drop(writer);

    let reader = std::sync::Arc::new(open_read_only(&data).unwrap());
    let view = mimlys_core::runtime::Reader::new(
        reader,
        std::sync::Arc::new(Manifests(manifests)),
        std::sync::Arc::new(mimlys_core::clock::SystemClock),
    );
    let holdings = view.holdings().unwrap();
    assert_eq!(
        holdings.object_bytes_shared, 10,
        "f1 is listed by both tasks"
    );
    assert_eq!(holdings.object_bytes_exclusive, 20 + 5 + 3 + 7);
    assert_eq!(
        holdings.workspace_bytes,
        15 + 17,
        "the latest snapshot that is not a backup"
    );
    assert_eq!(holdings.blob_bytes, 140);
    assert_eq!(holdings.unreadable_manifests, 0);
    let of = |id| {
        holdings
            .tasks
            .iter()
            .find(|t| t.task.task_id == id)
            .unwrap()
    };
    assert_eq!(of(a.task_id).object_bytes_exclusive, 28);
    assert_eq!(of(a.task_id).blob_bytes, 100);
    assert_eq!(of(b.task_id).object_bytes_exclusive, 7);
    assert_eq!(of(b.task_id).blob_bytes, 40);
    assert_eq!(of(a.task_id).snapshots, 3);
    // Largest first: A (15 + 28 + 100) holds more than B (17 + 7 + 40).
    assert_eq!(holdings.tasks[0].task.task_id, a.task_id);
}
