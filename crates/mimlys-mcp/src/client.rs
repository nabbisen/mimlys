//! The SDK wrapper: one connection to one child process (RFC 009 §1, §6;
//! REQ-MCP-007). It spawns the process with an explicit environment inside
//! its own session and process group (or Job Object), initializes within the caller's
//! deadline, lists tools, calls one tool with a deadline and cancellation
//! (`notifications/cancelled`, then the kill of the group), and shuts down
//! by closing the transport and then killing.

use crate::lines::{BoundedLines, LineState};
use crate::server::{Listed, ServerConfig, ServerInfo};
use mimlys_core::limits::LIM_I22_SERVER_MESSAGE_BYTES;
use mimlys_core::spi::Cancellation;
use rmcp::ClientHandler;
use rmcp::model::{CallToolRequestParams, ClientRequest, PaginatedRequestParams, ServerResult};
use rmcp::service::{NotificationContext, Peer, PeerRequestOptions, RoleClient, RunningService};
use serde_json::{Map, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// How often cancellation is polled while a request is in flight.
const CANCEL_POLL: Duration = Duration::from_millis(100);

/// The most pages one listing may take (LIM-I11). rypsok lists three tools
/// in one page.
pub const LIST_MAX_PAGES: usize = 16;

/// The most tools one listing may hold (LIM-I12). rypsok lists three; a
/// model is shown a few dozen at most (LIM-I1).
pub const LIST_MAX_TOOLS: usize = 256;

/// Why a bounded wait ended before its future did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Interrupted {
    /// The time allowed passed.
    Deadline,
    /// The caller's interruption was requested.
    Cancelled,
}

/// Waits for `fut` for at most `within`, looking at `cancel` (when given)
/// before the first poll and then every [`CANCEL_POLL`]. A request future
/// dropped here is cancelled towards the server by the SDK.
pub(crate) async fn bounded<F: std::future::Future>(
    fut: F,
    within: Duration,
    cancel: Option<&Cancellation>,
) -> Result<F::Output, Interrupted> {
    let cancelled = || cancel.is_some_and(Cancellation::is_cancelled);
    if cancelled() {
        return Err(Interrupted::Cancelled);
    }
    let deadline = tokio::time::Instant::now() + within;
    tokio::pin!(fut);
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        tokio::select! {
            out = &mut fut => return Ok(out),
            () = tokio::time::sleep(left.min(CANCEL_POLL)) => {}
        }
        if cancelled() {
            return Err(Interrupted::Cancelled);
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(Interrupted::Deadline);
        }
    }
}

/// The client handler: records `list_changed`.
#[derive(Clone)]
struct Handler {
    dirty: Arc<AtomicBool>,
}

impl ClientHandler for Handler {
    fn on_tool_list_changed(
        &self,
        _context: NotificationContext<RoleClient>,
    ) -> impl std::future::Future<Output = ()> + Send + '_ {
        self.dirty.store(true, Ordering::SeqCst);
        std::future::ready(())
    }

    fn get_info(&self) -> rmcp::model::ClientConfig {
        let mut implementation = rmcp::model::Implementation::default();
        implementation.name = "mimlys".into();
        implementation.version = env!("CARGO_PKG_VERSION").into();
        let mut config = rmcp::model::ClientConfig::default();
        config.client_info = implementation;
        config
    }
}

/// What one call produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CallOutcome {
    /// The server answered.
    Result {
        /// `content[]` as the SDK parsed it, re-serialized.
        content: Vec<Value>,
        /// `structuredContent`, when present.
        structured: Option<Value>,
        /// `isError`.
        is_error: bool,
    },
    /// The deadline passed; the request was cancelled towards the server.
    Timeout,
    /// Cancelled on request; the server acknowledged by not answering.
    Cancelled,
    /// Cancelled, and the server did not stop within the grace period: the process was killed.
    Killed,
    /// Nothing was sent: the connection the call would have used lists the tool
    /// otherwise than it was pinned (a restart between the pin check and the call
    /// brought a changed definition).
    DefinitionChanged,
    /// The transport failed or the server answered a protocol error.
    Failed {
        /// A fixed-vocabulary reason.
        reason: String,
    },
}

/// The child process, owned by the connection. While it is here, dropping
/// the connection kills its whole group (or Job Object); a child that has
/// ended by itself is released first. The SDK's own stdio transport is not
/// used for the child, because its shutdown (standard input closed, 3 s, then
/// SIGTERM, 3 s, then the kill) cannot be given the server's own grace
/// (LIM-O16).
struct ChildGuard {
    child: Option<Box<dyn process_wrap::tokio::ChildWrapper>>,
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            // `kill` reaches the group, and waits for the leader (no zombie).
            tokio::spawn(async move {
                if let Err(e) = Box::into_pin(child.kill()).await {
                    tracing::warn!("Error killing child process: {e}");
                }
            });
        }
    }
}

