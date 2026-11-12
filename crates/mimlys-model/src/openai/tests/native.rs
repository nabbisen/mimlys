//! The `tool_calls` decision transport against the mock server (RFC 012;
//! REQ-MDL-009, REQ-DEC-003).

use super::*;
use crate::mock::ToolsSupport;
use mimlys_core::decision::{Decision, parse_strict};
use mimlys_core::native::{Offer, offer};
use mimlys_core::rejection::RejectionCode;
use mimlys_core::spi::ToolCall;

fn accepts_tools() -> MockConfig {
    MockConfig {
        tools: ToolsSupport::Accept,
        ..MockConfig::default()
    }
}

fn native_config(url: &str, transport: TransportMode) -> OpenAiConfig {
    OpenAiConfig {
        decision_transport: transport,
        ..config(url, Mode::Auto)
    }
}

fn tool(name: &str) -> mimlys_core::tool::ToolDefinition {
    use mimlys_core::action::{EffectKind, EffectView};
    use mimlys_core::tool::{Param, ParamSchema, ParamType, Provider, ResourceProfile};
    mimlys_core::tool::ToolDefinition {
        name: name.into(),
        description: format!("The tool {name}."),
        input: ParamSchema {
            params: vec![Param {
                name: "path".into(),
                ty: ParamType::WorkspacePath,
                optional: false,
                description: "The file.".into(),
            }],
        },
        output: None,
        effect: EffectView {
            kind: EffectKind::WorkspaceRead,
            reversible: true,
            idempotent: true,
        },
        deadline_ms: 1000,
        profile: ResourceProfile::Builtin,
        destination_arguments: vec![],
        provider: Provider::Builtin,
    }
}

fn offered() -> Offer {
    offer(&[tool("read_file"), tool("fs.read_text")])
}

/// A request in the `tool_calls` transport: native turns, no trailing
/// `assistant` message, the functions offered.
fn native_request() -> GenerateRequest {
    GenerateRequest {
        messages: vec![
            Message::text(Role::System, "sys"),
            Message::text(Role::User, "goal"),
            Message::call(ToolCall {
                id: "call_0".into(),
                name: "read_file".into(),
                arguments: "{\"path\":\"a\"}".into(),
            }),
            Message::answer("call_0", "<<observation>>"),
        ],
        schema_version: 1,
        max_bytes: LIM_D1_DECISION_BYTES,
        deadline_ms: 5000,
        transport: DecisionTransport::ToolCalls,
        functions: offered().functions,
    }
}

fn generate_native(
    backend: &OpenAiBackend,
    request: GenerateRequest,
) -> Result<Vec<u8>, BackendError> {
    rt().block_on(backend.generate(request, Cancellation::new()))
}

fn generate_envelope(backend: &OpenAiBackend) {
    rt().block_on(backend.generate(request(), Cancellation::new()))
        .unwrap();
}

