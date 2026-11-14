//! One configured MCP server as a child process (RFC 009 §1; REQ-MCP-001,
//! REQ-PRIV-003): an explicit environment, lazy start within LIM-O6, the
//! tool list with its hashes, restarts counted per task, shutdown by
//! closing stdin and then by killing the process group.

use crate::client::{CallOutcome, Connection};
use mimlys_core::action::EffectKind;
use mimlys_core::event::Event;
use mimlys_core::hash::Hash;
use mimlys_core::ids::TaskId;
use mimlys_core::limits::{
    LIM_O2_CANCEL_MS, LIM_O5_EXTERNAL_DEADLINE_MS, LIM_O6_SERVER_STARTUP_MS,
    LIM_O16_SERVER_SHUTDOWN_MS, LIM_X3_RESTARTS,
};
use mimlys_core::spi::Cancellation;
use serde_json::{Map, Value};
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

/// The administrator's declaration of one external tool (REQ-MCP-004).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Declaration {
    /// `external_read` or `external_write`.
    pub effect: EffectKind,
    /// Whether a repeat has the same result.
    pub idempotent: bool,
    /// Parameters holding destination URLs.
    pub destination_arguments: Vec<String>,
    /// The deadline in milliseconds (LIM-O5 range).
    pub timeout_ms: i64,
}

impl Declaration {
    /// The restrictive defaults of an undeclared tool.
    pub fn undeclared() -> Self {
        Self {
            effect: EffectKind::ExternalWrite,
            idempotent: false,
            destination_arguments: vec![],
            timeout_ms: LIM_O5_EXTERNAL_DEADLINE_MS,
        }
    }
}

/// `[servers.<name>]`, resolved: secrets are values here.
#[derive(Clone, Debug)]
pub struct ServerConfig {
    /// The configured name (the prefix of its tools).
    pub name: String,
    /// The program.
    pub command: PathBuf,
    /// Its arguments.
    pub args: Vec<String>,
    /// Environment variables copied from the runtime's environment.
    pub env_allow: Vec<String>,
    /// Secrets placed in the environment, resolved.
    pub env_secrets: Vec<(String, String)>,
    /// The working directory.
    pub current_dir: Option<PathBuf>,
    /// LIM-O6.
    pub startup_ms: i64,
    /// LIM-O16: the grace after the standard input closes, before the group is killed.
    pub shutdown_ms: i64,
    /// The server's declared maximum call time, below which no deadline may fall.
    pub max_call_ms: Option<i64>,
    /// The declared tools, by tool name (without the server prefix).
    pub tools: BTreeMap<String, Declaration>,
}

impl ServerConfig {
    /// A configuration with the defaults for everything but the command.
    pub fn new(name: &str, command: PathBuf, args: Vec<String>) -> Self {
        Self {
            name: name.to_owned(),
            command,
            args,
            env_allow: vec![],
            env_secrets: vec![],
            current_dir: None,
            startup_ms: LIM_O6_SERVER_STARTUP_MS,
            shutdown_ms: LIM_O16_SERVER_SHUTDOWN_MS,
            max_call_ms: None,
            tools: BTreeMap::new(),
        }
    }

    /// The explicit environment of the child (clause E): nothing inherited.
    pub fn environment(&self) -> Vec<(String, String)> {
        let mut env: Vec<(String, String)> = self
            .env_allow
            .iter()
            .filter_map(|name| std::env::var(name).ok().map(|v| (name.clone(), v)))
            .collect();
        env.extend(self.env_secrets.iter().cloned());
        env
    }
}

/// A listed tool with its hash-relevant parts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Listed {
    /// The tool's name on the server.
    pub name: String,
    /// Its description, when any.
    pub description: Option<String>,
    /// The input schema, verbatim.
    pub input_schema: Value,
    /// The output schema, verbatim, when declared.
    pub output_schema: Option<Value>,
    /// The annotations, verbatim, when any.
    pub annotations: Option<Value>,
}

/// What a server reported about itself.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct ServerInfo {
    /// The negotiated protocol revision.
    pub revision: Option<String>,
    /// The server's name.
    pub name: Option<String>,
    /// The server's version.
    pub version: Option<String>,
    /// The server's instructions, recorded and never placed in the context (RFC 009 §2).
    pub instructions: Option<String>,
}

/// Why a server could not serve.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ServerError {
    /// The process could not be started or did not initialize within LIM-O6.
    #[error("server {server} unavailable: {reason}")]
    Unavailable {
        /// The server.
        server: String,
        /// A fixed-vocabulary reason.
        reason: String,
    },
    /// The process is not running and the task's restart budget (LIM-X3) is
    /// used up: it is not restarted, so no call is made. The reason word the
    /// product gives for it is `restart_limit_reached`.
    #[error("server {server} not restarted: restart_limit_reached for the task")]
    Died {
        /// The server.
        server: String,
    },
}

