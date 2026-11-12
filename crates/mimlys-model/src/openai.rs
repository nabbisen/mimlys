//! The OpenAI-compatible chat-completions adapter (RFC 008; REQ-MDL-003,
//! 004, 006, 007; REQ-SEC-009, REQ-SEC-010): one `POST {url}/chat/completions`
//! per decision with the assembled messages unchanged, a deadline, a
//! completion bound, a structured-output mode resolved once per process at
//! the probe, a bounded read, and usage when the server reports it. It never
//! repairs output and never fabricates a decision: a failure is
//! `unavailable` and the loop decides what follows. Retries of the identical
//! request live in the runtime loop (LIM-M3); this adapter only spaces them
//! after a 429 or a 5xx.
//!
//! Since 0.9 (RFC 012, REQ-MDL-009) a request may ask for the decision as a
//! native function call: the functions travel as `tools`, the earlier
//! decisions as `assistant` and `tool` messages, and the reply's one
//! structured call is mapped onto the decision envelope's bytes
//! (`mimlys_core::native`). A reply with anything else is answered with no
//! decision at all, which stage 1 rejects as `malformed`; a call written into
//! the text is never read.

pub mod context;
pub mod schema;

use mimlys_core::limits::LIM_M4_PROBE_MS;
use mimlys_core::native::{self, DecisionTransport, ReplyCall};
use mimlys_core::spi::{
    BackendDescriptor, BackendError, BoxFuture, Cancellation, GenerateRequest, Locality, Message,
    ModelBackend, Role, Usage,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Mutex;
use std::time::Duration;

/// How often cancellation is polled while a request is in flight.
const CANCEL_POLL: Duration = Duration::from_millis(100);
/// The pause before an `unavailable` result after a 429 or a 5xx, so that
/// the loop's identical retry does not hit the server at once.
const BACKOFF: Duration = Duration::from_millis(250);
/// The longest `Retry-After` honoured.
const MAX_RETRY_AFTER: Duration = Duration::from_secs(5);
/// Bytes of response envelope read beyond the decision bound (REQ-SEC-009).
const ENVELOPE_BYTES: usize = 64 * 1024;

/// The configured structured-output mode (REQ-MDL-004).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// Resolve at the probe: `json_schema`, then `json_object`, then none.
    Auto,
    /// Always send the decision schema; a server that refuses it is unusable.
    Schema,
    /// Never send `response_format`.
    None,
}

/// The configured decision transport (REQ-MDL-009).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportMode {
    /// `tool_calls` when the engine accepts `tools`, else `envelope` with the reason.
    Auto,
    /// Always `tool_calls`; an engine that refuses `tools` is unavailable.
    ToolCalls,
    /// Always the envelope; no request carries `tools`.
    Envelope,
}

/// The transport in force and what was learned while settling it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransportState {
    /// The transport requests use.
    pub transport: DecisionTransport,
    /// Why `auto` chose `envelope`: the engine's own reason.
    pub reason: Option<String>,
    /// Whether the engine accepts `parallel_tool_calls: false` (sent only then).
    pub parallel_tool_calls: bool,
}

/// How the transport was settled.
enum Settled {
    /// In force.
    Resolved(TransportState),
    /// `tool_calls` was configured and the engine refuses `tools`, with its reason.
    ConfiguredRefused(String),
}

/// What the server accepts, resolved once per process.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Resolved {
    /// `response_format = {type: "json_schema", …}`.
    JsonSchema,
    /// `response_format = {type: "json_object"}`.
    JsonObject,
    /// No `response_format`; the strict parser alone.
    None,
}

/// The adapter's configuration, resolved by the caller (the key is a value here).
#[derive(Clone, Debug)]
pub struct OpenAiConfig {
    /// The endpoint, up to and including the API prefix (`http://127.0.0.1:8080/v1`).
    pub url: String,
    /// The model name.
    pub model: String,
    /// The bearer token, resolved.
    pub api_key: Option<String>,
    /// LIM-M1.
    pub timeout_ms: i64,
    /// LIM-M2.
    pub max_tokens: u32,
    /// Sampling temperature.
    pub temperature: f64,
    /// The mode (applies to the `envelope` transport only).
    pub structured_output: Mode,
    /// The decision transport.
    pub decision_transport: TransportMode,
    /// Declared locality.
    pub locality: Locality,
}

