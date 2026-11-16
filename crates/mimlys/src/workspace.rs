//! Task workspaces on the local disk (external design §14.1, §14.2;
//! REQ-WS-002, REQ-WS-003, REQ-WS-008). The scope is walked first and
//! refused above the limits before anything is written; then it is copied
//! through `cap-std` directory handles on both sides. Symbolic links are
//! recreated only when their target stays inside the scope; the rest are
//! omitted and counted. The project scope is never written (INV-10).

use cap_std::ambient_authority;
use cap_std::fs::Dir;
use mimlys_core::ids::TaskId;
use mimlys_core::path::{CaseMode, WorkspacePath};
use mimlys_core::policy::ExclusionMatcher;
use mimlys_core::snapshot::target_resolves_inside_root;
use mimlys_core::spi::{
    ToolProvider, WorkspaceError, WorkspaceInfo, WorkspaceProvider, WorkspaceRead,
};
use mimlys_core::task::ScopeSpec;
use mimlys_exec::builtin::BuiltinTools;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

mod snapshot;
mod walk;
use walk::{Flow, Found, Node, walk};

/// The metadata file beside a task's tree.
const INFO_FILE: &str = "workspace.json";
/// The case probe's file name (created in the tree, removed at once).
const CASE_PROBE: &str = ".mimlys-case-probe";

/// Workspaces under `<data_dir>/workspaces/<task_id>/`.
pub struct LocalWorkspaces {
    data_dir: PathBuf,
    patterns: Vec<String>,
    max_bytes: u64,
    max_files: u64,
    tools: Mutex<HashMap<TaskId, Arc<dyn ToolProvider>>>,
    sandbox: Option<SandboxTools>,
    external: Option<Arc<mimlys_mcp::tools::Servers>>,
    files: mimlys_persist::snapshots::SnapshotFiles,
    /// Test only: called with the path of each entry a promotion is about to apply; an error stops it there.
    #[cfg(test)]
    apply_hook: Mutex<Option<ApplyHook>>,
}

/// A test's step into the apply of a promotion.
#[cfg(test)]
type ApplyHook = Box<dyn Fn(&str) -> Result<(), WorkspaceError> + Send + Sync>;

/// The built-in (and sandbox) tools of a task beside its external tools
/// (RFC 009): a tool with a dot in its name belongs to a server.
struct ComposedTools {
    local: Arc<dyn ToolProvider>,
    external: mimlys_mcp::McpTools,
}

impl ToolProvider for ComposedTools {
    fn definitions(&self) -> Vec<mimlys_core::tool::ToolDefinition> {
        let mut all = self.local.definitions();
        all.extend(self.external.definitions());
        all
    }

    fn imports(&self) -> std::collections::BTreeMap<String, mimlys_core::tool::ImportRecord> {
        self.external.imports()
    }

    fn refresh_pins(
        &self,
        pinned: &std::collections::BTreeMap<String, mimlys_core::tool::ImportRecord>,
        interruption: &mimlys_core::spi::Cancellation,
    ) -> Vec<String> {
        self.external.refresh_pins(pinned, interruption)
    }

    fn drain_events(&self) -> Vec<mimlys_core::event::Event> {
        self.external.drain_events()
    }

    fn execute<'a>(
        &'a self,
        action: &'a mimlys_core::validate::AuthorizedAction,
        context: mimlys_core::spi::ExecutionContext,
    ) -> mimlys_core::spi::BoxFuture<'a, mimlys_core::spi::ExecutionOutcome> {
        if action.tool().contains('.') {
            self.external.execute(action, context)
        } else {
            self.local.execute(action, context)
        }
    }
}

/// The worker and what the `run_*` definitions need.
pub struct SandboxTools {
    /// The worker.
    pub worker: Arc<mimlys_exec::worker::Worker>,
    /// The declared host tools (RFC 011 §6).
    pub host_tools: Vec<mimlys_core::host_tool::HostToolDeclaration>,
    /// Whether host tools are available here (tier `full`).
    pub host_tools_available: bool,
    /// The dependency step's worker (RFC 011 §8).
    pub fetch_worker: Option<Arc<mimlys_exec::worker::Worker>>,
    /// Configured languages.
    pub languages: Vec<String>,
    /// The sandbox deadline (`[sandbox] deadline_ms`).
    pub deadline_ms: i64,
}

