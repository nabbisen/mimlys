//! The `ToolProvider` over every configured server for one task (RFC 009
//! §4, §6, §7; REQ-MCP-005 … 009): the imported definitions, the pin check
//! at the boundary and at the call, the call with its deadline and
//! cancellation, and the result mapped to an execution outcome with the
//! content bounded, sanitized and validated against the pinned output
//! schema.

use crate::bound::{self, Cleaned, Unit};
use crate::client::CallOutcome;
use crate::import::{Imported, import};
use crate::server::{Server, ServerError, ServerInfo};
use mimlys_core::action::{ExecutionMetrics, FailureCause, HostFaultDetail, Termination};
use mimlys_core::context::Redactor;
use mimlys_core::event::Event;
use mimlys_core::ids::TaskId;
use mimlys_core::limits::{LIM_I5_MESSAGE_CHARS, LIM_S4_OUTPUT_BYTES, PROPOSED_RESULT_TOTAL_CHARS};
use mimlys_core::observation::Notice;
use mimlys_core::spi::{BoxFuture, Cancellation, ExecutionContext, ExecutionOutcome, ToolProvider};
use mimlys_core::tool::{ImportRecord, ToolDefinition};
use mimlys_core::validate::AuthorizedAction;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

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
                Err(e) => (ServerInfo::default(), vec![], Some(e.reason())),
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
                // The names and the reasons come from the server: shown cleaned.
                match imported.record.unavailable {
                    Some(reason) => unavailable.push((shown(&imported.name), shown(&reason))),
                    None => tools.push(shown(&imported.name)),
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

/// Text that came from a server, as `status` shows it: one line, cleaned of
/// invisible and control characters, and bounded.
fn shown(text: &str) -> String {
    mimlys_core::panic_text::bounded_message(text, None)
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
        full_payload: None,
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
    // `restart_limit_reached` is a refusal to restart, not one more death:
    // `server.stopped` and `status` keep `died` for a death (external design
    // §11.1); it keeps the last start's reason when that start failed.
    unavailable(&error.reason(), metrics)
}

/// The external tools of one task.
pub struct McpTools {
    task_id: TaskId,
    servers: Arc<Servers>,
    /// Imported at creation, by full name.
    imported: BTreeMap<String, Imported>,
    /// Tools unpinned for this task (RFC 009 §7).
    unpinned: Mutex<BTreeSet<String>>,
    /// The servers whose relisting failed at the step's boundary, with the
    /// reason, until the next boundary: a call to one of them is
    /// `tool_unavailable` with that reason without asking the server again, so
    /// that a server that does not answer costs one wait (LIM-O6) in a step.
    boundary_failures: Mutex<BTreeMap<String, String>>,
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
            boundary_failures: Mutex::new(BTreeMap::new()),
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

    /// Maps a call outcome to the execution outcome the loop records. The
    /// validation of a result runs on a thread of its own, within `remaining`,
    /// the time the call's deadline has left.
    async fn map_outcome(
        &self,
        name: &str,
        outcome: CallOutcome,
        wall_ms: i64,
        remaining: Duration,
        redactor: &Redactor,
    ) -> ExecutionOutcome {
        let metrics = ExecutionMetrics {
            wall_ms,
            ..ExecutionMetrics::default()
        };
        let failed = |failure: FailureCause, payload: Value, mut notices: Vec<Notice>| {
            let (payload, full_payload) = bounded(payload, &mut notices);
            ExecutionOutcome {
                termination: Termination::Exited { exit_code: 1 },
                failure: Some(failure),
                payload,
                full_payload,
                stdout: Vec::new(),
                stderr: Vec::new(),
                notices,
                metrics: metrics.clone(),
            }
        };
        match outcome {
            CallOutcome::Timeout => self.map_outcome_timeout(metrics),
            // Nothing was sent (the pin check answers this before the result is
            // mapped; it is what a call to a changed definition gives).
            CallOutcome::DefinitionChanged => ExecutionOutcome::tool_error(
                "definition_changed",
                json!({"hint": "a new task pins the new definition"}),
                wall_ms,
            ),
            CallOutcome::Cancelled | CallOutcome::Killed => ExecutionOutcome {
                termination: Termination::Cancelled,
                failure: None,
                payload: Value::Null,
                full_payload: None,
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
                    full_payload: None,
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
                // The blocks that are not the payload, when there is no structured
                // content to take it from: all but the first text block.
                let omitted_blocks = if structured.is_none() {
                    content.len().saturating_sub(usize::from(text.is_some()))
                } else {
                    0
                };
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
                // With no structured content only the first text block is passed
                // on: any other block is dropped, and the observation says how
                // many (REQ-CTX-006: no content is dropped without a word).
                if omitted_blocks > 0 {
                    notices.push(Notice::ContentOmitted {
                        count: omitted_blocks as u64,
                    });
                }
                // Cleaned whole, validated, and only then bounded: a cut never
                // makes a result `output_invalid` (handoff 009i, A-26).
                let mut cleaned = Cleaned::default();
                bound::clean(&mut payload, redactor, &mut cleaned);
                if cleaned.removed > 0 {
                    notices.push(Notice::CharactersRemoved {
                        count: cleaned.removed,
                    });
                }
                mimlys_core::observation::add_secret_notices(&mut notices, cleaned.found);
                if is_error {
                    let kind: String = payload
                        .pointer("/error/kind")
                        .and_then(Value::as_str)
                        .unwrap_or("error")
                        .chars()
                        .take(LIM_I5_MESSAGE_CHARS)
                        .collect();
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
                if let Some(output) = self.imported.get(name).and_then(|i| i.output.clone()) {
                    if remaining.is_zero() {
                        // The call's deadline has passed: nothing is checked or passed.
                        return self.map_outcome_timeout(metrics);
                    }
                    // Off the async executor: the walk is bounded by a count of
                    // visits, but it is CPU work all the same.
                    let walk = tokio::task::spawn_blocking(move || {
                        let verdict = output.validate(&payload);
                        (verdict, payload)
                    });
                    match tokio::time::timeout(remaining, walk).await {
                        Ok(Ok((Ok(()), checked))) => payload = checked,
                        Ok(Ok((Err(v), _))) => {
                            tracing::warn!(target: "mimlys_mcp", tool = name, "{v}");
                            return failed(
                                FailureCause::OutputInvalid,
                                json!({ "path": v.path, "reason": v.reason }),
                                notices,
                            );
                        }
                        // The validation itself failed: the result is not passed.
                        Ok(Err(_)) => {
                            return failed(
                                FailureCause::OutputInvalid,
                                json!({ "path": "$", "reason": "the result could not be checked" }),
                                notices,
                            );
                        }
                        // The call's deadline passed while the result was checked.
                        Err(_) => return self.map_outcome_timeout(metrics),
                    }
                }
                let (payload, full_payload) = bounded(payload, &mut notices);
                ExecutionOutcome {
                    termination: Termination::Exited { exit_code: 0 },
                    failure: None,
                    payload,
                    full_payload,
                    stdout: Vec::new(),
                    stderr: Vec::new(),
                    notices,
                    metrics,
                }
            }
        }
    }
}

impl McpTools {
    /// What a call gives when the definition it is pinned with is not the
    /// server's: the tool is unpinned for this task and the model is told to
    /// start a new one. Nothing was sent.
    fn definition_changed(&self, name: &str, started: tokio::time::Instant) -> ExecutionOutcome {
        self.unpinned
            .lock()
            .expect("unpinned lock")
            .insert(name.to_owned());
        ExecutionOutcome::tool_error(
            "definition_changed",
            json!({"hint": "a new task pins the new definition"}),
            started.elapsed().as_millis() as i64,
        )
    }

    /// What a call that passed its deadline gives.
    fn map_outcome_timeout(&self, metrics: ExecutionMetrics) -> ExecutionOutcome {
        ExecutionOutcome {
            termination: Termination::Deadline,
            failure: Some(FailureCause::Timeout),
            payload: Value::Null,
            full_payload: None,
            stdout: Vec::new(),
            stderr: Vec::new(),
            notices: vec![],
            metrics,
        }
    }
}

/// A payload as the model reads it, and the whole result when that is a cut of
/// it (handoff 009i, A-26; REQ-MCP-006, REQ-CTX-006): the serialized payload is
/// bounded in total by cutting its longest strings first, and the cuts are said
/// once, in one `content_truncated` with its total. The whole result is kept
/// for the user, within LIM-S4 (REQ-PERS-005), and never shown to the model.
/// What is cut is cut after the output schema was checked.
fn bounded(payload: Value, notices: &mut Vec<Notice>) -> (Value, Option<Value>) {
    let Some(cut) = bound::bound(&payload, PROPOSED_RESULT_TOTAL_CHARS, Unit::Chars, true) else {
        return (payload, None);
    };
    notices.push(Notice::ContentTruncated {
        total_chars: cut.total_chars,
    });
    let full = match bound::bound(&payload, LIM_S4_OUTPUT_BYTES as usize, Unit::Bytes, false) {
        Some(whole) => whole.value,
        None => payload,
    };
    (cut.value, Some(full))
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

    fn refresh_pins(
        &self,
        pinned: &BTreeMap<String, ImportRecord>,
        interruption: &Cancellation,
    ) -> Vec<String> {
        let mut changed = Vec::new();
        // A new boundary: what the last one found is forgotten.
        self.boundary_failures
            .lock()
            .expect("boundary failures lock")
            .clear();
        for server in self.servers.all() {
            if !server.is_dirty() {
                continue;
            }
            let handle = self.handle.clone();
            // On this task's behalf: a restart the relisting makes counts
            // against the task (LIM-X3).
            let current = match tokio::task::block_in_place(|| {
                handle.block_on(server.list_for(self.task_id, interruption))
            }) {
                Ok((_, _, h)) => h,
                // The server cannot serve now (the restart limit is reached,
                // it could not be started, or it did not answer the listing in
                // time): its definitions did not change as far as is known. The
                // tools stay pinned, the list stays marked stale, and a call is
                // refused as `tool_unavailable` with this reason until the next
                // boundary, without the server being asked, and waited for, again.
                Err(error) => {
                    self.boundary_failures
                        .lock()
                        .expect("boundary failures lock")
                        .insert(server.name().to_owned(), error.reason());
                    continue;
                }
            };
            for (name, record) in pinned {
                if record.server != server.name() || record.unavailable.is_some() {
                    continue;
                }
                let tool = name.split_once('.').map(|(_, t)| t).unwrap_or(name);
                if current.get(tool) != Some(&record.hash) {
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
            // A server the boundary could not list is not asked again in this
            // step: one wait, not two (the reason is the boundary's).
            let failed = self
                .boundary_failures
                .lock()
                .expect("boundary failures lock")
                .get(server.name())
                .cloned();
            if let Some(reason) = failed {
                let metrics = ExecutionMetrics {
                    wall_ms: started.elapsed().as_millis() as i64,
                    ..ExecutionMetrics::default()
                };
                return unavailable(&reason, metrics);
            }
            // Second line of pinning (RFC 009 §7): the hash at call time.
            // The listing is made for this task: a restart it makes counts
            // against the task (LIM-X3).
            let pinned = self.imported.get(&name).map(|i| i.record.hash.clone());
            if let Some(record) = self.imported.get(&name).map(|i| &i.record) {
                // Nothing has been sent yet, so this wait observes the first stop
                // request (`start_gate`), not the token of a call in flight: the
                // second signal's token, for an action that cannot be undone.
                let current = match server
                    .current_hash(self.task_id, tool, &context.start_gate)
                    .await
                {
                    Ok(current) => current,
                    // The first stop request ended the wait: nothing was sent, so
                    // the action is not begun and the task pauses. Not
                    // `tool_unavailable`, which halts a task whose action is not
                    // idempotent (external design §7.5, §19.2).
                    Err(error) if error.was_cancelled() && context.start_gate.is_cancelled() => {
                        return ExecutionOutcome::not_started(started.elapsed().as_millis() as i64);
                    }
                    // The server cannot serve (it could not start, or its restart
                    // limit is reached for this task): the call is not made, and
                    // the tool is not unpinned for a definition that did not change.
                    Err(error) => {
                        return map_server_error(error, started.elapsed().as_millis() as i64);
                    }
                };
                if current.as_ref() != Some(&record.hash) {
                    return self.definition_changed(&name, started);
                }
            }
            // The pinned hash is compared once more on the connection the call
            // is sent on: a restart since the check may have changed it.
            let outcome = server
                .call(
                    self.task_id,
                    tool,
                    action.arguments().clone(),
                    pinned.as_ref(),
                    context.deadline_ms,
                    &context.start_gate,
                    &context.cancel,
                )
                .await;
            let wall_ms = started.elapsed().as_millis() as i64;
            // The wait for the server to be started again, made before the
            // request is sent, ended at the first stop request: nothing was
            // sent, so the action is not begun and the task pauses, as for the
            // pin check above. Not `tool_unavailable`, which halts a task whose
            // action is not idempotent (external design §7.5, §19.2).
            if let Err(error) = &outcome
                && error.was_cancelled()
                && context.start_gate.is_cancelled()
            {
                return ExecutionOutcome::not_started(wall_ms);
            }
            if matches!(outcome, Ok(CallOutcome::DefinitionChanged)) {
                return self.definition_changed(&name, started);
            }
            match outcome {
                Ok(o) => {
                    let deadline = Duration::from_millis(context.deadline_ms.max(1) as u64);
                    self.map_outcome(
                        &name,
                        o,
                        wall_ms,
                        deadline.saturating_sub(started.elapsed()),
                        &context.redactor,
                    )
                    .await
                }
                Err(error) => map_server_error(error, wall_ms),
            }
        })
    }
}

#[cfg(test)]
mod tests;
