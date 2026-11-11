//! The process: configuration, data directory, store, recovery, runtime and
//! session (external design §19; RFC 010 deployment v1). One `App` per
//! command invocation; the store's lock keeps a second process out.

use crate::config::{BackendKind, Config, ConfigError, Loaded};
use crate::workspace::LocalWorkspaces;
use mimlys_core::clock::SystemClock;
use mimlys_core::ids::{IdSource, SystemIds, TaskId};
use mimlys_core::path::PlatformRules;
use mimlys_core::runtime::{Runtime, RuntimeDeps, RuntimeError, Session};
use mimlys_core::spi::{ModelBackend, Persistence, PersistenceError};
use mimlys_model::fake::FakeBackend;
use mimlys_model::openai::{Mode, OpenAiBackend, OpenAiConfig, Probe, Resolved, TransportMode};
use mimlys_persist::recovery::{Recovered, RecoveryContext, recover_all};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Anything a command can fail with (external design §18). Exit code 1.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum AppError {
    /// The configuration is invalid.
    #[error("{0}")]
    Config(#[from] ConfigError),
    /// The control API refused.
    #[error("{0}")]
    Runtime(#[from] RuntimeError),
    /// The store failed.
    #[error("{0}")]
    Persistence(#[from] PersistenceError),
    /// The workspace refused or failed.
    #[error("{0}")]
    Workspace(#[from] mimlys_core::spi::WorkspaceError),
    /// A local failure outside the runtime.
    #[error("{detail}")]
    Io {
        /// What failed.
        detail: String,
    },
    /// The permissions of a directory could not be written because their
    /// lock was held by another runtime past LIM-O7 or could not be taken
    /// (Windows; REQ-CONF-008, external design §13.5). Expected, and
    /// retrying is the remedy: the kind is `unavailable`, not `internal`.
    #[error("{0}")]
    Permissions(#[from] mimlys_exec::confine::dacl_wait::Refusal),
    /// An interpreter download that failed after its retries (REQ-SBX-001,
    /// external design §12 and §18): expected on a flaky network, and the
    /// remedy is in the message (`--from FILE`), so the kind is `unavailable`,
    /// not `internal`.
    #[error("{0}")]
    Download(#[from] crate::download::DownloadError),
    /// A command finished only in part: the message names what did not
    /// happen and the report (the command's own JSON shape) says what did.
    #[error("{message}")]
    Partial {
        /// What did not happen.
        message: String,
        /// The command's report, printed as the JSON value.
        report: serde_json::Value,
    },
    /// A task reference matched nothing or more than one task.
    #[error("{detail}")]
    TaskRef {
        /// What was wrong.
        detail: String,
        /// Whether nothing matched (else: ambiguous).
        not_found: bool,
    },
}

impl AppError {
    /// The error kind of external design §18.
    pub fn kind(&self) -> &'static str {
        match self {
            AppError::Config(_) => "invalid_request",
            AppError::Runtime(e) => e.kind(),
            AppError::Permissions(_) | AppError::Download(_) => "unavailable",
            AppError::Persistence(_) | AppError::Io { .. } | AppError::Partial { .. } => "internal",
            AppError::Workspace(mimlys_core::spi::WorkspaceError::Conflict { .. }) => "conflict",
            AppError::Workspace(mimlys_core::spi::WorkspaceError::NotFound { .. }) => "not_found",
            AppError::Workspace(mimlys_core::spi::WorkspaceError::Refused { .. }) => "state",
            AppError::Workspace(_) => "internal",
            AppError::TaskRef {
                not_found: true, ..
            } => "not_found",
            AppError::TaskRef { .. } => "invalid_request",
        }
    }

    /// The JSON error object.
    pub fn to_json(&self) -> serde_json::Value {
        if let AppError::Partial { report, .. } = self {
            return report.clone();
        }
        let detail = match self {
            AppError::Config(e) => serde_json::json!({ "parameter": e.key }),
            AppError::Permissions(_) => serde_json::json!({ "component": "permissions" }),
            AppError::Download(e) => serde_json::json!({
                "component": "download",
                "url": e.url,
                "last_status": e.last_status,
            }),
            AppError::Workspace(mimlys_core::spi::WorkspaceError::Conflict { paths })
            | AppError::Runtime(RuntimeError::Workspace(
                mimlys_core::spi::WorkspaceError::Conflict { paths },
            )) => {
                serde_json::json!({ "paths": paths })
            }
            _ => serde_json::json!({}),
        };
        serde_json::json!({ "error": { "kind": self.kind(), "message": self.to_string(), "detail": detail } })
    }
}

/// The backend's probe result as `status` shows it (RFC 008 §4).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct BackendProbe {
    /// The server answered the model listing.
    pub reachable: bool,
    /// The configured model is listed, when the listing was readable.
    pub model_listed: Option<bool>,
    /// The structured-output mode in force; none under the `tool_calls` transport, which does not use it.
    pub structured_output: Option<Resolved>,
    /// `auto` ended below `json_schema`.
    pub fell_back: bool,
    /// A fixed-vocabulary reason when something was not as configured.
    pub detail: Option<String>,
    /// The decision transport in force (REQ-MDL-009), once settled.
    pub decision_transport: Option<mimlys_core::native::DecisionTransport>,
    /// Why `auto` chose `envelope`, or why a configured `tool_calls` is refused: the engine's reason.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transport_reason: Option<String>,
    /// Whether the engine accepts `parallel_tool_calls: false`; under `tool_calls` only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parallel_tool_calls: Option<bool>,
}

/// The running process.
pub struct App {
    /// The configuration and where it came from.
    pub loaded: Loaded,
    /// The data directory.
    pub data_dir: PathBuf,
    /// The runtime.
    pub runtime: Arc<Runtime>,
    /// This process as a session.
    pub session: Session,
    /// Which backend is configured.
    pub backend_kind: BackendKind,
    /// The adapter, when the backend is `openai_compatible`.
    openai: Option<Arc<OpenAiBackend>>,
    /// What the backend probe found, once it ran (LIM-M4).
    backend_probe: std::sync::Mutex<Option<BackendProbe>>,
    /// What recovery did at startup.
    pub recovered: Vec<Recovered>,
    /// The worker handle (started at the first `run_*` execution).
    pub worker: Arc<mimlys_exec::worker::Worker>,
    /// The interpreters in force.
    pub interpreters: Vec<mimlys_exec::interpreters::Configured>,
    /// The configured MCP servers (RFC 009).
    pub servers: Arc<mimlys_mcp::tools::Servers>,
    /// The declared host tools (RFC 011 §6).
    pub host_tools: Vec<mimlys_core::host_tool::HostToolDeclaration>,
    /// The dependency step's worker, when a tool declares `network` (RFC 011 §8).
    pub fetch_worker: Option<Arc<mimlys_exec::worker::Worker>>,
    /// The AppContainer and its permission record (Windows; RFC 011 §3, §7).
    pub container: std::sync::Mutex<Option<crate::toolchain::Container>>,
    /// Declared host tools this machine cannot run yet, with the reason
    /// (`acl_not_applied: <path>` until `toolchain grant` is run).
    pub host_tools_unavailable: std::collections::BTreeMap<String, String>,
    /// macOS, an Xcode selected: `xcrun`'s cache file when the runtime could
    /// not make it before starting the worker (external design §13.3); `status`
    /// says so with the remedy. `None` everywhere else and when it is there.
    pub xcrun_cache_missing: Option<PathBuf>,
}

/// Makes `xcrun`'s cache (macOS, an Xcode selected, the cache absent) by
/// running `xcrun --find <tool>` for each tool of the warming's list here,
/// outside any sandbox, waiting at most LIM-O8 for each (RFC 011 Amendment 2,
/// implementation decisions, point 8). Returns
/// the cache file if it is still missing afterwards; the run goes on all the
/// same (the tool's own error then names the cause). Nothing elsewhere.
pub fn warm_xcrun_cache() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        match mimlys_exec::confine::macos::ensure_xcrun_cache() {
            mimlys_exec::confine::macos::xcrun::CacheState::Missing { file } => Some(file),
            _ => None,
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

/// The read and write sets of the worker (RFC 011 §1, §6): the workspaces
/// and the temporary directory writable; the interpreters, the worker's own
/// directory, the declared toolchain paths and the system's read-only.
pub fn confinement_spec(
    data_dir: &Path,
    interpreters: &[mimlys_exec::interpreters::Configured],
    host_tools: &[mimlys_core::host_tool::HostToolDeclaration],
    deny_network: bool,
) -> mimlys_exec::confine::ConfinementSpec {
    let mut read_paths: Vec<PathBuf> = vec![data_dir.join("interpreters"), data_dir.join("tmp")];
    for i in interpreters {
        if let Some(dir) = i.module.parent() {
            read_paths.push(dir.to_path_buf());
        }
    }
    if let Some(dir) = worker_binary().parent() {
        read_paths.push(dir.to_path_buf());
    }
    read_paths.extend(system_read_paths());
    if !deny_network {
        read_paths.extend(network_read_paths());
    }
    for d in host_tools {
        read_paths.extend(d.read_paths.iter().map(PathBuf::from));
    }
    read_paths.sort();
    read_paths.dedup();
    let mut write_paths = vec![data_dir.join("workspaces"), data_dir.join("tmp")];
    if cfg!(unix) {
        write_paths.push(PathBuf::from("/dev/null"));
    }
    mimlys_exec::confine::ConfinementSpec {
        read_paths,
        write_paths,
        deny_network,
        memory_bytes: mimlys_core::limits::LIM_H2_HOST_MEMORY_BYTES,
        max_processes: mimlys_core::limits::LIM_H3_HOST_PROCESSES,
        data_dir: data_dir.to_path_buf(),
    }
}

/// The system's directories the worker and its tools read (RFC 011 §1,
/// external design §13.3). On Windows they are readable by every application
/// package already, so the set names only what mimlys adds (none). macOS has
/// a list of its own, the measured minimum of RFC 011 Amendment 2 (settled),
/// point 8; not `/opt`, which is Homebrew's. Linux has none of `/etc` or `/opt`
/// either (handoff 011j): what a tool needs of `/etc` was measured, and the
/// only entries it asked for are the dependency step's ([`network_read_paths`]);
/// a toolchain under `/opt` is declared in its tool's `read_paths`.
fn system_read_paths() -> Vec<PathBuf> {
    if cfg!(target_os = "macos") {
        mimlys_exec::confine::macos::system_read_paths()
    } else if cfg!(unix) {
        [
            "/usr",
            "/lib",
            "/lib64",
            "/bin",
            "/sbin",
            "/dev/urandom",
            "/dev/random",
            "/dev/zero",
        ]
        .iter()
        .map(PathBuf::from)
        .collect()
    } else {
        Vec::new()
    }
}

/// What the dependency step's worker (the one with the network) reads of `/etc`
/// on Linux, and no other worker: the resolver's configuration and the
/// system's certificate bundle. Each entry is one the step failed without
/// (handoff 011j, measured with `cargo fetch`, `curl` and `getent` on Ubuntu
/// x86_64 and aarch64 and on Arch): without `resolv.conf` a name does not
/// resolve; without the bundle the TLS handshake has no trust anchors.
/// `nsswitch.conf`, `hosts`, `passwd` and `group` were tried and no failure
/// asked for them. A link is followed, so a bundle that is a link into
/// `/etc/ca-certificates` is admitted as the file it points to.
pub fn network_read_paths() -> Vec<PathBuf> {
    if cfg!(target_os = "linux") {
        ["/etc/resolv.conf", "/etc/ssl/certs/ca-certificates.crt"]
            .iter()
            .map(PathBuf::from)
            .collect()
    } else {
        Vec::new()
    }
}

/// The read paths that hold whatever the declarations say: the data
/// directory's `interpreters` and `tmp`, the worker's own directory and the
/// system's. A `command` must lie under these or under its tool's declared
/// `read_paths` (external design §13.5). On Windows the system's directories
/// are named here too, as the container reads them already.
pub fn fixed_read_paths(data_dir: &Path) -> Vec<PathBuf> {
    let mut paths = vec![data_dir.join("interpreters"), data_dir.join("tmp")];
    if let Some(dir) = worker_binary().parent() {
        paths.push(dir.to_path_buf());
    }
    paths.extend(system_read_paths());
    if cfg!(windows) {
        for var in ["SystemRoot", "ProgramFiles", "ProgramFiles(x86)"] {
            if let Some(dir) = std::env::var_os(var).filter(|v| !v.is_empty()) {
                paths.push(PathBuf::from(dir));
            }
        }
    }
    paths
}

/// The worker binary: `MIMLYS_WORKER_BIN`, else `mimlys-worker` beside this executable.
pub fn worker_binary() -> PathBuf {
    if let Some(p) = std::env::var_os("MIMLYS_WORKER_BIN").filter(|v| !v.is_empty()) {
        return PathBuf::from(p);
    }
    let name = if cfg!(windows) {
        "mimlys-worker.exe"
    } else {
        "mimlys-worker"
    };
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join(name)))
        .unwrap_or_else(|| PathBuf::from(name))
}

/// The error of a failed `create_private_dir`. A refused permission lock
/// (another runtime is writing the directory's permissions past LIM-O7, or
/// the lock cannot be taken) is recognized by its type, never by its text:
/// the start fails `unavailable` and the next applies them (external design
/// §13.5, §18). Any other failure carries its text.
fn data_dir_error(data_dir: &Path, e: std::io::Error) -> AppError {
    match e
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<mimlys_exec::confine::dacl_wait::Refusal>())
    {
        Some(refusal) => AppError::Permissions(refusal.clone()),
        None => AppError::Io {
            detail: format!(
                "cannot create the data directory {}: {e}",
                data_dir.display()
            ),
        },
    }
}

impl App {
    /// Opens the data directory, recovers, checks consistency and builds
    /// the runtime (REQ-PERS-006, REQ-PERS-007).
    pub fn open(loaded: Loaded) -> Result<Self, AppError> {
        Self::open_with(loaded, None)
    }

    /// As [`App::open`], with a backend supplied by the caller (the test harness).
    pub fn open_with(
        loaded: Loaded,
        backend: Option<Arc<dyn ModelBackend>>,
    ) -> Result<Self, AppError> {
        let confinement = mimlys_exec::confine::platform();
        let config = loaded.config.clone();
        let data_dir = config.data_dir()?;
        // REQ-WS-001: owner-only from the start (Unix mode 0700; Windows a
        // protected DACL for the current user), before anything is placed
        // in it and before the container's entries go on the subdirectories.
        mimlys_exec::confine::create_private_dir(&data_dir)
            .map_err(|e| data_dir_error(&data_dir, e))?;
        let policy = config.resolve_policy()?;
        let store = mimlys_persist::sqlite::open(&data_dir)?;
        let ids = SystemIds;
        let now = mimlys_core::clock::Clock::now(&SystemClock);

        let patterns: Vec<String> = policy
            .exclusions
            .materialize
            .iter()
            .chain(policy.exclusions.secrets.iter())
            .cloned()
            .collect();
        let interpreters = config.interpreters(&data_dir)?;
        let languages: Vec<String> = interpreters.iter().map(|i| i.language.clone()).collect();
        let host_tools = config.host_tools();
        // The write set is the workspaces and `<data>/tmp`, on every platform.
        for dir in ["workspaces", "tmp", "interpreters"] {
            mimlys_exec::confine::create_private_subdir(&data_dir.join(dir)).map_err(|e| {
                AppError::Io {
                    detail: format!(
                        "cannot create {}: {}",
                        data_dir.join(dir).display(),
                        e.kind()
                    ),
                }
            })?;
        }
        // Windows (RFC 011 §3, §7): the AppContainer exists and mimlys's own
        // directories carry its entries; a declared path without its entry
        // leaves that tool unavailable until `toolchain grant`.
        let mut runtime_read: Vec<PathBuf> = vec![data_dir.join("interpreters")];
        runtime_read.extend(
            interpreters
                .iter()
                .filter_map(|i| i.module.parent().map(Path::to_path_buf)),
        );
        if let Some(dir) = worker_binary().parent() {
            runtime_read.push(dir.to_path_buf());
        }
        let container = crate::toolchain::prepare(
            &data_dir,
            &crate::toolchain::RuntimePaths {
                workspaces: &data_dir.join("workspaces"),
                tmp: &data_dir.join("tmp"),
                read: runtime_read,
            },
        )?;
        let host_tools_unavailable: std::collections::BTreeMap<String, String> = host_tools
            .iter()
            .filter_map(|d| {
                crate::toolchain::unavailable_reason(container.as_ref(), d)
                    .map(|reason| (d.name.clone(), reason))
            })
            .collect();
        let usable_host_tools: Vec<mimlys_core::host_tool::HostToolDeclaration> = host_tools
            .iter()
            .filter(|d| !host_tools_unavailable.contains_key(&d.name))
            .cloned()
            .collect();
        // REQ-CONF-009: what a previous runtime left behind dies first.
        let record = data_dir.join("worker.json");
        let orphans = mimlys_exec::worker::reap_orphans(&record);
        let spec = confinement_spec(&data_dir, &interpreters, &host_tools, true);
        let limits = mimlys_exec::sandbox::Limits {
            fuel: config.sandbox.fuel,
            deadline_ms: config.sandbox.deadline_ms.max(1) as u64,
            memory_bytes: config.sandbox.memory_mib as usize * 1024 * 1024,
            output_bytes: config.sandbox.output_bytes as usize,
        };
        let worker = mimlys_exec::worker::Worker::new(mimlys_exec::worker::WorkerConfig {
            program: worker_binary(),
            interpreters: interpreters.clone(),
            confinement,
            limits,
            spec: Some(spec),
            host_tools: host_tools.clone(),
            record: Some(record),
        });
        // RFC 011 §8: the dependency step's worker keeps every clause but N.
        let fetch_worker = host_tools.iter().any(|d| d.network).then(|| {
            mimlys_exec::worker::Worker::new(mimlys_exec::worker::WorkerConfig {
                program: worker_binary(),
                interpreters: vec![],
                confinement: mimlys_exec::confine::platform(),
                limits,
                spec: Some(confinement_spec(
                    &data_dir,
                    &interpreters,
                    &host_tools,
                    false,
                )),
                host_tools: host_tools.clone(),
                record: None,
            })
        });
        // Before the first worker starts (the probe is one; the dependency
        // step's starts later): `xcrun`'s cache, which a sandboxed tool cannot make.
        let xcrun_cache_missing = warm_xcrun_cache();
        // RFC 011 §1: the probe is a real worker's report of its own confinement.
        let probe = worker.probe_confined();
        let tier = probe.tier_with(&policy.accept_best_effort);
        let host_tools_available = tier.tier == mimlys_core::event::Tier::Full;
        let servers = Arc::new(mimlys_mcp::tools::Servers::new(
            config
                .servers(&data_dir, &loaded.secrets)
                .into_iter()
                .map(mimlys_mcp::server::Server::new)
                .collect(),
        ));
        let workspaces = Arc::new(
            LocalWorkspaces::new(
                data_dir.clone(),
                patterns,
                config.workspace.max_bytes,
                config.workspace.max_files,
            )?
            .with_sandbox(crate::workspace::SandboxTools {
                worker: Arc::clone(&worker),
                languages,
                deadline_ms: config.sandbox.deadline_ms,
                host_tools: usable_host_tools,
                host_tools_available,
                fetch_worker: fetch_worker.clone(),
            })
            .with_external(Arc::clone(&servers)),
        );
        let recovered = recover_all(
            &store,
            &RecoveryContext {
                ids: &ids,
                now,
                policy_hash: policy.hash(),
                recovery: policy.recovery,
            },
            &*workspaces,
        )?;
        mimlys_persist::integrity::check(&store, now)?;
        let mut openai: Option<Arc<OpenAiBackend>> = None;
        let backend: Arc<dyn ModelBackend> = match backend {
            Some(b) => b,
            None => match config.backend.kind {
                BackendKind::Fake => match &config.backend.script {
                    Some(path) => {
                        Arc::new(FakeBackend::from_script(path).map_err(|e| AppError::Io {
                            detail: format!(
                                "cannot read the backend script {}: {}",
                                path.display(),
                                e.kind()
                            ),
                        })?)
                    }
                    None => Arc::new(FakeBackend::with_decisions(vec![])),
                },
                BackendKind::OpenaiCompatible => {
                    let adapter = Arc::new(OpenAiBackend::new(OpenAiConfig {
                        url: config.backend.url.clone().unwrap_or_default(),
                        model: config.backend.model.clone().unwrap_or_default(),
                        api_key: loaded
                            .secrets
                            .iter()
                            .find(|(name, _)| name == "backend.api_key")
                            .map(|(_, value)| value.clone()),
                        timeout_ms: config.backend.timeout_ms,
                        max_tokens: config.backend.max_tokens,
                        temperature: config.backend.temperature,
                        decision_transport: match config.backend.decision_transport {
                            crate::config::DecisionTransport::Auto => TransportMode::Auto,
                            crate::config::DecisionTransport::ToolCalls => TransportMode::ToolCalls,
                            crate::config::DecisionTransport::Envelope => TransportMode::Envelope,
                        },
                        structured_output: match config.backend.structured_output {
                            crate::config::StructuredOutput::Auto => Mode::Auto,
                            crate::config::StructuredOutput::Schema => Mode::Schema,
                            crate::config::StructuredOutput::None => Mode::None,
                        },
                        locality: config.backend.locality,
                    }));
                    openai = Some(Arc::clone(&adapter));
                    adapter
                }
            },
        };
        let store: Arc<dyn Persistence> = Arc::new(store);
        let runtime = Runtime::new(RuntimeDeps {
            persistence: store,
            backend,
            workspaces,
            policy,
            ids: Arc::new(ids),
            clock: Arc::new(SystemClock),
            runtime_version: env!("CARGO_PKG_VERSION").to_owned(),
            rules: PlatformRules::current(),
            probe: probe.clone(),
            secrets: loaded.secrets.clone(),
            context_chars: config.backend.context_chars,
        })?;
        runtime.record_started()?;
        if orphans > 0 {
            runtime.record_server_events(&[mimlys_core::event::Event::RuntimeOrphansKilled {
                count: orphans,
            }])?;
        }
        Ok(Self {
            loaded,
            data_dir,
            runtime: Arc::new(runtime),
            session: Session {
                id: format!("pid:{}", std::process::id()),
                os_user: os_user(),
            },
            backend_kind: config.backend.kind,
            openai,
            backend_probe: std::sync::Mutex::new(None),
            recovered,
            worker,
            interpreters,
            servers,
            host_tools,
            fetch_worker,
            container: std::sync::Mutex::new(container),
            host_tools_unavailable,
            xcrun_cache_missing,
        })
    }

    /// A `host_tool` grant for a tool this machine cannot run yet is refused
    /// with the reason, rather than issued to change nothing (RFC 011 §7).
    pub fn check_host_tool_grant(
        &self,
        kind: &mimlys_core::grant::GrantKind,
    ) -> Result<(), AppError> {
        if let mimlys_core::grant::GrantKind::HostTool { name } = kind
            && let Some(reason) = self.host_tools_unavailable.get(name)
        {
            return Err(AppError::Io {
                detail: crate::messages::host_tool_unavailable_grant(name, reason),
            });
        }
        Ok(())
    }

    /// Whether a path of the read set carries the container's entry on
    /// Windows (`Some`), or needs none (`None`).
    pub fn path_applied(&self, path: &str) -> Option<bool> {
        let guard = self.container.lock().expect("container");
        guard
            .as_ref()
            .and_then(|c| crate::toolchain::applied(c, Path::new(path)))
    }

    /// The read set the worker runs under (REQ-CONF-008), for `toolchain list` and `status`.
    pub fn read_set(&self) -> Vec<String> {
        confinement_spec(&self.data_dir, &self.interpreters, &self.host_tools, true)
            .read_paths
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect()
    }

    /// Probes the backend once (LIM-M4; RFC 008 §4): the model listing and
    /// the structured-output mode. A `schema` backend whose server refuses
    /// the schema is an error naming the fix. Returns the notice to print
    /// when `auto` fell back to plain text.
    pub async fn probe_backend(&self) -> Result<Option<String>, AppError> {
        if self.backend_probe.lock().expect("probe lock").is_some() {
            return Ok(None);
        }
        let probe = match &self.openai {
            Some(adapter) => {
                let Probe {
                    reachable,
                    model_listed,
                    structured_output,
                    fell_back,
                    detail,
                    decision_transport,
                    transport_reason,
                    parallel_tool_calls,
                } = adapter.probe().await;
                BackendProbe {
                    reachable,
                    model_listed,
                    structured_output,
                    fell_back,
                    detail,
                    decision_transport,
                    transport_reason,
                    parallel_tool_calls,
                }
            }
            None => {
                let b = &self.loaded.config.backend;
                let no_script = b.kind == crate::config::BackendKind::Fake && b.script.is_none();
                BackendProbe {
                    reachable: !no_script,
                    model_listed: None,
                    structured_output: Some(Resolved::None),
                    fell_back: false,
                    detail: no_script.then(|| "no_script".to_owned()),
                    decision_transport: Some(mimlys_core::native::DecisionTransport::Envelope),
                    transport_reason: None,
                    parallel_tool_calls: None,
                }
            }
        };
        if self.loaded.config.backend.structured_output == crate::config::StructuredOutput::Schema
            && probe.reachable
            && probe.detail.as_deref() == Some("format_refused")
        {
            return Err(AppError::Io {
                detail: "backend unusable: the server refuses json_schema; set backend.structured_output = \"none\" or \"auto\"".into(),
            });
        }
        let notice = if probe.detail.as_deref() == Some("tool_calls_refused") {
            Some(crate::messages::tool_calls_refused(
                probe
                    .transport_reason
                    .as_deref()
                    .unwrap_or("no reason given"),
            ))
        } else {
            (probe.fell_back && probe.structured_output == Some(Resolved::None))
                .then(|| crate::messages::structured_output_none().to_owned())
        };
        *self.backend_probe.lock().expect("probe lock") = Some(probe);
        Ok(notice)
    }

    /// Whether the configured backend can produce decisions at all: the fake
    /// backend without a script cannot, and a command that runs steps says so
    /// before creating or continuing anything (REQ-CFG-002 lets such a
    /// process run `status`, `check` and the task commands that need no decision).
    pub fn backend_ready(&self) -> Result<(), AppError> {
        let b = &self.loaded.config.backend;
        if self.openai.is_none() && b.kind == crate::config::BackendKind::Fake && b.script.is_none()
        {
            return Err(AppError::Io {
                detail: crate::messages::FAKE_BACKEND_NO_SCRIPT.into(),
            });
        }
        Ok(())
    }

    /// The probe result, when the probe ran.
    pub fn backend_probe(&self) -> Option<BackendProbe> {
        self.backend_probe.lock().expect("probe lock").clone()
    }

    /// Ends the worker (REQ-CONF-009) and records `runtime.stopped`.
    pub fn close(&self) -> Result<(), AppError> {
        self.worker.shutdown();
        if !self.servers.is_empty() {
            let servers = Arc::clone(&self.servers);
            tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(servers.shutdown())
            });
            // Their stop events belong to the audit (RFC 009 §1).
            self.runtime
                .record_server_events(&self.servers.drain_events())?;
        }
        self.runtime.record_stopped()?;
        Ok(())
    }

    /// The servers' status without starting any (RFC 009 §8).
    pub fn server_status(&self) -> Vec<mimlys_mcp::ServerStatus> {
        if self.servers.is_empty() {
            return Vec::new();
        }
        let servers = Arc::clone(&self.servers);
        tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(servers.status()))
    }

    /// Resolves a task id or a unique prefix of one.
    pub fn resolve_task(&self, text: &str) -> Result<TaskId, AppError> {
        if let Ok(uuid) = uuid::Uuid::parse_str(text) {
            return Ok(TaskId::from_uuid(uuid));
        }
        let text = text.trim();
        if text.len() < 4 {
            return Err(AppError::TaskRef {
                detail: "give a task id or at least four characters of it".into(),
                not_found: false,
            });
        }
        let matches: Vec<TaskId> = self
            .runtime
            .list_tasks()?
            .into_iter()
            .map(|t| t.task_id)
            .filter(|id| id.to_string().starts_with(text))
            .collect();
        match matches.as_slice() {
            [one] => Ok(*one),
            [] => Err(AppError::TaskRef {
                detail: format!("no task starts with {text}"),
                not_found: true,
            }),
            many => Err(AppError::TaskRef {
                detail: format!(
                    "{} tasks start with {text}; give more characters",
                    many.len()
                ),
                not_found: false,
            }),
        }
    }

    /// The configuration in force.
    pub fn config(&self) -> &Config {
        &self.loaded.config
    }

    /// The id source (for records the commands build).
    pub fn ids(&self) -> &dyn IdSource {
        &SystemIds
    }
}

/// The operating-system user, for audit records (REQ-APR-009).
pub fn os_user() -> String {
    ["USER", "USERNAME", "LOGNAME"]
        .iter()
        .find_map(|k| std::env::var(k).ok().filter(|v| !v.is_empty()))
        .unwrap_or_else(|| "unknown".into())
}

#[cfg(test)]
mod tests;
