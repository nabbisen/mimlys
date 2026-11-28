//! The process of a command that goes through a daemon (RFC 014, Amendment 1; external design
//! §19.5; REQ-ARCH-009, REQ-PERS-013, REQ-SCHED-008).
//!
//! The client owns no store: the daemon holds its lock, runs its tasks and takes its slots. This
//! process has what a command reads directly (the store read-only, the configuration, the
//! backends it may probe) and a client whose transport is the daemon's socket. It holds no runtime
//! and no worker, and starts nothing that writes: the commands that need those are not served
//! ([`App::runtime`]).

use super::{App, AppError, os_user};
use crate::backends::{Backend, ConfiguredBackends};
use crate::client::{Client, Transport};
use crate::config::Loaded;
use crate::workspace::LocalWorkspaces;
use mimlys_core::clock::SystemClock;
use mimlys_core::runtime::{Reader, Session};
use mimlys_core::spi::{PersistenceRead, WorkspaceRead};
use std::sync::Arc;

impl App {
    /// The app of a command whose requests go to the daemon through `transport`.
    pub fn open_served(
        loaded: Loaded,
        transport: Arc<dyn Transport>,
        registry: Option<std::path::PathBuf>,
    ) -> Result<Self, AppError> {
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
        let reader = Reader::new(
            store as Arc<dyn PersistenceRead>,
            workspaces as Arc<dyn WorkspaceRead>,
            Arc::new(SystemClock),
        );
        let interpreters = config.interpreters(&data_dir)?;
        let host_tools = config.host_tools();
        let container = crate::toolchain::inspect(&data_dir)?;
        let host_tools_unavailable = host_tools
            .iter()
            .filter_map(|d| {
                crate::toolchain::unavailable_reason(container.as_ref(), d)
                    .map(|reason| (d.name.clone(), reason))
            })
            .collect();
        let backend = Backend::make_default(&loaded, None)?;
        let registry_dir = registry
            .map(Ok)
            .unwrap_or_else(crate::registry::default_dir);
        let owner = crate::registry::Owner {
            pid: std::process::id(),
            data_dir: data_dir.to_string_lossy().into_owned(),
        };
        // The backends a command probes, which the runtime of the daemon will run a task on: the
        // probe asks the engine; it takes no slot and holds no gate.
        let backends = Arc::new(ConfiguredBackends::new(
            loaded.clone(),
            registry_dir.clone(),
            owner.clone(),
            config
                .backends
                .contains_key(crate::backends::DEFAULT)
                .then(|| Arc::new(backend.clone())),
        ));
        let session = Session {
            id: format!("pid:{}", std::process::id()),
            os_user: os_user(),
        };
        let worker = mimlys_exec::worker::Worker::new(mimlys_exec::worker::WorkerConfig {
            program: super::worker_binary(),
            interpreters: interpreters.clone(),
            confinement: mimlys_exec::confine::platform(),
            limits: mimlys_exec::sandbox::Limits {
                fuel: config.sandbox.fuel,
                deadline_ms: config.sandbox.deadline_ms.max(1) as u64,
                memory_bytes: config.sandbox.memory_mib as usize * 1024 * 1024,
                output_bytes: config.sandbox.output_bytes as usize,
            },
            spec: None,
            host_tools: host_tools.clone(),
            record: None,
        });
        Ok(Self {
            client: Client::over(transport, session.clone()),
            loaded,
            data_dir,
            runtime: None,
            reader,
            detach: false,
            policy,
            session,
            backend,
            backends,
            backend_probe: std::sync::Mutex::new(std::collections::BTreeMap::new()),
            store: None,
            confinement: mimlys_core::confine::Probe::unavailable(
                "the daemon runs the worker; this process starts none",
            ),
            registry_dir,
            owner,
            recovered: Vec::new(),
            left_as_is: Vec::new(),
            recovered_promotions: Vec::new(),
            worker,
            interpreters,
            servers: Arc::new(mimlys_mcp::tools::Servers::new(Vec::new())),
            host_tools,
            fetch_worker: None,
            container: std::sync::Mutex::new(container),
            host_tools_unavailable,
            xcrun_cache_missing: None,
        })
    }
}