/// What the walk measured.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Measured {
    bytes: u64,
    files: u64,
}

impl LocalWorkspaces {
    /// A provider over `data_dir` with the policy's exclusion patterns and
    /// the workspace limits (LIM-W1, LIM-W2).
    pub fn new(
        data_dir: PathBuf,
        patterns: Vec<String>,
        max_bytes: u64,
        max_files: u64,
    ) -> Result<Self, WorkspaceError> {
        let files = mimlys_persist::snapshots::SnapshotFiles::open(&data_dir).map_err(|e| {
            WorkspaceError::Storage {
                detail: e.to_string(),
            }
        })?;
        Ok(Self {
            data_dir,
            patterns,
            max_bytes,
            max_files,
            tools: Mutex::new(HashMap::new()),
            sandbox: None,
            external: None,
            files,
            #[cfg(test)]
            apply_hook: Mutex::new(None),
        })
    }

    /// A provider for commands that only read (REQ-PERS-013): nothing is
    /// created under the data directory. Used through [`WorkspaceRead`], which
    /// has no writing method; the limits are those of a writer's and unused.
    pub fn for_reading(data_dir: PathBuf, patterns: Vec<String>) -> Self {
        let files = mimlys_persist::snapshots::SnapshotFiles::open_for_reading(&data_dir);
        Self {
            data_dir,
            patterns,
            max_bytes: 0,
            max_files: 0,
            tools: Mutex::new(HashMap::new()),
            sandbox: None,
            external: None,
            files,
            #[cfg(test)]
            apply_hook: Mutex::new(None),
        }
    }

    fn write_info(&self, task_id: TaskId, info: &WorkspaceInfo) -> Result<(), WorkspaceError> {
        let text = serde_json::to_string_pretty(info).map_err(|_| WorkspaceError::Storage {
            detail: "workspace_info".into(),
        })?;
        std::fs::write(self.task_dir(task_id).join(INFO_FILE), text).map_err(Self::storage("write"))
    }

    /// Adds the worker-backed `run_script` and `run_wasm` (M2).
    pub fn with_sandbox(mut self, sandbox: SandboxTools) -> Self {
        self.sandbox = Some(sandbox);
        self
    }

    /// Adds the configured MCP servers (RFC 009); their tools are imported
    /// per task when its provider is first opened.
    pub fn with_external(mut self, servers: Arc<mimlys_mcp::tools::Servers>) -> Self {
        if !servers.is_empty() {
            self.external = Some(servers);
        }
        self
    }

    fn task_dir(&self, task_id: TaskId) -> PathBuf {
        self.data_dir.join("workspaces").join(task_id.to_string())
    }

    /// The task's working tree.
    pub fn tree_path(&self, task_id: TaskId) -> PathBuf {
        self.task_dir(task_id).join("tree")
    }

    fn storage(detail: &'static str) -> impl Fn(std::io::Error) -> WorkspaceError {
        move |_| WorkspaceError::Storage {
            detail: detail.to_owned(),
        }
    }

    /// Probes the case mode of the filesystem under `dir` (REQ-WS-011).
    fn probe_case(dir: &Dir) -> Result<CaseMode, WorkspaceError> {
        dir.write(CASE_PROBE, b"")
            .map_err(Self::storage("case_probe"))?;
        let upper = CASE_PROBE.to_ascii_uppercase();
        let insensitive = dir.symlink_metadata(&upper).is_ok();
        dir.remove_file(CASE_PROBE)
            .map_err(Self::storage("case_probe"))?;
        Ok(if insensitive {
            CaseMode::Insensitive
        } else {
            CaseMode::Sensitive
        })
    }

