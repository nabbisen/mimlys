//! Snapshots over the local tree (external design §14.3 … §14.5; RFC 006):
//! every file hashed on every snapshot, objects and the manifest written
//! through the persistence crate's stores under a journal, restore that
//! makes the tree equal to the manifest, promotion with its backup, fork
//! and deletion of the files the database released. Links are entries and
//! are never followed (REQ-WS-005).

use super::walk::{Flow, Found, Node, target_of, walk};
use super::{LocalWorkspaces, is_excluded, join};
use cap_std::ambient_authority;
use cap_std::fs::Dir;
use mimlys_core::clock::Instant;
use mimlys_core::hash::{Hash, hash_bytes};
use mimlys_core::ids::{ActionId, SnapshotId, TaskId};
use mimlys_core::limits::LIM_W3_FILE_BYTES;
use mimlys_core::path::CaseMode;
use mimlys_core::policy::ExclusionMatcher;
use mimlys_core::snapshot::{
    Change, ChangeKind, Entry, Manifest, Promotion, PromotionPlan, PromotionState, RestoreReport,
    RevertAction, RevertEntry, RevertPlan, SnapshotReason, SnapshotRecord, diff,
    target_inside_root,
};
use mimlys_core::spi::{
    DeletedRows, EditedPaths, PreparedSnapshot, PromotionRequest, WorkspaceError, WorkspaceInfo,
};
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

/// A read of the tree that failed at `path` (relative to the tree; "" is its root): the
/// error names the path, cleaned of control characters and bounded, so that the task can halt
/// with it. A refused permission is `Unreadable`, which asks the user to restore the
/// permissions; any other failure is `ReadFailed`, with the kind of the failure and no advice.
pub(super) fn unreadable(path: &str) -> impl Fn(std::io::Error) -> WorkspaceError + '_ {
    move |error| {
        let path = mimlys_core::panic_text::bounded_message(
            if path.is_empty() { "." } else { path },
            None,
        );
        match error.kind() {
            std::io::ErrorKind::PermissionDenied => WorkspaceError::Unreadable { path },
            kind => WorkspaceError::ReadFailed {
                path,
                kind: kind.to_string(),
            },
        }
    }
}

fn persist(e: mimlys_core::spi::PersistenceError) -> WorkspaceError {
    WorkspaceError::Storage {
        detail: e.to_string(),
    }
}

/// What a path is when it is neither a regular file, a directory nor a link.
pub(super) fn special_name(file_type: &cap_std::fs::FileType) -> &'static str {
    #[cfg(unix)]
    {
        use cap_std::fs::FileTypeExt;
        if file_type.is_fifo() {
            return "named pipe";
        }
        if file_type.is_socket() {
            return "socket";
        }
        if file_type.is_block_device() || file_type.is_char_device() {
            return "device";
        }
    }
    if file_type.is_dir() {
        return "directory";
    }
    if file_type.is_symlink() {
        return "link";
    }
    "special file"
}

/// The refusal for a path that is not a regular file, a directory or a link (REQ-WS-007):
/// it names the path and what it is.
fn not_regular(path: &str, what: &str) -> WorkspaceError {
    WorkspaceError::Refused {
        reason: format!(
            "{} is a {what}, not a regular file, a directory or a link; it is neither read nor promoted. Remove it (or exclude it), then try again",
            mimlys_core::panic_text::bounded_message(path, None)
        ),
    }
}

/// Opens the regular file at `rel` for reading without blocking (`O_NONBLOCK` on Unix, so that
/// a named pipe never waits for its other end) and looks at what was opened through the handle
/// (`fstat`), which also decides when the path changed after it was looked at. A path that is
/// not a regular file is refused by name, as 002e's `open_regular` does for a tool.
pub(super) fn open_file(dir: &Dir, rel: &str) -> Result<cap_std::fs::File, WorkspaceError> {
    let mut options = cap_std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        options.custom_flags(rustix::fs::OFlags::NONBLOCK.bits() as i32);
    }
    let file = dir.open_with(rel, &options).map_err(unreadable(rel))?;
    let file_type = file.metadata().map_err(unreadable(rel))?.file_type();
    if !file_type.is_file() {
        return Err(not_regular(rel, special_name(&file_type)));
    }
    Ok(file)
}

