//! The `ToolProvider` over every configured server for one task (RFC 009
//! §4, §6, §7; REQ-MCP-005 … 009): the imported definitions, the pin check
//! at the boundary and at the call, the call with its deadline and
//! cancellation, and the result mapped to an execution outcome with the
//! content bounded, sanitized and validated against the pinned output
//! schema.

use crate::client::CallOutcome;
use crate::import::{Imported, import};
use crate::server::{Server, ServerError, ServerInfo};
use mimlys_core::action::{ExecutionMetrics, FailureCause, HostFaultDetail, Termination};
use mimlys_core::event::Event;
use mimlys_core::ids::TaskId;
use mimlys_core::limits::{LIM_C2_EXCERPT_CEILING_CHARS, LIM_I5_MESSAGE_CHARS};
use mimlys_core::observation::{Notice, excerpt, sanitize};
use mimlys_core::spi::{BoxFuture, ExecutionContext, ExecutionOutcome, ToolProvider};
use mimlys_core::tool::{ImportRecord, ToolDefinition};
use mimlys_core::validate::AuthorizedAction;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

/// What `status` shows per server (RFC 009 §8).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct ServerStatus {
    /// The configured name.
    pub name: String,
    /// `running`; `unavailable` when the server could not be started for this
    /// listing; `not_started` when it was not tried (no data directory yet).
    pub state: String,
    /// Why it is unavailable, when it is.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// What the server reported, when it ran.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
    /// The server's own name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_name: Option<String>,
    /// The server's own version.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_version: Option<String>,
    /// The server's instructions, recorded and never placed in the context.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    /// Tools imported and available, full names.
    pub tools: Vec<String>,
    /// Tools listed but unavailable, with the reason.
    pub unavailable: Vec<(String, String)>,
    /// Declared tools the server does not list.
    pub undeclared: Vec<String>,
    /// The tools the configuration declares, full names, when the server was not
    /// started (`state` is `not_started`); empty otherwise.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub declared: Vec<String>,
    /// The list changed since the last import; a new task pins the new definitions.
    pub list_changed: bool,
}

/// The servers of this process, shared by every task's provider.
pub struct Servers {
    servers: Vec<Arc<Server>>,
}

impl Servers {
    /// Wraps the configured servers.
    pub fn new(servers: Vec<Server>) -> Self {
        Self {
            servers: servers.into_iter().map(Arc::new).collect(),
        }
    }

    /// The configured servers.
    pub fn all(&self) -> &[Arc<Server>] {
        &self.servers
    }

    /// Whether any server is configured.
    pub fn is_empty(&self) -> bool {
        self.servers.is_empty()
    }

    /// Imports every server's tools for a new task (RFC 009 §2). A server
    /// that cannot start contributes nothing and is reported by `status`.
    pub async fn import_all(&self) -> Vec<Imported> {
        let mut out = Vec::new();
        for server in &self.servers {
            match server.list().await {
                Ok((_, tools, _)) => {
                    for listed in &tools {
                        let declaration = server.config().tools.get(&listed.name);
                        out.push(import(
                            server.name(),
                            listed,
                            declaration,
                            server.config().max_call_ms,
                        ));
                    }
                }
                Err(e) => {
                    tracing::warn!(target: "mimlys_mcp", server = server.name(), "{e}");
                }
            }
        }
        out
    }

    /// The status of every server: each is started (within LIM-O6) and
    /// listed, so that `status` shows the tools as a task would import them.
    pub async fn status(&self) -> Vec<ServerStatus> {
        let mut out = Vec::new();
        for server in &self.servers {
            let (info, listed, detail) = match server.list().await {
                Ok((info, tools, _)) => (info, tools, None),
                Err(e) => (
                    ServerInfo::default(),
                    vec![],
                    Some(match e {
                        ServerError::Unavailable { reason, .. } => reason,
                        ServerError::RestartLimitReached { .. } => {
                            "restart_limit_reached".to_owned()
                        }
                    }),
                ),
            };
            let running = detail.is_none();
            let mut tools = Vec::new();
            let mut unavailable = Vec::new();
            for l in &listed {
                let imported = import(
                    server.name(),
                    l,
                    server.config().tools.get(&l.name),
                    server.config().max_call_ms,
                );
                match imported.record.unavailable {
                    Some(reason) => unavailable.push((imported.name, reason)),
                    None => tools.push(imported.name),
                }
            }
            let undeclared = server
                .config()
                .tools
                .keys()
                .filter(|t| !listed.iter().any(|l| &l.name == *t))
                .map(|t| format!("{}.{t}", server.name()))
                .collect();
            out.push(ServerStatus {
                name: server.name().to_owned(),
                state: if running { "running" } else { "unavailable" }.into(),
                detail,
                revision: info.revision,
                server_name: info.name,
                server_version: info.version,
                instructions: info.instructions,
                tools,
                unavailable,
                undeclared,
                declared: Vec::new(),
                list_changed: server.is_dirty(),
            });
        }
        out
    }