/// What the probe found (LIM-M4).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Probe {
    /// `GET {url}/models` answered 2xx.
    pub reachable: bool,
    /// Whether the configured model is listed, when the listing was readable.
    pub model_listed: Option<bool>,
    /// The resolved mode, when resolution ran to an end.
    pub structured_output: Option<Resolved>,
    /// `auto` ended below `json_schema`.
    pub fell_back: bool,
    /// A fixed-vocabulary reason when something was not as configured.
    pub detail: Option<String>,
    /// The decision transport in force, when it was settled.
    pub decision_transport: Option<DecisionTransport>,
    /// Why `auto` resolved to `envelope`: the engine's reason.
    pub transport_reason: Option<String>,
    /// Whether the engine accepts `parallel_tool_calls: false`, when the transport is `tool_calls`.
    pub parallel_tool_calls: Option<bool>,
    /// The engine's context length in tokens, as the engine reports it
    /// (llama.cpp's `/props`, Ollama's `/api/show`); `None` is unknown (REQ-MDL-003).
    pub context_tokens: Option<u64>,
    /// The engine that reported it.
    pub context_engine: Option<context::Engine>,
}

/// The adapter.
pub struct OpenAiBackend {
    client: reqwest::Client,
    config: OpenAiConfig,
    resolved: Mutex<Option<Resolved>>,
    transport: Mutex<Option<TransportState>>,
    last_usage: Mutex<Option<Usage>>,
    last_text_chars: Mutex<Option<u64>>,
}

/// One HTTP exchange's outcome.
enum Exchange {
    /// Status and the body read within the bound.
    Response {
        status: u16,
        body: Vec<u8>,
        retry_after: Option<Duration>,
    },
    /// The body exceeded the bound; what was read.
    Oversized(Vec<u8>),
}

/// Does the listed model id name the configured model? An exact match always
/// does. A configured name without a tag also matches `<name>:latest`, the way
/// Ollama itself resolves it (`/v1/models` lists `qwen2.5:7b-instruct` and
/// `mymodel:latest`, while `mymodel` is accepted in a request). A name with a
/// tag matches only exactly. The tag is what follows a colon in the last path
/// segment, so a registry host with a port (`localhost:5000/ns/model`) is not one.
fn model_matches(listed: &str, configured: &str) -> bool {
    if listed == configured {
        return true;
    }
    let tagged = configured
        .rsplit('/')
        .next()
        .is_some_and(|last| last.contains(':'));
    !tagged && listed.strip_suffix(":latest") == Some(configured)
}

impl OpenAiBackend {
    /// Builds the adapter; nothing is contacted.
    pub fn new(config: OpenAiConfig) -> Self {
        let client = reqwest::Client::builder()
            .no_proxy()
            .build()
            .expect("a default client builds");
        let resolved = match config.structured_output {
            Mode::Auto => None,
            Mode::Schema => Some(Resolved::JsonSchema),
            Mode::None => Some(Resolved::None),
        };
        Self {
            client,
            config,
            resolved: Mutex::new(resolved),
            transport: Mutex::new(None),
            last_usage: Mutex::new(None),
            last_text_chars: Mutex::new(None),
        }
    }

    /// The configuration.
    pub fn config(&self) -> &OpenAiConfig {
        &self.config
    }

    /// The mode in force, once resolved.
    pub fn resolved_mode(&self) -> Option<Resolved> {
        *self.resolved.lock().expect("mode lock")
    }

    fn endpoint(&self, path: &str) -> String {
        format!("{}/{}", self.config.url.trim_end_matches('/'), path)
    }

    fn with_auth(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.config.api_key {
            Some(key) => request.bearer_auth(key),
            None => request,
        }
    }

    fn response_format(resolved: Resolved) -> Option<Value> {
        match resolved {
            Resolved::JsonSchema => Some(json!({
                "type": "json_schema",
                "json_schema": {
                    "name": schema::SCHEMA_NAME,
                    "strict": true,
                    "schema": schema::decision_schema(),
                }
            })),
            Resolved::JsonObject => Some(json!({"type": "json_object"})),
            Resolved::None => None,
        }
    }