/// Hashes a file through a capability handle, streaming.
fn hash_file(dir: &Dir, rel: &str) -> Result<(Hash, u64), WorkspaceError> {
    let mut file = open_file(dir, rel)?;
    let mut hasher = Sha256::default();
    let mut buf = vec![0u8; 64 * 1024];
    let mut size = 0u64;
    loop {
        let n = file.read(&mut buf).map_err(unreadable(rel))?;
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
pub(super) fn is_executable(meta: &cap_std::fs::Metadata) -> bool {
    use cap_std::fs::PermissionsExt;
    meta.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
pub(super) fn is_executable(_meta: &cap_std::fs::Metadata) -> bool {
    false
}

/// A tree walked into a manifest: what the manifest holds (files hashed, directories, links
/// with their raw target) and the paths of what it cannot hold, a named pipe, a socket or a
/// device, which a restore removes and nothing else ever reads.
pub(super) struct Walked {
    pub manifest: Manifest,
    pub specials: Vec<String>,
}

/// Walks a tree into a manifest, hashing every regular file; links are
/// entries with their raw target; excluded paths are skipped.
pub(crate) fn walk_manifest(
    dir: &Dir,
    matcher: Option<&ExclusionMatcher>,
) -> Result<Manifest, WorkspaceError> {
    Ok(walk_state(dir, matcher)?.manifest)
}

/// As [`walk_manifest`], also naming the entries a manifest cannot hold.
pub(super) fn walk_state(
    dir: &Dir,
    matcher: Option<&ExclusionMatcher>,
) -> Result<Walked, WorkspaceError> {
    let mut entries = Vec::new();
    let mut specials = Vec::new();
    walk(dir, "", matcher, |found| {
        let Found { path, node } = found;
        match node {
            Node::File { executable, .. } => {
                let (hash, size) = hash_file(dir, &path)?;
                entries.push(Entry::File {
                    path,
                    executable,
                    size,
                    hash,
                });
            }
            Node::Dir => entries.push(Entry::Dir { path }),
            Node::Link { target } => entries.push(Entry::Symlink { path, target }),
            Node::Special(_) => specials.push(path),
        }
        Ok(Flow::Descend)
    })?;
    Ok(Walked {
        manifest: Manifest::new(entries),
        specials,
    })
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
        set_file_mode(self.dir, &self.path, executable);
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

/// Sets the mode of the file at `path`: executable or not (Unix).
fn set_file_mode(dir: &Dir, path: &str, executable: bool) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = if executable { 0o755 } else { 0o644 };
        let _ = dir.set_permissions(
            path,
            cap_std::fs::Permissions::from_std(std::fs::Permissions::from_mode(mode)),
        );
    }
    #[cfg(not(unix))]
    {
        let _ = (dir, path, executable);
    }
}

const STAGE_PREFIX: &str = ".mimlys-stage-";
const TEMP_PREFIX: &str = ".mimlys-tmp-";

/// The directory a restore verifies and writes into before it removes anything (REQ-WS-006),
/// inside the root being restored so that a file is renamed into place and never copied again.
/// It is removed when dropped, whatever is left in it; one a crash leaves is an entry the
/// manifest does not hold, which the next restore removes.
pub(super) struct Stage<'a> {
    dir: &'a Dir,
    name: String,
    made: usize,
}

impl<'a> Stage<'a> {
    /// Makes a new staging directory in the root `dir`.
    pub(super) fn create(dir: &'a Dir) -> Result<Self, WorkspaceError> {
        for _ in 0..TEMP_ATTEMPTS {
            let n = TEMP_NAMES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let name = format!("{STAGE_PREFIX}{:x}-{n:x}", std::process::id());
            match dir.create_dir(&name) {
                Ok(()) => {
                    return Ok(Self { dir, name, made: 0 });
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(storage("create_dir")(e)),
            }
        }
        Err(storage("create_dir")(
            std::io::ErrorKind::AlreadyExists.into(),
        ))
    }

    /// A new file of the stage, opened for writing, with its path.
    fn next(&mut self) -> Result<(String, cap_std::fs::File), WorkspaceError> {
        let path = format!("{}/{}", self.name, self.made);
        let mut options = cap_std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        let file = self
            .dir
            .open_with(&path, &options)
            .map_err(storage("create"))?;
        self.made += 1;
        Ok((path, file))
    }
}

impl Drop for Stage<'_> {
    fn drop(&mut self) {
        let _ = self.dir.remove_dir_all(&self.name);
    }
}

/// Moves a staged file onto `rel` in the same root: a rename, which replaces what is there. Where
/// the root holds a mount point and the rename cannot cross it, the verified copy is written
/// beside the entry and renamed from there.
fn place_staged(dir: &Dir, staged: &str, rel: &str) -> Result<(), WorkspaceError> {
    match dir.rename(staged, dir, rel) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::CrossesDevices => {
            let mut from = open_file(dir, staged)?;
            let permissions = from.metadata().map_err(storage("stat"))?.permissions();
            let (tmp, mut out) = TempFile::create(dir, rel)?;
            std::io::copy(&mut from, &mut out).map_err(storage("write"))?;
            out.set_permissions(permissions)
                .map_err(storage("set_permissions"))?;
            drop(out);
            tmp.place(rel)
        }
        Err(e) => Err(storage("rename")(e)),
    }
}