/// What a listing yields: the server's information, its tools and their hashes.
pub type Listing = (ServerInfo, Vec<Listed>, BTreeMap<String, Hash>);

/// A running server with its list.
struct Live {
    connection: Connection,
    info: ServerInfo,
    tools: Vec<Listed>,
    hashes: BTreeMap<String, Hash>,
}

/// One configured server.
pub struct Server {
    config: ServerConfig,
    live: tokio::sync::Mutex<Option<Live>>,
    /// Restarts per task (LIM-X3): every start of a server that had started
    /// before, made for the task (a listing for no task is not counted).
    restarts: Mutex<HashMap<TaskId, u32>>,
    /// Whether the process was started at least once: a start after that is a
    /// restart, whatever ended the process before (its death, the kill after
    /// LIM-O2).
    started_once: std::sync::atomic::AtomicBool,
    /// Events produced and not yet drained (RFC 009 §1).
    events: Mutex<Vec<Event>>,
    /// Whether the last known list may be stale (`list_changed` or a
    /// restart); shared with the connection's notification handler.
    dirty: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// The last list seen, for a provider that needs it without starting the server.
    last: Mutex<Option<Listing>>,
}

impl Server {
    /// A server that has not started.
    pub fn new(config: ServerConfig) -> Self {
        Self {
            config,
            live: tokio::sync::Mutex::new(None),
            restarts: Mutex::new(HashMap::new()),
            started_once: std::sync::atomic::AtomicBool::new(false),
            events: Mutex::new(Vec::new()),
            dirty: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            last: Mutex::new(None),
        }
    }

    /// The configuration.
    pub fn config(&self) -> &ServerConfig {
        &self.config
    }

    /// The configured name.
    pub fn name(&self) -> &str {
        &self.config.name
    }

