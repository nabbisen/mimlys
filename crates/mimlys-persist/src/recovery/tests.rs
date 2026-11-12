//! The last recovery step: pending snapshot journals (REQ-WS-012, external
//! design 16.3). The journals are made the way a crash leaves them: by
//! `SnapshotFiles::begin`, the object and manifest writes, and no `finish`.

use super::*;
use crate::integrity;
use crate::sqlite::open;
use crate::store::tests::{Fx, sequence};
use mimlys_core::hash::hash_bytes;
use mimlys_core::ids::SnapshotId;
use mimlys_core::policy::Recovery;
use mimlys_core::snapshot::{Entry, Manifest, SnapshotReason, SnapshotRecord};
use mimlys_core::task::Task;

struct World {
    _dir: tempfile::TempDir,
    data: std::path::PathBuf,
    store: Store,
    files: SnapshotFiles,
    fx: Fx,
    task: Task,
}

fn world() -> World {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let store = open(&data).unwrap();
    let files = SnapshotFiles::open(&data).unwrap();
    let fx = Fx::new();
    let (task, _) = sequence(&store, &fx, 0, false);
    World {
        _dir: dir,
        data,
        store,
        files,
        fx,
        task,
    }
}

fn manifest_of(content: &[u8]) -> Manifest {
    Manifest::new(vec![Entry::File {
        path: "f".into(),
        executable: false,
        size: content.len() as u64,
        hash: hash_bytes(content),
    }])
}

impl World {
    fn ctx(&self) -> RecoveryContext<'_> {
        RecoveryContext {
            ids: &self.fx.ids,
            now: self.fx.now(),
            policy_hash: hash_bytes(b"policy"),
            recovery: Recovery { auto_resume: false },
        }
    }

    /// A snapshot as far as the crash got: the journal (naming `listed`), the
    /// object of `content`, the manifest; with `row` the database row too.
    /// The journal is left pending either way.
    fn snapshot(&self, content: &[u8], listed: &[&[u8]], row: bool) -> SnapshotId {
        let id = SnapshotId::new(&self.fx.ids);
        let hashes: Vec<_> = listed.iter().map(|c| hash_bytes(c)).collect();
        self.files.begin(id, &hashes).unwrap();
        self.files
            .put_object(&hash_bytes(content), content)
            .unwrap();
        let manifest_hash = self.files.put_manifest(id, &manifest_of(content)).unwrap();
        if row {
            let record = SnapshotRecord {
                snapshot_id: id,
                task_id: self.task.task_id,
                reason: SnapshotReason::After,
                manifest_hash,
                bytes: content.len() as u64,
                files: 1,
                created_at: self.fx.now(),
                action_id: None,
                project_root: None,
            };
            self.store
                .record_snapshot(
                    &record,
                    &[(hash_bytes(content), content.len() as u64)],
                    &self.fx.event(
                        &self.task,
                        Event::WorkspaceSnapshot {
                            snapshot_id: id,
                            reason: "after".into(),
                        },
                    ),
                )
                .unwrap();
        }
        id
    }

    fn recovered_events(&self) -> Vec<Event> {
        self.store
            .events_since(0, None, 10_000)
            .unwrap()
            .into_iter()
            .map(|e| e.record.event)
            .filter(|e| matches!(e, Event::SnapshotJournalsRecovered { .. }))
            .collect()
    }

    fn pending(&self) -> Vec<SnapshotId> {
        self.files.pending().unwrap()
    }

    fn manifest_exists(&self, id: SnapshotId) -> bool {
        self.data
            .join("snapshots/manifests")
            .join(format!("{id}.json"))
            .exists()
    }
}