/// Copies `source` to `out` and returns the SHA-256 of what was copied, in hex.
fn copy_hashed(source: &mut impl Read, out: &mut impl Write) -> Result<String, WorkspaceError> {
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
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
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
                let file = open_file(dir, path)?;
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

    /// Verifies the object by its hash as it is written into the stage; returns the staged
    /// path. A missing object, a damaged one or a write that fails ends here, before anything
    /// is changed.
    fn stage_object(
        &self,
        stage: &mut Stage<'_>,
        hash: &Hash,
        executable: bool,
    ) -> Result<String, WorkspaceError> {
        let mut source = self.files.open_object(hash).map_err(persist)?;
        let (path, mut out) = stage.next()?;
        let actual = copy_hashed(&mut source, &mut out)?;
        if actual != hash.as_str() {
            return Err(WorkspaceError::Storage {
                detail: "object_hash_mismatch".into(),
            });
        }
        drop(out);
        set_file_mode(stage.dir, &path, executable);
        Ok(path)
    }

    /// Copies a file from one handle to another through a temporary file, verifying its hash.
    fn copy_verified(
        src: &Dir,
        dst: &Dir,
        rel: &str,
        hash: &Hash,
        executable: bool,
    ) -> Result<(), WorkspaceError> {
        let mut source = open_file(src, rel)?;
        let (tmp, mut out) = TempFile::create(dst, rel)?;
        let actual = copy_hashed(&mut source, &mut out)?;
        if actual != hash.as_str() {
            return Err(WorkspaceError::Refused {
                reason: "the workspace changed while promoting; plan again".into(),
            });
        }
        drop(out);
        tmp.set_mode(executable);
        tmp.place(rel)
    }

    /// Makes `dir` equal to `manifest` (REQ-WS-006): **verify, then replace.** Every object a
    /// file needs is verified by its hash and written into a staging directory inside the root
    /// before anything is removed, so that a missing or damaged object, or a disk too full to
    /// hold the copies, leaves the tree as it was. Then what the manifest does not hold is
    /// removed (a named pipe, a socket or a device included), and the staged files are renamed
    /// into place; links are written last and only inside the root.
    fn restore_into(
        &self,
        dir: &Dir,
        manifest: &Manifest,
    ) -> Result<RestoreReport, WorkspaceError> {
        let current = walk_state(dir, None)?;
        let wanted = manifest.by_path();
        let have = current.manifest.by_path();
        let mut report = RestoreReport::default();
        // 1. Verify: the files whose content is not in the tree are written into the stage.
        // (A stage left behind by a crash is in the walk above, and is removed as an extra.)
        let mut stage = Stage::create(dir)?;
        let mut staged: std::collections::BTreeMap<&str, String> =
            std::collections::BTreeMap::new();
        for entry in &manifest.entries {
            if let Entry::File {
                path,
                hash,
                executable,
                ..
            } = entry
            {
                let same = matches!(have.get(path.as_str()), Some(Entry::File { hash: h, executable: x, .. }) if h == hash && x == executable);
                if !same {
                    staged.insert(path, self.stage_object(&mut stage, hash, *executable)?);
                }
            }
        }
        // 2. Replace. Delete what the manifest does not have, deepest first (every entry of the
        // tree was walked, so a directory is empty by the time it is reached).
        let mut extras: Vec<(&str, bool)> = current
            .manifest
            .entries
            .iter()
            .filter(|e| !wanted.contains_key(e.path()))
            .map(|e| (e.path(), matches!(e, Entry::Dir { .. })))
            .chain(current.specials.iter().map(|p| (p.as_str(), false)))
            .collect();
        extras.sort_by_key(|(path, _)| std::cmp::Reverse(path.len()));
        for (path, is_dir) in extras {
            if is_dir {
                dir.remove_dir(path).map_err(storage("remove_dir"))?;
            } else {
                dir.remove_file(path).map_err(storage("remove_file"))?;
            }
            report.deleted += 1;
        }
        // Entries whose type changed are replaced.
        let mut links: Vec<(&str, &str)> = Vec::new();
        for entry in &manifest.entries {
            let path = entry.path();
            let existing = have.get(path).copied();
            let changed_type = existing.is_some_and(|e| Kind::of(e) != Kind::of(entry));
            if changed_type {
                match existing {
                    Some(Entry::Dir { .. }) => {
                        dir.remove_dir(path).map_err(storage("remove_dir"))?
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
                Entry::File { path, .. } => {
                    if let Some(from) = staged.get(path.as_str()) {
                        place_staged(dir, from, path)?;
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
                    links.push((path, target));
                }
            }
        }
        // Links are written last: what a link points at is in place, and a link to a directory
        // is made as one where the system tells them apart (Windows).
        for (path, target) in links {
            if dir.symlink_metadata(path).is_ok() {
                dir.remove_file(path).map_err(storage("remove_file"))?;
            }
            let to_dir = {
                let _ = (&wanted, target);
                false
            };
            super::recreate_symlink_in(dir, path, Path::new(target), to_dir)?;
            report.written += 1;
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
        let report = self.restore_into(&dir, &manifest)?;
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
        let report = self.restore_into(&dir, &manifest)?;
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
        let project = Self::scope_dir(project_root)?;
        let base_by_path = base_manifest.by_path();
        let current_by_path = current.by_path();
        // Nothing is written through a link: every parent of a path is a real directory
        // (REQ-WS-005, REQ-WS-007). The path of a change is then also its resolved path,
        // which is what the exclusion above was judged on.
        for c in &changes {
            walk_parents(&project, &c.path, false)?;
        }
        // A link whose target leaves the project is not promoted, nor skipped and recorded as
        // promoted: the plan is refused and names it (REQ-WS-005, REQ-WS-007).
        for c in &changes {
            if let Some(Entry::Symlink { target, .. }) = current_by_path.get(c.path.as_str())
                && !target_inside_root(&c.path, target)
            {
                return Err(WorkspaceError::Refused {
                    reason: format!(
                        "the workspace holds a link at {} whose target ({target}) is outside the project; it is not promoted. Remove it or point it inside the project, then plan again",
                        c.path
                    ),
                });
            }
        }
        // Conflicts: the project's target must equal the base entry (REQ-WS-007).
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
        // A directory the promotion removes holds nothing the plan does not list (REQ-WS-007).
        let unlisted = unlisted_entries(&project, &changes, &base_by_path, &current_by_path)?;
        if !unlisted.is_empty() {
            return Err(WorkspaceError::Unlisted { paths: unlisted });
        }
        let mut text_diffs = Vec::new();
        for c in &changes {
            let before = in_base_text(&base_by_path, &c.path, &self.files).unwrap_or_default();
            let after = match current_by_path.get(c.path.as_str()) {
                Some(Entry::File { size, .. }) if *size as usize <= LIM_W3_FILE_BYTES => {
                    open_file(&dir, &c.path).ok().and_then(|mut f| {
                        let mut bytes = Vec::new();
                        f.read_to_end(&mut bytes).ok()?;
                        String::from_utf8(bytes).ok()
                    })
                }
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

    /// Step 1 of a promotion: the backup is written and the promotion made, `applying`. The
    /// project is not touched; the runtime commits the backup before `promote_apply_impl`.
    pub(crate) fn promote_backup_impl(
        &self,
        task_id: TaskId,
        request: &PromotionRequest<'_>,
    ) -> Result<(PreparedSnapshot, Promotion), WorkspaceError> {
        let PromotionRequest {
            plan,
            base,
            project_root,
            backup_id,
            promotion_id,
            now,
        } = *request;
        let dir = self.tree_dir(task_id)?;
        let current = walk_manifest(&dir, None)?;
        if current.hash() != plan.current_hash {
            return Err(WorkspaceError::Refused {
                reason: "the workspace changed since the plan was made; plan again".into(),
            });
        }
        let base_manifest = self
            .files
            .read_manifest(base.snapshot_id, &base.manifest_hash)
            .map_err(persist)?;
        let base_by_path = base_manifest.by_path();
        let project = Self::scope_dir(project_root)?;
        // Backup: the project's current versions of every path the plan touches. Each is
        // the base entry, or the plan is stale (REQ-WS-007).
        let mut backup_entries = Vec::new();
        for c in &plan.changes {
            walk_parents(&project, &c.path, false)?;
            let entry = project_entry(&project, &c.path)?;
            if !matches_base(entry.as_ref(), base_by_path.get(c.path.as_str()).copied()) {
                return Err(WorkspaceError::Conflict {
                    paths: vec![c.path.clone()],
                });
            }
            backup_entries.extend(entry);
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
        let current_by_path = current.by_path();
        let after: Vec<Entry> = plan
            .changes
            .iter()
            .filter(|c| c.kind != ChangeKind::Deleted)
            .filter_map(|c| current_by_path.get(c.path.as_str()).map(|e| (*e).clone()))
            .collect();
        let promotion = Promotion {
            promotion_id,
            task_id,
            backup_snapshot: backup_id,
            project_root: project_root.to_owned(),
            files: plan.changes.clone(),
            applied_at: now,
            reversed_at: None,
            state: PromotionState::Applying,
            after,
            reason: None,
            restored: true,
        };
        Ok((backup, promotion))
    }

    /// Step 2 of a promotion, once the backup is committed and the promotion recorded
    /// `applying`: the listed paths only, each compared with its base entry first.
    pub(crate) fn promote_apply_impl(
        &self,
        task_id: TaskId,
        promotion: &Promotion,
        base: &SnapshotRecord,
    ) -> Result<(), WorkspaceError> {
        let dir = self.tree_dir(task_id)?;
        let base_manifest = self
            .files
            .read_manifest(base.snapshot_id, &base.manifest_hash)
            .map_err(persist)?;
        let base_by_path = base_manifest.by_path();
        let project = Self::scope_dir(&promotion.project_root)?;
        let after: std::collections::BTreeMap<&str, &Entry> =
            promotion.after.iter().map(|e| (e.path(), e)).collect();
        let mut wants = Vec::with_capacity(promotion.files.len());
        for c in &promotion.files {
            let entry = match c.kind {
                ChangeKind::Deleted => None,
                ChangeKind::Added | ChangeKind::Modified => Some(
                    *after
                        .get(c.path.as_str())
                        .ok_or_else(|| WorkspaceError::Refused {
                            reason: format!("the promotion record has no entry for {}", c.path),
                        })?,
                ),
            };
            wants.push(Want {
                path: &c.path,
                entry,
            });
        }
        self.converge(
            &project,
            Source::Tree(&dir),
            &wants,
            Expect::Base(&base_by_path),
            Mode::Promote,
            &mut RestoreReport::default(),
        )
    }

    /// What a reversal would do: the paths it restores or removes, and the ones edited since.
    pub(crate) fn promotion_revert_plan_impl(
        &self,
        promotion: &Promotion,
        backup: &SnapshotRecord,
    ) -> Result<RevertPlan, WorkspaceError> {
        let manifest = self
            .files
            .read_manifest(backup.snapshot_id, &backup.manifest_hash)
            .map_err(persist)?;
        let project = Self::scope_dir(&promotion.project_root)?;
        let by_path = manifest.by_path();
        let decided = revert_decisions(&project, promotion, &by_path)?;
        let mut entries: Vec<RevertEntry> = decided
            .wants
            .iter()
            .map(|w| RevertEntry {
                path: w.path.to_owned(),
                action: if w.entry.is_some() {
                    RevertAction::Restore
                } else {
                    RevertAction::Remove
                },
            })
            .collect();
        entries.sort_by(|a, b| a.path.as_bytes().cmp(b.path.as_bytes()));
        let mut conflicts = decided.edited;
        conflicts.sort();
        Ok(RevertPlan {
            promotion_id: promotion.promotion_id,
            entries,
            conflicts,
            edits_detected: decided.knows_after,
        })
    }

    /// Restores the project from the backup, also the part of a promotion that was applied: a
    /// path is restored while it holds what the promotion wrote (or is missing, as in the middle
    /// of a replacement), is left alone when it already holds the backup's entry, and is kept and
    /// reported when it holds something else, an edit made since (or restored too, when the user
    /// confirmed). Only listed paths are removed, and a directory only when it is empty.
    pub(crate) fn revert_promotion_impl(
        &self,
        promotion: &Promotion,
        backup: &SnapshotRecord,
        edited: EditedPaths,
    ) -> Result<RestoreReport, WorkspaceError> {
        let manifest = self
            .files
            .read_manifest(backup.snapshot_id, &backup.manifest_hash)
            .map_err(persist)?;
        let project = Self::scope_dir(&promotion.project_root)?;
        // What a crash of the promotion left of its own: temporary files beside the paths it
        // wrote, and a staging directory of an earlier reversal (REQ-WS-006, REQ-WS-007).
        remove_leftovers(&project, promotion.files.iter().map(|c| c.path.as_str()));
        let by_path = manifest.by_path();
        let decided = revert_decisions(&project, promotion, &by_path)?;
        let mut report = RestoreReport::default();
        let mut wants = decided.wants;
        match edited {
            EditedPaths::Overwrite => {
                for c in &promotion.files {
                    if decided.edited.contains(&c.path) {
                        wants.push(Want {
                            path: &c.path,
                            entry: by_path.get(c.path.as_str()).copied(),
                        });
                    }
                }
            }
            EditedPaths::Keep => {
                report.left = decided.edited;
                // Under a path that is kept, nothing is touched.
                let kept = report.left.clone();
                wants.retain(|w| !kept.iter().any(|k| w.path.starts_with(&format!("{k}/"))));
            }
        }
        self.converge(
            &project,
            Source::Objects,
            &wants,
            Expect::Nothing,
            Mode::Revert,
            &mut report,
        )?;
        Ok(report)
    }

    /// The test hook of the apply of a promotion; nothing outside tests.
    fn before_entry(&self, expect: &Expect<'_>, path: &str) -> Result<(), WorkspaceError> {
        #[cfg(test)]
        if matches!(expect, Expect::Base(_))
            && let Some(hook) = self.apply_hook.lock().expect("hook").as_ref()
        {
            return hook(path);
        }
        let _ = (expect, path);
        Ok(())
    }

    /// Makes each wanted path of `project` what is wanted of it, and nothing else
    /// (REQ-WS-007): a path is removed only when it is listed, deepest first, a
    /// directory only when it is empty; then the entries are written, parents first.
    /// Before the first change to a path, it is compared with the entry expected of it.
    /// Nothing is written through a link.
    fn converge(
        &self,
        project: &Dir,
        source: Source<'_>,
        wants: &[Want<'_>],
        expect: Expect<'_>,
        mode: Mode,
        report: &mut RestoreReport,
    ) -> Result<(), WorkspaceError> {
        let mut checked: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
        // Verify, then replace (REQ-WS-006): the objects a reversal writes are verified, and
        // written into a staging directory of the project's root, before anything is removed, so
        // that a missing or damaged object leaves the project as it was.
        let mut staged: std::collections::BTreeMap<&str, String> =
            std::collections::BTreeMap::new();
        let mut stage = None;
        if matches!(source, Source::Objects) {
            let mut made = Stage::create(project)?;
            for w in wants {
                if let Some(Entry::File {
                    hash, executable, ..
                }) = w.entry
                {
                    staged.insert(w.path, self.stage_object(&mut made, hash, *executable)?);
                }
            }
            stage = Some(made);
        }
        // Removals: what is there and is not what is wanted.
        let mut removals: Vec<(&str, Kind)> = Vec::new();
        for w in wants {
            if let Some(link) = link_parent(project, w.path)? {
                let replaced = mode == Mode::Revert
                    && wants
                        .iter()
                        .any(|x| x.path == link && x.entry.map(Kind::of) != Some(Kind::Symlink));
                if replaced {
                    continue;
                }
                return Err(link_refused(&link, w.path));
            }
            if let Some(kind) = project_kind(project, w.path)?
                && w.entry.map(Kind::of) != Some(kind)
            {
                removals.push((w.path, kind));
            }
        }
        removals.sort_by(|a, b| {
            depth(b.0)
                .cmp(&depth(a.0))
                .then_with(|| b.0.as_bytes().cmp(a.0.as_bytes()))
        });
        for (path, kind) in removals {
            self.before_entry(&expect, path)?;
            recheck(project, path, &expect, &mut checked)?;
            match kind {
                Kind::Dir => match project.remove_dir(path) {
                    Ok(()) => report.deleted += 1,
                    Err(e) if e.kind() == std::io::ErrorKind::DirectoryNotEmpty => match mode {
                        Mode::Revert => report.left.push(path.to_owned()),
                        Mode::Promote => {
                            return Err(WorkspaceError::Refused {
                                reason: format!(
                                    "{path} holds entries the plan does not list; nothing was removed from it"
                                ),
                            });
                        }
                    },
                    Err(e) => return Err(storage("remove_dir")(e)),
                },
                Kind::File | Kind::Symlink => {
                    project.remove_file(path).map_err(storage("remove_file"))?;
                    report.deleted += 1;
                }
            }
        }
        // Writes, parents first.
        let mut writes: Vec<(&str, &Entry)> = wants
            .iter()
            .filter_map(|w| w.entry.map(|e| (w.path, e)))
            .collect();
        // Links are written last: what a link points at is in place (REQ-WS-005).
        writes.sort_by(|a, b| {
            matches!(a.1, Entry::Symlink { .. })
                .cmp(&matches!(b.1, Entry::Symlink { .. }))
                .then_with(|| a.0.as_bytes().cmp(b.0.as_bytes()))
        });
        for (path, entry) in writes {
            if report
                .left
                .iter()
                .any(|l| path.starts_with(&format!("{l}/")) || l == path)
            {
                continue;
            }
            self.before_entry(&expect, path)?;
            walk_parents(project, path, true)?;
            recheck(project, path, &expect, &mut checked)?;
            match entry {
                Entry::Dir { .. } => {
                    if project_kind(project, path)? != Some(Kind::Dir) {
                        project.create_dir(path).map_err(storage("create_dir"))?;
                    }
                }
                Entry::File {
                    hash, executable, ..
                } => {
                    match &source {
                        Source::Tree(tree) => {
                            Self::copy_verified(tree, project, path, hash, *executable)?
                        }
                        Source::Objects => {
                            let from = staged.get(path).ok_or_else(|| WorkspaceError::Storage {
                                detail: "staged".into(),
                            })?;
                            place_staged(project, from, path)?
                        }
                    }
                    report.written += 1;
                }
                Entry::Symlink { target, .. } => {
                    if !target_inside_root(path, target) {
                        report.links_omitted.push(path.to_owned());
                        continue;
                    }
                    if project_kind(project, path)?.is_some() {
                        project.remove_file(path).map_err(storage("remove_file"))?;
                    }
                    let to_dir =
                        cfg!(windows) && super::points_at_directory(project, path, target, None);
                    super::recreate_symlink_in(project, path, Path::new(target), to_dir)?;
                    report.written += 1;
                }
            }
        }
        drop(stage);
        Ok(())
    }

    /// Removes the files of a deleted task (RFC 006 §6; handoff 006b): the manifests, objects and
    /// blobs only the task referenced first, then its working tree and temporary directory. A
    /// directory is made writable before it is emptied (a toolchain may leave read-only ones),
    /// every removal is attempted, and each failure is reported by path. The journal of the
    /// deletion is closed only when nothing is left; otherwise the next start tries again.
    pub(crate) fn delete_files_impl(
        &self,
        task_id: TaskId,
        rows: &DeletedRows,
    ) -> Result<(), WorkspaceError> {
        let mut failed: Vec<String> = Vec::new();
        for id in &rows.snapshots {
            if self.files.remove_manifest(*id).is_err() {
                failed.push(format!("snapshots/manifests/{id}.json"));
            }
        }
        for hash in &rows.objects_at_zero {
            if self.files.remove_object(hash).is_err() {
                failed.push(format!("snapshots/objects/{}", hash.as_str()));
            }
        }
        for blob in &rows.blobs {
            let path = self.data_dir.join("blobs").join(blob.to_string());
            remove_tree(&path, &self.data_dir, &mut failed);
        }
        remove_tree(&self.task_dir(task_id), &self.data_dir, &mut failed);
        self.tools.lock().expect("workspace tools").remove(&task_id);
        if failed.is_empty() {
            self.files.finish_deletion(task_id).map_err(persist)?;
            Ok(())
        } else {
            failed.sort();
            failed.dedup();
            Err(WorkspaceError::NotRemoved {
                task_id,
                paths: failed,
            })
        }
    }
}

/// Whether `name` is `prefix`, then two hexadecimal numbers joined by `-`: the form of the
/// temporary names and staging names this program makes.
fn is_leftover(name: &str, prefix: &str) -> bool {
    name.strip_prefix(prefix)
        .and_then(|rest| rest.split_once('-'))
        .is_some_and(|(pid, n)| {
            [pid, n]
                .iter()
                .all(|h| !h.is_empty() && h.chars().all(|c| c.is_ascii_hexdigit()))
        })
}

/// Removes what a crashed promotion or reversal left in the project (REQ-WS-006, REQ-WS-007):
/// the temporary files (`.mimlys-tmp-<pid>-<n>`) in the directories above the given paths, and a
/// staging directory (`.mimlys-stage-<pid>-<n>`) in the root. Only entries of exactly those
/// names are removed, a file or a directory as the name says and never through a link, and only
/// in directories that are real directories on the way from the root; a failure to remove is
/// not an error (the reversal does not depend on it, and the next one tries again).
fn remove_leftovers<'a>(project: &Dir, paths: impl Iterator<Item = &'a str>) {
    let mut parents: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    parents.insert(String::new());
    for path in paths {
        let mut prefix = String::new();
        let components: Vec<&str> = path.split('/').collect();
        for part in &components[..components.len() - 1] {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(part);
            parents.insert(prefix.clone());
        }
    }
    for parent in parents {
        // Every directory on the way is a real one: a link or a file ends the look.
        if !parent.is_empty() && !matches!(project_kind(project, &parent), Ok(Some(Kind::Dir))) {
            continue;
        }
        if matches!(link_parent(project, &parent), Ok(Some(_)) | Err(_)) {
            continue;
        }
        let Ok(entries) = project.read_dir(if parent.is_empty() { "." } else { &parent }) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            let path = join(&parent, &name);
            if file_type.is_file() && is_leftover(&name, TEMP_PREFIX) {
                let _ = project.remove_file(&path);
            } else if file_type.is_dir() && parent.is_empty() && is_leftover(&name, STAGE_PREFIX) {
                let _ = project.remove_dir_all(&path);
            }
        }
    }
}

/// Removes `path` and everything below it without following links, attempting every removal;
/// each failure is added to `failed` as a path relative to `root`.
fn remove_tree(path: &Path, root: &Path, failed: &mut Vec<String>) {
    let name = |p: &Path| {
        p.strip_prefix(root)
            .unwrap_or(p)
            .to_string_lossy()
            .replace('\\', "/")
    };
    let meta = match std::fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
        Err(_) => {
            failed.push(name(path));
            return;
        }
    };
    if meta.is_dir() {
        make_accessible(path, &meta);
        match std::fs::read_dir(path) {
            Ok(entries) => {
                for entry in entries.flatten() {
                    remove_tree(&entry.path(), root, failed);
                }
            }
            Err(_) => failed.push(name(path)),
        }
        if std::fs::remove_dir(path).is_err() && std::fs::symlink_metadata(path).is_ok() {
            failed.push(name(path));
        }
    } else if std::fs::remove_file(path).is_err() {
        make_accessible(path, &meta);
        if std::fs::remove_file(path).is_err() && std::fs::symlink_metadata(path).is_ok() {
            failed.push(name(path));
        }
    }
}

/// Makes a directory readable, writable and searchable by its owner (and a file writable), so
/// that it can be emptied and removed.
fn make_accessible(path: &Path, meta: &std::fs::Metadata) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = meta.permissions().mode();
        let wanted = if meta.is_dir() { 0o700 } else { 0o200 };
        if mode & wanted != wanted {
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode | wanted));
        }
    }
    #[cfg(not(unix))]
    {
        let mut perms = meta.permissions();
        if perms.readonly() {
            #[allow(clippy::permissions_set_readonly_false)]
            perms.set_readonly(false);
            let _ = std::fs::set_permissions(path, perms);
        }
    }
}

/// What the project holds at a path.
enum Probed {
    Absent,
    Entry(Entry),
    /// A named pipe, a socket, a device or anything else that is not a file, a directory or a
    /// link: looked at and never opened. The name says what it is.
    Special(&'static str),
}

/// Looks at the project's entry at `rel` (hashing a file, with the non-blocking open).
fn probe(project: &Dir, rel: &str) -> Result<Probed, WorkspaceError> {
    let Ok(meta) = project.symlink_metadata(rel) else {
        return Ok(Probed::Absent);
    };
    if meta.is_symlink() {
        return Ok(Probed::Entry(Entry::Symlink {
            path: rel.to_owned(),
            target: target_of(project, rel, rel)?,
        }));
    }
    if meta.is_dir() {
        return Ok(Probed::Entry(Entry::Dir {
            path: rel.to_owned(),
        }));
    }
    if !meta.is_file() {
        return Ok(Probed::Special(special_name(&meta.file_type())));
    }
    let (hash, size) = hash_file(project, rel)?;
    Ok(Probed::Entry(Entry::File {
        path: rel.to_owned(),
        executable: is_executable(&meta),
        size,
        hash,
    }))
}

/// The project's entry at `rel`, hashed, or `None` when absent. A path that is a named pipe, a
/// socket or a device is refused, naming it: nothing of the project that is not a file, a
/// directory or a link is ever opened (REQ-WS-007).
fn project_entry(project: &Dir, rel: &str) -> Result<Option<Entry>, WorkspaceError> {
    match probe(project, rel)? {
        Probed::Absent => Ok(None),
        Probed::Entry(entry) => Ok(Some(entry)),
        Probed::Special(what) => Err(not_regular(rel, what)),
    }
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

/// What a reversal is to do, path by path.
struct RevertDecisions<'a> {
    /// What each path to change is to become (`None`: removed).
    wants: Vec<Want<'a>>,
    /// Paths that hold neither what the promotion wrote nor what it replaced: edited since.
    edited: Vec<String>,
    /// Whether an edit can be told from the promotion's own write.
    knows_after: bool,
}

/// Decides, for each path of the promotion, whether the reversal restores it, leaves it (it
/// already holds the backup's entry) or reports it as edited since.
fn revert_decisions<'a>(
    project: &Dir,
    promotion: &'a Promotion,
    backed_up: &std::collections::BTreeMap<&str, &'a Entry>,
) -> Result<RevertDecisions<'a>, WorkspaceError> {
    let after: std::collections::BTreeMap<&str, &Entry> =
        promotion.after.iter().map(|e| (e.path(), e)).collect();
    // A record from before the entries were recorded cannot tell an edit from the promotion's
    // own write: every path is restored.
    let knows_after = !promotion.after.is_empty()
        || promotion
            .files
            .iter()
            .all(|c| c.kind == ChangeKind::Deleted);
    let mut wants = Vec::with_capacity(promotion.files.len());
    let mut edited = Vec::new();
    for c in &promotion.files {
        let path = c.path.as_str();
        let backup_entry = backed_up.get(path).copied();
        if knows_after {
            let current = if link_parent(project, path)?.is_some() {
                None
            } else {
                match probe(project, path)? {
                    Probed::Absent => None,
                    Probed::Entry(entry) => Some(entry),
                    // Something put there since the promotion: an edit, kept as it is.
                    Probed::Special(_) => {
                        edited.push(path.to_owned());
                        continue;
                    }
                }
            };
            if matches_base(current.as_ref(), backup_entry) {
                continue;
            }
            let wrote = after.get(path).copied();
            if current.is_some() && !matches_base(current.as_ref(), wrote) {
                edited.push(path.to_owned());
                continue;
            }
        }
        wants.push(Want {
            path,
            entry: backup_entry,
        });
    }
    Ok(RevertDecisions {
        wants,
        edited,
        knows_after,
    })
}

/// The kind of an entry, without its content.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    File,
    Dir,
    Symlink,
}