    /// Walks the scope once, measuring what would be copied and refusing it above the limits
    /// as soon as one is passed. No write happens. A name that is not valid UTF-8, or a part
    /// of the scope that cannot be read, ends the walk with an error that names the path.
    fn measure(
        &self,
        src: &Dir,
        matcher: &ExclusionMatcher,
    ) -> Result<(Vec<Found>, Measured), WorkspaceError> {
        let mut found = Vec::new();
        let mut measured = Measured::default();
        walk(src, "", Some(matcher), |entry| {
            if let Node::File { len, .. } = entry.node {
                measured.files += 1;
                measured.bytes += len;
                if measured.bytes > self.max_bytes || measured.files > self.max_files {
                    return Err(WorkspaceError::TooLarge {
                        bytes: measured.bytes,
                        files: measured.files,
                    });
                }
            }
            found.push(entry);
            Ok(Flow::Descend)
        })?;
        Ok((found, measured))
    }

    /// Copies the walked entries, parents first; symbolic links are recreated, last, only when
    /// their target stays inside the scope, lexically. A named pipe, a socket or a device of the
    /// scope is not copied.
    fn copy_found(
        src: &Dir,
        dst: &Dir,
        found: &[Found],
        omitted: &mut u64,
    ) -> Result<(), WorkspaceError> {
        let mut links = Vec::new();
        for entry in found {
            match &entry.node {
                Node::Dir => dst
                    .create_dir(&entry.path)
                    .map_err(Self::storage("create_dir"))?,
                Node::File { .. } => copy_file(src, dst, &entry.path)?,
                Node::Link { target } => links.push((entry.path.as_str(), target.as_str())),
                Node::Special(_) => {}
            }
        }
        // A link stays inside the scope when it is resolved through the scope's own links too
        // (REQ-WS-005): a chain that leaves it is omitted like a link that does.
        let scope_links: std::collections::BTreeMap<&str, &str> = links.iter().copied().collect();
        for (path, target) in links {
            if target_resolves_inside_root(path, target, &scope_links) {
                recreate_symlink(src, dst, path, Path::new(target))?;
            } else {
                *omitted += 1;
            }
        }
        Ok(())
    }

    fn read_info(&self, task_id: TaskId) -> Result<WorkspaceInfo, WorkspaceError> {
        let text = std::fs::read_to_string(self.task_dir(task_id).join(INFO_FILE))
            .map_err(|_| WorkspaceError::Missing)?;
        serde_json::from_str(&text).map_err(|_| WorkspaceError::Storage {
            detail: "workspace_info".into(),
        })
    }

    /// The bytes of the regular files of the tree, by the one walk (REQ-WS-008).
    fn tree_size(dir: &Dir) -> Result<u64, WorkspaceError> {
        let mut total = 0;
        walk(dir, "", None, |entry| {
            if let Node::File { len, .. } = entry.node {
                total += len;
            }
            Ok(Flow::Descend)
        })?;
        Ok(total)
    }
}

/// Copies a regular file of the scope into the tree: the source is opened without blocking and
/// looked at through the handle, so that a named pipe put at the path since the walk is refused
/// by name instead of waited for. The content is copied by the operating system where it can
/// (a reflink on a filesystem that has them); the permissions follow the file.
fn copy_file(src: &Dir, dst: &Dir, rel: &str) -> Result<(), WorkspaceError> {
    let mut from = snapshot::open_file(src, rel)?.into_std();
    let permissions = from
        .metadata()
        .map_err(LocalWorkspaces::storage("copy"))?
        .permissions();
    let mut to = dst
        .create(rel)
        .map_err(LocalWorkspaces::storage("copy"))?
        .into_std();
    std::io::copy(&mut from, &mut to).map_err(LocalWorkspaces::storage("copy"))?;
    to.set_permissions(permissions)
        .map_err(LocalWorkspaces::storage("copy"))
}

fn join(rel: &str, name: &str) -> String {
    if rel.is_empty() {
        name.to_owned()
    } else {
        format!("{rel}/{name}")
    }
}

fn is_excluded(matcher: &ExclusionMatcher, rel: &str) -> bool {
    match WorkspacePath::canonicalize(rel) {
        Ok(path) => matcher.is_excluded(&path),
        // A name the canonicalizer refuses (control characters, reserved names) is never copied.
        Err(_) => true,
    }
}

/// How many links are followed to tell what a link points at (the most the system itself follows
/// on Linux: `MAXSYMLINKS`).
const KIND_HOPS: usize = 40;

