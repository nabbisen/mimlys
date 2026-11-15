//! One configured MCP server as a child process (RFC 009 §1; REQ-MCP-001,
//! REQ-PRIV-003): an explicit environment, lazy start within LIM-O6, the
//! tool list with its hashes, restarts counted per task, shutdown by
//! closing stdin and then by killing the process group.

use crate::client::{CallOutcome, Connection, Interrupted, bounded};
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
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
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
    RestartLimitReached {
        /// The server.
        server: String,
    },
}

/// What a listing yields: the server's information, its tools and their hashes.
pub type Listing = (ServerInfo, Vec<Listed>, BTreeMap<String, Hash>);

/// The tools a connection last listed.
struct Current {
    tools: Vec<Listed>,
    hashes: BTreeMap<String, Hash>,
}

/// A running server with its list. Shared: a request clones the `Arc` and
/// holds nothing of the server while it waits.
struct Live {
    connection: Connection,
    info: ServerInfo,
    current: Mutex<Current>,
}

/// The reason a wait that ended early gives, for a listing.
fn list_reason(why: Interrupted) -> String {
    match why {
        Interrupted::Deadline => "list:timeout",
        Interrupted::Cancelled => "list:cancelled",
    }
    .into()
}

/// The reason a wait that ended early gives, for a start.
fn start_reason(why: Interrupted) -> String {
    match why {
        Interrupted::Deadline => "startup_timeout",
        Interrupted::Cancelled => "startup_cancelled",
    }
    .into()
}

/// One configured server.
///
/// No lock is held across a wait that a deadline and the interruption do not
/// bound: the process is started under `start` and a changed list is read
/// under `relist`, each taken within the caller's own deadline and
/// interruption; `live` is held for a moment only. A call, the pin check of
/// another task and `shutdown` therefore never queue behind a listing that
/// does not end.
pub struct Server {
    config: ServerConfig,
    /// The running process; held for a moment, never across a wait.
    live: Mutex<Option<Arc<Live>>>,
    /// One start at a time.
    start: tokio::sync::Mutex<()>,
    /// One relisting at a time, so that a caller who finds the list marked
    /// stale after another's relisting began waits for its result.
    relist: tokio::sync::Mutex<()>,
    /// Counts the shutdowns, so that a start that a shutdown overtook ends
    /// the process it made.
    epoch: AtomicU64,
    /// Restarts per task (LIM-X3): every start of a server that had started
    /// before, made for the task (a listing for no task is not counted).
    restarts: Mutex<HashMap<TaskId, u32>>,
    /// Whether the process was started at least once: a start after that is a
    /// restart, whatever ended the process before (its death, the kill after
    /// LIM-O2).
    started_once: AtomicBool,
    /// Events produced and not yet drained (RFC 009 §1).
    events: Mutex<Vec<Event>>,
    /// Whether the last known list may be stale (`list_changed` or a
    /// restart); shared with the connection's notification handler.
    dirty: Arc<AtomicBool>,
    /// The last list seen, for a provider that needs it without starting the server.
    last: Mutex<Option<Listing>>,
}