impl Kind {
    fn of(entry: &Entry) -> Self {
        match entry {
            Entry::File { .. } => Kind::File,
            Entry::Dir { .. } => Kind::Dir,
            Entry::Symlink { .. } => Kind::Symlink,
        }
    }
}

/// What the project's entry at a path is to become (`None`: absent).
struct Want<'a> {
    path: &'a str,
    entry: Option<&'a Entry>,
}

/// Where the content of a written file comes from.
enum Source<'a> {
    /// The task's working tree.
    Tree(&'a Dir),
    /// The snapshot objects.
    Objects,
}

/// What the project's entry is expected to be before it is first changed.
enum Expect<'a> {
    /// Nothing is checked.
    Nothing,
    /// The entry of the base manifest, or absent when the base has none.
    Base(&'a std::collections::BTreeMap<&'a str, &'a Entry>),
}

/// What converges the project: a promotion, or the reversal of one.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// A directory that is not empty stops the promotion, and so does a link as a parent.
    Promote,
    /// A directory that is not empty is kept and reported; a link that is itself replaced is
    /// removed first and then nothing is below it.
    Revert,
}

/// The number of components above the last.
fn depth(path: &str) -> usize {
    path.matches('/').count()
}

/// The kind of the project's entry at `rel` (a link is a link), or `None` when absent. The
/// parents of `rel` are real directories (`walk_parents`).
fn project_kind(project: &Dir, rel: &str) -> Result<Option<Kind>, WorkspaceError> {
    match project.symlink_metadata(rel) {
        Ok(m) if m.is_symlink() => Ok(Some(Kind::Symlink)),
        Ok(m) if m.is_dir() => Ok(Some(Kind::Dir)),
        Ok(_) => Ok(Some(Kind::File)),
        // Nothing lies below a file.
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
            ) =>
        {
            Ok(None)
        }
        Err(e) => Err(storage("stat")(e)),
    }
}