    fn body(&self, messages: &[Message], max_tokens: u32, resolved: Resolved) -> Value {
        let messages: Vec<Value> = messages.iter().map(message_json).collect();
        let mut body = json!({
            "model": self.config.model,
            "messages": messages,
            "max_tokens": max_tokens,
            "temperature": self.config.temperature,
            "stream": false,
        });
        if let Some(format) = Self::response_format(resolved) {
            body["response_format"] = format;
        }
        body
    }

    /// The body of a `tool_calls` request: the messages (with their native
    /// turns), the functions as `tools`, `parallel_tool_calls: false` where the
    /// engine accepts it, no `tool_choice` and no `response_format`.
    fn native_body(
        &self,
        messages: &[Message],
        functions: &[native::Function],
        max_tokens: u32,
        parallel_tool_calls: bool,
    ) -> Value {
        let mut body = json!({
            "model": self.config.model,
            "messages": messages.iter().map(message_json).collect::<Vec<Value>>(),
            "tools": native::tools_json(functions),
            "max_tokens": max_tokens,
            "temperature": self.config.temperature,
            "stream": false,
        });
        if parallel_tool_calls {
            body["parallel_tool_calls"] = Value::Bool(false);
        }
        body
    }

    /// Sends one request within `deadline`, reading at most `limit` bytes of
    /// body; cancellation drops the connection within one poll interval.
    async fn exchange(
        &self,
        request: reqwest::RequestBuilder,
        deadline: Duration,
        limit: usize,
        cancel: &Cancellation,
    ) -> Result<Exchange, BackendError> {
        let send = async {
            let response = request.send().await.map_err(|e| transport(&e))?;
            let status = response.status().as_u16();
            let retry_after = response
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.trim().parse::<u64>().ok())
                .map(Duration::from_secs);
            let mut response = response;
            let mut body = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(|e| transport(&e))? {
                body.extend_from_slice(&chunk);
                if body.len() > limit {
                    return Ok(Exchange::Oversized(body));
                }
            }
            Ok(Exchange::Response {
                status,
                body,
                retry_after,
            })
        };
        let watch = async {
            loop {
                if cancel.is_cancelled() {
                    return;
                }
                tokio::time::sleep(CANCEL_POLL).await;
            }
        };
        tokio::select! {
            r = tokio::time::timeout(deadline, send) => match r {
                Ok(r) => r,
                Err(_) => Err(BackendError::Timeout),
            },
            () = watch => Err(BackendError::Cancelled),
        }
    }

    /// A one-token completion with `resolved`'s format: `Ok(true)` accepted,
    /// `Ok(false)` refused for the format, `Err` anything else.
    async fn try_format(
        &self,
        resolved: Resolved,
        deadline: Duration,
        cancel: &Cancellation,
    ) -> Result<bool, BackendError> {
        let messages = [Message {
            role: Role::User,
            call: None,
            call_id: None,
            content: "ok".into(),
        }];
        let body = self.body(&messages, 1, resolved);
        let request = self
            .with_auth(self.client.post(self.endpoint("chat/completions")))
            .json(&body);
        match self
            .exchange(request, deadline, ENVELOPE_BYTES, cancel)
            .await?
        {
            Exchange::Response { status, .. } if (200..300).contains(&status) => Ok(true),
            Exchange::Response { status, body, .. } if status == 400 && refuses_format(&body) => {
                Ok(false)
            }
            Exchange::Response { status, .. } => Err(BackendError::Unavailable {
                detail: format!("status:{status}"),
            }),
            Exchange::Oversized(_) => Err(BackendError::Unavailable {
                detail: "response_shape".into(),
            }),
        }
    }

    /// Resolves `auto` (or verifies `schema`) within `deadline`.
    async fn resolve(
        &self,
        deadline: Duration,
        cancel: &Cancellation,
    ) -> Result<(Resolved, bool), BackendError> {
        if let Some(r) = self.resolved_mode() {
            return Ok((r, false));
        }
        let candidates: &[Resolved] = match self.config.structured_output {
            Mode::Schema => &[Resolved::JsonSchema],
            Mode::None => &[Resolved::None],
            Mode::Auto => &[Resolved::JsonSchema, Resolved::JsonObject, Resolved::None],
        };
        let mut fell_back = false;
        for candidate in candidates {
            let accepted = match candidate {
                Resolved::None => true,
                other => self.try_format(*other, deadline, cancel).await?,
            };
            if accepted {
                *self.resolved.lock().expect("mode lock") = Some(*candidate);
                return Ok((*candidate, fell_back));
            }
            fell_back = true;
        }
        Err(BackendError::Unavailable {
            detail: "format_refused".into(),
        })
    }

    /// One request with one function (`tools` and nothing else beyond a
    /// one-token completion): `Ok(None)` accepted, `Ok(Some(reason))` refused
    /// by the engine with its reason, `Err` anything else (not reachable, not
    /// answering, an error that is not a refusal of `tools`).
    async fn try_tools(
        &self,
        parallel_tool_calls: bool,
        deadline: Duration,
        cancel: &Cancellation,
    ) -> Result<Option<(String, bool)>, BackendError> {
        let messages = [Message::text(Role::User, "ok")];
        let ping = [native::Function {
            name: "ping".into(),
            tool: None,
            description: "Does nothing.".into(),
            parameters: json!({"type": "object", "properties": {}, "additionalProperties": false}),
        }];
        let body = self.native_body(&messages, &ping, 1, parallel_tool_calls);
        let request = self
            .with_auth(self.client.post(self.endpoint("chat/completions")))
            .json(&body);
        match self
            .exchange(request, deadline, ENVELOPE_BYTES, cancel)
            .await?
        {
            Exchange::Response { status, .. } if (200..300).contains(&status) => Ok(None),
            Exchange::Response { status, body, .. } if refuses_tools(status) => {
                let mentions_parallel = String::from_utf8_lossy(&body)
                    .to_ascii_lowercase()
                    .contains("parallel_tool_calls");
                Ok(Some((refusal_reason(status, &body), mentions_parallel)))
            }
            Exchange::Response { status, .. } => Err(BackendError::Unavailable {
                detail: format!("status:{status}"),
            }),
            Exchange::Oversized(_) => Err(BackendError::Unavailable {
                detail: "response_shape".into(),
            }),
        }
    }

    /// Settles the decision transport (REQ-MDL-009) once per process. `auto`
    /// asks the engine with one request carrying one function: HTTP 200 is
    /// `tool_calls`; a refusal (llama.cpp without jinja answers HTTP 500
    /// `tools param requires --jinja flag`) is `envelope` with the reason.
    /// `tool_calls` configured and refused is never a fallback. A timeout or
    /// any other failure settles nothing and is not cached.
    async fn settle_transport(
        &self,
        deadline: Duration,
        cancel: &Cancellation,
    ) -> Result<Settled, BackendError> {
        if let Some(state) = self.transport.lock().expect("transport lock").clone() {
            return Ok(Settled::Resolved(state));
        }
        let state = match self.config.decision_transport {
            TransportMode::Envelope => TransportState {
                transport: DecisionTransport::Envelope,
                reason: None,
                parallel_tool_calls: false,
            },
            configured => {
                let mut parallel = true;
                let mut refusal = self.try_tools(parallel, deadline, cancel).await?;
                // An engine that objects to `parallel_tool_calls` alone is asked again without it.
                if let Some((_, true)) = &refusal {
                    parallel = false;
                    refusal = self.try_tools(parallel, deadline, cancel).await?;
                }
                match (refusal, configured) {
                    (None, _) => TransportState {
                        transport: DecisionTransport::ToolCalls,
                        reason: None,
                        parallel_tool_calls: parallel,
                    },
                    (Some((reason, _)), TransportMode::ToolCalls) => {
                        return Ok(Settled::ConfiguredRefused(reason));
                    }
                    (Some((reason, _)), _) => TransportState {
                        transport: DecisionTransport::Envelope,
                        reason: Some(reason),
                        parallel_tool_calls: false,
                    },
                }
            }
        };
        *self.transport.lock().expect("transport lock") = Some(state.clone());
        Ok(Settled::Resolved(state))
    }

    /// The transport in force, once settled.
    pub fn settled_transport(&self) -> Option<TransportState> {
        self.transport.lock().expect("transport lock").clone()
    }

    /// The startup probe (LIM-M4): the model listing, then the decision
    /// transport, then (for the `envelope` transport only) the structured-output mode.
    pub async fn probe(&self) -> Probe {
        self.probe_within(Duration::from_millis(LIM_M4_PROBE_MS.max(1) as u64))
            .await
    }

    /// [`OpenAiBackend::probe`] under `deadline` (LIM-M4 is the value the product uses).
    /// The engine's context length is read beside the rest, inside the same
    /// bound, and kept only where the engine is reachable.
    pub async fn probe_within(&self, deadline: Duration) -> Probe {
        let (mut probe, context) =
            tokio::join!(self.probe_core(deadline), self.read_context(deadline));
        if probe.reachable
            && let Some((tokens, engine)) = context
        {
            probe.context_tokens = Some(tokens);
            probe.context_engine = Some(engine);
        }
        probe
    }

    /// The engine's context length (REQ-MDL-003), within `deadline`: llama.cpp's
    /// `GET /props`, then Ollama's `POST /api/show`, both beside the API prefix.
    /// Whatever fails, is oversized, or is not shaped as an engine's answer is `None`.
    async fn read_context(&self, deadline: Duration) -> Option<(u64, context::Engine)> {
        let started = tokio::time::Instant::now();
        let cancel = Cancellation::new();
        let base = self.config.url.trim_end_matches('/');
        let base = base.strip_suffix("/v1").unwrap_or(base);
        let request = self.with_auth(self.client.get(format!("{base}/props")));
        if let Ok(Exchange::Response { status, body, .. }) = self
            .exchange(request, deadline, ENVELOPE_BYTES, &cancel)
            .await
            && (200..300).contains(&status)
            && let Some(n) = serde_json::from_slice::<Value>(&body)
                .ok()
                .and_then(|v| context::from_llama_cpp_props(&v))
        {
            return Some((n, context::Engine::LlamaCpp));
        }
        let remaining = deadline.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            return None;
        }
        let request = self
            .with_auth(self.client.post(format!("{base}/api/show")))
            .json(&json!({"model": self.config.model}));
        match self
            .exchange(request, remaining, ENVELOPE_BYTES, &cancel)
            .await
        {
            Ok(Exchange::Response { status, body, .. }) if (200..300).contains(&status) => {
                serde_json::from_slice::<Value>(&body)
                    .ok()
                    .and_then(|v| context::from_ollama_show(&v))
                    .map(|n| (n, context::Engine::Ollama))
            }
            _ => None,
        }
    }

    async fn probe_core(&self, deadline: Duration) -> Probe {
        let started = tokio::time::Instant::now();
        let cancel = Cancellation::new();
        let request = self.with_auth(self.client.get(self.endpoint("models")));
        let (reachable, model_listed, mut detail) = match self
            .exchange(request, deadline, ENVELOPE_BYTES, &cancel)
            .await
        {
            Ok(Exchange::Response { status, body, .. }) if (200..300).contains(&status) => {
                let listed = serde_json::from_slice::<Value>(&body).ok().and_then(|v| {
                    v.get("data").and_then(Value::as_array).map(|models| {
                        models.iter().any(|m| {
                            m.get("id")
                                .and_then(Value::as_str)
                                .is_some_and(|id| model_matches(id, &self.config.model))
                        })
                    })
                });
                let detail = match listed {
                    Some(false) => Some("model_not_listed".to_owned()),
                    _ => None,
                };
                (true, listed, detail)
            }
            Ok(Exchange::Response { status, .. }) => {
                (false, None, Some(format!("status:{status}")))
            }
            Ok(Exchange::Oversized(_)) => (false, None, Some("response_shape".to_owned())),
            Err(BackendError::Timeout) => (false, None, Some("timeout".to_owned())),
            Err(BackendError::Unavailable { detail }) => (false, None, Some(detail)),
            Err(e) => (false, None, Some(e.to_string())),
        };
        let mut probe = Probe {
            reachable,
            model_listed,
            structured_output: None,
            fell_back: false,
            detail: None,
            decision_transport: None,
            transport_reason: None,
            parallel_tool_calls: None,
            context_tokens: None,
            context_engine: None,
        };
        if !reachable {
            probe.detail = detail;
            return probe;
        }
        // The transport first: a request that times out here is an engine
        // still loading its model, and says so (RFC 012).
        let remaining = deadline
            .saturating_sub(started.elapsed())
            .max(Duration::from_millis(1));
        match self.settle_transport(remaining, &cancel).await {
            Ok(Settled::Resolved(state)) => {
                probe.decision_transport = Some(state.transport);
                probe.transport_reason = state.reason;
                if state.transport == DecisionTransport::ToolCalls {
                    probe.parallel_tool_calls = Some(state.parallel_tool_calls);
                }
            }
            Ok(Settled::ConfiguredRefused(reason)) => {
                probe.transport_reason = Some(reason);
                probe.detail = Some("tool_calls_refused".to_owned());
                return probe;
            }
            Err(e) => {
                probe.detail = Some(probe_detail(e));
                return probe;
            }
        }
        // `structured_output` applies to the envelope transport only.
        if probe.decision_transport != Some(DecisionTransport::Envelope) {
            probe.detail = detail;
            return probe;
        }
        let remaining = deadline
            .saturating_sub(started.elapsed())
            .max(Duration::from_millis(1));
        // `schema` is verified here so that a refusing server is a startup error, not a halted step.
        let verified = match self.config.structured_output {
            Mode::Schema => self
                .try_format(Resolved::JsonSchema, remaining, &cancel)
                .await
                .and_then(|accepted| {
                    if accepted {
                        Ok((Resolved::JsonSchema, false))
                    } else {
                        Err(BackendError::Unavailable {
                            detail: "format_refused".into(),
                        })
                    }
                }),
            _ => self.resolve(remaining, &cancel).await,
        };
        match verified {
            Ok((r, fell_back)) => {
                probe.structured_output = Some(r);
                probe.fell_back = fell_back;
            }
            Err(BackendError::Unavailable { detail: d }) if d == "format_refused" => {
                detail = Some("format_refused".to_owned());
                probe.fell_back = true;
            }
            Err(e) => {
                detail.get_or_insert_with(|| probe_detail(e));
            }
        }
        probe.detail = detail;
        probe
    }
}