impl Server {
    /// A server that has not started.
    pub fn new(config: ServerConfig) -> Self {
        Self {
            config,
            live: Mutex::new(None),
            start: tokio::sync::Mutex::new(()),
            relist: tokio::sync::Mutex::new(()),
            epoch: AtomicU64::new(0),
            restarts: Mutex::new(HashMap::new()),
            started_once: AtomicBool::new(false),
            events: Mutex::new(Vec::new()),
            dirty: Arc::new(AtomicBool::new(false)),
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
        self.dirty.load(Ordering::SeqCst)
    }

    /// Takes the events produced since the last call.
    pub fn drain_events(&self) -> Vec<Event> {
        std::mem::take(&mut *self.events.lock().expect("events lock"))
    }

    /// Whether the process is running (as far as the transport knows).
    pub async fn is_running(&self) -> bool {
        self.live
            .lock()
            .expect("live lock")
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

    /// LIM-O6: the time a start, a listing and each of its pages are given.
    fn startup(&self) -> Duration {
        Duration::from_millis(self.config.startup_ms.max(1) as u64)
    }

    fn unavailable(&self, reason: String) -> ServerError {
        ServerError::Unavailable {
            server: self.config.name.clone(),
            reason,
        }
    }

    /// Starts the process if needed and returns the current list, for no
    /// task (`status`, the import at startup): a restart it makes is not
    /// counted against any task.
    pub async fn list(&self) -> Result<Listing, ServerError> {
        self.list_counted(None, None).await
    }

    /// As [`Server::list`], on behalf of `task`: a restart it makes counts
    /// against the task's allowance (LIM-X3), and past the limit the server is
    /// not started. The start and the listing end when `cancel` is requested.
    pub async fn list_for(
        &self,
        task: TaskId,
        cancel: &Cancellation,
    ) -> Result<Listing, ServerError> {
        self.list_counted(Some(task), Some(cancel)).await
    }

    async fn list_counted(
        &self,
        task: Option<TaskId>,
        cancel: Option<&Cancellation>,
    ) -> Result<Listing, ServerError> {
        let begun = tokio::time::Instant::now();
        let within = self.startup();
        let live = self.ensure_started(task, cancel).await?;
        if self.dirty.load(Ordering::SeqCst) {
            let left = |begun: tokio::time::Instant| within.saturating_sub(begun.elapsed());
            let gate = bounded(self.relist.lock(), left(begun), cancel)
                .await
                .map_err(|why| self.unavailable(list_reason(why)))?;
            // The relisting waited for may have brought the list up to date.
            if self.dirty.swap(false, Ordering::SeqCst) {
                match live.connection.list_tools(left(begun), cancel).await {
                    Ok(tools) => {
                        let hashes = crate::import::hashes(&self.config.name, &tools);
                        *live.current.lock().expect("current lock") = Current { tools, hashes };
                    }
                    Err(reason) => {
                        self.dirty.store(true, Ordering::SeqCst);
                        return Err(self.unavailable(reason));
                    }
                }
            }
            drop(gate);
        }
        let current = live.current.lock().expect("current lock");
        let snapshot = (
            live.info.clone(),
            current.tools.clone(),
            current.hashes.clone(),
        );
        drop(current);
        *self.last.lock().expect("last lock") = Some(snapshot.clone());
        Ok(snapshot)
    }

    /// The running process, started when it is not running. A start after the
    /// first is a restart: made for `task`, it counts against the task
    /// (LIM-X3) and, past the limit, is refused
    /// (`ServerError::RestartLimitReached`) every time it is asked again, not
    /// only once. A start for no task is never counted. The wait for another
    /// start, the start and its first listing each take at most LIM-O6 and
    /// end when `cancel` is requested.
    async fn ensure_started(
        &self,
        task: Option<TaskId>,
        cancel: Option<&Cancellation>,
    ) -> Result<Arc<Live>, ServerError> {
        if let Some(live) = self.running() {
            return Ok(live);
        }
        let startup = self.startup();
        let _gate = bounded(self.start.lock(), startup, cancel)
            .await
            .map_err(|why| self.unavailable(start_reason(why)))?;
        // Another caller may have started it while this one waited.
        let died = {
            let mut slot = self.live.lock().expect("live lock");
            match slot.as_ref() {
                Some(live) if !live.connection.is_closed() => return Ok(Arc::clone(live)),
                Some(_) => slot.take(),
                None => None,
            }
        };
        if died.is_some() {
            // Died: record it; the restart below is counted.
            self.push_event(Event::ServerStopped {
                server: self.config.name.clone(),
                reason: "died".into(),
            });
        }
        if let Some(task) = task
            && self.started_once.load(Ordering::SeqCst)
        {
            let mut restarts = self.restarts.lock().expect("restarts lock");
            let count = restarts.entry(task).or_insert(0);
            if *count >= LIM_X3_RESTARTS {
                return Err(ServerError::RestartLimitReached {
                    server: self.config.name.clone(),
                });
            }
            *count += 1;
        }
        let epoch = self.epoch.load(Ordering::SeqCst);
        let connection = bounded(
            Connection::start(&self.config, Arc::clone(&self.dirty)),
            startup,
            cancel,
        )
        .await
        .map_err(|why| self.unavailable(start_reason(why)))?
        .map_err(|reason| self.unavailable(reason))?;
        let info = connection.info();
        let tools = connection
            .list_tools(startup, cancel)
            .await
            .map_err(|reason| self.unavailable(reason))?;
        if self.epoch.load(Ordering::SeqCst) != epoch {
            // A shutdown came while this start was in flight: it ends the
            // process it made.
            connection.shutdown(self.shutdown_grace()).await;
            return Err(self.unavailable("shutting_down".into()));
        }
        let hashes = crate::import::hashes(&self.config.name, &tools);
        self.push_event(Event::ServerStarted {
            server: self.config.name.clone(),
            version: info.version.clone(),
        });
        self.dirty.store(false, Ordering::SeqCst);
        self.started_once.store(true, Ordering::SeqCst);
        let live = Arc::new(Live {
            connection,
            info,
            current: Mutex::new(Current { tools, hashes }),
        });
        *self.live.lock().expect("live lock") = Some(Arc::clone(&live));
        Ok(live)
    }

    /// The process, when it is running.
    fn running(&self) -> Option<Arc<Live>> {
        self.live
            .lock()
            .expect("live lock")
            .as_ref()
            .filter(|l| !l.connection.is_closed())
            .map(Arc::clone)
    }

    fn shutdown_grace(&self) -> Duration {
        Duration::from_millis(self.config.shutdown_ms.max(1) as u64)
    }

    /// Makes the next listing a relist without the server's `list_changed`
    /// notification, which only the connection's handler sets in the product
    /// (handoff 009f). For tests of that edge: with the SDK's response cache
    /// off, such a relist still reaches the server.
    #[cfg(feature = "test-server")]
    pub fn force_relist(&self) {
        self.dirty.store(true, Ordering::SeqCst);
    }

    /// The current hash of `tool` as the server lists it now (refreshing a
    /// dirty list), `None` when the tool is gone. The listing is made for
    /// `task`: a restart it makes counts against the task (LIM-X3), and a
    /// server that cannot serve is the error, not a missing tool. It ends
    /// within LIM-O6 and when `cancel` is requested.
    pub async fn current_hash(
        &self,
        task: TaskId,
        tool: &str,
        cancel: &Cancellation,
    ) -> Result<Option<Hash>, ServerError> {
        Ok(self.list_for(task, cancel).await?.2.get(tool).cloned())
    }

    /// Calls `tool` for `task` with the deadline; cancellation sends
    /// `notifications/cancelled` and, after LIM-O2, kills the process. The
    /// call waits for no other request of the server.
    pub async fn call(
        &self,
        task: TaskId,
        tool: &str,
        arguments: Map<String, Value>,
        deadline_ms: i64,
        cancel: &Cancellation,
    ) -> Result<CallOutcome, ServerError> {
        let live = self.ensure_started(Some(task), Some(cancel)).await?;
        let deadline = Duration::from_millis(deadline_ms.max(1) as u64);
        let outcome = live
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
            // Only the process this call used: a restart another caller made
            // meanwhile stays.
            let mut slot = self.live.lock().expect("live lock");
            if slot.as_ref().is_some_and(|l| Arc::ptr_eq(l, &live)) {
                *slot = None;
            }
            drop(slot);
            self.push_event(Event::ServerStopped {
                server: self.config.name.clone(),
                reason: "killed".into(),
            });
            self.dirty.store(true, Ordering::SeqCst);
        }
        Ok(outcome)
    }

    /// Ends the process: stdin closed, then the group killed after LIM-O16,
    /// the server's `shutdown_ms` (REQ-MCP-001). It waits for no request in
    /// flight.
    pub async fn shutdown(&self) {
        self.epoch.fetch_add(1, Ordering::SeqCst);
        let live = self.live.lock().expect("live lock").take();
        if let Some(l) = live {
            l.connection.shutdown(self.shutdown_grace()).await;
            self.push_event(Event::ServerStopped {
                server: self.config.name.clone(),
                reason: "shutdown".into(),
            });
        }
    }
}
