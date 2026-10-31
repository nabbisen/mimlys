//! Snapshots as pure data (external design §14.3 … §14.5; REQ-WS-004,
//! REQ-WS-005, REQ-WS-007): the manifest of a tree, its canonical hash,
//! the difference between two manifests, the rule for a symbolic link's
//! target, and the records the control API shows. No I/O here; the stores
//! live in `mimlys-persist` and the walks in `mimlys`.

use crate::clock::Instant;
use crate::hash::{Hash, hash_bytes};
use crate::ids::{ActionId, PromotionId, SnapshotId, TaskId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Component, Path};

/// One entry of a manifest. Paths are workspace-relative, `/`-separated,
/// canonical (handoff 002); entries are sorted by path.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Entry {
    /// A regular file, stored as the object `hash`.
    File {
        /// Path.
        path: String,
        /// Executable bit (POSIX).
        executable: bool,
        /// Size in bytes.
        size: u64,
        /// SHA-256 of the content (LIM-U6).
        hash: Hash,
    },
    /// A directory (present even when empty).
    Dir {
        /// Path.
        path: String,
    },
    /// A symbolic link, stored as its raw target, never followed.
    Symlink {
        /// Path.
        path: String,
        /// The link's target as written.
        target: String,
    },
}

impl Entry {
    /// The entry's path.
    pub fn path(&self) -> &str {
        match self {
            Entry::File { path, .. } | Entry::Dir { path } | Entry::Symlink { path, .. } => path,
        }
    }

    /// Whether two entries have the same content (same type and payload).
    fn same_content(&self, other: &Entry) -> bool {
        match (self, other) {
            (
                Entry::File {
                    executable: a,
                    size: s,
                    hash: h,
                    ..
                },
                Entry::File {
                    executable: b,
                    size: t,
                    hash: i,
                    ..
                },
            ) => a == b && s == t && h == i,
            (Entry::Dir { .. }, Entry::Dir { .. }) => true,
            (Entry::Symlink { target: a, .. }, Entry::Symlink { target: b, .. }) => a == b,
            _ => false,
        }
    }
}

/// A snapshot's content: the tree's entries, sorted by path.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Manifest {
    /// Entries sorted by path (byte order).
    pub entries: Vec<Entry>,
}

impl Manifest {
    /// Builds a manifest, sorting the entries.
    pub fn new(mut entries: Vec<Entry>) -> Self {
        entries.sort_by(|a, b| a.path().as_bytes().cmp(b.path().as_bytes()));
        Self { entries }
    }

    /// The canonical bytes (sorted entries, canonical JSON) that the hash covers.
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let value = serde_json::to_value(self).expect("a manifest serializes");
        crate::hash::canonical_json(&value).into_bytes()
    }

    /// The manifest's hash (LIM-U6).
    pub fn hash(&self) -> Hash {
        hash_bytes(&self.canonical_bytes())
    }

    /// Total bytes of regular files.
    pub fn bytes(&self) -> u64 {
        self.entries
            .iter()
            .map(|e| match e {
                Entry::File { size, .. } => *size,
                _ => 0,
            })
            .sum()
    }

    /// Number of regular files.
    pub fn files(&self) -> u64 {
        self.entries
            .iter()
            .filter(|e| matches!(e, Entry::File { .. }))
            .count() as u64
    }

    /// Entries by path.
    pub fn by_path(&self) -> BTreeMap<&str, &Entry> {
        self.entries.iter().map(|e| (e.path(), e)).collect()
    }

    /// The object hashes the manifest references, with their sizes, once each.
    pub fn objects(&self) -> BTreeMap<Hash, u64> {
        let mut out = BTreeMap::new();
        for e in &self.entries {
            if let Entry::File { hash, size, .. } = e {
                out.insert(hash.clone(), *size);
            }
        }
        out
    }
}

/// How one path changed between two manifests.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    /// Present in the new manifest only.
    Added,
    /// Present in both with different content.
    Modified,
    /// Present in the old manifest only.
    Deleted,
}

/// One changed path.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Change {
    /// Path.
    pub path: String,
    /// How.
    pub kind: ChangeKind,
    /// Bytes of the new content (0 for a deletion or a non-file).
    pub bytes: u64,
}

/// The difference between two manifests, sorted by path.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Diff {
    /// Changed paths (files and links; directories only when added or deleted).
    pub changes: Vec<Change>,
}

impl Diff {
    /// Whether nothing changed.
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }
}

/// Computes `diff(base, current)` (external design §14.5).
pub fn diff(base: &Manifest, current: &Manifest) -> Diff {
    let old = base.by_path();
    let new = current.by_path();
    let mut changes = Vec::new();
    for (path, entry) in &new {
        let bytes = match entry {
            Entry::File { size, .. } => *size,
            _ => 0,
        };
        match old.get(path) {
            None => changes.push(Change {
                path: (*path).to_owned(),
                kind: ChangeKind::Added,
                bytes,
            }),
            Some(before) if !before.same_content(entry) => changes.push(Change {
                path: (*path).to_owned(),
                kind: ChangeKind::Modified,
                bytes,
            }),
            Some(_) => {}
        }
    }
    for path in old.keys() {
        if !new.contains_key(path) {
            changes.push(Change {
                path: (*path).to_owned(),
                kind: ChangeKind::Deleted,
                bytes: 0,
            });
        }
    }
    changes.sort_by(|a, b| a.path.as_bytes().cmp(b.path.as_bytes()));
    Diff { changes }
}

