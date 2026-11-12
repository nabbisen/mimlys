//! A scripted OpenAI-compatible server on a loopback port, for tests only
//! (feature `mock-server`). It speaks just enough HTTP/1.1 for one request
//! per connection, records every request, and answers `GET /models` and
//! `POST /chat/completions` from a script. A request carrying `tools` is
//! answered as an engine would: refused (the default, as llama.cpp without
//! jinja does), or accepted, in which case a scripted decision is returned as
//! one structured call (RFC 012).

use serde_json::{Value, json};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// One recorded request.
#[derive(Clone, Debug)]
pub struct Recorded {
    /// Method and path, `POST /v1/chat/completions`.
    pub line: String,
    /// Lower-cased header names with their values.
    pub headers: Vec<(String, String)>,
    /// The parsed JSON body, `Null` for none.
    pub body: Value,
}

/// How usage is reported.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UsageStyle {
    /// No `usage` field.
    None,
    /// OpenAI's `usage` with `prompt_tokens_details.cached_tokens`.
    OpenAi,
    /// llama-server's `timings` (`prompt_n`, `predicted_n`, `cache_n`).
    LlamaTimings,
}

/// One scripted answer to a decision request.
#[derive(Clone, Debug)]
pub enum Reply {
    /// A decision object, sent as the message's content (compact JSON).
    Decision(Value),
    /// Arbitrary content text.
    Content(String),
    /// A status with a raw body and extra headers.
    Raw {
        /// HTTP status.
        status: u16,
        /// The body.
        body: String,
        /// Extra headers.
        headers: Vec<(String, String)>,
    },
    /// Wait, then answer with the inner reply.
    Slow(u64, Box<Reply>),
    /// Drop the connection without answering.
    Close,
    /// A reply with structured `tool_calls`, as `(name, arguments text)`
    /// pairs, and text beside them when given. For a request that carries
    /// `tools`; sent as is, so a malformed shape can be scripted.
    Calls {
        /// The message's text, when any.
        text: Option<String>,
        /// The calls: function name and `arguments` string.
        calls: Vec<(String, String)>,
        /// Whether `arguments` is sent as a JSON object instead of a string.
        arguments_as_object: bool,
    },
}

impl Reply {
    /// One structured call of `name` with `arguments`.
    pub fn call(name: &str, arguments: &Value) -> Self {
        Reply::Calls {
            text: None,
            calls: vec![(name.to_owned(), arguments.to_string())],
            arguments_as_object: false,
        }
    }

    /// One structured call with `text` beside it.
    pub fn call_with_text(text: &str, name: &str, arguments: &Value) -> Self {
        Reply::Calls {
            text: Some(text.to_owned()),
            calls: vec![(name.to_owned(), arguments.to_string())],
            arguments_as_object: false,
        }
    }
}

/// How the server answers a request that carries `tools`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolsSupport {
    /// Accepted: the probe gets a completion, a decision request a structured call.
    Accept,
    /// Refused with this status and message (llama.cpp without jinja: 500,
    /// `tools param requires --jinja flag`).
    Refuse {
        /// HTTP status.
        status: u16,
        /// The error message in the body.
        message: String,
    },
}

/// The server's settings.
#[derive(Clone, Debug)]
pub struct MockConfig {
    /// Models listed by `GET /models`.
    pub models: Vec<String>,
    /// `response_format` types answered with a 400 naming the parameter.
    pub refuse_formats: Vec<String>,
    /// Usage style of every successful completion.
    pub usage: UsageStyle,
    /// The bearer token expected, when any (a mismatch is 401).
    pub expect_bearer: Option<String>,
    /// How a request with `tools` is answered; the default refuses, like an
    /// engine without function calling, so that `auto` resolves to `envelope`.
    pub tools: ToolsSupport,
    /// Refuse a request that carries `parallel_tool_calls` (HTTP 400 naming it).
    pub refuse_parallel_tool_calls: bool,
    /// Wait this long before answering the probe request that carries `tools`.
    pub tools_probe_delay_ms: u64,
    /// The body of `GET /props`, as llama.cpp answers it; `None` is a 404, as
    /// any other engine does.
    pub props: Option<Value>,
    /// The body of `POST /api/show`, as Ollama answers it; `None` is a 404.
    pub api_show: Option<Value>,
}