    /// Ends every server (REQ-MCP-001). Every server's input is closed at
    /// once and each is waited for up to its own `shutdown_ms`, so the exit
    /// takes the longest grace, not their sum.
    pub async fn shutdown(&self) {
        let mut ending = tokio::task::JoinSet::new();
        for server in &self.servers {
            let server = Arc::clone(server);
            ending.spawn(async move { server.shutdown().await });
        }
        while ending.join_next().await.is_some() {}
    }

    /// Server events produced since the last call.
    pub fn drain_events(&self) -> Vec<Event> {
        self.servers.iter().flat_map(|s| s.drain_events()).collect()
    }
}

/// What a call that never left gives: the server could not be reached, its
/// connection was closed before the request was written, or its restart limit
/// (LIM-X3) was reached. Nothing was started: `host_fault{unavailable}`, the
/// cause `tool_unavailable` (external design §7.5; REQ-TOOL-009).
fn unavailable(reason: &str, metrics: ExecutionMetrics) -> ExecutionOutcome {
    ExecutionOutcome {
        termination: Termination::HostFault {
            detail: HostFaultDetail::Unavailable,
        },
        failure: Some(FailureCause::ToolUnavailable),
        payload: json!({ "reason": reason }),
        stdout: Vec::new(),
        stderr: Vec::new(),
        notices: vec![],
        metrics,
    }
}

/// What a server that could not serve gives: nothing was sent (the server could
/// not be started, or the restart limit LIM-X3 was reached before the call), so
/// both are what a call that never left gives.
fn map_server_error(error: ServerError, wall_ms: i64) -> ExecutionOutcome {
    let metrics = ExecutionMetrics {
        wall_ms,
        ..ExecutionMetrics::default()
    };
    match error {
        ServerError::Unavailable { reason, .. } => unavailable(&reason, metrics),
        // A refusal to restart, not one more death: `server.stopped` and
        // `status` keep `died` for a death (external design §11.1).
        ServerError::RestartLimitReached { .. } => unavailable("restart_limit_reached", metrics),
    }
}

/// The external tools of one task.
pub struct McpTools {
    task_id: TaskId,
    servers: Arc<Servers>,
    /// Imported at creation, by full name.
    imported: BTreeMap<String, Imported>,
    /// Tools unpinned for this task (RFC 009 §7).
    unpinned: Mutex<BTreeSet<String>>,
    /// Decisions on the loop's runtime.
    handle: tokio::runtime::Handle,
}

impl McpTools {
    /// The provider for `task_id` with the tools imported now.
    pub async fn open(task_id: TaskId, servers: Arc<Servers>) -> Self {
        let imported = servers
            .import_all()
            .await
            .into_iter()
            .map(|i| (i.name.clone(), i))
            .collect();
        Self {
            task_id,
            servers,
            imported,
            unpinned: Mutex::new(BTreeSet::new()),
            handle: tokio::runtime::Handle::current(),
        }
    }

