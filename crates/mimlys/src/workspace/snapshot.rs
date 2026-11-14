//! Snapshots over the local tree (external design §14.3 … §14.5; RFC 006):
//! every file hashed on every snapshot, objects and the manifest written
//! through the persistence crate's stores under a journal, restore that
//! makes the tree equal to the manifest, promotion with its backup, fork
//! and deletion of the files the database released. Links are entries and
//! are never followed (REQ-WS-005).

use super::{LocalWorkspaces, is_excluded, join};
use cap_std::ambient_authority;
use cap_std::fs::Dir;
use mimlys_core::clock::Instant;
use mimlys_core::hash::{Hash, hash_bytes};
use mimlys_core::ids::{ActionId, PromotionId, SnapshotId, TaskId};
use mimlys_core::limits::LIM_W3_FILE_BYTES;
use mimlys_core::path::CaseMode;
use mimlys_core::policy::ExclusionMatcher;
use mimlys_core::snapshot::{
    Change, ChangeKind, Entry, Manifest, Promotion, PromotionPlan, RestoreReport, SnapshotReason,
    SnapshotRecord, diff, target_inside_root,
};
use mimlys_core::spi::{DeletedRows, PreparedSnapshot, WorkspaceError, WorkspaceInfo};
use mimlys_core::task::ScopeSpec;
use mimlys_persist::snapshots::SnapshotFiles;
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

fn storage(detail: &'static str) -> impl Fn(std::io::Error) -> WorkspaceError {
    move |_| WorkspaceError::Storage {
        detail: detail.to_owned(),
    }
}

fn persist(e: mimlys_core::spi::PersistenceError) -> WorkspaceError {
    WorkspaceError::Storage {
        detail: e.to_string(),
    }
}

/// Hashes a file through a capability handle, streaming.
fn hash_file(dir: &Dir, rel: &str) -> Result<(Hash, u64), WorkspaceError> {
    let mut file = dir.open(rel).map_err(storage("open"))?;
    let mut hasher = Sha256::default();
    let mut buf = vec![0u8; 64 * 1024];
    let mut size = 0u64;
    loop {
        let n = file.read(&mut buf).map_err(storage("read"))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        size += n as u64;
    }
    let hex: String = hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    Ok((Hash::from_hex(&hex).expect("hex"), size))
}

#[cfg(unix)]
fn is_executable(meta: &cap_std::fs::Metadata) -> bool {
    use cap_std::fs::PermissionsExt;
    meta.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(_meta: &cap_std::fs::Metadata) -> bool {
    false
}

/// Walks a tree into a manifest, hashing every regular file; links are
/// entries with their raw target; excluded paths are skipped.
pub(crate) fn walk_manifest(
    dir: &Dir,
    matcher: Option<&ExclusionMatcher>,
) -> Result<Manifest, WorkspaceError> {
    let mut entries = Vec::new();
    walk(dir, "", matcher, &mut entries)?;
    Ok(Manifest::new(entries))
}

fn walk(
    dir: &Dir,
    rel: &str,
    matcher: Option<&ExclusionMatcher>,
    out: &mut Vec<Entry>,
) -> Result<(), WorkspaceError> {
    for entry in dir
        .read_dir(if rel.is_empty() { "." } else { rel })
        .map_err(storage("walk"))?
    {
        let entry = entry.map_err(storage("walk"))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let child = join(rel, &name);
        if matcher.is_some_and(|m| is_excluded(m, &child)) {
            continue;
        }
        let ty = entry.file_type().map_err(storage("walk"))?;
        if ty.is_symlink() {
            let target = dir
                .read_link_contents(&child)
                .map_err(storage("read_link"))?;
            out.push(Entry::Symlink {
                path: child,
                target: target.to_string_lossy().replace('\\', "/"),
            });
        } else if ty.is_dir() {
            out.push(Entry::Dir {
                path: child.clone(),
            });
            walk(dir, &child, matcher, out)?;
        } else if ty.is_file() {
            let meta = entry.metadata().map_err(storage("walk"))?;
            let (hash, size) = hash_file(dir, &child)?;
            out.push(Entry::File {
                path: child,
                executable: is_executable(&meta),
                size,
                hash,
            });
        }
    }
    Ok(())
}

/// The number of temporary names made by this process.
static TEMP_NAMES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// How many names are tried before a write gives up (a name is taken only by a leftover of a crashed process).
const TEMP_ATTEMPTS: u32 = 64;

/// A temporary file beside the entry it will replace (REQ-WS-006, REQ-WS-007).
///
/// Its name is short and unique per write and is never derived from the
/// entry's name, so it fits wherever the entry fits. It is made with
/// `create_new`, so an existing file or link of any name is never opened or
/// followed. It is removed when dropped, unless it was placed.
pub(super) struct TempFile<'a> {
    dir: &'a Dir,
    path: String,
    placed: bool,
}