/// The detail of a probe request that failed: a timeout of a completion
/// request is an engine that may still be loading its model.
fn probe_detail(error: BackendError) -> String {
    match error {
        BackendError::Timeout => "timeout (the model may still be loading)".to_owned(),
        BackendError::Unavailable { detail } => detail,
        other => other.to_string(),
    }
}

/// The statuses by which an engine objects to `tools` in a request it
/// understood: a bad request (Ollama: the model does not support tools), an
/// unprocessable one, llama.cpp without jinja (500), or an unimplemented one.
fn refuses_tools(status: u16) -> bool {
    matches!(status, 400 | 422 | 500 | 501)
}

/// The longest engine reason kept.
const REASON_CHARS: usize = 200;

/// The engine's own reason for a refusal, bounded and made printable: its
/// error message when the body has one, else the first bytes of the body.
fn refusal_reason(status: u16, body: &[u8]) -> String {
    let text = serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|v| {
            v.pointer("/error/message")
                .or_else(|| v.get("error"))
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_else(|| String::from_utf8_lossy(body).into_owned());
    let printable: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let words = printable.split_whitespace().collect::<Vec<_>>().join(" ");
    let bounded: String = words.chars().take(REASON_CHARS).collect();
    if bounded.is_empty() {
        format!("HTTP {status}")
    } else {
        format!("HTTP {status}: {bounded}")
    }
}

/// One message of a request in the OpenAI shape: plain text, an `assistant`
/// message carrying its call, or the `tool` message answering a call.
fn message_json(m: &Message) -> Value {
    match (m.role, &m.call, &m.call_id) {
        (Role::Assistant, Some(call), _) => json!({
            "role": "assistant",
            "content": m.content,
            "tool_calls": [{
                "id": call.id,
                "type": "function",
                "function": {"name": call.name, "arguments": call.arguments},
            }],
        }),
        (Role::Tool, _, Some(id)) => json!({
            "role": "tool",
            "tool_call_id": id,
            "content": m.content,
        }),
        _ => json!({
            "role": match m.role {
                Role::System => "system",
                Role::User => "user",
                Role::Assistant => "assistant",
                Role::Tool => "tool",
            },
            "content": m.content,
        }),
    }
}

fn transport(error: &reqwest::Error) -> BackendError {
    if error.is_timeout() {
        BackendError::Timeout
    } else {
        BackendError::Unavailable {
            detail: "transport".into(),
        }
    }
}

/// Whether a 400 body names the structured-output parameter.
fn refuses_format(body: &[u8]) -> bool {
    let text = String::from_utf8_lossy(body).to_ascii_lowercase();
    [
        "response_format",
        "json_schema",
        "schema",
        "grammar",
        "format",
    ]
    .iter()
    .any(|needle| text.contains(needle))
}

/// Extracts the usage the server reported, in OpenAI's field names or
/// llama-server's `timings`.
fn usage_of(value: &Value) -> Option<Usage> {
    let usage = value.get("usage");
    let prompt = usage
        .and_then(|u| u.get("prompt_tokens"))
        .and_then(Value::as_u64)
        .or_else(|| value.pointer("/timings/prompt_n").and_then(Value::as_u64));
    let completion = usage
        .and_then(|u| u.get("completion_tokens"))
        .and_then(Value::as_u64)
        .or_else(|| {
            value
                .pointer("/timings/predicted_n")
                .and_then(Value::as_u64)
        });
    let cached = usage
        .and_then(|u| u.pointer("/prompt_tokens_details/cached_tokens"))
        .and_then(Value::as_u64)
        .or_else(|| value.pointer("/timings/cache_n").and_then(Value::as_u64));
    if prompt.is_none() && completion.is_none() && cached.is_none() {
        return None;
    }
    Some(Usage {
        prompt_tokens: prompt,
        completion_tokens: completion,
        cached_tokens: cached,
    })
}

impl ModelBackend for OpenAiBackend {
    fn describe(&self) -> BackendDescriptor {
        BackendDescriptor {
            kind: "openai_compatible".into(),
            model: Some(self.config.model.clone()),
            locality: self.config.locality,
        }
    }

    fn generate<'a>(
        &'a self,
        request: GenerateRequest,
        cancel: Cancellation,
    ) -> BoxFuture<'a, Result<Vec<u8>, BackendError>> {
        Box::pin(async move {
            *self.last_usage.lock().expect("usage lock") = None;
            *self.last_text_chars.lock().expect("text lock") = None;
            let deadline = Duration::from_millis(
                request.deadline_ms.min(self.config.timeout_ms).max(1) as u64,
            );
            let native_request = request.transport == DecisionTransport::ToolCalls;
            let (body, limit) = if native_request {
                let state = match self.settle_transport(deadline, &cancel).await? {
                    Settled::Resolved(state) if state.transport == DecisionTransport::ToolCalls => {
                        state
                    }
                    Settled::Resolved(_) => {
                        return Err(BackendError::Unavailable {
                            detail: "transport_mismatch".into(),
                        });
                    }
                    Settled::ConfiguredRefused(reason) => {
                        return Err(tool_calls_refused(&reason));
                    }
                };
                let body = self.native_body(
                    &request.messages,
                    &request.functions,
                    self.config.max_tokens,
                    state.parallel_tool_calls,
                );
                // The arguments travel as a string inside the body: JSON-escaped once more.
                (
                    body,
                    request
                        .max_bytes
                        .saturating_mul(3)
                        .saturating_add(ENVELOPE_BYTES),
                )
            } else {
                let (resolved, _) = self.resolve(deadline, &cancel).await?;
                (
                    self.body(&request.messages, self.config.max_tokens, resolved),
                    request.max_bytes.saturating_add(ENVELOPE_BYTES),
                )
            };
            let http = self
                .with_auth(self.client.post(self.endpoint("chat/completions")))
                .json(&body);
            match self.exchange(http, deadline, limit, &cancel).await? {
                Exchange::Oversized(_) if native_request => {
                    // A reply too large to hold is no decision; no prefix of it is read.
                    tracing::warn!("a tool_calls reply exceeded its bound; no decision");
                    Ok(native::NO_DECISION.to_vec())
                }
                Exchange::Oversized(mut body) => {
                    // REQ-MDL-007: bounded while read; the strict parser rejects the cut.
                    body.truncate(request.max_bytes.saturating_add(1));
                    Ok(body)
                }
                Exchange::Response {
                    status,
                    body,
                    retry_after,
                } if status == 429 || (500..600).contains(&status) => {
                    let pause = retry_after
                        .filter(|_| status == 429)
                        .map(|d| d.min(MAX_RETRY_AFTER))
                        .unwrap_or(BACKOFF);
                    tracing::warn!(
                        status,
                        body_bytes = body.len(),
                        "backend answered an error status"
                    );
                    tokio::time::sleep(pause).await;
                    Err(BackendError::Unavailable {
                        detail: format!("status:{status}"),
                    })
                }
                Exchange::Response { status, .. } if !(200..300).contains(&status) => {
                    Err(BackendError::Unavailable {
                        detail: format!("status:{status}"),
                    })
                }
                Exchange::Response { body, .. } if native_request => {
                    let value: Value =
                        serde_json::from_slice(&body).map_err(|_| BackendError::Unavailable {
                            detail: "response_shape".into(),
                        })?;
                    if !value
                        .pointer("/choices/0/message")
                        .is_some_and(Value::is_object)
                    {
                        return Err(BackendError::Unavailable {
                            detail: "response_shape".into(),
                        });
                    }
                    *self.last_usage.lock().expect("usage lock") = usage_of(&value);
                    Ok(self.read_native_reply(&value, &request.functions))
                }
                Exchange::Response { body, .. } => {
                    let value: Value =
                        serde_json::from_slice(&body).map_err(|_| BackendError::Unavailable {
                            detail: "response_shape".into(),
                        })?;
                    let content = value
                        .pointer("/choices/0/message/content")
                        .and_then(Value::as_str)
                        .ok_or_else(|| BackendError::Unavailable {
                            detail: "response_shape".into(),
                        })?;
                    *self.last_usage.lock().expect("usage lock") = usage_of(&value);
                    let mut bytes = content.as_bytes().to_vec();
                    if bytes.len() > request.max_bytes.saturating_add(1) {
                        bytes.truncate(request.max_bytes.saturating_add(1));
                    }
                    Ok(bytes)
                }
            }
        })
    }

    fn last_usage(&self) -> Option<Usage> {
        *self.last_usage.lock().expect("usage lock")
    }

    fn decision_transport<'a>(
        &'a self,
        cancel: Cancellation,
    ) -> BoxFuture<'a, Result<DecisionTransport, BackendError>> {
        Box::pin(async move {
            let deadline = Duration::from_millis(self.config.timeout_ms.max(1) as u64);
            match self.settle_transport(deadline, &cancel).await? {
                Settled::Resolved(state) => Ok(state.transport),
                Settled::ConfiguredRefused(reason) => Err(tool_calls_refused(&reason)),
            }
        })
    }

    fn last_text_chars(&self) -> Option<u64> {
        *self.last_text_chars.lock().expect("text lock")
    }
}