/// Where a link at `link` pointing at `target` lands, written from the root: the link's directory,
/// then the target with `.` and `..` resolved by text (a backslash separates, as everywhere in the
/// manifest's rules). `None` when the target is absolute or leaves the root.
pub(crate) fn resolve_target(link: &str, target: &str) -> Option<String> {
    let bytes = target.as_bytes();
    if bytes.first().is_some_and(|b| *b == b'/' || *b == b'\\')
        || (bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':')
    {
        return None;
    }
    let mut parts: Vec<&str> = link.split('/').collect();
    parts.pop();
    for piece in target.split(['/', '\\']) {
        match piece {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            name => parts.push(name),
        }
    }
    Some(parts.join("/"))
}

/// Whether the link at `link` pointing at `target` points at a directory (REQ-WS-006): the entry
/// the target names is looked up in `known`, the entries of the manifest being restored, following
/// a link it finds a bounded number of times; what `known` does not hold is looked at in `dst` as
/// it stands, after its writes. Windows makes a directory link another way than a file link; the
/// other systems do not ask.
pub(crate) fn points_at_directory(
    dst: &Dir,
    link: &str,
    target: &str,
    known: Option<&std::collections::BTreeMap<&str, &mimlys_core::snapshot::Entry>>,
) -> bool {
    use mimlys_core::snapshot::Entry;
    let (mut link, mut target) = (link.to_owned(), target.to_owned());
    for _ in 0..KIND_HOPS {
        let Some(path) = resolve_target(&link, &target) else {
            return false;
        };
        if path.is_empty() {
            return true;
        }
        match known.and_then(|k| k.get(path.as_str())) {
            Some(Entry::Dir { .. }) => return true,
            Some(Entry::File { .. }) => return false,
            Some(Entry::Symlink { target: next, .. }) => {
                target = next.clone();
                link = path;
            }
            None => return dst.metadata(&path).map(|m| m.is_dir()).unwrap_or(false),
        }
    }
    false
}

/// Recreates a link inside one directory handle (restore and promotion). `to_dir`: it points at a
/// directory, which Windows needs to know and the other systems ignore.
#[cfg(not(windows))]
pub(crate) fn recreate_symlink_in(
    dst: &Dir,
    link: &str,
    target: &Path,
    _to_dir: bool,
) -> Result<(), WorkspaceError> {
    dst.symlink(target, link)
        .map_err(|_| WorkspaceError::Storage {
            detail: "symlink".into(),
        })
}

/// As above; the target is written with the separator Windows needs in a link, and a directory
/// link is made as one.
#[cfg(windows)]
pub(crate) fn recreate_symlink_in(
    dst: &Dir,
    link: &str,
    target: &Path,
    to_dir: bool,
) -> Result<(), WorkspaceError> {
    let target = PathBuf::from(target.to_string_lossy().replace('/', "\\"));
    let result = if to_dir {
        dst.symlink_dir(&target, link)
    } else {
        dst.symlink_file(&target, link)
    };
    result.map_err(|_| WorkspaceError::Storage {
        detail: "symlink".into(),
    })
}

#[cfg(not(windows))]
fn recreate_symlink(
    _src: &Dir,
    dst: &Dir,
    link: &str,
    target: &Path,
) -> Result<(), WorkspaceError> {
    dst.symlink(target, link)
        .map_err(|_| WorkspaceError::Storage {
            detail: "symlink".into(),
        })
}

#[cfg(windows)]
fn recreate_symlink(src: &Dir, dst: &Dir, link: &str, target: &Path) -> Result<(), WorkspaceError> {
    let is_dir = src.metadata(link).map(|m| m.is_dir()).unwrap_or(false);
    let target = PathBuf::from(target.to_string_lossy().replace('/', "\\"));
    let result = if is_dir {
        dst.symlink_dir(&target, link)
    } else {
        dst.symlink_file(&target, link)
    };
    result.map_err(|_| WorkspaceError::Storage {
        detail: "symlink".into(),
    })
}

impl WorkspaceRead for LocalWorkspaces {
    fn manifest(
        &self,
        snapshot: &mimlys_core::snapshot::SnapshotRecord,
    ) -> Result<mimlys_core::snapshot::Manifest, WorkspaceError> {
        self.files
            .read_manifest(snapshot.snapshot_id, &snapshot.manifest_hash)
            .map_err(|e| WorkspaceError::Storage {
                detail: e.to_string(),
            })
    }