impl<'a> TempFile<'a> {
    /// Makes a new temporary file in the directory of `rel`, opened for writing.
    pub(super) fn create(
        dir: &'a Dir,
        rel: &str,
    ) -> Result<(Self, cap_std::fs::File), WorkspaceError> {
        let parent = rel.rfind('/').map(|i| &rel[..=i]).unwrap_or("");
        let mut options = cap_std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        for _ in 0..TEMP_ATTEMPTS {
            let n = TEMP_NAMES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let path = format!("{parent}.mimlys-tmp-{:x}-{n:x}", std::process::id());
            match dir.open_with(&path, &options) {
                Ok(file) => {
                    return Ok((
                        Self {
                            dir,
                            path,
                            placed: false,
                        },
                        file,
                    ));
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(storage("create")(e)),
            }
        }
        Err(storage("create")(std::io::ErrorKind::AlreadyExists.into()))
    }

    /// Sets the mode: executable or not (Unix).
    fn set_mode(&self, executable: bool) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = if executable { 0o755 } else { 0o644 };
            let _ = self.dir.set_permissions(
                &self.path,
                cap_std::fs::Permissions::from_std(std::fs::Permissions::from_mode(mode)),
            );
        }
        #[cfg(not(unix))]
        {
            let _ = executable;
        }
    }

    /// Renames the file onto `rel` in the same directory.
    fn place(mut self, rel: &str) -> Result<(), WorkspaceError> {
        self.dir
            .rename(&self.path, self.dir, rel)
            .map_err(storage("rename"))?;
        self.placed = true;
        Ok(())
    }
}

impl Drop for TempFile<'_> {
    fn drop(&mut self) {
        if !self.placed {
            let _ = self.dir.remove_file(&self.path);
        }
    }
}

impl LocalWorkspaces {
    fn tree_dir(&self, task_id: TaskId) -> Result<Dir, WorkspaceError> {
        Dir::open_ambient_dir(self.tree_path(task_id), ambient_authority())
            .map_err(|_| WorkspaceError::Missing)
    }

    fn matcher(&self, case: CaseMode) -> Result<ExclusionMatcher, WorkspaceError> {
        ExclusionMatcher::new(&self.patterns, case).map_err(|_| WorkspaceError::Storage {
            detail: "exclusion_pattern".into(),
        })
    }

    /// Writes the manifest's objects and the manifest under a journal, from
    /// files read through `dir` (the tree or the project root).
    fn store_snapshot(
        &self,
        dir: &Dir,
        record: SnapshotRecord,
        manifest: &Manifest,
    ) -> Result<PreparedSnapshot, WorkspaceError> {
        let objects = manifest.objects();
        let new: Vec<Hash> = objects
            .keys()
            .filter(|h| !self.files.has_object(h))
            .cloned()
            .collect();
        self.files
            .begin(record.snapshot_id, &new)
            .map_err(persist)?;
        for entry in &manifest.entries {
            if let Entry::File { path, hash, .. } = entry
                && !self.files.has_object(hash)
            {
                let file = dir.open(path).map_err(storage("open"))?;
                self.files.put_object(hash, file).map_err(persist)?;
            }
        }
        let manifest_hash = self
            .files
            .put_manifest(record.snapshot_id, manifest)
            .map_err(persist)?;
        Ok(PreparedSnapshot {
            record: SnapshotRecord {
                manifest_hash,
                bytes: manifest.bytes(),
                files: manifest.files(),
                ..record
            },
            objects: objects.into_iter().collect(),
            reused: false,
        })
    }