impl Default for MockConfig {
    fn default() -> Self {
        Self {
            models: vec!["test-model".into()],
            refuse_formats: vec![],
            usage: UsageStyle::OpenAi,
            expect_bearer: None,
            tools: ToolsSupport::Refuse {
                status: 500,
                message: "tools param requires --jinja flag".into(),
            },
            refuse_parallel_tool_calls: false,
            tools_probe_delay_ms: 0,
            props: None,
            api_show: None,
        }
    }
}

/// The running server.
pub struct MockServer {
    url: String,
    requests: Arc<Mutex<Vec<Recorded>>>,
    script: Arc<Mutex<std::collections::VecDeque<Reply>>>,
    stop: Arc<AtomicBool>,
}

impl MockServer {
    /// Starts the server with `script` as the decision answers, in order.
    pub fn start(config: MockConfig, script: Vec<Reply>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("loopback bind");
        let port = listener.local_addr().expect("addr").port();
        listener
            .set_nonblocking(true)
            .expect("nonblocking listener");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let script = Arc::new(Mutex::new(script.into()));
        let stop = Arc::new(AtomicBool::new(false));
        let state = State {
            config,
            requests: Arc::clone(&requests),
            script: Arc::clone(&script),
        };
        let stop_flag = Arc::clone(&stop);
        std::thread::spawn(move || {
            while !stop_flag.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        // macOS hands the accepted socket the listener's non-blocking flag; Linux does not.
                        let _ = stream.set_nonblocking(false);
                        let state = state.clone();
                        std::thread::spawn(move || state.serve(stream));
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    // A client that went away before accept, or an interrupted call, is not the end of the server.
                    Err(e)
                        if matches!(
                            e.kind(),
                            std::io::ErrorKind::ConnectionAborted
                                | std::io::ErrorKind::ConnectionReset
                                | std::io::ErrorKind::Interrupted
                        ) =>
                    {
                        continue;
                    }
                    Err(_) => break,
                }
            }
        });
        Self {
            url: format!("http://127.0.0.1:{port}/v1"),
            requests,
            script,
            stop,
        }
    }

    /// The endpoint, API prefix included.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Every request so far.
    pub fn requests(&self) -> Vec<Recorded> {
        self.requests.lock().expect("mock lock").clone()
    }

    /// The decision requests so far (probe completions excluded).
    pub fn decision_requests(&self) -> Vec<Recorded> {
        self.requests()
            .into_iter()
            .filter(|r| {
                r.line.starts_with("POST")
                    && r.line.contains("/chat/completions")
                    && !is_probe(&r.body)
            })
            .collect()
    }

    /// Replies still queued.
    pub fn remaining(&self) -> usize {
        self.script.lock().expect("mock lock").len()
    }

    /// Queues more replies.
    pub fn push(&self, reply: Reply) {
        self.script.lock().expect("mock lock").push_back(reply);
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

#[derive(Clone)]
struct State {
    config: MockConfig,
    requests: Arc<Mutex<Vec<Recorded>>>,
    script: Arc<Mutex<std::collections::VecDeque<Reply>>>,
}

/// The one-token resolution request of the adapter's probe.
fn is_probe(body: &Value) -> bool {
    body.get("max_tokens").and_then(Value::as_u64) == Some(1)
        && body.pointer("/messages/0/content").and_then(Value::as_str) == Some("ok")
}

impl State {
    fn serve(&self, mut stream: TcpStream) {
        let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
        let Some((line, headers, body)) = read_request(&mut stream) else {
            return;
        };
        let body_value: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
        self.requests.lock().expect("mock lock").push(Recorded {
            line: line.clone(),
            headers: headers.clone(),
            body: body_value.clone(),
        });
        if let Some(expected) = &self.config.expect_bearer {
            let auth = headers
                .iter()
                .find(|(k, _)| k == "authorization")
                .map(|(_, v)| v.as_str());
            if auth != Some(&format!("Bearer {expected}")) {
                respond(
                    &mut stream,
                    401,
                    "{\"error\":{\"message\":\"unauthorized\"}}",
                    &[],
                );
                return;
            }
        }
        if line.starts_with("GET ") && line.contains("/models") {
            let data: Vec<Value> = self
                .config
                .models
                .iter()
                .map(|m| json!({"id": m, "object": "model"}))
                .collect();
            respond(
                &mut stream,
                200,
                &json!({"object": "list", "data": data}).to_string(),
                &[],
            );
            return;
        }
        // The engine's own endpoints, beside the API prefix (REQ-MDL-003).
        let served = if line.starts_with("GET /props") {
            self.config.props.as_ref()
        } else if line.starts_with("POST /api/show") {
            self.config.api_show.as_ref()
        } else {
            None
        };
        if let Some(value) = served {
            respond(&mut stream, 200, &value.to_string(), &[]);
            return;
        }
        if !(line.starts_with("POST ") && line.contains("/chat/completions")) {
            respond(
                &mut stream,
                404,
                "{\"error\":{\"message\":\"not found\"}}",
                &[],
            );
            return;
        }
        if let Some(kind) = body_value
            .pointer("/response_format/type")
            .and_then(Value::as_str)
            && self.config.refuse_formats.iter().any(|f| f == kind)
        {
            respond(
                &mut stream,
                400,
                &json!({"error": {"message": format!("response_format type {kind} is not supported")}})
                    .to_string(),
                &[],
            );
            return;
        }
        if body_value.get("tools").is_some() {
            match &self.config.tools {
                ToolsSupport::Refuse { status, message } => {
                    respond(
                        &mut stream,
                        *status,
                        &json!({"error": {"message": message}}).to_string(),
                        &[],
                    );
                    return;
                }
                ToolsSupport::Accept => {}
            }
            if self.config.refuse_parallel_tool_calls
                && body_value.get("parallel_tool_calls").is_some()
            {
                respond(
                    &mut stream,
                    400,
                    &json!({"error": {"message": "unknown parameter: parallel_tool_calls"}})
                        .to_string(),
                    &[],
                );
                return;
            }
            if is_probe(&body_value) && self.config.tools_probe_delay_ms > 0 {
                std::thread::sleep(Duration::from_millis(self.config.tools_probe_delay_ms));
            }
        }
        if is_probe(&body_value) {
            respond(
                &mut stream,
                200,
                &self.completion("{}", &body_value).to_string(),
                &[],
            );
            return;
        }
        let reply = self
            .script
            .lock()
            .expect("mock lock")
            .pop_front()
            .unwrap_or(Reply::Raw {
                status: 500,
                body: "{\"error\":{\"message\":\"script exhausted\"}}".into(),
                headers: vec![],
            });
        self.reply(&mut stream, reply, &body_value);
    }

    fn reply(&self, stream: &mut TcpStream, reply: Reply, request: &Value) {
        match reply {
            Reply::Decision(value) if request.get("tools").is_some() => {
                let (name, arguments) = decision_as_call(&value);
                self.reply(
                    stream,
                    Reply::Calls {
                        text: None,
                        calls: vec![(name, arguments.to_string())],
                        arguments_as_object: false,
                    },
                    request,
                );
            }
            Reply::Calls {
                text,
                calls,
                arguments_as_object,
            } => {
                let calls: Vec<Value> = calls
                    .iter()
                    .enumerate()
                    .map(|(i, (name, arguments))| {
                        let arguments = if arguments_as_object {
                            serde_json::from_str(arguments).unwrap_or(Value::Null)
                        } else {
                            Value::String(arguments.clone())
                        };
                        json!({
                            "id": format!("call_{i}"),
                            "type": "function",
                            "function": {"name": name, "arguments": arguments},
                        })
                    })
                    .collect();
                let mut value = self.completion(text.as_deref().unwrap_or(""), request);
                value["choices"][0]["message"]["tool_calls"] = Value::Array(calls);
                value["choices"][0]["finish_reason"] = json!("tool_calls");
                if text.is_none() {
                    value["choices"][0]["message"]["content"] = Value::Null;
                }
                respond(stream, 200, &value.to_string(), &[]);
            }
            Reply::Decision(value) => {
                let content = serde_json::to_string(&value).expect("serializes");
                respond(
                    stream,
                    200,
                    &self.completion(&content, request).to_string(),
                    &[],
                );
            }
            Reply::Content(content) => {
                respond(
                    stream,
                    200,
                    &self.completion(&content, request).to_string(),
                    &[],
                );
            }
            Reply::Raw {
                status,
                body,
                headers,
            } => respond(stream, status, &body, &headers),
            Reply::Slow(ms, inner) => {
                std::thread::sleep(Duration::from_millis(ms));
                self.reply(stream, *inner, request);
            }
            Reply::Close => {
                let _ = stream.shutdown(std::net::Shutdown::Both);
            }
        }
    }

    fn completion(&self, content: &str, request: &Value) -> Value {
        let message_chars: usize = request
            .get("messages")
            .and_then(Value::as_array)
            .map(|ms| {
                ms.iter()
                    .map(|m| {
                        let content = m
                            .get("content")
                            .and_then(Value::as_str)
                            .map_or(0, |c| c.chars().count());
                        let calls = m
                            .get("tool_calls")
                            .map_or(0, |c| c.to_string().chars().count());
                        content + calls
                    })
                    .sum()
            })
            .unwrap_or(0);
        let tools_chars = request
            .get("tools")
            .map_or(0, |t| t.to_string().chars().count());
        let prompt_chars = message_chars + tools_chars;
        let prompt_tokens = prompt_chars.div_ceil(4) as u64;
        let completion_tokens = content.chars().count().div_ceil(4) as u64;
        let cached = prompt_tokens / 2;
        let mut value = json!({
            "id": "chatcmpl-mock",
            "object": "chat.completion",
            "model": request.get("model").cloned().unwrap_or(Value::Null),
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": content},
                "finish_reason": "stop"
            }]
        });
        match self.config.usage {
            UsageStyle::None => {}
            UsageStyle::OpenAi => {
                value["usage"] = json!({
                    "prompt_tokens": prompt_tokens,
                    "completion_tokens": completion_tokens,
                    "total_tokens": prompt_tokens + completion_tokens,
                    "prompt_tokens_details": {"cached_tokens": cached}
                });
            }
            UsageStyle::LlamaTimings => {
                value["timings"] = json!({
                    "prompt_n": prompt_tokens,
                    "predicted_n": completion_tokens,
                    "cache_n": cached
                });
            }
        }
        value
    }
}

