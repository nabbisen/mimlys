//! Adapter tests against the mock server (handoff 008 §4).

use super::*;
use crate::mock::{MockConfig, MockServer, Reply, UsageStyle};
use mimlys_core::limits::{LIM_D1_DECISION_BYTES, LIM_M2_MAX_TOKENS};
use serde_json::json;

mod native;

fn config(url: &str, mode: Mode) -> OpenAiConfig {
    OpenAiConfig {
        url: url.to_owned(),
        model: "test-model".into(),
        api_key: None,
        timeout_ms: 5000,
        max_tokens: LIM_M2_MAX_TOKENS,
        temperature: 0.0,
        structured_output: mode,
        decision_transport: TransportMode::Envelope,
        locality: Locality::Local,
    }
}

fn request() -> GenerateRequest {
    GenerateRequest {
        messages: vec![
            Message::text(Role::System, "sys"),
            Message::text(Role::User, "goal"),
            Message::text(Role::Assistant, "{\"decision\":\"plan\",\"steps\":[]}"),
        ],
        schema_version: 1,
        max_bytes: LIM_D1_DECISION_BYTES,
        deadline_ms: 5000,
        transport: Default::default(),
        functions: vec![],
    }
}

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
}

/// REQ-MDL-003 / REQ-MDL-004 (`schema`): the body carries the messages
/// unchanged, `max_tokens`, `temperature`, `stream: false` and the schema;
/// the content comes back as bytes and usage is recorded.
#[test]
fn req_mdl_003_request_and_response_shape() {
    let server = MockServer::start(
        MockConfig::default(),
        vec![Reply::Decision(
            json!({"decision": "complete", "summary": "done"}),
        )],
    );
    let backend = OpenAiBackend::new(config(server.url(), Mode::Schema));
    let bytes = rt()
        .block_on(backend.generate(request(), Cancellation::new()))
        .unwrap();
    assert_eq!(bytes, br#"{"decision":"complete","summary":"done"}"#);
    let seen = server.requests();
    assert_eq!(seen.len(), 1, "schema mode probes nothing by itself");
    let body = &seen[0].body;
    assert_eq!(body["model"], "test-model");
    assert_eq!(body["max_tokens"], LIM_M2_MAX_TOKENS);
    assert_eq!(body["temperature"], 0.0);
    assert_eq!(body["stream"], false);
    assert_eq!(body["messages"][0]["role"], "system");
    assert_eq!(body["messages"][0]["content"], "sys");
    assert_eq!(body["messages"][2]["role"], "assistant");
    assert_eq!(body["response_format"]["type"], "json_schema");
    assert_eq!(
        body["response_format"]["json_schema"]["name"],
        schema::SCHEMA_NAME
    );
    assert_eq!(body["response_format"]["json_schema"]["strict"], true);
    assert_eq!(
        body["response_format"]["json_schema"]["schema"],
        schema::decision_schema()
    );
    let usage = backend.last_usage().unwrap();
    assert!(usage.prompt_tokens.unwrap() > 0);
    assert!(usage.completion_tokens.unwrap() > 0);
    assert_eq!(usage.cached_tokens, Some(usage.prompt_tokens.unwrap() / 2));
    assert_eq!(
        backend.describe(),
        BackendDescriptor {
            kind: "openai_compatible".into(),
            model: Some("test-model".into()),
            locality: Locality::Local,
        }
    );
}

/// REQ-MDL-004 (`auto`): the probe resolves the mode with one-token
/// completions that consume no script; a server refusing `json_schema`
/// resolves to `json_object`, one refusing both to none, and the mode is
/// reported as fallen back.
#[test]
fn req_mdl_004_auto_resolves_at_probe() {
    let rt = rt();
    // Accepts everything: json_schema, no fallback.
    let server = MockServer::start(MockConfig::default(), vec![Reply::Content("{}".into())]);
    let backend = OpenAiBackend::new(config(server.url(), Mode::Auto));
    assert_eq!(backend.resolved_mode(), None);
    let probe = rt.block_on(backend.probe());
    assert_eq!(
        probe,
        Probe {
            reachable: true,
            model_listed: Some(true),
            structured_output: Some(Resolved::JsonSchema),
            fell_back: false,
            detail: None,
            decision_transport: Some(DecisionTransport::Envelope),
            transport_reason: None,
            parallel_tool_calls: None,
        }
    );
    assert_eq!(server.remaining(), 1, "probe completions consume no script");
    rt.block_on(backend.generate(request(), Cancellation::new()))
        .unwrap();
    let decision = server.decision_requests();
    assert_eq!(decision.len(), 1);
    assert_eq!(decision[0].body["response_format"]["type"], "json_schema");

    // Refuses json_schema: json_object.
    let server = MockServer::start(
        MockConfig {
            refuse_formats: vec!["json_schema".into()],
            ..MockConfig::default()
        },
        vec![Reply::Content("{}".into())],
    );
    let backend = OpenAiBackend::new(config(server.url(), Mode::Auto));
    let probe = rt.block_on(backend.probe());
    assert_eq!(probe.structured_output, Some(Resolved::JsonObject));
    assert!(probe.fell_back);
    assert_eq!(probe.detail, None);
    rt.block_on(backend.generate(request(), Cancellation::new()))
        .unwrap();
    assert_eq!(
        server.decision_requests()[0].body["response_format"]["type"],
        "json_object"
    );

    // Refuses both: none, and the body has no response_format.
    let server = MockServer::start(
        MockConfig {
            refuse_formats: vec!["json_schema".into(), "json_object".into()],
            ..MockConfig::default()
        },
        vec![Reply::Content("{}".into())],
    );
    let backend = OpenAiBackend::new(config(server.url(), Mode::Auto));
    let probe = rt.block_on(backend.probe());
    assert_eq!(probe.structured_output, Some(Resolved::None));
    assert!(probe.fell_back);
    rt.block_on(backend.generate(request(), Cancellation::new()))
        .unwrap();
    assert!(
        server.decision_requests()[0]
            .body
            .get("response_format")
            .is_none()
    );

    // `schema` on a refusing server: the probe says so; `none` never sends the parameter.
    let server = MockServer::start(
        MockConfig {
            refuse_formats: vec!["json_schema".into()],
            ..MockConfig::default()
        },
        vec![Reply::Content("{}".into())],
    );
    let backend = OpenAiBackend::new(config(server.url(), Mode::Schema));
    let probe = rt.block_on(backend.probe());
    assert_eq!(probe.structured_output, None);
    assert_eq!(probe.detail.as_deref(), Some("format_refused"));
    let backend = OpenAiBackend::new(config(server.url(), Mode::None));
    let probe = rt.block_on(backend.probe());
    assert_eq!(probe.structured_output, Some(Resolved::None));
    assert!(!probe.fell_back);
    rt.block_on(backend.generate(request(), Cancellation::new()))
        .unwrap();
    assert!(
        server.decision_requests()[0]
            .body
            .get("response_format")
            .is_none()
    );
}

/// LIM-M4: the probe reports an unlisted model, an unreachable server and
/// a non-JSON answer honestly.
#[test]
fn lim_m4_probe_reports_reachability() {
    let rt = rt();
    let server = MockServer::start(
        MockConfig {
            models: vec!["other".into()],
            ..MockConfig::default()
        },
        vec![],
    );
    let backend = OpenAiBackend::new(config(server.url(), Mode::None));
    let probe = rt.block_on(backend.probe());
    assert!(probe.reachable);
    assert_eq!(probe.model_listed, Some(false));
    assert_eq!(probe.detail.as_deref(), Some("model_not_listed"));
    assert_eq!(probe.structured_output, Some(Resolved::None));

    let backend = OpenAiBackend::new(config("http://127.0.0.1:9/v1", Mode::Auto));
    let probe = rt.block_on(backend.probe());
    assert!(!probe.reachable);
    assert_eq!(probe.structured_output, None);
    assert_eq!(probe.detail.as_deref(), Some("transport"));
}

/// LIM-M4 (handoff 012, correction 2): a configured model name without a tag
/// matches Ollama's `<name>:latest`; a name with a tag matches only exactly.
/// 0.8 compared exactly and reported a false `model_not_listed`.
#[test]
fn lim_m4_an_untagged_name_matches_the_latest_tag() {
    let rt = rt();
    let probe_for = |listed: &str, configured: &str| {
        let server = MockServer::start(
            MockConfig {
                models: vec![listed.into()],
                ..MockConfig::default()
            },
            vec![],
        );
        let backend = OpenAiBackend::new(OpenAiConfig {
            model: configured.into(),
            ..config(server.url(), Mode::None)
        });
        let probe = rt.block_on(backend.probe());
        (probe.model_listed, probe.detail)
    };
    let latest = "qwen2.5-7b-instruct-m4:latest";
    assert_eq!(
        probe_for(latest, "qwen2.5-7b-instruct-m4"),
        (Some(true), None)
    );
    assert_eq!(probe_for(latest, latest), (Some(true), None));
    // A tag matches only exactly.
    for configured in ["qwen2.5-7b-instruct-m4:q4", "qwen2.5-7b-instruct-m4:Latest"] {
        let (listed, detail) = probe_for(latest, configured);
        assert_eq!(listed, Some(false), "{configured}");
        assert_eq!(detail.as_deref(), Some("model_not_listed"));
    }
    assert_eq!(probe_for("qwen2.5-7b-instruct-m4", latest).0, Some(false));
    // The rule, not the usual listing: a tagged name does not take `:latest` on top of its tag.
    assert_eq!(probe_for("m:q4:latest", "m:q4").0, Some(false));
    // llama-server's ids have no tag and match exactly, as before.
    assert_eq!(probe_for("m.gguf", "m.gguf"), (Some(true), None));
    assert_eq!(probe_for("m.gguf", "m").0, Some(false));
    // A different model's `:latest` is not this one; a registry port is not a tag.
    assert_eq!(
        probe_for("other:latest", "qwen2.5-7b-instruct-m4").0,
        Some(false)
    );
    assert_eq!(
        probe_for("localhost:5000/ns/model:latest", "localhost:5000/ns/model").0,
        Some(true)
    );
}

/// REQ-MDL-003: a 5xx or a 429 is `unavailable` after a pause (the loop's
/// identical retry is spaced, not hammered); a 404 is `unavailable` at once;
/// a dropped connection is `unavailable{transport}`; a wrong shape is
/// `unavailable{response_shape}`. No case yields a decision.
#[test]
fn req_mdl_003_failures_are_unavailable() {
    let rt = rt();
    let server = MockServer::start(
        MockConfig::default(),
        vec![
            Reply::Raw {
                status: 503,
                body: "{\"error\":{\"message\":\"loading\"}}".into(),
                headers: vec![],
            },
            Reply::Raw {
                status: 429,
                body: "{}".into(),
                headers: vec![("Retry-After".into(), "1".into())],
            },
            Reply::Raw {
                status: 404,
                body: "{}".into(),
                headers: vec![],
            },
            Reply::Close,
            Reply::Raw {
                status: 200,
                body: "{\"choices\":[]}".into(),
                headers: vec![],
            },
            Reply::Raw {
                status: 200,
                body: "not json".into(),
                headers: vec![],
            },
        ],
    );
    let backend = OpenAiBackend::new(config(server.url(), Mode::None));
    let call = |label: &str| -> (BackendError, std::time::Duration) {
        let started = std::time::Instant::now();
        let err = rt
            .block_on(backend.generate(request(), Cancellation::new()))
            .unwrap_err();
        eprintln!("{label}: {err} after {:?}", started.elapsed());
        (err, started.elapsed())
    };
    let (err, took) = call("503");
    assert!(matches!(err, BackendError::Unavailable { ref detail } if detail == "status:503"));
    assert!(
        took >= BACKOFF,
        "back-off before the loop retries: {took:?}"
    );
    let (err, took) = call("429");
    assert!(matches!(err, BackendError::Unavailable { ref detail } if detail == "status:429"));
    assert!(
        took >= std::time::Duration::from_millis(900),
        "Retry-After honoured: {took:?}"
    );
    let (err, took) = call("404");
    assert!(matches!(err, BackendError::Unavailable { ref detail } if detail == "status:404"));
    assert!(
        took < BACKOFF,
        "no pause for a plain client error: {took:?}"
    );
    let (err, _) = call("close");
    assert!(matches!(err, BackendError::Unavailable { ref detail } if detail == "transport"));
    let (err, _) = call("no choices");
    assert!(matches!(err, BackendError::Unavailable { ref detail } if detail == "response_shape"));
    let (err, _) = call("not json");
    assert!(matches!(err, BackendError::Unavailable { ref detail } if detail == "response_shape"));
    assert!(backend.last_usage().is_none());
}

/// LIM-M1 and REQ-SCHED-004: a slow server is `timeout` at the deadline;
/// cancellation ends the request within a poll interval.
#[test]
fn lim_m1_deadline_and_cancellation() {
    let rt = rt();
    let server = MockServer::start(
        MockConfig::default(),
        vec![
            Reply::Slow(2000, Box::new(Reply::Content("{}".into()))),
            Reply::Slow(2000, Box::new(Reply::Content("{}".into()))),
        ],
    );
    let backend = OpenAiBackend::new(config(server.url(), Mode::None));
    let mut short = request();
    short.deadline_ms = 300;
    let started = std::time::Instant::now();
    let err = rt
        .block_on(backend.generate(short, Cancellation::new()))
        .unwrap_err();
    assert!(matches!(err, BackendError::Timeout), "{err}");
    assert!(started.elapsed() < std::time::Duration::from_millis(1500));

    let cancel = Cancellation::new();
    let flag = cancel.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(150));
        flag.cancel();
    });
    let started = std::time::Instant::now();
    let err = rt
        .block_on(backend.generate(request(), cancel))
        .unwrap_err();
    assert!(matches!(err, BackendError::Cancelled), "{err}");
    assert!(
        started.elapsed() < std::time::Duration::from_millis(600),
        "{:?}",
        started.elapsed()
    );
}