    /// Copies an object into `dir` at `rel` through a temporary file, verifying its hash.
    fn write_object(
        &self,
        dir: &Dir,
        rel: &str,
        hash: &Hash,
        executable: bool,
    ) -> Result<(), WorkspaceError> {
        let mut source = self.files.open_object(hash).map_err(persist)?;
        let (tmp, mut out) = TempFile::create(dir, rel)?;
        let mut hasher = Sha256::default();
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let n = source.read(&mut buf).map_err(storage("read"))?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            out.write_all(&buf[..n]).map_err(storage("write"))?;
        }
        let actual: String = hasher
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        if actual != hash.as_str() {
            return Err(WorkspaceError::Storage {
                detail: "object_hash_mismatch".into(),
            });
        }
        drop(out);
        tmp.set_mode(executable);
        tmp.place(rel)
    }

    /// Copies a file from one handle to another through a temporary file, verifying its hash.
    fn copy_verified(
        src: &Dir,
        dst: &Dir,
        rel: &str,
        hash: &Hash,
        executable: bool,
    ) -> Result<(), WorkspaceError> {
        let mut source = src.open(rel).map_err(storage("open"))?;
        let (tmp, mut out) = TempFile::create(dst, rel)?;
        let mut hasher = Sha256::default();
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let n = source.read(&mut buf).map_err(storage("read"))?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            out.write_all(&buf[..n]).map_err(storage("write"))?;
        }
        let actual: String = hasher
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        if actual != hash.as_str() {
            return Err(WorkspaceError::Refused {
                reason: "the workspace changed while promoting; plan again".into(),
            });
        }
        drop(out);
        tmp.set_mode(executable);
        tmp.place(rel)
    }

    /// Makes `dir` equal to `manifest` (REQ-WS-006): extras deleted, objects
    /// verified as they are copied, links recreated only inside the root.
    fn restore_into(
        &self,
        dir: &Dir,
        manifest: &Manifest,
        matcher: Option<&ExclusionMatcher>,
    ) -> Result<RestoreReport, WorkspaceError> {
        let current = walk_manifest(dir, matcher)?;
        let wanted = manifest.by_path();
        let mut report = RestoreReport::default();
        // Delete what the manifest does not have, deepest first.
        let mut extras: Vec<&Entry> = current
            .entries
            .iter()
            .filter(|e| !wanted.contains_key(e.path()))
            .collect();
        extras.sort_by_key(|e| std::cmp::Reverse(e.path().len()));
        for e in extras {
            match e {
                Entry::Dir { path } => dir.remove_dir_all(path).map_err(storage("remove_dir"))?,
                Entry::File { path, .. } | Entry::Symlink { path, .. } => {
                    dir.remove_file(path).map_err(storage("remove_file"))?
                }
            }
            report.deleted += 1;
        }
        // Replace entries whose type changed, then create or write.
        let have = current.by_path();
        for entry in &manifest.entries {
            let path = entry.path();
            let existing = have.get(path);
            let changed_type = matches!(
                (existing, entry),
                (
                    Some(Entry::Dir { .. }),
                    Entry::File { .. } | Entry::Symlink { .. }
                ) | (
                    Some(Entry::File { .. }),
                    Entry::Dir { .. } | Entry::Symlink { .. }
                ) | (
                    Some(Entry::Symlink { .. }),
                    Entry::Dir { .. } | Entry::File { .. }
                )
            );
            if changed_type {
                match existing {
                    Some(Entry::Dir { .. }) => {
                        dir.remove_dir_all(path).map_err(storage("remove_dir"))?
                    }
                    Some(_) => dir.remove_file(path).map_err(storage("remove_file"))?,
                    None => {}
                }
                report.deleted += 1;
            }
            match entry {
                Entry::Dir { path } => {
                    if !dir.is_dir(path) {
                        dir.create_dir(path).map_err(storage("create_dir"))?;
                    }
                }
                Entry::File {
                    path,
                    hash,
                    executable,
                    ..
                } => {
                    let same = matches!(existing, Some(Entry::File { hash: h, executable: x, .. }) if h == hash && x == executable)
                        && !changed_type;
                    if !same {
                        self.write_object(dir, path, hash, *executable)?;
                        report.written += 1;
                    }
                }
                Entry::Symlink { path, target } => {
                    let same = matches!(existing, Some(Entry::Symlink { target: t, .. }) if t == target)
                        && !changed_type;
                    if same {
                        continue;
                    }
                    if !target_inside_root(path, target) {
                        report.links_omitted.push(path.clone());
                        continue;
                    }
                    if dir.symlink_metadata(path).is_ok() {
                        dir.remove_file(path).map_err(storage("remove_file"))?;
                    }
                    super::recreate_symlink_in(dir, path, Path::new(target))?;
                    report.written += 1;
                }
            }
        }
        Ok(report)
    }

    fn scope_dir(project_root: &str) -> Result<Dir, WorkspaceError> {
        Dir::open_ambient_dir(project_root, ambient_authority())
            .map_err(|_| WorkspaceError::ScopeNotFound)
    }

    fn task_case(&self, task_id: TaskId) -> CaseMode {
        self.read_info(task_id)
            .map(|i| i.case_mode)
            .unwrap_or(CaseMode::Sensitive)
    }

    pub(crate) fn snapshot_impl(
        &self,
        task_id: TaskId,
        snapshot_id: SnapshotId,
        reason: SnapshotReason,
        action_id: Option<ActionId>,
        previous: Option<&SnapshotRecord>,
        now: Instant,
    ) -> Result<PreparedSnapshot, WorkspaceError> {
        let dir = self.tree_dir(task_id)?;
        let manifest = walk_manifest(&dir, None)?;
        let hash = manifest.hash();
        if let Some(p) = previous
            && p.manifest_hash == hash
        {
            return Ok(PreparedSnapshot {
                record: p.clone(),
                objects: vec![],
                reused: true,
            });
        }
        self.store_snapshot(
            &dir,
            SnapshotRecord {
                snapshot_id,
                task_id,
                reason,
                manifest_hash: hash,
                bytes: 0,
                files: 0,
                created_at: now,
                action_id,
                project_root: None,
            },
            &manifest,
        )
    }

    pub(crate) fn restore_impl(
        &self,
        task_id: TaskId,
        snapshot: &SnapshotRecord,
    ) -> Result<RestoreReport, WorkspaceError> {
        let manifest = self
            .files
            .read_manifest(snapshot.snapshot_id, &snapshot.manifest_hash)
            .map_err(persist)?;
        let dir = self.tree_dir(task_id)?;
        let report = self.restore_into(&dir, &manifest, None)?;
        // The ephemeral tmp of the task is not part of the tree; leave it.
        Ok(report)
    }

    pub(crate) fn materialize_from_impl(
        &self,
        task_id: TaskId,
        snapshot_id: SnapshotId,
        from: &SnapshotRecord,
        scope: &ScopeSpec,
        now: Instant,
    ) -> Result<(WorkspaceInfo, PreparedSnapshot), WorkspaceError> {
        let manifest = self
            .files
            .read_manifest(from.snapshot_id, &from.manifest_hash)
            .map_err(persist)?;
        let task_dir = self.task_dir(task_id);
        std::fs::create_dir_all(task_dir.join("tree")).map_err(storage("create_dir"))?;
        std::fs::create_dir_all(task_dir.join("tmp")).map_err(storage("create_dir"))?;
        let dir = Dir::open_ambient_dir(task_dir.join("tree"), ambient_authority())
            .map_err(storage("open"))?;
        let case_mode = Self::probe_case(&dir)?;
        let report = self.restore_into(&dir, &manifest, None)?;
        let info = WorkspaceInfo {
            root: task_dir.join("tree").to_string_lossy().into_owned(),
            case_mode,
            bytes: manifest.bytes(),
            files: manifest.files(),
            symlinks_omitted: report.links_omitted.len() as u64,
        };
        self.write_info(task_id, &info)?;
        let _ = scope;
        let prepared = PreparedSnapshot {
            record: SnapshotRecord {
                snapshot_id,
                task_id,
                reason: SnapshotReason::Fork,
                manifest_hash: from.manifest_hash.clone(),
                bytes: from.bytes,
                files: from.files,
                created_at: now,
                action_id: None,
                project_root: None,
            },
            objects: manifest.objects().into_iter().collect(),
            reused: false,
        };
        // The manifest file of the fork's record: the same content under the new id.
        self.files.begin(snapshot_id, &[]).map_err(persist)?;
        self.files
            .put_manifest(snapshot_id, &manifest)
            .map_err(persist)?;
        Ok((info, prepared))
    }

    pub(crate) fn promotion_plan_impl(
        &self,
        task_id: TaskId,
        base: &SnapshotRecord,
        project_root: &str,
    ) -> Result<PromotionPlan, WorkspaceError> {
        let base_manifest = self
            .files
            .read_manifest(base.snapshot_id, &base.manifest_hash)
            .map_err(persist)?;
        let dir = self.tree_dir(task_id)?;
        let current = walk_manifest(&dir, None)?;
        let matcher = self.matcher(self.task_case(task_id))?;
        let full = diff(&base_manifest, &current);
        let mut changes = Vec::new();
        let mut excluded = Vec::new();
        for c in full.changes {
            if is_excluded(&matcher, &c.path) {
                excluded.push(c.path);
            } else {
                changes.push(c);
            }
        }
        // Conflicts: the project's target must equal the base entry (REQ-WS-007).
        let project = Self::scope_dir(project_root)?;
        let base_by_path = base_manifest.by_path();
        let mut conflicts = Vec::new();
        for c in &changes {
            let in_project = project_entry(&project, &c.path)?;
            let in_base = base_by_path.get(c.path.as_str());
            let same = match (in_project, in_base) {
                (None, None) => true,
                (Some(p), Some(b)) => entry_equal(&p, b),
                _ => false,
            };
            if !same {
                conflicts.push(c.path.clone());
            }
        }
        if !conflicts.is_empty() {
            return Err(WorkspaceError::Conflict { paths: conflicts });
        }
        let current_by_path = current.by_path();
        let mut text_diffs = Vec::new();
        for c in &changes {
            let before = in_base_text(&base_by_path, &c.path, &self.files).unwrap_or_default();
            let after = match current_by_path.get(c.path.as_str()) {
                Some(Entry::File { size, .. }) if *size as usize <= LIM_W3_FILE_BYTES => dir
                    .read(&c.path)
                    .ok()
                    .and_then(|b| String::from_utf8(b).ok()),
                _ => None,
            };
            let after = match (c.kind, after) {
                (ChangeKind::Deleted, _) => Some(String::new()),
                (_, Some(t)) => Some(t),
                _ => None,
            };
            if let Some(after) = after
                && (c.kind != ChangeKind::Deleted || !before.is_empty())
            {
                let text = similar::TextDiff::from_lines(&before, &after)
                    .unified_diff()
                    .header(&format!("a/{}", c.path), &format!("b/{}", c.path))
                    .to_string();
                text_diffs.push((c.path.clone(), text));
            }
        }
        Ok(PromotionPlan {
            task_id,
            base_snapshot: base.snapshot_id,
            current_hash: current.hash(),
            changes,
            excluded,
            text_diffs,
        })
    }

    pub(crate) fn promote_impl(
        &self,
        task_id: TaskId,
        plan: &PromotionPlan,
        project_root: &str,
        backup_id: SnapshotId,
        promotion_id: PromotionId,
        now: Instant,
    ) -> Result<(PreparedSnapshot, Promotion), WorkspaceError> {
        let dir = self.tree_dir(task_id)?;
        let current = walk_manifest(&dir, None)?;
        if current.hash() != plan.current_hash {
            return Err(WorkspaceError::Refused {
                reason: "the workspace changed since the plan was made; plan again".into(),
            });
        }
        let project = Self::scope_dir(project_root)?;
        // Backup: the project's current versions of every path the plan touches.
        let mut backup_entries = Vec::new();
        for c in &plan.changes {
            if let Some(entry) = project_entry(&project, &c.path)? {
                backup_entries.push(entry);
            }
        }
        let backup_manifest = Manifest::new(backup_entries);
        let backup = self.store_snapshot(
            &project,
            SnapshotRecord {
                snapshot_id: backup_id,
                task_id,
                reason: SnapshotReason::Backup,
                manifest_hash: backup_manifest.hash(),
                bytes: 0,
                files: 0,
                created_at: now,
                action_id: None,
                project_root: Some(project_root.to_owned()),
            },
            &backup_manifest,
        )?;
        // Apply: temp-and-rename writes, directories created, deletions, inside links only.
        let current_by_path = current.by_path();
        for c in &plan.changes {
            match c.kind {
                ChangeKind::Deleted => {
                    if project.symlink_metadata(&c.path).is_ok() {
                        if project.is_dir(&c.path)
                            && project
                                .symlink_metadata(&c.path)
                                .map(|m| !m.is_symlink())
                                .unwrap_or(false)
                        {
                            project
                                .remove_dir_all(&c.path)
                                .map_err(storage("remove_dir"))?;
                        } else {
                            project
                                .remove_file(&c.path)
                                .map_err(storage("remove_file"))?;
                        }
                    }
                }
                ChangeKind::Added | ChangeKind::Modified => {
                    if let Some(parent) = Path::new(&c.path)
                        .parent()
                        .filter(|p| !p.as_os_str().is_empty())
                    {
                        project
                            .create_dir_all(parent)
                            .map_err(storage("create_dir"))?;
                    }
                    match current_by_path.get(c.path.as_str()) {
                        Some(Entry::File {
                            hash, executable, ..
                        }) => {
                            if project.is_dir(&c.path) {
                                project
                                    .remove_dir_all(&c.path)
                                    .map_err(storage("remove_dir"))?;
                            }
                            Self::copy_verified(&dir, &project, &c.path, hash, *executable)?;
                        }
                        Some(Entry::Dir { .. }) => {
                            if !project.is_dir(&c.path) {
                                project
                                    .create_dir_all(&c.path)
                                    .map_err(storage("create_dir"))?;
                            }
                        }
                        Some(Entry::Symlink { target, .. }) => {
                            if !target_inside_root(&c.path, target) {
                                continue;
                            }
                            if project.symlink_metadata(&c.path).is_ok() {
                                project
                                    .remove_file(&c.path)
                                    .map_err(storage("remove_file"))?;
                            }
                            super::recreate_symlink_in(&project, &c.path, Path::new(target))?;
                        }
                        None => {}
                    }
                }
            }
        }
        let promotion = Promotion {
            promotion_id,
            task_id,
            backup_snapshot: backup_id,
            project_root: project_root.to_owned(),
            files: plan.changes.clone(),
            applied_at: now,
            reversed_at: None,
        };
        Ok((backup, promotion))
    }

    pub(crate) fn revert_promotion_impl(
        &self,
        promotion: &Promotion,
        backup: &SnapshotRecord,
    ) -> Result<RestoreReport, WorkspaceError> {
        let manifest = self
            .files
            .read_manifest(backup.snapshot_id, &backup.manifest_hash)
            .map_err(persist)?;
        let project = Self::scope_dir(&promotion.project_root)?;
        let by_path = manifest.by_path();
        let mut report = RestoreReport::default();
        // Paths the promotion added have no backup entry: they are removed; the rest restored.
        for c in &promotion.files {
            match by_path.get(c.path.as_str()) {
                None => {
                    if project.symlink_metadata(&c.path).is_ok() {
                        if project.is_dir(&c.path) {
                            project
                                .remove_dir_all(&c.path)
                                .map_err(storage("remove_dir"))?;
                        } else {
                            project
                                .remove_file(&c.path)
                                .map_err(storage("remove_file"))?;
                        }
                        report.deleted += 1;
                    }
                }
                Some(Entry::File {
                    hash, executable, ..
                }) => {
                    if let Some(parent) = Path::new(&c.path)
                        .parent()
                        .filter(|p| !p.as_os_str().is_empty())
                    {
                        project
                            .create_dir_all(parent)
                            .map_err(storage("create_dir"))?;
                    }
                    self.write_object(&project, &c.path, hash, *executable)?;
                    report.written += 1;
                }
                Some(Entry::Dir { .. }) => {
                    if !project.is_dir(&c.path) {
                        project
                            .create_dir_all(&c.path)
                            .map_err(storage("create_dir"))?;
                    }
                }
                Some(Entry::Symlink { target, .. }) => {
                    if target_inside_root(&c.path, target) {
                        if project.symlink_metadata(&c.path).is_ok() {
                            project
                                .remove_file(&c.path)
                                .map_err(storage("remove_file"))?;
                        }
                        super::recreate_symlink_in(&project, &c.path, Path::new(target))?;
                        report.written += 1;
                    } else {
                        report.links_omitted.push(c.path.clone());
                    }
                }
            }
        }
        Ok(report)
    }

    pub(crate) fn delete_files_impl(
        &self,
        task_id: TaskId,
        rows: &DeletedRows,
    ) -> Result<(), WorkspaceError> {
        let task_dir = self.task_dir(task_id);
        if task_dir.exists() {
            std::fs::remove_dir_all(&task_dir).map_err(storage("remove_dir"))?;
        }
        self.tools.lock().expect("workspace tools").remove(&task_id);
        for id in &rows.snapshots {
            self.files.remove_manifest(*id).map_err(persist)?;
        }
        for hash in &rows.objects_at_zero {
            self.files.remove_object(hash).map_err(persist)?;
        }
        for blob in &rows.blobs {
            let path = self.data_dir.join("blobs").join(blob.to_string());
            if path.exists() {
                std::fs::remove_file(&path).map_err(storage("remove_file"))?;
            }
        }
        Ok(())
    }
}