    fn server_of<'a>(&'a self, name: &'a str) -> Option<(&'a Arc<Server>, &'a str)> {
        let (server, tool) = name.split_once('.')?;
        self.servers
            .all()
            .iter()
            .find(|s| s.name() == server)
            .map(|s| (s, tool))
    }

    /// Maps a call outcome to the execution outcome the loop records.
    fn map_outcome(&self, name: &str, outcome: CallOutcome, wall_ms: i64) -> ExecutionOutcome {
        let metrics = ExecutionMetrics {
            wall_ms,
            ..ExecutionMetrics::default()
        };
        let failed =
            |failure: FailureCause, payload: Value, notices: Vec<Notice>| ExecutionOutcome {
                termination: Termination::Exited { exit_code: 1 },
                failure: Some(failure),
                payload,
                stdout: Vec::new(),
                stderr: Vec::new(),
                notices,
                metrics: metrics.clone(),
            };
        match outcome {
            CallOutcome::Timeout => ExecutionOutcome {
                termination: Termination::Deadline,
                failure: Some(FailureCause::Timeout),
                payload: Value::Null,
                stdout: Vec::new(),
                stderr: Vec::new(),
                notices: vec![],
                metrics,
            },
            CallOutcome::Cancelled | CallOutcome::Killed => ExecutionOutcome {
                termination: Termination::Cancelled,
                failure: None,
                payload: Value::Null,
                stdout: Vec::new(),
                stderr: Vec::new(),
                notices: vec![],
                metrics,
            },
            // The request was not written: nothing reached the server, so the
            // call never left. It is what a server not reachable gives
            // (`host_fault{unavailable}`, the cause `tool_unavailable`), not a
            // lost worker: an unknown outcome would send the user to check an
            // effect that cannot exist (external design §7.5, REQ-TOOL-009).
            // Every `send:` reason is this: the SDK returns from the send only
            // once the request is queued for the transport (`rmcp` 3.9.0
            // `service.rs` 875 to 930), so what fails there precedes any write.
            // `send:closed` is the only kind it gives a client; `send:transport`
            // and `send:other` have no source there and are mapped the same way,
            // so that none is ever a tool error. A write that fails arrives with
            // the response (`call:transport`, below).
            CallOutcome::Failed { reason } if reason.starts_with("send:") => {
                unavailable(&reason, metrics)
            }
            // The connection closed after the request was written, or the write
            // of it failed (`call:transport`: `rmcp` 3.9.0 `service.rs` 1470 to
            // 1473 gives the failed write as the request's response). Whether any
            // byte of the line reached the server cannot be told, and a line
            // complete but for the pipe closing is a call. The call may have
            // taken effect: the worker is lost.
            CallOutcome::Failed { reason }
                if reason.starts_with("call:closed") || reason.starts_with("call:transport") =>
            {
                ExecutionOutcome {
                    termination: Termination::HostFault {
                        detail: HostFaultDetail::WorkerDied,
                    },
                    failure: Some(FailureCause::SandboxFault {
                        detail: HostFaultDetail::WorkerDied,
                    }),
                    payload: Value::Null,
                    stdout: Vec::new(),
                    stderr: Vec::new(),
                    notices: vec![],
                    metrics,
                }
            }
            CallOutcome::Failed { reason } => failed(
                FailureCause::ToolError { detail: reason },
                Value::Null,
                vec![],
            ),
            CallOutcome::Result {
                content,
                structured,
                is_error,
            } => {
                // The payload: structuredContent, else the text content parsed, else the text.
                let text: Option<String> = content.iter().find_map(|c| {
                    (c.get("type") == Some(&json!("text")))
                        .then(|| c.get("text").and_then(Value::as_str).map(str::to_owned))
                        .flatten()
                });
                let mut payload = match structured {
                    Some(v) => v,
                    None => match text
                        .as_deref()
                        .and_then(|t| serde_json::from_str::<Value>(t).ok())
                    {
                        Some(v) if v.is_object() => v,
                        _ => json!({ "text": text.clone().unwrap_or_default() }),
                    },
                };
                let mut notices = Vec::new();
                bound_strings(&mut payload, &mut notices);
                if is_error {
                    let kind = payload
                        .pointer("/error/kind")
                        .and_then(Value::as_str)
                        .unwrap_or("error")
                        .to_owned();
                    let message: String = payload
                        .pointer("/error/message")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .chars()
                        .take(LIM_I5_MESSAGE_CHARS)
                        .collect();
                    let detail = if message.is_empty() {
                        kind
                    } else {
                        format!("{kind}: {message}")
                    };
                    return failed(FailureCause::ToolError { detail }, payload, notices);
                }
                if let Some(output) = self.imported.get(name).and_then(|i| i.output.as_ref())
                    && let Err(v) = output.validate(&payload)
                {
                    tracing::warn!(target: "mimlys_mcp", tool = name, "{v}");
                    return failed(
                        FailureCause::OutputInvalid,
                        json!({ "path": v.path, "reason": v.reason }),
                        notices,
                    );
                }
                ExecutionOutcome {
                    termination: Termination::Exited { exit_code: 0 },
                    failure: None,
                    payload,
                    stdout: Vec::new(),
                    stderr: Vec::new(),
                    notices,
                    metrics,
                }
            }
        }
    }
}