/// A configured `tool_calls` that the engine refuses (REQ-MDL-009): the
/// backend is unavailable, with the engine's reason; there is no fallback.
fn tool_calls_refused(reason: &str) -> BackendError {
    BackendError::Unavailable {
        detail: format!("tool_calls_refused: {reason}"),
    }
}

impl OpenAiBackend {
    /// Reads a `tool_calls` reply (REQ-DEC-003, amended): the one structured
    /// call, mapped onto the envelope's bytes; anything else is no decision.
    /// The message's text is neither parsed nor kept: only its length is
    /// recorded, and only beside a call. A call written into the text is
    /// never read.
    fn read_native_reply(&self, value: &Value, functions: &[native::Function]) -> Vec<u8> {
        let message = value.pointer("/choices/0/message");
        let calls: Vec<ReplyCall> = message
            .and_then(|m| m.get("tool_calls"))
            .and_then(Value::as_array)
            .map(|calls| {
                calls
                    .iter()
                    .map(|call| ReplyCall {
                        name: call
                            .pointer("/function/name")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                        arguments: call
                            .pointer("/function/arguments")
                            .and_then(Value::as_str)
                            .map(str::to_owned),
                    })
                    .collect()
            })
            .unwrap_or_default();
        if calls.len() == 1 {
            let chars = message
                .and_then(|m| m.get("content"))
                .and_then(Value::as_str)
                .map_or(0, |text| text.chars().count() as u64);
            *self.last_text_chars.lock().expect("text lock") = (chars > 0).then_some(chars);
        }
        match native::map_reply(functions, &calls) {
            Ok(bytes) => bytes,
            Err(problem) => {
                tracing::warn!(
                    problem = problem.as_str(),
                    "a reply carries no valid decision"
                );
                native::NO_DECISION.to_vec()
            }
        }
    }
}

#[cfg(test)]
mod tests;