/// The first parent of `rel` in the project that is a link, found one parent at a time without
/// following links. On Windows a junction is a link here: cap-std reports a reparse point that
/// redirects the path as a link (`is_symlink`).
fn link_parent(project: &Dir, rel: &str) -> Result<Option<String>, WorkspaceError> {
    let parts: Vec<&str> = rel.split('/').collect();
    let mut prefix = String::new();
    for part in &parts[..parts.len().saturating_sub(1)] {
        if !prefix.is_empty() {
            prefix.push('/');
        }
        prefix.push_str(part);
        match project_kind(project, &prefix)? {
            Some(Kind::Symlink) => return Ok(Some(prefix)),
            Some(Kind::Dir) => {}
            // Nothing lies below a file or an absent parent.
            Some(Kind::File) | None => return Ok(None),
        }
    }
    Ok(None)
}

fn link_refused(link: &str, rel: &str) -> WorkspaceError {
    WorkspaceError::Refused {
        reason: format!(
            "the project holds a link at {link}, a parent of {rel}; nothing is written through a link"
        ),
    }
}

/// Checks the parents of `rel` in the project (REQ-WS-005, REQ-WS-007): a link as a parent is
/// refused and named. With `create`, a missing parent is made and a parent that is not a
/// directory is refused.
fn walk_parents(project: &Dir, rel: &str, create: bool) -> Result<(), WorkspaceError> {
    if let Some(link) = link_parent(project, rel)? {
        return Err(link_refused(&link, rel));
    }
    if !create {
        return Ok(());
    }
    let parts: Vec<&str> = rel.split('/').collect();
    let mut prefix = String::new();
    for part in &parts[..parts.len().saturating_sub(1)] {
        if !prefix.is_empty() {
            prefix.push('/');
        }
        prefix.push_str(part);
        match project_kind(project, &prefix)? {
            Some(Kind::Dir) => {}
            Some(_) => {
                return Err(WorkspaceError::Refused {
                    reason: format!("{prefix}, a parent of {rel}, is not a directory"),
                });
            }
            None => project.create_dir(&prefix).map_err(storage("create_dir"))?,
        }
    }
    Ok(())
}