    fn promotion_plan(
        &self,
        task_id: TaskId,
        base: &mimlys_core::snapshot::SnapshotRecord,
        project_root: &str,
    ) -> Result<mimlys_core::snapshot::PromotionPlan, WorkspaceError> {
        self.promotion_plan_impl(task_id, base, project_root)
    }

    fn tree_bytes(&self, task_id: TaskId) -> Result<u64, WorkspaceError> {
        let dir = Dir::open_ambient_dir(self.tree_path(task_id), ambient_authority())
            .map_err(|_| WorkspaceError::Missing)?;
        Self::tree_size(&dir)
    }
}

impl WorkspaceProvider for LocalWorkspaces {
    fn materialize(
        &self,
        task_id: TaskId,
        scope: &ScopeSpec,
    ) -> Result<WorkspaceInfo, WorkspaceError> {
        let root = PathBuf::from(&scope.root);
        let root = std::fs::canonicalize(&root).map_err(|_| WorkspaceError::ScopeNotFound)?;
        if !root.is_dir() {
            return Err(WorkspaceError::ScopeNotFound);
        }
        let data_dir =
            std::fs::canonicalize(&self.data_dir).unwrap_or_else(|_| self.data_dir.clone());
        if root.starts_with(&data_dir) {
            return Err(WorkspaceError::InsideDataDir);
        }
        if data_dir.starts_with(&root) {
            return Err(WorkspaceError::ContainsDataDir);
        }
        let task_dir = self.task_dir(task_id);
        std::fs::create_dir_all(task_dir.join("tree")).map_err(Self::storage("create_dir"))?;
        std::fs::create_dir_all(task_dir.join("tmp")).map_err(Self::storage("create_dir"))?;
        let dst = Dir::open_ambient_dir(task_dir.join("tree"), ambient_authority())
            .map_err(Self::storage("open"))?;
        let case_mode = Self::probe_case(&dst)?;
        let matcher = ExclusionMatcher::new(&self.patterns, case_mode).map_err(|_| {
            WorkspaceError::Storage {
                detail: "exclusion_pattern".into(),
            }
        })?;
        let src = Dir::open_ambient_dir(&root, ambient_authority())
            .map_err(|_| WorkspaceError::ScopeNotFound)?;
        // Walk first, copy second (REQ-WS-002).
        let (found, measured) = self.measure(&src, &matcher)?;
        let mut omitted = 0;
        Self::copy_found(&src, &dst, &found, &mut omitted)?;
        let info = WorkspaceInfo {
            root: task_dir.join("tree").to_string_lossy().into_owned(),
            case_mode,
            bytes: measured.bytes,
            files: measured.files,
            symlinks_omitted: omitted,
        };
        let text = serde_json::to_string_pretty(&info).map_err(|_| WorkspaceError::Storage {
            detail: "workspace_info".into(),
        })?;
        std::fs::write(task_dir.join(INFO_FILE), text).map_err(Self::storage("write"))?;
        Ok(info)
    }

    fn open(&self, task_id: TaskId) -> Result<WorkspaceInfo, WorkspaceError> {
        self.read_info(task_id)
    }

