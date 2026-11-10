//! The process of a command that only reads (REQ-PERS-013, external design
//! §16, §19; RFC 013). `ReadApp` opens the store as a reader, without the
//! exclusive lock, and starts nothing that writes: no recovery, no integrity
//! check, no reaping of a previous runtime's worker, no migration, no
//! `runtime.started` event, no directories, no container permissions. It
//! holds the reading views of the runtime (`Reader`) and no runtime: a command
//! built on it cannot call a writing method because the type of its handle
//! has none.

use super::{AppError, resolve_task_ref};
use crate::config::Loaded;
use crate::toolchain::Container;
use crate::workspace::LocalWorkspaces;
use mimlys_core::clock::SystemClock;
use mimlys_core::ids::TaskId;
use mimlys_core::runtime::Reader;
use mimlys_core::spi::{PersistenceRead, WorkspaceRead};
use mimlys_persist::{LockState, ReadStore};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// What a command that only reads needs.
pub struct ReadApp {
    /// The configuration and where it came from.
    pub loaded: Loaded,
    /// The data directory.
    pub data_dir: PathBuf,
    /// The store, read-only.
    pub store: Arc<ReadStore>,
    /// The reading views of the control API.
    pub reader: Reader,
    /// The interpreters in force.
    pub interpreters: Vec<mimlys_exec::interpreters::Configured>,
    /// The declared host tools (RFC 011 §6).
    pub host_tools: Vec<mimlys_core::host_tool::HostToolDeclaration>,
    /// The AppContainer and its permission record, read as they are (Windows).
    pub container: Option<Container>,
    /// Declared host tools this machine cannot run yet, with the reason.
    pub host_tools_unavailable: std::collections::BTreeMap<String, String>,
}

impl ReadApp {
    /// Opens the store for reading and loads what the configuration names.
    /// Creates nothing and waits for nothing: a store in use by another
    /// runtime is read all the same.
    pub fn open(loaded: Loaded) -> Result<Self, AppError> {
        let config = loaded.config.clone();
        let data_dir = config.data_dir()?;
        let policy = config.resolve_policy()?;
        let store = Arc::new(mimlys_persist::open_read_only(&data_dir)?);
        let patterns: Vec<String> = policy
            .exclusions
            .materialize
            .iter()
            .chain(policy.exclusions.secrets.iter())
            .cloned()
            .collect();
        let workspaces = Arc::new(LocalWorkspaces::for_reading(data_dir.clone(), patterns));
        let persistence: Arc<dyn PersistenceRead> = store.clone();
        let workspaces: Arc<dyn WorkspaceRead> = workspaces;
        let reader = Reader::new(persistence, workspaces, Arc::new(SystemClock));
        let interpreters = config.interpreters(&data_dir)?;
        let host_tools = config.host_tools();
        let container = crate::toolchain::inspect(&data_dir);
        let host_tools_unavailable = host_tools
            .iter()
            .filter_map(|d| {
                crate::toolchain::unavailable_reason(container.as_ref(), d)
                    .map(|reason| (d.name.clone(), reason))
            })
            .collect();
        Ok(Self {
            loaded,
            data_dir,
            store,
            reader,
            interpreters,
            host_tools,
            container,
            host_tools_unavailable,
        })
    }

    /// Resolves a task id or a unique prefix of one.
    pub fn resolve_task(&self, text: &str) -> Result<TaskId, AppError> {
        resolve_task_ref(text, || {
            Ok(self
                .reader
                .list_tasks()?
                .into_iter()
                .map(|t| t.task_id)
                .collect())
        })
    }

    /// The read set the worker runs under (REQ-CONF-008), for `toolchain list` and `status`.
    pub fn read_set(&self) -> Vec<String> {
        super::confinement_spec(&self.data_dir, &self.interpreters, &self.host_tools, true)
            .read_paths
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect()
    }

    /// Whether a path of the read set carries the container's entry on
    /// Windows (`Some`), or needs none (`None`).
    pub fn path_applied(&self, path: &str) -> Option<bool> {
        self.container
            .as_ref()
            .and_then(|c| crate::toolchain::applied(c, Path::new(path)))
    }

    /// The store as `status` tells it (REQ-PERS-013): whether another runtime
    /// holds it, whether it needs recovery, whether it exists.
    pub fn store_view(&self) -> Result<StoreView, AppError> {
        let state = if !self.store.exists() {
            StoreState::Absent
        } else {
            match self.store.lock_state() {
                LockState::Held(_) => StoreState::InUse,
                LockState::Unknown => StoreState::LockUntested,
                LockState::Free => {
                    if self.store.recovery_pending()? {
                        StoreState::RecoveryPending
                    } else {
                        StoreState::Free
                    }
                }
            }
        };
        let holder = match self.store.lock_state() {
            LockState::Held(Some(h)) => Some(HolderView {
                pid: h.pid,
                since: h.since.clone(),
            }),
            _ => None,
        };
        Ok(StoreView { state, holder })
    }
}

/// The state of the store as a reader finds it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StoreState {
    /// No database yet: the first command that writes creates it.
    Absent,
    /// Nobody holds it and nothing needs recovery.
    Free,
    /// Another runtime holds the lock; what it committed is read.
    InUse,
    /// Nobody holds the lock and a task is still `running`: the next command
    /// that writes recovers it; what is committed is read as it stands.
    RecoveryPending,
    /// The lock could not be tested here; nothing is claimed.
    LockUntested,
}

/// The runtime that holds the lock, as it recorded itself.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct HolderView {
    /// Its process id.
    pub pid: u32,
    /// Since when (RFC 3339, UTC).
    pub since: String,
}

/// `status`'s store section.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct StoreView {
    /// The state.
    pub state: StoreState,
    /// Who holds the lock, when someone does and recorded itself.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub holder: Option<HolderView>,
}

/// The worker's confinement as the probe finds it, for `status`: the same
/// specification a runtime starts its worker with, a worker that records
/// nothing (REQ-CONF-009's record belongs to the runtime that holds the lock).
pub fn probe_confinement(
    config: &crate::config::Config,
    data_dir: &Path,
    interpreters: &[mimlys_exec::interpreters::Configured],
    host_tools: &[mimlys_core::host_tool::HostToolDeclaration],
) -> mimlys_core::confine::Probe {
    let limits = mimlys_exec::sandbox::Limits {
        fuel: config.sandbox.fuel,
        deadline_ms: config.sandbox.deadline_ms.max(1) as u64,
        memory_bytes: config.sandbox.memory_mib as usize * 1024 * 1024,
        output_bytes: config.sandbox.output_bytes as usize,
    };
    let worker = mimlys_exec::worker::Worker::new(mimlys_exec::worker::WorkerConfig {
        program: super::worker_binary(),
        interpreters: interpreters.to_vec(),
        confinement: mimlys_exec::confine::platform(),
        limits,
        spec: Some(super::confinement_spec(
            data_dir,
            interpreters,
            host_tools,
            true,
        )),
        host_tools: host_tools.to_vec(),
        record: None,
    });
    let probe = worker.probe_confined();
    worker.shutdown();
    probe
}