/// What ends the connection: the service task and the child. Taken, once,
/// by the shutdown or by the kill; requests go through the peer and never
/// touch it.
struct Closer {
    service: RunningService<RoleClient, Handler>,
    child: ChildGuard,
}

/// One live connection. Every request goes through the peer, which is shared:
/// nothing here is held across a wait.
pub struct Connection {
    peer: Peer<RoleClient>,
    closer: Mutex<Option<Closer>>,
    process_id: Option<u32>,
    info: ServerInfo,
    dirty: Arc<AtomicBool>,
    /// The calls in flight, for the span of the standard-error reader's lines.
    reader_span: mimlys_core::log_span::ReaderSpan,
    /// What the reader of the server's standard output knows: whether a message went past
    /// LIM-I22 and ended the stream, and the most bytes of one message it passed on.
    lines: Arc<LineState>,
}

impl Connection {
    /// Spawns and initializes; the caller bounds the time. `dirty` is set
    /// when the server announces a changed tool list.
    pub async fn start(config: &ServerConfig, dirty: Arc<AtomicBool>) -> Result<Self, String> {
        let reader_span = mimlys_core::log_span::ReaderSpan::new();
        // The start is in flight too, so that a line written while the server
        // initializes carries the ids of the step that needed it.
        let starting = reader_span.begin();
        let mut command = tokio::process::Command::new(&config.command);
        command
            .args(&config.args)
            .env_clear()
            .envs(config.environment())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        if let Some(dir) = &config.current_dir {
            command.current_dir(dir);
        }
        // Clause K: the child leads a session of its own (`setsid`, which is also
        // a process group of its own; a Job Object on Windows), so that a kill
        // reaches whatever it spawned and the server has no controlling terminal.
        let mut wrapped = process_wrap::tokio::CommandWrap::from(command);
        #[cfg(unix)]
        wrapped.wrap(process_wrap::tokio::ProcessSession);
        #[cfg(windows)]
        wrapped.wrap(process_wrap::tokio::JobObject);
        wrapped.wrap(process_wrap::tokio::KillOnDrop);
        let mut child = wrapped.spawn().map_err(|e| format!("spawn:{}", e.kind()))?;
        let process_id = child.id();
        if let (Some(hook), Some(pid)) = (&config.on_spawn, process_id) {
            (hook.0)(pid);
        }
        let stdin = child.stdin().take();
        let stdout = child.stdout().take();
        let stderr = child.stderr().take();
        let mut guard = ChildGuard { child: Some(child) };
        let (Some(stdin), Some(stdout)) = (stdin, stdout) else {
            return Err("spawn:no_stdio".into());
        };
        // What the server writes is read through a bound on one message (LIM-I22): past it the
        // stream ends with an error, before the message is held whole.
        let (stdout, lines) = BoundedLines::new(stdout, &config.name, LIM_I22_SERVER_MESSAGE_BYTES);
        if let Some(stderr) = stderr {
            // The server outlives the step that started it and serves many
            // calls: each line is written in the span of the call in flight
            // when it is read (none between calls), not in a span captured here.
            // A line is cut and the lines are counted: what a server writes on
            // its standard error is bounded (`stderr`).
            tokio::spawn(crate::stderr::pump(
                tokio::io::BufReader::new(stderr),
                config.name.clone(),
                reader_span.clone(),
            ));
        }
        let handler = Handler {
            dirty: Arc::clone(&dirty),
        };
        let service = rmcp::ServiceExt::serve(handler, (stdout, stdin))
            .await
            .map_err(|e| format!("initialize:{}", kind_of_init(&e)))?;
        // The SDK's client keeps `tools/list` answers that carry a `ttlMs`, and by
        // default serves an expired one when a later listing fails. mimlys pins what
        // a server offers (REQ-MCP-002) and must never believe it listed when it did
        // not (THR-25): the cache is off, so the server is always asked and a failed
        // listing is `unavailable` with its reason, never an old list.
        service
            .peer()
            .set_response_cache_config(rmcp::service::ClientCacheConfig::disabled())
            .await;
        let child = std::mem::replace(&mut guard, ChildGuard { child: None });
        let peer_info = service.peer().peer_info();
        // What the server says of itself is cleaned where it enters: it is shown
        // by `status` and recorded in `server.started`.
        let shown = |text: &str| mimlys_core::panic_text::bounded_message(text, None);
        let info = ServerInfo {
            revision: peer_info
                .as_ref()
                .map(|i| shown(&i.protocol_version.to_string())),
            name: peer_info
                .as_ref()
                .and_then(|i| i.server_info.as_ref().map(|s| shown(&s.name))),
            version: peer_info
                .as_ref()
                .and_then(|i| i.server_info.as_ref().map(|s| shown(&s.version))),
            instructions: peer_info
                .as_ref()
                .and_then(|i| i.instructions.as_deref())
                .map(|text| mimlys_core::observation::sanitize(text).0),
        };
        drop(starting);
        let peer = service.peer().clone();
        Ok(Self {
            peer,
            closer: Mutex::new(Some(Closer { service, child })),
            process_id,
            info,
            dirty,
            reader_span,
            lines,
        })
    }