/// REQ-MDL-007 / REQ-SEC-009: the body is read through a bound; content
/// beyond LIM-D1 comes back cut at LIM-D1 + 1 bytes so the strict parser
/// rejects it; an oversized envelope likewise.
#[test]
fn req_mdl_007_output_is_bounded() {
    let rt = rt();
    let huge = "x".repeat(LIM_D1_DECISION_BYTES + 5000);
    let server = MockServer::start(
        MockConfig::default(),
        vec![
            Reply::Content(huge.clone()),
            Reply::Raw {
                status: 200,
                body: format!(
                    "{{\"pad\":\"{}\"}}",
                    "y".repeat(LIM_D1_DECISION_BYTES + ENVELOPE_BYTES + 10)
                ),
                headers: vec![],
            },
        ],
    );
    let backend = OpenAiBackend::new(config(server.url(), Mode::None));
    let bytes = rt
        .block_on(backend.generate(request(), Cancellation::new()))
        .unwrap();
    assert_eq!(bytes.len(), LIM_D1_DECISION_BYTES + 1);
    let mut small = request();
    small.max_bytes = 100;
    let bytes = rt
        .block_on(backend.generate(small, Cancellation::new()))
        .unwrap();
    assert_eq!(bytes.len(), 101);
    assert!(serde_json::from_slice::<Value>(&bytes).is_err());
}