    /// Whether the list may be stale.
    pub fn is_dirty(&self) -> bool {
        self.dirty.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Takes the events produced since the last call.
    pub fn drain_events(&self) -> Vec<Event> {
        std::mem::take(&mut *self.events.lock().expect("events lock"))
    }

    /// Whether the process is running (as far as the transport knows).
    pub async fn is_running(&self) -> bool {
        self.live
            .lock()
            .await
            .as_ref()
            .is_some_and(|l| !l.connection.is_closed())
    }

    /// The last list seen, without starting the server.
    pub fn last_list(&self) -> Option<Listing> {
        self.last.lock().expect("last lock").clone()
    }

    fn push_event(&self, event: Event) {
        self.events.lock().expect("events lock").push(event);
    }

    /// Starts the process if needed and returns the current list, for no
    /// task (`status`, the import at startup): a restart it makes is not
    /// counted against any task.
    pub async fn list(&self) -> Result<Listing, ServerError> {
        self.list_counted(None).await
    }

    /// As [`Server::list`], on behalf of `task`: a restart it makes counts
    /// against the task's allowance (LIM-X3), and past the limit the server is
    /// not started.
    pub async fn list_for(&self, task: TaskId) -> Result<Listing, ServerError> {
        self.list_counted(Some(task)).await
    }

    async fn list_counted(&self, task: Option<TaskId>) -> Result<Listing, ServerError> {
        let mut live = self.live.lock().await;
        self.ensure_started(&mut live, task).await?;
        let l = live.as_mut().expect("started");
        if self.dirty.swap(false, std::sync::atomic::Ordering::SeqCst) {
            match l.connection.list_tools().await {
                Ok(tools) => {
                    l.hashes = crate::import::hashes(&self.config.name, &tools);
                    l.tools = tools;
                }
                Err(reason) => {
                    self.dirty.store(true, std::sync::atomic::Ordering::SeqCst);
                    return Err(ServerError::Unavailable {
                        server: self.config.name.clone(),
                        reason,
                    });
                }
            }
        }
        let snapshot = (l.info.clone(), l.tools.clone(), l.hashes.clone());
        *self.last.lock().expect("last lock") = Some(snapshot.clone());
        Ok(snapshot)
    }

    /// Starts the process when it is not running. A start after the first is a
    /// restart: made for `task`, it counts against the task (LIM-X3) and, past
    /// the limit, is refused (`ServerError::Died`) every time it is asked
    /// again, not only once. A start for no task is never counted.
    async fn ensure_started(
        &self,
        live: &mut Option<Live>,
        task: Option<TaskId>,
    ) -> Result<(), ServerError> {
        if let Some(l) = live.as_ref() {
            if !l.connection.is_closed() {
                return Ok(());
            }
            // Died: record it; the restart below is counted.
            self.push_event(Event::ServerStopped {
                server: self.config.name.clone(),
                reason: "died".into(),
            });
            *live = None;
        }
        if let Some(task) = task
            && self.started_once.load(std::sync::atomic::Ordering::SeqCst)
        {
            let mut restarts = self.restarts.lock().expect("restarts lock");
            let count = restarts.entry(task).or_insert(0);
            if *count >= LIM_X3_RESTARTS {
                return Err(ServerError::Died {
                    server: self.config.name.clone(),
                });
            }
            *count += 1;
        }
        let startup = Duration::from_millis(self.config.startup_ms.max(1) as u64);
        let connection = tokio::time::timeout(
            startup,
            Connection::start(&self.config, std::sync::Arc::clone(&self.dirty)),
        )
        .await
        .map_err(|_| ServerError::Unavailable {
            server: self.config.name.clone(),
            reason: "startup_timeout".into(),
        })?
        .map_err(|reason| ServerError::Unavailable {
            server: self.config.name.clone(),
            reason,
        })?;
        let info = connection.info();
        let tools = tokio::time::timeout(startup, connection.list_tools())
            .await
            .map_err(|_| ServerError::Unavailable {
                server: self.config.name.clone(),
                reason: "startup_timeout".into(),
            })?
            .map_err(|reason| ServerError::Unavailable {
                server: self.config.name.clone(),
                reason,
            })?;
        let hashes = crate::import::hashes(&self.config.name, &tools);
        self.push_event(Event::ServerStarted {
            server: self.config.name.clone(),
            version: info.version.clone(),
        });
        self.dirty.store(false, std::sync::atomic::Ordering::SeqCst);
        self.started_once
            .store(true, std::sync::atomic::Ordering::SeqCst);
        *live = Some(Live {
            connection,
            info,
            tools,
            hashes,
        });
        Ok(())
    }

    /// Makes the next listing a relist without the server's `list_changed`
    /// notification, which only the connection's handler sets in the product
    /// (handoff 009f). For tests of that edge: with the SDK's response cache
    /// off, such a relist still reaches the server.
    #[cfg(feature = "test-server")]
    pub fn force_relist(&self) {
        self.dirty.store(true, std::sync::atomic::Ordering::SeqCst);
    }

    /// The current hash of `tool` as the server lists it now (refreshing a
    /// dirty list), `None` when the tool is gone. The listing is made for
    /// `task`: a restart it makes counts against the task (LIM-X3), and a
    /// server that cannot serve is the error, not a missing tool.
    pub async fn current_hash(
        &self,
        task: TaskId,
        tool: &str,
    ) -> Result<Option<Hash>, ServerError> {
        Ok(self.list_for(task).await?.2.get(tool).cloned())
    }

    /// Calls `tool` for `task` with the deadline; cancellation sends
    /// `notifications/cancelled` and, after LIM-O2, kills the process.
    pub async fn call(
        &self,
        task: TaskId,
        tool: &str,
        arguments: Map<String, Value>,
        deadline_ms: i64,
        cancel: &Cancellation,
    ) -> Result<CallOutcome, ServerError> {
        let mut live = self.live.lock().await;
        self.ensure_started(&mut live, Some(task)).await?;
        let l = live.as_mut().expect("started");
        let deadline = Duration::from_millis(deadline_ms.max(1) as u64);
        let outcome = l
            .connection
            .call(
                tool,
                arguments,
                deadline,
                cancel,
                Duration::from_millis(LIM_O2_CANCEL_MS),
            )
            .await;
        if matches!(outcome, CallOutcome::Killed) {
            self.push_event(Event::ServerStopped {
                server: self.config.name.clone(),
                reason: "killed".into(),
            });
            *live = None;
            self.dirty.store(true, std::sync::atomic::Ordering::SeqCst);
        }
        Ok(outcome)
    }

    /// Ends the process: stdin closed, then the group killed after LIM-O16,
    /// the server's `shutdown_ms` (REQ-MCP-001).
    pub async fn shutdown(&self) {
        let mut live = self.live.lock().await;
        if let Some(l) = live.take() {
            l.connection
                .shutdown(Duration::from_millis(self.config.shutdown_ms.max(1) as u64))
                .await;
            self.push_event(Event::ServerStopped {
                server: self.config.name.clone(),
                reason: "shutdown".into(),
            });
        }
    }
}