/// Whether a link at `link` (workspace-relative) pointing to `target`
/// stays inside the root, resolved lexically (REQ-WS-005): never absolute,
/// never more `..` than the link's depth.
pub fn target_inside_root(link: &str, target: &str) -> bool {
    // Absolute on any platform: a leading separator, a drive letter, or a UNC prefix.
    let bytes = target.as_bytes();
    if bytes.first().is_some_and(|b| *b == b'/' || *b == b'\\')
        || (bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':')
    {
        return false;
    }
    let target = Path::new(target);
    if target.is_absolute() || target.has_root() {
        return false;
    }
    let mut depth: i64 = link.split('/').filter(|c| !c.is_empty()).count() as i64 - 1;
    for component in target.components() {
        match component {
            Component::Normal(part) => {
                // A backslash-separated segment is split on every platform (REQ-WS-011).
                let text = part.to_string_lossy();
                for piece in text.split('\\') {
                    match piece {
                        "" | "." => {}
                        ".." => {
                            depth -= 1;
                            if depth < 0 {
                                return false;
                            }
                        }
                        _ => depth += 1,
                    }
                }
            }
            Component::ParentDir => {
                depth -= 1;
                if depth < 0 {
                    return false;
                }
            }
            Component::CurDir => {}
            Component::RootDir | Component::Prefix(_) => return false,
        }
    }
    true
}

/// Why a snapshot was taken.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotReason {
    /// At materialization.
    Base,
    /// Before a write-effect action.
    Before,
    /// After a successful write-effect action.
    After,
    /// The project's targets before a promotion.
    Backup,
    /// Materialized from another task's snapshot.
    Fork,
}

impl SnapshotReason {
    /// The snake_case name.
    pub fn name(self) -> &'static str {
        match self {
            SnapshotReason::Base => "base",
            SnapshotReason::Before => "before",
            SnapshotReason::After => "after",
            SnapshotReason::Backup => "backup",
            SnapshotReason::Fork => "fork",
        }
    }
}

/// A snapshot as recorded (external design §14.3).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotRecord {
    /// Identifier.
    pub snapshot_id: SnapshotId,
    /// Owning task.
    pub task_id: TaskId,
    /// Why.
    pub reason: SnapshotReason,
    /// The manifest's hash; the manifest file is named by the id.
    pub manifest_hash: Hash,
    /// Bytes of regular files.
    pub bytes: u64,
    /// Regular files.
    pub files: u64,
    /// When.
    pub created_at: Instant,
    /// The action it belongs to (`before`, `after`).
    pub action_id: Option<ActionId>,
    /// The project root, for a `backup`.
    pub project_root: Option<String>,
}

/// What a restore reported.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RestoreReport {
    /// Files written.
    pub written: u64,
    /// Entries deleted.
    pub deleted: u64,
    /// Links whose target leaves the root, refused (REQ-WS-005).
    pub links_omitted: Vec<String>,
}

/// A workspace's lifecycle state (REQ-WS-009). `archived` is derived from
/// the task's terminal state and never recorded.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceState {
    /// Directory created, nothing copied yet.
    Allocated,
    /// Materialized.
    Active,
    /// Removed by the user.
    Deleted,
}

impl WorkspaceState {
    /// The snake_case name.
    pub fn name(self) -> &'static str {
        match self {
            WorkspaceState::Allocated => "allocated",
            WorkspaceState::Active => "active",
            WorkspaceState::Deleted => "deleted",
        }
    }
}

/// What a task holds on disk (RFC 006 §6).
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Footprint {
    /// Bytes of the working tree.
    pub workspace_bytes: u64,
    /// Snapshot rows.
    pub snapshots: u64,
    /// Bytes of objects no other task references.
    pub object_bytes_exclusive: u64,
    /// Bytes of objects shared with other tasks.
    pub object_bytes_shared: u64,
    /// Bytes of blobs.
    pub blob_bytes: u64,
    /// Promotions whose backups would be lost.
    pub promotions: u64,
}

/// The plan of a promotion, phase 1 (external design §14.5).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromotionPlan {
    /// The task.
    pub task_id: TaskId,
    /// The base snapshot the diff is against.
    pub base_snapshot: SnapshotId,
    /// The current workspace's manifest hash; `apply` checks it again.
    pub current_hash: Hash,
    /// The changes to apply, sorted by path.
    pub changes: Vec<Change>,
    /// Excluded paths dropped from the plan (LIM-W4, LIM-W5).
    pub excluded: Vec<String>,
    /// Unified diffs of text files, by path, for display.
    pub text_diffs: Vec<(String, String)>,
}

/// A promotion as recorded (REQ-WS-007).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Promotion {
    /// Identifier.
    pub promotion_id: PromotionId,
    /// The task.
    pub task_id: TaskId,
    /// The backup of the targets, for reversal.
    pub backup_snapshot: SnapshotId,
    /// The project root written.
    pub project_root: String,
    /// Paths written or deleted.
    pub files: Vec<Change>,
    /// When.
    pub applied_at: Instant,
    /// Reversed at, when it was.
    pub reversed_at: Option<Instant>,
}

#[cfg(test)]
mod tests;
