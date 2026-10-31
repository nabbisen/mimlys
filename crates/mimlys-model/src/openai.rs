//! The OpenAI-compatible chat-completions adapter (RFC 008; REQ-MDL-003,
//! 004, 006, 007; REQ-SEC-009, REQ-SEC-010): one `POST {url}/chat/completions`
//! per decision with the assembled messages unchanged, a deadline, a
//! completion bound, a structured-output mode resolved once per process at
//! the probe, a bounded read, and usage when the server reports it. It never
//! repairs output and never fabricates a decision: a failure is
//! `unavailable` and the loop decides what follows. Retries of the identical
//! request live in the runtime loop (LIM-M3); this adapter only spaces them
//! after a 429 or a 5xx.

pub mod schema;

use mimlys_core::limits::LIM_M4_PROBE_MS;
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
    /// The mode.
    pub structured_output: Mode,
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
}

/// The adapter.
pub struct OpenAiBackend {
    client: reqwest::Client,
    config: OpenAiConfig,
    resolved: Mutex<Option<Resolved>>,
    last_usage: Mutex<Option<Usage>>,
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
            last_usage: Mutex::new(None),
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
        let messages: Vec<Value> = messages
            .iter()
            .map(|m| {
                json!({
                    "role": match m.role {
                        Role::System => "system",
                        Role::User => "user",
                        Role::Assistant => "assistant",
                    },
                    "content": m.content,
                })
            })
            .collect();
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

    /// The startup probe (LIM-M4): the model listing, then the mode.
    pub async fn probe(&self) -> Probe {
        let deadline = Duration::from_millis(LIM_M4_PROBE_MS.max(1) as u64);
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
                            m.get("id").and_then(Value::as_str) == Some(&self.config.model)
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
        if !reachable {
            return Probe {
                reachable,
                model_listed,
                structured_output: None,
                fell_back: false,
                detail,
            };
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
        let (structured_output, fell_back) = match verified {
            Ok((r, fell_back)) => (Some(r), fell_back),
            Err(BackendError::Unavailable { detail: d }) if d == "format_refused" => {
                detail = Some("format_refused".to_owned());
                (None, true)
            }
            Err(e) => {
                detail.get_or_insert_with(|| match e {
                    BackendError::Timeout => "timeout".to_owned(),
                    BackendError::Unavailable { detail } => detail,
                    other => other.to_string(),
                });
                (None, false)
            }
        };
        Probe {
            reachable,
            model_listed,
            structured_output,
            fell_back,
            detail,
        }
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
            let deadline = Duration::from_millis(
                request.deadline_ms.min(self.config.timeout_ms).max(1) as u64,
            );
            let (resolved, _) = self.resolve(deadline, &cancel).await?;
            let body = self.body(&request.messages, self.config.max_tokens, resolved);
            let http = self
                .with_auth(self.client.post(self.endpoint("chat/completions")))
                .json(&body);
            let limit = request.max_bytes.saturating_add(ENVELOPE_BYTES);
            match self.exchange(http, deadline, limit, &cancel).await? {
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
}

#[cfg(test)]
mod tests;