/// The top-most entries of the project that lie in a directory the plan removes and that the
/// plan does not list as deleted (REQ-WS-007).
fn unlisted_entries(
    project: &Dir,
    changes: &[Change],
    base: &std::collections::BTreeMap<&str, &Entry>,
    current: &std::collections::BTreeMap<&str, &Entry>,
) -> Result<Vec<String>, WorkspaceError> {
    let listed: std::collections::BTreeSet<&str> = changes
        .iter()
        .filter(|c| c.kind == ChangeKind::Deleted)
        .map(|c| c.path.as_str())
        .collect();
    let mut out = Vec::new();
    for c in changes {
        let removed_dir = matches!(base.get(c.path.as_str()), Some(Entry::Dir { .. }))
            && !matches!(current.get(c.path.as_str()), Some(Entry::Dir { .. }));
        if removed_dir && project_kind(project, &c.path)? == Some(Kind::Dir) {
            unlisted_in(project, &c.path, &listed, &mut out)?;
        }
    }
    out.sort();
    out.dedup();
    Ok(out)
}

/// Adds to `out` the top-most entries below `dir` that are not in `listed`.
fn unlisted_in(
    project: &Dir,
    dir: &str,
    listed: &std::collections::BTreeSet<&str>,
    out: &mut Vec<String>,
) -> Result<(), WorkspaceError> {
    walk(project, dir, None, |found| {
        if listed.contains(found.path.as_str()) {
            Ok(Flow::Descend)
        } else {
            out.push(found.path);
            Ok(Flow::Skip)
        }
    })
}

/// Whether the project's entry is the base entry (both absent counts).
fn matches_base(project: Option<&Entry>, base: Option<&Entry>) -> bool {
    match (project, base) {
        (None, None) => true,
        (Some(p), Some(b)) => entry_equal(p, b),
        _ => false,
    }
}

/// Before a path is first changed, compares the project's entry with the one expected.
fn recheck<'a>(
    project: &Dir,
    path: &'a str,
    expect: &Expect<'_>,
    checked: &mut std::collections::BTreeSet<&'a str>,
) -> Result<(), WorkspaceError> {
    let Expect::Base(base) = expect else {
        return Ok(());
    };
    if !checked.insert(path) {
        return Ok(());
    }
    let entry = project_entry(project, path)?;
    if matches_base(entry.as_ref(), base.get(path).copied()) {
        Ok(())
    } else {
        Err(WorkspaceError::Conflict {
            paths: vec![path.to_owned()],
        })
    }
}