/// Sanitizes and bounds every string of a payload (REQ-MCP-006, REQ-TOOL-008).
fn bound_strings(value: &mut Value, notices: &mut Vec<Notice>) {
    match value {
        Value::String(s) => {
            let (clean, removed) = sanitize(s);
            if removed > 0 {
                notices.push(Notice::CharactersRemoved {
                    count: removed as u64,
                });
            }
            let total = clean.chars().count();
            if total > LIM_C2_EXCERPT_CEILING_CHARS {
                let (cut, _) = excerpt(
                    &clean,
                    LIM_C2_EXCERPT_CEILING_CHARS * 3 / 4,
                    LIM_C2_EXCERPT_CEILING_CHARS / 4,
                );
                notices.push(Notice::ContentTruncated {
                    total_chars: total as u64,
                });
                *s = cut;
            } else if removed > 0 {
                *s = clean;
            }
        }
        Value::Array(items) => {
            for item in items {
                bound_strings(item, notices);
            }
        }
        Value::Object(map) => {
            for item in map.values_mut() {
                bound_strings(item, notices);
            }
        }
        _ => {}
    }
}

impl ToolProvider for McpTools {
    fn definitions(&self) -> Vec<ToolDefinition> {
        let unpinned = self.unpinned.lock().expect("unpinned lock");
        self.imported
            .values()
            .filter(|i| !unpinned.contains(&i.name))
            .filter_map(|i| i.definition.clone())
            .collect()
    }

    fn imports(&self) -> BTreeMap<String, ImportRecord> {
        self.imported
            .iter()
            .map(|(k, v)| (k.clone(), v.record.clone()))
            .collect()
    }

    fn refresh_pins(&self, pinned: &BTreeMap<String, ImportRecord>) -> Vec<String> {
        let mut changed = Vec::new();
        for server in self.servers.all() {
            if !server.is_dirty() {
                continue;
            }
            let handle = self.handle.clone();
            // On this task's behalf: a restart the relisting makes counts
            // against the task (LIM-X3).
            let current = match tokio::task::block_in_place(|| {
                handle.block_on(server.list_for(self.task_id))
            }) {
                Ok((_, _, h)) => Some(h),
                // The restart limit is reached: the definitions did not change,
                // and the call will be refused as `tool_unavailable`.
                Err(ServerError::RestartLimitReached { .. }) => continue,
                Err(_) => None,
            };
            for (name, record) in pinned {
                if record.server != server.name() || record.unavailable.is_some() {
                    continue;
                }
                let tool = name.split_once('.').map(|(_, t)| t).unwrap_or(name);
                let still_same = current
                    .as_ref()
                    .and_then(|h| h.get(tool))
                    .is_some_and(|h| *h == record.hash);
                if !still_same {
                    changed.push(name.clone());
                }
            }
        }
        if !changed.is_empty() {
            let mut unpinned = self.unpinned.lock().expect("unpinned lock");
            for c in &changed {
                unpinned.insert(c.clone());
            }
        }
        changed
    }

    fn drain_events(&self) -> Vec<Event> {
        self.servers.drain_events()
    }

    fn execute<'a>(
        &'a self,
        action: &'a AuthorizedAction,
        context: ExecutionContext,
    ) -> BoxFuture<'a, ExecutionOutcome> {
        Box::pin(async move {
            let name = action.tool().to_owned();
            let started = tokio::time::Instant::now();
            let Some((server, tool)) = self.server_of(&name) else {
                return ExecutionOutcome::tool_error("unknown_server", Value::Null, 0);
            };
            // Second line of pinning (RFC 009 §7): the hash at call time.
            // The listing is made for this task: a restart it makes counts
            // against the task (LIM-X3).
            if let Some(record) = self.imported.get(&name).map(|i| &i.record) {
                let current = match server.current_hash(self.task_id, tool).await {
                    Ok(current) => current,
                    // The server cannot serve (it could not start, or its restart
                    // limit is reached for this task): the call is not made, and
                    // the tool is not unpinned for a definition that did not change.
                    Err(error) => {
                        return map_server_error(error, started.elapsed().as_millis() as i64);
                    }
                };
                if current.as_ref() != Some(&record.hash) {
                    self.unpinned
                        .lock()
                        .expect("unpinned lock")
                        .insert(name.clone());
                    return ExecutionOutcome::tool_error(
                        "definition_changed",
                        json!({"hint": "a new task pins the new definition"}),
                        started.elapsed().as_millis() as i64,
                    );
                }
            }
            let outcome = server
                .call(
                    self.task_id,
                    tool,
                    action.arguments().clone(),
                    context.deadline_ms,
                    &context.cancel,
                )
                .await;
            let wall_ms = started.elapsed().as_millis() as i64;
            match outcome {
                Ok(o) => self.map_outcome(&name, o, wall_ms),
                Err(error) => map_server_error(error, wall_ms),
            }
        })
    }
}

#[cfg(test)]
mod tests;