/// REQ-WS-012: a journal with an object and a manifest and no row is undone:
/// the three files go, the temporary file of an interrupted write goes, the
/// integrity check passes and the event records what was done.
#[test]
fn req_ws_012_a_journal_without_a_row_is_undone() {
    let w = world();
    let kept = w.snapshot(b"kept", &[b"kept"], true);
    w.files.finish(kept).unwrap();
    let crashed = w.snapshot(b"orphan", &[b"orphan"], false);
    let tmp = w
        .data
        .join("snapshots/objects")
        .join(format!("{}.tmp-1", hash_bytes(b"half").as_str()));
    std::fs::write(&tmp, b"ha").unwrap();
    assert_eq!(w.pending(), [crashed]);

    let report = recover_snapshot_journals(&w.store, &w.files, &w.ctx()).unwrap();

    assert_eq!(
        report,
        JournalReport {
            finished: 0,
            undone: 1,
            objects_removed: 1
        }
    );
    assert!(w.pending().is_empty());
    assert!(!w.files.has_object(&hash_bytes(b"orphan")));
    assert!(!w.manifest_exists(crashed));
    assert!(!tmp.exists(), "the temporary file of an interrupted write");
    assert!(w.files.has_object(&hash_bytes(b"kept")));
    assert!(w.manifest_exists(kept));
    integrity::check(&w.store, w.fx.now()).unwrap();
    assert_eq!(
        w.recovered_events(),
        [Event::SnapshotJournalsRecovered {
            finished: 0,
            undone: 1,
            objects_removed: 1
        }]
    );
}

/// REQ-WS-012: a journal whose row exists is finished: the journal goes, the
/// objects and the manifest stay.
#[test]
fn req_ws_012_a_journal_with_a_row_is_finished() {
    let w = world();
    let id = w.snapshot(b"content", &[b"content"], true);
    assert_eq!(w.pending(), [id]);

    let report = recover_snapshot_journals(&w.store, &w.files, &w.ctx()).unwrap();

    assert_eq!(
        report,
        JournalReport {
            finished: 1,
            undone: 0,
            objects_removed: 0
        }
    );
    assert!(w.pending().is_empty());
    assert!(w.files.has_object(&hash_bytes(b"content")));
    assert!(w.manifest_exists(id));
    integrity::check(&w.store, w.fx.now()).unwrap();
    assert_eq!(
        w.recovered_events(),
        [Event::SnapshotJournalsRecovered {
            finished: 1,
            undone: 0,
            objects_removed: 0
        }]
    );
}

/// REQ-WS-012: a stale journal (a store written by a release that never
/// recovered) lists an object that a later snapshot reused: the object stays,
/// the object no row references and the journal's manifest go, and the later
/// snapshot's manifest still reads back with its object.
#[test]
fn req_ws_012_a_stale_journal_keeps_what_a_row_references() {
    let w = world();
    // The crash: journal, objects `shared` and `lost`, manifest; no row.
    let stale = SnapshotId::new(&w.fx.ids);
    w.files
        .begin(stale, &[hash_bytes(b"shared"), hash_bytes(b"lost")])
        .unwrap();
    w.files
        .put_object(&hash_bytes(b"shared"), &b"shared"[..])
        .unwrap();
    w.files
        .put_object(&hash_bytes(b"lost"), &b"lost"[..])
        .unwrap();
    w.files
        .put_manifest(stale, &manifest_of(b"shared"))
        .unwrap();
    // Later, an old release snapshotted the same content: the object was
    // present, so it was not listed, and the row references it.
    let later = w.snapshot(b"shared", &[], true);
    w.files.finish(later).unwrap();
    assert_eq!(w.pending(), [stale]);

    let report = recover_snapshot_journals(&w.store, &w.files, &w.ctx()).unwrap();

    assert_eq!(
        report,
        JournalReport {
            finished: 0,
            undone: 1,
            objects_removed: 1
        }
    );
    assert!(
        w.files.has_object(&hash_bytes(b"shared")),
        "a row references it"
    );
    assert!(!w.files.has_object(&hash_bytes(b"lost")));
    assert!(!w.manifest_exists(stale));
    let record = w.store.load_snapshot(later).unwrap().unwrap();
    let manifest = w.files.read_manifest(later, &record.manifest_hash).unwrap();
    for hash in manifest.objects().keys() {
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut w.files.open_object(hash).unwrap(), &mut bytes).unwrap();
        assert_eq!(hash_bytes(&bytes), *hash, "the object reads back");
    }
    integrity::check(&w.store, w.fx.now()).unwrap();
}

/// REQ-WS-012: with nothing pending nothing is recorded.
#[test]
fn req_ws_012_nothing_pending_records_nothing() {
    let w = world();
    let done = w.snapshot(b"done", &[b"done"], true);
    w.files.finish(done).unwrap();
    let before = w.store.events_since(0, None, 10_000).unwrap().len();

    let report = recover_snapshot_journals(&w.store, &w.files, &w.ctx()).unwrap();

    assert_eq!(report, JournalReport::default());
    assert!(w.recovered_events().is_empty());
    assert_eq!(w.store.events_since(0, None, 10_000).unwrap().len(), before);
}