/// The project's entry at `rel`, hashed, or `None` when absent.
fn project_entry(project: &Dir, rel: &str) -> Result<Option<Entry>, WorkspaceError> {
    let Ok(meta) = project.symlink_metadata(rel) else {
        return Ok(None);
    };
    if meta.is_symlink() {
        let target = project
            .read_link_contents(rel)
            .map_err(storage("read_link"))?;
        return Ok(Some(Entry::Symlink {
            path: rel.to_owned(),
            target: target.to_string_lossy().replace('\\', "/"),
        }));
    }
    if meta.is_dir() {
        return Ok(Some(Entry::Dir {
            path: rel.to_owned(),
        }));
    }
    let (hash, size) = hash_file(project, rel)?;
    Ok(Some(Entry::File {
        path: rel.to_owned(),
        executable: is_executable(&meta),
        size,
        hash,
    }))
}

fn entry_equal(a: &Entry, b: &Entry) -> bool {
    match (a, b) {
        (Entry::File { hash: x, .. }, Entry::File { hash: y, .. }) => x == y,
        (Entry::Dir { .. }, Entry::Dir { .. }) => true,
        (Entry::Symlink { target: x, .. }, Entry::Symlink { target: y, .. }) => x == y,
        _ => false,
    }
}

fn in_base_text(
    base: &std::collections::BTreeMap<&str, &Entry>,
    path: &str,
    files: &SnapshotFiles,
) -> Option<String> {
    match base.get(path) {
        Some(Entry::File { hash, size, .. }) if *size as usize <= LIM_W3_FILE_BYTES => {
            let mut text = String::new();
            files
                .open_object(hash)
                .ok()?
                .read_to_string(&mut text)
                .ok()?;
            Some(text)
        }
        _ => None,
    }
}

/// Hashes bytes (for tests and the plan display).
#[allow(dead_code)]
pub(crate) fn hash_of(bytes: &[u8]) -> Hash {
    hash_bytes(bytes)
}

#[allow(dead_code)]
fn _paths(_: PathBuf, _: &Change) {}