    fn tools(&self, task_id: TaskId) -> Result<Arc<dyn ToolProvider>, WorkspaceError> {
        let mut cache = self.tools.lock().expect("workspace tools");
        if let Some(tools) = cache.get(&task_id) {
            return Ok(Arc::clone(tools));
        }
        self.read_info(task_id)?;
        let builtin =
            BuiltinTools::open(&self.tree_path(task_id)).map_err(|_| WorkspaceError::Missing)?;
        let local: Arc<dyn ToolProvider> = match &self.sandbox {
            Some(s) => Arc::new(
                mimlys_exec::tools::WorkerTools::new(
                    builtin,
                    Arc::clone(&s.worker),
                    task_id,
                    self.tree_path(task_id).to_string_lossy().into_owned(),
                    s.languages.clone(),
                    s.deadline_ms,
                )
                .with_host_tools(
                    s.host_tools.clone(),
                    s.host_tools_available,
                    s.fetch_worker.clone(),
                ),
            ),
            None => Arc::new(builtin),
        };
        let tools: Arc<dyn ToolProvider> = match &self.external {
            Some(servers) => {
                // The import lists the servers (and starts them when needed):
                // async work under a synchronous interface, on the runtime's
                // worker threads.
                let servers = Arc::clone(servers);
                let external = tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current()
                        .block_on(mimlys_mcp::McpTools::open(task_id, servers))
                });
                Arc::new(ComposedTools { local, external })
            }
            None => local,
        };
        cache.insert(task_id, Arc::clone(&tools));
        Ok(tools)
    }

    fn remaining_bytes(&self, task_id: TaskId) -> Result<Option<u64>, WorkspaceError> {
        let dir = Dir::open_ambient_dir(self.tree_path(task_id), ambient_authority())
            .map_err(|_| WorkspaceError::Missing)?;
        let used = Self::tree_size(&dir)?;
        Ok(Some(self.max_bytes.saturating_sub(used)))
    }

    fn snapshot(
        &self,
        task_id: TaskId,
        snapshot_id: mimlys_core::ids::SnapshotId,
        reason: mimlys_core::snapshot::SnapshotReason,
        action_id: Option<mimlys_core::ids::ActionId>,
        previous: Option<&mimlys_core::snapshot::SnapshotRecord>,
        now: mimlys_core::clock::Instant,
    ) -> Result<mimlys_core::spi::PreparedSnapshot, WorkspaceError> {
        self.snapshot_impl(task_id, snapshot_id, reason, action_id, previous, now)
    }

    fn seal(&self, snapshot_id: mimlys_core::ids::SnapshotId) -> Result<(), WorkspaceError> {
        self.files
            .finish(snapshot_id)
            .map_err(|e| WorkspaceError::Storage {
                detail: e.to_string(),
            })
    }

    fn restore(
        &self,
        task_id: TaskId,
        snapshot: &mimlys_core::snapshot::SnapshotRecord,
    ) -> Result<mimlys_core::snapshot::RestoreReport, WorkspaceError> {
        self.restore_impl(task_id, snapshot)
    }

    fn materialize_from(
        &self,
        task_id: TaskId,
        snapshot_id: mimlys_core::ids::SnapshotId,
        from: &mimlys_core::snapshot::SnapshotRecord,
        scope: &ScopeSpec,
        now: mimlys_core::clock::Instant,
    ) -> Result<(WorkspaceInfo, mimlys_core::spi::PreparedSnapshot), WorkspaceError> {
        self.materialize_from_impl(task_id, snapshot_id, from, scope, now)
    }

    fn promote_backup(
        &self,
        task_id: TaskId,
        request: &mimlys_core::spi::PromotionRequest<'_>,
    ) -> Result<
        (
            mimlys_core::spi::PreparedSnapshot,
            mimlys_core::snapshot::Promotion,
        ),
        WorkspaceError,
    > {
        self.promote_backup_impl(task_id, request)
    }

    fn promote_apply(
        &self,
        task_id: TaskId,
        promotion: &mimlys_core::snapshot::Promotion,
        base: &mimlys_core::snapshot::SnapshotRecord,
    ) -> Result<(), WorkspaceError> {
        self.promote_apply_impl(task_id, promotion, base)
    }

    fn promotion_revert_plan(
        &self,
        promotion: &mimlys_core::snapshot::Promotion,
        backup: &mimlys_core::snapshot::SnapshotRecord,
    ) -> Result<mimlys_core::snapshot::RevertPlan, WorkspaceError> {
        self.promotion_revert_plan_impl(promotion, backup)
    }

    fn revert_promotion(
        &self,
        promotion: &mimlys_core::snapshot::Promotion,
        backup: &mimlys_core::snapshot::SnapshotRecord,
        edited: mimlys_core::spi::EditedPaths,
    ) -> Result<mimlys_core::snapshot::RestoreReport, WorkspaceError> {
        self.revert_promotion_impl(promotion, backup, edited)
    }

    fn delete_files(
        &self,
        task_id: TaskId,
        rows: &mimlys_core::spi::DeletedRows,
    ) -> Result<(), WorkspaceError> {
        self.delete_files_impl(task_id, rows)
    }
}

#[cfg(test)]
mod tests;
