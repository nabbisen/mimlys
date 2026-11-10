//! The storage view (REQ-WS-013, external design §14.6; RFC 013): what the
//! data directory holds, by kind, and which tasks hold the most. The numbers
//! come from the store's own records (the snapshots' recorded sizes, the
//! manifests and their ownership, the blob rows); only the files that have no
//! record are measured on disk: `tmp/`, the interpreters and the database
//! files. No working tree is walked. Read-only.

use crate::app::{AppError, ReadApp};
use mimlys_core::clock::Instant;
use serde::Serialize;
use std::path::Path;

/// How many tasks `status --storage` lists in text (the JSON lists them all).
/// Not in the limits register: proposed to the architect in the review request
/// of handoff 013.
pub const STORAGE_TASKS_SHOWN: usize = 10;

/// Whether a filesystem can share the blocks of a copy (a reflink). Judged
/// from the filesystem's type: nothing is written to find out.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Reflink {
    /// The type always can (btrfs, bcachefs, APFS, ReFS).
    Yes,
    /// The type never can (ext4, tmpfs, NTFS, FAT, network filesystems …).
    No,
    /// It depends on how the filesystem was made or what lies under it (XFS
    /// made with `reflink=1`, ZFS with block cloning, overlay, FUSE), or the
    /// type is not known here.
    Unknown,
}

impl Reflink {
    /// The word `status --storage` prints.
    pub fn word(self) -> &'static str {
        match self {
            Reflink::Yes => "yes",
            Reflink::No => "no",
            Reflink::Unknown => "unknown",
        }
    }
}

/// Whether the filesystem named `name` (as `mountinfo`, `mount` or the volume
/// names it, lowercase) can reflink. The book's Storage page states the same table.
pub fn reflink_of(name: &str) -> Reflink {
    match name {
        "btrfs" | "bcachefs" | "apfs" | "refs" => Reflink::Yes,
        "ext2" | "ext3" | "ext4" | "tmpfs" | "ramfs" | "f2fs" | "vfat" | "fat" | "msdos"
        | "exfat" | "ntfs" | "ntfs3" | "hfs" | "hfsplus" | "nfs" | "nfs4" | "cifs" | "smb3"
        | "smbfs" | "9p" | "iso9660" | "squashfs" | "erofs" | "fat32" | "udf" => Reflink::No,
        _ => Reflink::Unknown,
    }
}

/// The filesystem that holds the store.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Filesystem {
    /// Its name as the platform gives it, when it could be told.
    pub name: Option<String>,
    /// Whether it reflinks.
    pub reflink: Reflink,
}

/// Bytes by kind.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Bytes {
    /// Working trees, as the latest snapshot of each task records them.
    pub working_trees: u64,
    /// Snapshot objects only one task uses.
    pub snapshot_objects_exclusive: u64,
    /// Snapshot objects more than one task uses.
    pub snapshot_objects_shared: u64,
    /// Captured outputs.
    pub blobs: u64,
    /// Installed interpreters (measured).
    pub interpreters: u64,
    /// The database, its write-ahead log and its shared-memory index (measured).
    pub database: u64,
    /// `tmp/` and the tasks' temporary directories (measured).
    pub temporary: u64,
    /// The sum of the kinds above.
    pub total: u64,
}

/// What one task holds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TaskBytes {
    /// The task.
    pub task_id: String,
    /// Its state.
    pub state: String,
    /// Its goal.
    pub goal: String,
    /// When it was created (RFC 3339).
    pub created_at: String,
    /// When it last changed state (RFC 3339); a terminal task's age counts from here.
    pub updated_at: String,
    /// Seconds since `updated_at`.
    pub age_seconds: i64,
    /// Its working tree.
    pub working_tree: u64,
    /// Snapshot objects only it uses.
    pub snapshot_objects_exclusive: u64,
    /// Its blobs.
    pub blobs: u64,
    /// What deleting it frees: the three above.
    pub total: u64,
}

/// `status --storage`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct StorageView {
    /// The data directory.
    pub path: String,
    /// Its filesystem.
    pub filesystem: Filesystem,
    /// Bytes by kind.
    pub bytes: Bytes,
    /// Every task, the one that holds the most first.
    pub tasks: Vec<TaskBytes>,
    /// Manifests that could not be read: their objects are not in the sums.
    pub unreadable_manifests: u64,
}

/// Builds the view: the records through the reader, the files that have no
/// record by measuring them.
pub fn build(app: &ReadApp, now: Instant) -> Result<StorageView, AppError> {
    let holdings = app.reader.holdings()?;
    let data = &app.data_dir;
    let fs_name = mimlys_exec::fsinfo::filesystem_name(data);
    let filesystem = Filesystem {
        reflink: fs_name.as_deref().map_or(Reflink::Unknown, reflink_of),
        name: fs_name,
    };
    let mut bytes = Bytes {
        working_trees: holdings.workspace_bytes,
        snapshot_objects_exclusive: holdings.object_bytes_exclusive,
        snapshot_objects_shared: holdings.object_bytes_shared,
        blobs: holdings.blob_bytes,
        interpreters: dir_bytes(&data.join("interpreters")),
        database: ["mimlys.db", "mimlys.db-wal", "mimlys.db-shm"]
            .iter()
            .map(|f| file_bytes(&data.join(f)))
            .sum(),
        temporary: temporary_bytes(data),
        total: 0,
    };
    bytes.total = bytes.working_trees
        + bytes.snapshot_objects_exclusive
        + bytes.snapshot_objects_shared
        + bytes.blobs
        + bytes.interpreters
        + bytes.database
        + bytes.temporary;
    let tasks = holdings
        .tasks
        .iter()
        .map(|t| TaskBytes {
            task_id: t.task.task_id.to_string(),
            state: t.task.state.clone(),
            goal: t.task.goal.clone(),
            created_at: t.task.created_at.to_rfc3339(),
            updated_at: t.task.updated_at.to_rfc3339(),
            age_seconds: t.task.updated_at.millis_until(now) / 1000,
            working_tree: t.workspace_bytes,
            snapshot_objects_exclusive: t.object_bytes_exclusive,
            blobs: t.blob_bytes,
            total: t.reclaimable_bytes(),
        })
        .collect();
    Ok(StorageView {
        path: data.to_string_lossy().into_owned(),
        filesystem,
        bytes,
        tasks,
        unreadable_manifests: holdings.unreadable_manifests,
    })
}

/// `tmp/` and each task's `workspaces/<id>/tmp`.
fn temporary_bytes(data: &Path) -> u64 {
    let mut total = dir_bytes(&data.join("tmp"));
    if let Ok(entries) = std::fs::read_dir(data.join("workspaces")) {
        for entry in entries.flatten() {
            total += dir_bytes(&entry.path().join("tmp"));
        }
    }
    total
}

fn file_bytes(path: &Path) -> u64 {
    std::fs::symlink_metadata(path)
        .ok()
        .filter(|m| m.is_file())
        .map_or(0, |m| m.len())
}

/// The bytes of the regular files under `dir`, links not followed. A file that
/// goes while it is being counted counts for nothing.
pub fn dir_bytes(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .map(|entry| match entry.file_type() {
            Ok(t) if t.is_dir() => dir_bytes(&entry.path()),
            Ok(t) if t.is_file() => entry.metadata().map_or(0, |m| m.len()),
            _ => 0,
        })
        .sum()
}

#[cfg(test)]
mod tests;
