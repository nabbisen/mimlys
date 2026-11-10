//! What the store holds, by kind and by task, from its own records
//! (REQ-WS-013, RFC 013): the snapshots' recorded sizes, the manifests'
//! object lists and ownership, the blob rows. Nothing is measured on disk and
//! no working tree is walked; the view of the files that have no record
//! (`tmp/`, the interpreters, the database) is the host's.

use super::RuntimeError;
use crate::ids::TaskId;
use crate::spi::{PersistenceRead, TaskSummary, WorkspaceRead};
use std::collections::{BTreeMap, BTreeSet};

/// What one task holds, from the store's records.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskHolding {
    /// The task.
    pub task: TaskSummary,
    /// Bytes of the working tree as of its latest snapshot (a promotion's
    /// backup is not the tree). Recorded, never measured by walking the tree.
    pub workspace_bytes: u64,
    /// Bytes of snapshot objects that no other task references.
    pub object_bytes_exclusive: u64,
    /// Bytes of the task's blobs.
    pub blob_bytes: u64,
    /// Snapshot rows.
    pub snapshots: u64,
}

impl TaskHolding {
    /// What deleting the task frees: its tree, the objects only it uses, its blobs.
    pub fn reclaimable_bytes(&self) -> u64 {
        self.workspace_bytes + self.object_bytes_exclusive + self.blob_bytes
    }
}

/// What the store holds, by kind, from its own records.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Holdings {
    /// Every task's holding, the largest first.
    pub tasks: Vec<TaskHolding>,
    /// Working trees, as recorded by each task's latest snapshot.
    pub workspace_bytes: u64,
    /// Snapshot objects referenced by one task only.
    pub object_bytes_exclusive: u64,
    /// Snapshot objects referenced by more than one task.
    pub object_bytes_shared: u64,
    /// Blobs.
    pub blob_bytes: u64,
    /// Manifests that could not be read: their objects are not counted.
    pub unreadable_manifests: u64,
}

pub(super) fn holdings(
    persistence: &dyn PersistenceRead,
    workspaces: &dyn WorkspaceRead,
) -> Result<Holdings, RuntimeError> {
    let mut out = Holdings::default();
    // object -> (size, the tasks whose manifests list it)
    let mut owners: BTreeMap<crate::hash::Hash, (u64, BTreeSet<TaskId>)> = BTreeMap::new();
    let mut tasks: BTreeMap<TaskId, TaskHolding> = BTreeMap::new();
    for summary in persistence.list_tasks()? {
        let id = summary.task_id;
        let snapshots = persistence.load_snapshots(id)?;
        for s in &snapshots {
            match workspaces.manifest(s) {
                Ok(manifest) => {
                    for (hash, size) in manifest.objects() {
                        owners
                            .entry(hash)
                            .or_insert((size, BTreeSet::new()))
                            .1
                            .insert(id);
                    }
                }
                Err(_) => out.unreadable_manifests += 1,
            }
        }
        let workspace_bytes = snapshots
            .iter()
            .rfind(|s| s.reason != crate::snapshot::SnapshotReason::Backup)
            .map_or(0, |s| s.bytes);
        let blob_bytes = persistence.blob_bytes(id)?;
        out.workspace_bytes += workspace_bytes;
        out.blob_bytes += blob_bytes;
        tasks.insert(
            id,
            TaskHolding {
                task: summary,
                workspace_bytes,
                object_bytes_exclusive: 0,
                blob_bytes,
                snapshots: snapshots.len() as u64,
            },
        );
    }
    for (size, who) in owners.values() {
        match who.iter().next() {
            Some(only) if who.len() == 1 => {
                out.object_bytes_exclusive += size;
                if let Some(t) = tasks.get_mut(only) {
                    t.object_bytes_exclusive += size;
                }
            }
            _ => out.object_bytes_shared += size,
        }
    }
    out.tasks = tasks.into_values().collect();
    out.tasks.sort_by(|a, b| {
        b.reclaimable_bytes()
            .cmp(&a.reclaimable_bytes())
            .then(a.task.task_id.to_string().cmp(&b.task.task_id.to_string()))
    });
    Ok(out)
}
