//! The SDK wrapper: one connection to one child process (RFC 009 §1, §6;
//! REQ-MCP-007). It spawns the process with an explicit environment inside
//! its own process group (or Job Object), initializes within the caller's
//! deadline, lists tools, calls one tool with a deadline and cancellation
//! (`notifications/cancelled`, then the kill of the group), and shuts down
//! by closing the transport and then killing.

use crate::server::{Listed, ServerConfig, ServerInfo};
use mimlys_core::spi::Cancellation;
use rmcp::ClientHandler;
use rmcp::model::{CallToolRequestParams, ClientRequest, ServerResult};
use rmcp::service::{NotificationContext, PeerRequestOptions, RoleClient, RunningService};
use serde_json::{Map, Value};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// How often cancellation is polled while a call is in flight.
const CANCEL_POLL: Duration = Duration::from_millis(100);

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

/// One live connection.
pub struct Connection {
    service: RunningService<RoleClient, Handler>,
    child: ChildGuard,
    process_id: Option<u32>,
    info: ServerInfo,
    dirty: Arc<AtomicBool>,
}

impl Connection {
    /// Spawns and initializes; the caller bounds the time. `dirty` is set
    /// when the server announces a changed tool list.
    pub async fn start(config: &ServerConfig, dirty: Arc<AtomicBool>) -> Result<Self, String> {
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
        // Clause K: the child leads its own process group (Job Object on
        // Windows), so that a kill reaches whatever it spawned.
        let mut wrapped = process_wrap::tokio::CommandWrap::from(command);
        #[cfg(unix)]
        wrapped.wrap(process_wrap::tokio::ProcessGroup::leader());
        #[cfg(windows)]
        wrapped.wrap(process_wrap::tokio::JobObject);
        wrapped.wrap(process_wrap::tokio::KillOnDrop);
        let mut child = wrapped.spawn().map_err(|e| format!("spawn:{}", e.kind()))?;
        let process_id = child.id();
        let stdin = child.stdin().take();
        let stdout = child.stdout().take();
        let stderr = child.stderr().take();
        let mut guard = ChildGuard { child: Some(child) };
        let (Some(stdin), Some(stdout)) = (stdin, stdout) else {
            return Err("spawn:no_stdio".into());
        };
        if let Some(stderr) = stderr {
            let server = config.name.clone();
            tokio::spawn(async move {
                use tokio::io::AsyncBufReadExt;
                let mut lines = tokio::io::BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    tracing::info!(target: "mimlys_mcp", server = %server, "{line}");
                }
            });
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
        let info = ServerInfo {
            revision: peer_info.as_ref().map(|i| i.protocol_version.to_string()),
            name: peer_info
                .as_ref()
                .and_then(|i| i.server_info.as_ref().map(|s| s.name.clone())),
            version: peer_info
                .as_ref()
                .and_then(|i| i.server_info.as_ref().map(|s| s.version.clone())),
            instructions: peer_info.as_ref().and_then(|i| i.instructions.clone()),
        };
        Ok(Self {
            service,
            child,
            process_id,
            info,
            dirty,
        })
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
        self.service.is_closed() || self.service.peer().is_transport_closed()
    }

    /// `tools/list`, every page.
    pub async fn list_tools(&self) -> Result<Vec<Listed>, String> {
        let tools = self
            .service
            .peer()
            .list_all_tools()
            .await
            .map_err(|e| format!("list:{}", kind_of(&e)))?;
        Ok(tools
            .into_iter()
            .map(|t| Listed {
                name: t.name.to_string(),
                description: t.description.map(|d| d.to_string()),
                input_schema: Value::Object((*t.input_schema).clone()),
                output_schema: t.output_schema.map(|s| Value::Object((*s).clone())),
                annotations: t.annotations.and_then(|a| serde_json::to_value(a).ok()),
            })
            .collect())
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
        let request = ClientRequest::CallToolRequest(rmcp::model::Request::new(
            CallToolRequestParams::new(tool.to_owned()).with_arguments(arguments),
        ));
        let handle = match self
            .service
            .peer()
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
                        Err(e) => CallOutcome::Failed { reason: format!("call:{}", kind_of(&e)) },
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

    /// Ends the service task; dropping the connection kills the process
    /// group or Job Object (clause K, through the child guard).
    fn kill(&self) {
        self.service.cancellation_token().cancel();
    }

    /// Closes the transport (the child's stdin) and waits up to `grace`
    /// for the process to exit by itself; what remains is killed with the
    /// group when the connection drops (REQ-MCP-001).
    pub async fn shutdown(mut self, grace: Duration) {
        let deadline = tokio::time::Instant::now() + grace;
        // Closing the service drops the child's standard input.
        let _ = self.service.close_with_timeout(grace).await;
        if let Some(child) = self.child.child.as_mut() {
            let ended = tokio::time::timeout_at(deadline, child.wait()).await;
            if matches!(ended, Ok(Ok(_))) {
                // It ended by itself: nothing to kill.
                self.child.child = None;
            }
            // Otherwise the guard kills the group as `self` drops.
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
