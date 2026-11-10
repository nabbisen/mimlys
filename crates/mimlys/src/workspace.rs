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
use mimlys_core::spi::{
    ToolProvider, WorkspaceError, WorkspaceInfo, WorkspaceProvider, WorkspaceRead,
};
use mimlys_core::task::ScopeSpec;
use mimlys_exec::builtin::BuiltinTools;
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};

mod snapshot;

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
}

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
    ) -> Vec<String> {
        self.external.refresh_pins(pinned)
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

    /// Walks the scope, measuring what would be copied. No write happens.
    fn measure(
        &self,
        src: &Dir,
        rel: &str,
        matcher: &ExclusionMatcher,
        out: &mut Measured,
    ) -> Result<(), WorkspaceError> {
        for entry in src
            .read_dir(if rel.is_empty() { "." } else { rel })
            .map_err(Self::storage("walk"))?
        {
            let entry = entry.map_err(Self::storage("walk"))?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let child = join(rel, &name);
            if is_excluded(matcher, &child) {
                continue;
            }
            let ty = entry.file_type().map_err(Self::storage("walk"))?;
            if ty.is_symlink() {
                continue;
            }
            if ty.is_dir() {
                self.measure(src, &child, matcher, out)?;
            } else if ty.is_file() {
                out.files += 1;
                out.bytes += entry.metadata().map_err(Self::storage("walk"))?.len();
                if out.bytes > self.max_bytes || out.files > self.max_files {
                    return Err(WorkspaceError::TooLarge {
                        bytes: out.bytes,
                        files: out.files,
                    });
                }
            }
        }
        Ok(())
    }

    /// Copies the walked entries; symbolic links are recreated only when
    /// their target stays inside the scope, lexically.
    fn copy_tree(
        &self,
        src: &Dir,
        dst: &Dir,
        rel: &str,
        matcher: &ExclusionMatcher,
        omitted: &mut u64,
    ) -> Result<(), WorkspaceError> {
        for entry in src
            .read_dir(if rel.is_empty() { "." } else { rel })
            .map_err(Self::storage("walk"))?
        {
            let entry = entry.map_err(Self::storage("walk"))?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let child = join(rel, &name);
            if is_excluded(matcher, &child) {
                continue;
            }
            let ty = entry.file_type().map_err(Self::storage("walk"))?;
            if ty.is_symlink() {
                let target = src
                    .read_link_contents(&child)
                    .map_err(Self::storage("read_link"))?;
                if target_inside_scope(&child, &target) {
                    recreate_symlink(src, dst, &child, &target)?;
                } else {
                    *omitted += 1;
                }
            } else if ty.is_dir() {
                dst.create_dir(&child)
                    .map_err(Self::storage("create_dir"))?;
                self.copy_tree(src, dst, &child, matcher, omitted)?;
            } else if ty.is_file() {
                src.copy(&child, dst, &child)
                    .map_err(Self::storage("copy"))?;
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

    fn tree_size(dir: &Dir, rel: &str) -> Result<u64, WorkspaceError> {
        let mut total = 0;
        for entry in dir
            .read_dir(if rel.is_empty() { "." } else { rel })
            .map_err(Self::storage("walk"))?
        {
            let entry = entry.map_err(Self::storage("walk"))?;
            let ty = entry.file_type().map_err(Self::storage("walk"))?;
            let child = join(rel, &entry.file_name().to_string_lossy());
            if ty.is_dir() {
                total += Self::tree_size(dir, &child)?;
            } else if ty.is_file() {
                total += entry.metadata().map_err(Self::storage("walk"))?.len();
            }
        }
        Ok(total)
    }
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

/// Whether a link at `link` (relative to the scope root) pointing to
/// `target` stays inside the scope, resolved lexically against the root.
fn target_inside_scope(link: &str, target: &Path) -> bool {
    if target.is_absolute() {
        return false;
    }
    let mut depth: i64 = link.split('/').count() as i64 - 1;
    for component in target.components() {
        match component {
            Component::Normal(_) => depth += 1,
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

/// Recreates a link inside one directory handle (restore and promotion).
#[cfg(not(windows))]
pub(crate) fn recreate_symlink_in(
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
pub(crate) fn recreate_symlink_in(
    dst: &Dir,
    link: &str,
    target: &Path,
) -> Result<(), WorkspaceError> {
    // Without the source, a link is recreated as a file link; a directory target is rare in a project tree.
    dst.symlink_file(target, link)
        .map_err(|_| WorkspaceError::Storage {
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
    let result = if is_dir {
        dst.symlink_dir(target, link)
    } else {
        dst.symlink_file(target, link)
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
        Self::tree_size(&dir, "")
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
        let mut measured = Measured::default();
        self.measure(&src, "", &matcher, &mut measured)?;
        let mut omitted = 0;
        self.copy_tree(&src, &dst, "", &matcher, &mut omitted)?;
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
        let used = Self::tree_size(&dir, "")?;
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

    fn promote(
        &self,
        task_id: TaskId,
        plan: &mimlys_core::snapshot::PromotionPlan,
        project_root: &str,
        backup_id: mimlys_core::ids::SnapshotId,
        promotion_id: mimlys_core::ids::PromotionId,
        now: mimlys_core::clock::Instant,
    ) -> Result<
        (
            mimlys_core::spi::PreparedSnapshot,
            mimlys_core::snapshot::Promotion,
        ),
        WorkspaceError,
    > {
        self.promote_impl(task_id, plan, project_root, backup_id, promotion_id, now)
    }

    fn revert_promotion(
        &self,
        promotion: &mimlys_core::snapshot::Promotion,
        backup: &mimlys_core::snapshot::SnapshotRecord,
    ) -> Result<mimlys_core::snapshot::RestoreReport, WorkspaceError> {
        self.revert_promotion_impl(promotion, backup)
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