    /// What the reader of the server's standard output knows (a test's seam, and `is_closed`'s).
    pub fn lines(&self) -> &Arc<LineState> {
        &self.lines
    }

    /// A message of the server went past LIM-I22: the stream ended, and the server is stopped
    /// as one that ended. Its process is still running (it is writing what nobody reads), so it
    /// is killed with its group now, not at the next start.
    fn stop_if_a_message_was_too_large(&self) {
        if self.lines.exceeded() {
            self.kill();
        }
    }

    /// What the server reported at initialize.
    pub fn info(&self) -> ServerInfo {
        self.info.clone()
    }

    /// The child's process id.
    pub fn process_id(&self) -> Option<u32> {
        self.process_id
    }

    /// Whether the server announced a changed list since the last check (cleared here).
    pub fn take_dirty(&self) -> bool {
        self.dirty.swap(false, Ordering::SeqCst)
    }

    /// Whether the transport is gone.
    pub fn is_closed(&self) -> bool {
        self.stop_if_a_message_was_too_large();
        self.peer.is_transport_closed()
            || self
                .closer
                .lock()
                .expect("closer lock")
                .as_ref()
                .is_none_or(|c| c.service.is_closed())
    }

    /// `tools/list`, every page, within `within` (each page and the whole
    /// listing), observing `cancel`. It stops at [`LIST_MAX_PAGES`] pages and
    /// at [`LIST_MAX_TOOLS`] tools. The reasons are `list:timeout`,
    /// `list:cancelled`, `list:too_many_pages`, `list:too_many_tools` and
    /// `list:<kind>` of the SDK's error.
    pub async fn list_tools(
        &self,
        within: Duration,
        cancel: Option<&Cancellation>,
    ) -> Result<Vec<Listed>, String> {
        let _in_flight = self.reader_span.begin();
        let started = tokio::time::Instant::now();
        let mut listed: Vec<Listed> = Vec::new();
        let mut cursor = None;
        for _ in 0..LIST_MAX_PAGES {
            let left = within.saturating_sub(started.elapsed());
            if left.is_zero() {
                return Err("list:timeout".into());
            }
            let page = bounded(
                self.peer
                    .list_tools(Some(PaginatedRequestParams::default().with_cursor(cursor))),
                left,
                cancel,
            )
            .await;
            let page = match page {
                Ok(Ok(page)) => page,
                Ok(Err(e)) => return Err(format!("list:{}", kind_of(&e))),
                Err(Interrupted::Deadline) => return Err("list:timeout".into()),
                Err(Interrupted::Cancelled) => return Err("list:cancelled".into()),
            };
            listed.extend(page.tools.into_iter().map(|t| Listed {
                name: t.name.to_string(),
                description: t.description.map(|d| d.to_string()),
                input_schema: Value::Object((*t.input_schema).clone()),
                output_schema: t.output_schema.map(|s| Value::Object((*s).clone())),
                annotations: t.annotations.and_then(|a| serde_json::to_value(a).ok()),
            }));
            if listed.len() > LIST_MAX_TOOLS {
                return Err("list:too_many_tools".into());
            }
            cursor = page.next_cursor;
            if cursor.is_none() {
                return Ok(listed);
            }
        }
        Err("list:too_many_pages".into())
    }

