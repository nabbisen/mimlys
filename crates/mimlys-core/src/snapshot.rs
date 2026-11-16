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

/// How many links a path may be resolved through before it is refused (REQ-WS-005): the most a
/// system itself follows on Linux (`MAXSYMLINKS`). A chain longer than that, or a loop, is not
/// one a tree needs, and the bound is what ends the resolution of a loop.
pub const MAX_LINK_HOPS: usize = 40;

/// The links of a tree, by path: what a link chain is resolved through (REQ-WS-005).
pub fn links_of(manifest: &Manifest) -> BTreeMap<&str, &str> {
    manifest
        .entries
        .iter()
        .filter_map(|e| match e {
            Entry::Symlink { path, target } => Some((path.as_str(), target.as_str())),
            _ => None,
        })
        .collect()
}

/// Whether a link at `link` (workspace-relative) pointing to `target` stays inside the root once
/// it is **resolved through the links of the tree** (`links`, by path): each name on the way that
/// is itself a link is replaced by that link's target, as the system does, so that `y -> s/x/..`
/// with `s/x -> ..` is the parent of the root and not `s` (REQ-WS-005). The lexical rule of
/// [`target_inside_root`] holds first. The resolution is refused when it passes through more than
/// [`MAX_LINK_HOPS`] links (a loop included), when a link on the way is absolute, and when it
/// would go above the root at any step. A name that is not a link of the tree is taken as it is.
pub fn target_resolves_inside_root(link: &str, target: &str, links: &BTreeMap<&str, &str>) -> bool {
    if !target_inside_root(link, target) {
        return false;
    }
    // The components of the path still to resolve, the next one last; the directory resolved
    // so far, from the root.
    let mut todo: Vec<String> = components_of(target).into_iter().rev().collect();
    let mut resolved: Vec<String> = link.split('/').map(str::to_owned).collect();
    resolved.pop();
    let mut hops = 0;
    while let Some(component) = todo.pop() {
        match component.as_str() {
            "" | "." => {}
            ".." => {
                if resolved.pop().is_none() {
                    return false;
                }
            }
            name => {
                resolved.push(name.to_owned());
                if let Some(next) = links.get(resolved.join("/").as_str()) {
                    hops += 1;
                    if hops > MAX_LINK_HOPS || !target_inside_root_text(next) {
                        return false;
                    }
                    resolved.pop();
                    todo.extend(components_of(next).into_iter().rev());
                }
            }
        }
    }
    true
}

/// The components of a relative target, split at `/` and at `\` (a backslash separates on every
/// platform, REQ-WS-011).
fn components_of(target: &str) -> Vec<String> {
    target.split(['/', '\\']).map(str::to_owned).collect()
}

/// Whether a link's target is relative (not rooted, no drive): the part of the lexical rule that
/// does not depend on where the link is.
fn target_inside_root_text(target: &str) -> bool {
    let bytes = target.as_bytes();
    !(bytes.first().is_some_and(|b| *b == b'/' || *b == b'\\')
        || (bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'))
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
    /// Paths kept as they are (REQ-WS-007): a directory that holds entries the operation did not
    /// write, or a path that holds neither what the promotion wrote nor what it replaced.
    #[serde(default)]
    pub left: Vec<String>,
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

/// Where a promotion is (external design §14.5, §16.3). A record written before the state
/// existed reads as `applied`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromotionState {
    /// Recorded after its backup was committed, before the first write to the project.
    Applying,
    /// Applied; the backup reverses it.
    #[default]
    Applied,
    /// Not applied: what was written was restored from the backup (or, where a path had been
    /// edited since, left as it is and listed in the reason).
    Failed,
}

impl PromotionState {
    /// The snake_case name.
    pub fn name(self) -> &'static str {
        match self {
            PromotionState::Applying => "applying",
            PromotionState::Applied => "applied",
            PromotionState::Failed => "failed",
        }
    }
}

/// What a reversal does to one path.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RevertAction {
    /// The path gets back the entry the backup holds.
    Restore,
    /// The path was added by the promotion: it is removed (a directory only when empty).
    Remove,
}

/// One path a reversal changes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevertEntry {
    /// Path.
    pub path: String,
    /// What is done to it.
    pub action: RevertAction,
}

/// What reversing a promotion would do, before it does it (REQ-WS-007).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevertPlan {
    /// The promotion.
    pub promotion_id: PromotionId,
    /// The paths it changes, sorted.
    pub entries: Vec<RevertEntry>,
    /// Paths edited since the promotion: restoring them would overwrite the edit, so the
    /// reversal is refused unless it is confirmed.
    pub conflicts: Vec<String>,
    /// Whether an edit can be told from the promotion's own write (a record from before 0.10
    /// holds no entries to tell it by).
    pub edits_detected: bool,
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
    /// Where it is.
    #[serde(default)]
    pub state: PromotionState,
    /// The entries the promotion writes (a path deleted has none). A reversal restores a path
    /// only while it still holds what the promotion wrote. Empty in a record from before 0.10.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub after: Vec<Entry>,
    /// Why it failed, once it has.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Whether the project is as the backup holds it, once the promotion has failed. `false`
    /// only when the restore from the backup failed too: the backup is kept and
    /// `rollback --promotion` restores the project from it. A record without it reads `true`.
    #[serde(default = "restored_default", skip_serializing_if = "is_true")]
    pub restored: bool,
}

impl Promotion {
    /// Whether its backup can still reverse it: applied, or failed with a restore that failed
    /// too, and not reversed yet.
    pub fn can_be_reversed(&self) -> bool {
        self.reversed_at.is_none()
            && match self.state {
                PromotionState::Applied => true,
                PromotionState::Failed => !self.restored,
                PromotionState::Applying => false,
            }
    }

    /// What to say when the restore of the project from the backup failed: the backup is kept,
    /// and this command restores the project from it.
    pub fn restore_hint(&self) -> String {
        format!(
            "the backup is kept: run mimlys rollback {} --promotion {} to restore the project",
            self.task_id, self.promotion_id
        )
    }
}

fn restored_default() -> bool {
    true
}

fn is_true(value: &bool) -> bool {
    *value
}

#[cfg(test)]
mod tests;