/// REQ-MDL-009: HTTP 200 for a request with one function resolves `auto` to
/// `tool_calls`; the probe consumes no script, and `structured_output`, which
/// applies to the envelope transport only, is neither probed nor sent.
#[test]
fn req_mdl_009_probe_200_resolves_auto_to_tool_calls() {
    let server = MockServer::start(
        MockConfig {
            // A server that would refuse every format: never asked.
            refuse_formats: vec!["json_schema".into(), "json_object".into()],
            ..accepts_tools()
        },
        vec![Reply::call("read_file", &json!({"path": "a"}))],
    );
    let backend = OpenAiBackend::new(native_config(server.url(), TransportMode::Auto));
    let probe = rt().block_on(backend.probe());
    assert_eq!(probe.decision_transport, Some(DecisionTransport::ToolCalls));
    assert_eq!(probe.transport_reason, None);
    assert_eq!(probe.parallel_tool_calls, Some(true));
    assert_eq!(probe.structured_output, None, "not used by this transport");
    assert!(!probe.fell_back);
    assert_eq!(probe.detail, None);
    assert_eq!(server.remaining(), 1, "the probe consumes no script");
    let seen = server.requests();
    let probe_request = seen.last().unwrap();
    assert_eq!(
        probe_request.body["tools"].as_array().map(Vec::len),
        Some(1),
        "one request, one function"
    );
    assert_eq!(probe_request.body["max_tokens"], 1);
    assert_eq!(
        seen.iter().filter(|r| r.line.starts_with("POST")).count(),
        1,
        "no structured-output probe"
    );

    let bytes = generate_native(&backend, native_request()).unwrap();
    assert_eq!(
        parse_strict(&bytes).unwrap(),
        Decision::Act {
            tool: "read_file".into(),
            arguments: serde_json::from_str(r#"{"path":"a"}"#).unwrap(),
            reason: None,
        }
    );
    let body = &server.decision_requests()[0].body;
    assert!(
        body.get("response_format").is_none(),
        "structured_output applies to the envelope transport only"
    );
    assert!(body.get("tool_choice").is_none(), "no tool_choice is sent");
    assert_eq!(body["parallel_tool_calls"], false);
    assert_eq!(
        body["tools"],
        mimlys_core::native::tools_json(&offered().functions),
        "the functions travel as `tools`, exactly as the budget counts them"
    );
    assert_eq!(body["messages"][2]["role"], "assistant");
    assert_eq!(body["messages"][2]["tool_calls"][0]["id"], "call_0");
    assert_eq!(
        body["messages"][2]["tool_calls"][0]["function"]["arguments"],
        "{\"path\":\"a\"}"
    );
    assert_eq!(body["messages"][3]["role"], "tool");
    assert_eq!(body["messages"][3]["tool_call_id"], "call_0");
    assert_eq!(body["messages"][3]["content"], "<<observation>>");
}

/// REQ-MDL-009: an engine that refuses `tools` (llama.cpp without jinja:
/// HTTP 500 `tools param requires --jinja flag`) resolves `auto` to
/// `envelope`, with the engine's reason; the structured-output probe then
/// runs as before and no request carries `tools` again.
#[test]
fn req_mdl_009_probe_jinja_refusal_resolves_auto_to_envelope_with_the_reason() {
    let server = MockServer::start(MockConfig::default(), vec![Reply::Content("{}".into())]);
    let backend = OpenAiBackend::new(native_config(server.url(), TransportMode::Auto));
    let probe = rt().block_on(backend.probe());
    assert_eq!(probe.decision_transport, Some(DecisionTransport::Envelope));
    assert_eq!(
        probe.transport_reason.as_deref(),
        Some("HTTP 500: tools param requires --jinja flag")
    );
    assert_eq!(probe.parallel_tool_calls, None);
    assert_eq!(probe.structured_output, Some(Resolved::JsonSchema));
    let state = backend.settled_transport().unwrap();
    assert_eq!(state.transport, DecisionTransport::Envelope);
    // The envelope request is the one of 0.8: no `tools`.
    generate_envelope(&backend);
    let sent = server.decision_requests();
    assert!(sent[0].body.get("tools").is_none());
    assert_eq!(sent[0].body["response_format"]["type"], "json_schema");
}

/// REQ-MDL-009: `tool_calls` configured and refused is `backend_unavailable`
/// with the engine's reason, never a silent fallback: the probe says so, the
/// transport query and every request fail, and no request reaches the engine
/// without `tools`.
#[test]
fn req_mdl_009_configured_tool_calls_refused_is_unavailable_never_a_fallback() {
    let server = MockServer::start(MockConfig::default(), vec![Reply::Content("{}".into())]);
    let backend = OpenAiBackend::new(native_config(server.url(), TransportMode::ToolCalls));
    let probe = rt().block_on(backend.probe());
    assert_eq!(probe.decision_transport, None);
    assert_eq!(probe.detail.as_deref(), Some("tool_calls_refused"));
    assert_eq!(
        probe.transport_reason.as_deref(),
        Some("HTTP 500: tools param requires --jinja flag")
    );
    assert_eq!(probe.structured_output, None);
    let err = rt()
        .block_on(backend.decision_transport(Cancellation::new()))
        .unwrap_err();
    assert!(
        matches!(&err, BackendError::Unavailable { detail }
            if detail == "tool_calls_refused: HTTP 500: tools param requires --jinja flag"),
        "{err:?}"
    );
    let err = generate_native(&backend, native_request()).unwrap_err();
    assert!(matches!(err, BackendError::Unavailable { .. }), "{err:?}");
    assert!(
        server.decision_requests().is_empty(),
        "no decision request went out: neither with nor without tools"
    );
    assert_eq!(server.remaining(), 1, "the script was never consumed");
}

/// REQ-MDL-009, LIM-M4: a probe that times out says the model may still be
/// loading, settles nothing and caches nothing, so the next request asks again.
#[test]
fn req_mdl_009_probe_timeout_says_the_model_may_still_be_loading() {
    let server = MockServer::start(
        MockConfig {
            // Far longer than the probe's deadline below (120 s against 5 s).
            tools_probe_delay_ms: 120_000,
            ..accepts_tools()
        },
        vec![],
    );
    let backend = OpenAiBackend::new(native_config(server.url(), TransportMode::Auto));
    // The deadline covers the model listing too (`reachable` below): 5 s, far
    // above what a local answer takes, so that a slow machine does not time the
    // listing out instead of the probe of the tools.
    let probe = rt().block_on(backend.probe_within(Duration::from_secs(5)));
    assert!(probe.reachable);
    assert_eq!(probe.decision_transport, None);
    assert_eq!(
        probe.detail.as_deref(),
        Some("timeout (the model may still be loading)")
    );
    assert!(backend.settled_transport().is_none());
}

/// REQ-MDL-009: an engine that objects to `parallel_tool_calls` alone is asked
/// again without it, and the parameter is not sent afterwards.
#[test]
fn req_mdl_009_parallel_tool_calls_is_sent_only_where_accepted() {
    let server = MockServer::start(
        MockConfig {
            refuse_parallel_tool_calls: true,
            ..accepts_tools()
        },
        vec![Reply::call("read_file", &json!({"path": "a"}))],
    );
    let backend = OpenAiBackend::new(native_config(server.url(), TransportMode::Auto));
    let probe = rt().block_on(backend.probe());
    assert_eq!(probe.decision_transport, Some(DecisionTransport::ToolCalls));
    assert_eq!(probe.parallel_tool_calls, Some(false));
    generate_native(&backend, native_request()).unwrap();
    assert!(
        server.decision_requests()[0]
            .body
            .get("parallel_tool_calls")
            .is_none()
    );
}

/// REQ-MDL-009: `envelope` configured sends no request with `tools`, not even
/// the probe.
#[test]
fn req_mdl_009_envelope_configured_never_sends_tools() {
    let server = MockServer::start(accepts_tools(), vec![Reply::Content("{}".into())]);
    let backend = OpenAiBackend::new(native_config(server.url(), TransportMode::Envelope));
    let transport = rt()
        .block_on(backend.decision_transport(Cancellation::new()))
        .unwrap();
    assert_eq!(transport, DecisionTransport::Envelope);
    let probe = rt().block_on(backend.probe());
    assert_eq!(probe.decision_transport, Some(DecisionTransport::Envelope));
    generate_envelope(&backend);
    assert!(
        server
            .requests()
            .iter()
            .all(|r| r.body.get("tools").is_none()),
        "no request carried tools"
    );
}

/// Sends one `tool_calls` request against a server that answers with `reply`.
fn reply_bytes(reply: Reply) -> (Vec<u8>, Option<u64>) {
    let server = MockServer::start(accepts_tools(), vec![reply]);
    let backend = OpenAiBackend::new(native_config(server.url(), TransportMode::ToolCalls));
    let bytes = generate_native(&backend, native_request()).unwrap();
    (bytes, backend.last_text_chars())
}

/// REQ-DEC-003: text beside one call is accepted, its length is recorded and
/// the text itself is not part of the decision; a reply without text records none.
#[test]
fn req_dec_003_text_beside_one_call_is_counted_and_not_read() {
    let text = "I will read the file now.";
    let (bytes, chars) = reply_bytes(Reply::call_with_text(
        text,
        "read_file",
        &json!({"path": "a"}),
    ));
    assert_eq!(chars, Some(text.chars().count() as u64));
    let mapped = String::from_utf8(bytes).unwrap();
    assert_eq!(
        mapped,
        r#"{"arguments":{"path":"a"},"decision":"act","tool":"read_file"}"#
    );
    assert!(!mapped.contains("read the file"));
    let (_, chars) = reply_bytes(Reply::call("read_file", &json!({"path": "a"})));
    assert_eq!(chars, None);
}

/// REQ-DEC-003: none, two, an unknown name, `arguments` not an object, an
/// object over LIM-D1, and a call only in the text are `malformed`: the
/// adapter returns no decision, and stage 1 rejects it.
#[test]
fn req_dec_003_every_other_reply_is_malformed() {
    let call = |name: &str, arguments: &str| (name.to_owned(), arguments.to_owned());
    let calls = |calls: Vec<(String, String)>, as_object: bool| Reply::Calls {
        text: None,
        calls,
        arguments_as_object: as_object,
    };
    let huge = format!(r#"{{"path":"{}"}}"#, "a".repeat(LIM_D1_DECISION_BYTES));
    let table: Vec<(&str, Reply)> = vec![
        ("no call, plain text", Reply::Content("I cannot.".into())),
        (
            "two calls",
            calls(
                vec![
                    call("read_file", r#"{"path":"a"}"#),
                    call("read_file", r#"{"path":"b"}"#),
                ],
                false,
            ),
        ),
        (
            "unknown name",
            calls(vec![call("delete_path", r#"{"path":"a"}"#)], false),
        ),
        (
            "the function name with the dot back",
            calls(vec![call("fs.read_text", "{}")], false),
        ),
        (
            "arguments an array",
            calls(vec![call("read_file", r#"["a"]"#)], false),
        ),
        (
            "arguments not text (a JSON object)",
            calls(vec![call("read_file", r#"{"path":"a"}"#)], true),
        ),
        (
            "arguments with trailing text",
            calls(vec![call("read_file", r#"{"path":"a"} ok"#)], false),
        ),
        (
            "arguments over LIM-D1",
            calls(vec![call("read_file", &huge)], false),
        ),
        (
            "a decision envelope written into the text",
            Reply::Content(
                r#"{"decision":"act","tool":"read_file","arguments":{"path":"a"}}"#.into(),
            ),
        ),
        (
            "a <tool_call> written into the text",
            Reply::Content(
                r#"<tool_call>{"name":"read_file","arguments":{"path":"a"}}</tool_call>"#.into(),
            ),
        ),
        (
            "a call in a fenced block in the text",
            Reply::Content(
                "```json\n{\"name\":\"read_file\",\"arguments\":{\"path\":\"a\"}}\n```".into(),
            ),
        ),
        (
            "text beside two calls",
            Reply::Calls {
                text: Some("{\"decision\":\"complete\",\"summary\":\"x\"}".into()),
                calls: vec![
                    call("read_file", r#"{"path":"a"}"#),
                    call("read_file", r#"{"path":"b"}"#),
                ],
                arguments_as_object: false,
            },
        ),
    ];
    for (what, reply) in table {
        let (bytes, _) = reply_bytes(reply);
        let rejection = parse_strict(&bytes)
            .expect_err(&format!("{what}: no decision may come out of this reply"));
        assert_eq!(rejection.code, RejectionCode::Malformed, "{what}");
    }
}

/// REQ-DEC-003: a reply too large to hold is no decision; no prefix of it is read.
#[test]
fn req_dec_003_an_oversized_native_reply_is_no_decision() {
    let big = "x".repeat(LIM_D1_DECISION_BYTES * 4);
    let (bytes, _) = reply_bytes(Reply::Content(big));
    assert!(bytes.is_empty());
}

/// REQ-MDL-009: the transport is asked of the engine once per process, and a
/// `tool_calls` request still reports usage.
#[test]
fn req_mdl_009_the_transport_is_settled_once() {
    let server = MockServer::start(
        accepts_tools(),
        vec![
            Reply::call("read_file", &json!({"path": "a"})),
            Reply::call("read_file", &json!({"path": "b"})),
        ],
    );
    let backend = OpenAiBackend::new(native_config(server.url(), TransportMode::Auto));
    for _ in 0..2 {
        assert_eq!(
            rt().block_on(backend.decision_transport(Cancellation::new()))
                .unwrap(),
            DecisionTransport::ToolCalls
        );
        generate_native(&backend, native_request()).unwrap();
    }
    let probes = server
        .requests()
        .iter()
        .filter(|r| r.line.starts_with("POST") && r.body["max_tokens"] == 1)
        .count();
    assert_eq!(probes, 1);
    assert!(backend.last_usage().is_some());
}