/// A scripted decision as the structured call a model would make: the tool's
/// name with `.` written `__` and its arguments, or the reserved function
/// named like the decision with the decision's one field.
fn decision_as_call(decision: &Value) -> (String, Value) {
    let kind = decision
        .get("decision")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if kind == "act" {
        let tool = decision
            .get("tool")
            .and_then(Value::as_str)
            .unwrap_or_default();
        return (
            tool.replace('.', "__"),
            decision.get("arguments").cloned().unwrap_or(json!({})),
        );
    }
    let mut arguments = decision.clone();
    if let Some(object) = arguments.as_object_mut() {
        object.remove("decision");
    }
    (kind.to_owned(), arguments)
}

/// A parsed request: the request line, the headers and the body.
type Request = (String, Vec<(String, String)>, Vec<u8>);

/// Reads one HTTP/1.1 request: the request line, headers and a
/// `Content-Length` body.
fn read_request(stream: &mut TcpStream) -> Option<Request> {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 4096];
    let header_end = loop {
        if let Some(pos) = find(&buffer, b"\r\n\r\n") {
            break pos;
        }
        let n = stream.read(&mut chunk).ok()?;
        if n == 0 {
            return None;
        }
        buffer.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8_lossy(&buffer[..header_end]).into_owned();
    let mut lines = head.split("\r\n");
    let line = lines.next()?.to_owned();
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| {
            let (k, v) = l.split_once(':')?;
            Some((k.trim().to_ascii_lowercase(), v.trim().to_owned()))
        })
        .collect();
    let length: usize = headers
        .iter()
        .find(|(k, _)| k == "content-length")
        .and_then(|(_, v)| v.parse().ok())
        .unwrap_or(0);
    let mut body = buffer[header_end + 4..].to_vec();
    while body.len() < length {
        let n = stream.read(&mut chunk).ok()?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    body.truncate(length);
    Some((line, headers, body))
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn respond(stream: &mut TcpStream, status: u16, body: &str, headers: &[(String, String)]) {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "Status",
    };
    let mut head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    for (k, v) in headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("\r\n");
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body.as_bytes());
    let _ = stream.flush();
}