    /// One `tools/call` within `deadline`; cancellation sends the
    /// notification and, after `grace`, kills the process.
    pub async fn call(
        &self,
        tool: &str,
        arguments: Map<String, Value>,
        deadline: Duration,
        cancel: &Cancellation,
        grace: Duration,
    ) -> CallOutcome {
        let _in_flight = self.reader_span.begin();
        let request = ClientRequest::CallToolRequest(rmcp::model::Request::new(
            CallToolRequestParams::new(tool.to_owned()).with_arguments(arguments),
        ));
        let handle = match self
            .peer
            .send_cancellable_request(request, PeerRequestOptions::no_options())
            .await
        {
            Ok(h) => h,
            Err(e) => {
                return CallOutcome::Failed {
                    reason: format!("send:{}", kind_of(&e)),
                };
            }
        };
        let id = handle.id.clone();
        let peer = handle.peer.clone();
        let response = handle.await_response();
        tokio::pin!(response);
        let started = tokio::time::Instant::now();
        loop {
            tokio::select! {
                r = &mut response => {
                    return match r {
                        Ok(ServerResult::CallToolResult(result)) => CallOutcome::Result {
                            content: result
                                .content
                                .iter()
                                .filter_map(|c| serde_json::to_value(c).ok())
                                .collect(),
                            structured: result.structured_content,
                            is_error: result.is_error.unwrap_or(false),
                        },
                        Ok(_) => CallOutcome::Failed { reason: "response_shape".into() },
                        Err(e) => {
                            // A server whose message went past LIM-I22 is a server that ended.
                            self.stop_if_a_message_was_too_large();
                            CallOutcome::Failed { reason: format!("call:{}", kind_of(&e)) }
                        }
                    };
                }
                () = tokio::time::sleep(CANCEL_POLL) => {}
            }
            let cancelled = cancel.is_cancelled();
            if cancelled || started.elapsed() >= deadline {
                let reason = if cancelled { "cancelled" } else { "deadline" };
                let _ = peer
                    .send_notification(rmcp::model::ClientNotification::CancelledNotification(
                        rmcp::model::Notification::new(
                            rmcp::model::CancelledNotificationParam::new(
                                Some(id),
                                Some(reason.into()),
                            ),
                        ),
                    ))
                    .await;
                // The SDK abandons the request at once; the protocol expects no
                // response to a cancelled request, so whether the server still
                // lives is asked with a ping within the grace period (REQ-MCP-007):
                // a server that no longer answers is killed with its group.
                let alive = tokio::time::timeout(
                    grace,
                    peer.send_request(ClientRequest::PingRequest(Default::default())),
                )
                .await;
                return match alive {
                    Ok(Ok(_)) if cancelled => CallOutcome::Cancelled,
                    Ok(Ok(_)) => CallOutcome::Timeout,
                    _ => {
                        self.kill();
                        CallOutcome::Killed
                    }
                };
            }
        }
    }

    /// Ends the service task and kills the process group or Job Object
    /// (clause K, through the child guard). Requests in flight on the peer
    /// fail at once.
    fn kill(&self) {
        let closer = self.closer.lock().expect("closer lock").take();
        if let Some(closer) = closer {
            closer.service.cancellation_token().cancel();
            drop(closer);
        }
    }

    /// Closes the transport (the child's stdin) and waits up to `grace`
    /// for the process to exit by itself; what remains is killed with the
    /// group (REQ-MCP-001). It waits for no request: a listing or a call in
    /// flight on another task ends with the transport.
    pub async fn shutdown(&self, grace: Duration) {
        let closer = self.closer.lock().expect("closer lock").take();
        let Some(mut closer) = closer else { return };
        let deadline = tokio::time::Instant::now() + grace;
        // Closing the service drops the child's standard input.
        let _ = closer.service.close_with_timeout(grace).await;
        if let Some(child) = closer.child.child.as_mut() {
            let ended = tokio::time::timeout_at(deadline, child.wait()).await;
            if matches!(ended, Ok(Ok(_))) {
                // It ended by itself: nothing to kill.
                closer.child.child = None;
            }
            // Otherwise the guard kills the group as `closer` drops.
        }
    }
}

fn kind_of(error: &rmcp::ServiceError) -> &'static str {
    match error {
        rmcp::ServiceError::McpError(_) => "protocol",
        rmcp::ServiceError::TransportSend(_) => "transport",
        rmcp::ServiceError::TransportClosed => "closed",
        rmcp::ServiceError::UnexpectedResponse => "response_shape",
        rmcp::ServiceError::Cancelled { .. } => "cancelled",
        rmcp::ServiceError::Timeout { .. } => "timeout",
        _ => "other",
    }
}

fn kind_of_init(error: &rmcp::service::ClientInitializeError) -> String {
    let text = error.to_string().to_ascii_lowercase();
    if text.contains("timeout") {
        "timeout".into()
    } else if text.contains("transport") || text.contains("closed") {
        "transport".into()
    } else {
        "protocol".into()
    }
}