/// REQ-MDL-006: usage in llama-server's `timings` form, and none at all.
#[test]
fn req_mdl_006_usage_forms() {
    let rt = rt();
    let server = MockServer::start(
        MockConfig {
            usage: UsageStyle::LlamaTimings,
            ..MockConfig::default()
        },
        vec![Reply::Content("{}".into())],
    );
    let backend = OpenAiBackend::new(config(server.url(), Mode::None));
    rt.block_on(backend.generate(request(), Cancellation::new()))
        .unwrap();
    let usage = backend.last_usage().unwrap();
    assert!(usage.prompt_tokens.is_some());
    assert!(usage.cached_tokens.is_some());
    let server = MockServer::start(
        MockConfig {
            usage: UsageStyle::None,
            ..MockConfig::default()
        },
        vec![Reply::Content("{}".into())],
    );
    let backend = OpenAiBackend::new(config(server.url(), Mode::None));
    rt.block_on(backend.generate(request(), Cancellation::new()))
        .unwrap();
    assert_eq!(backend.last_usage(), None);
}

/// REQ-CFG-003: the key travels as a bearer token and nowhere else.
#[test]
fn req_cfg_003_bearer_token() {
    let rt = rt();
    let server = MockServer::start(
        MockConfig {
            expect_bearer: Some("sk-test-123456".into()),
            ..MockConfig::default()
        },
        vec![Reply::Content("{}".into()), Reply::Content("{}".into())],
    );
    let mut cfg = config(server.url(), Mode::None);
    cfg.api_key = Some("sk-test-123456".into());
    let backend = OpenAiBackend::new(cfg);
    let probe = rt.block_on(backend.probe());
    assert!(probe.reachable);
    rt.block_on(backend.generate(request(), Cancellation::new()))
        .unwrap();
    for r in server.requests() {
        let auth = r
            .headers
            .iter()
            .find(|(k, _)| k == "authorization")
            .unwrap();
        assert_eq!(auth.1, "Bearer sk-test-123456");
        assert!(!r.body.to_string().contains("sk-test"));
    }
    let backend = OpenAiBackend::new(config(server.url(), Mode::None));
    let err = rt
        .block_on(backend.generate(request(), Cancellation::new()))
        .unwrap_err();
    assert!(matches!(err, BackendError::Unavailable { ref detail } if detail == "status:401"));
}
