//! Whether the engine holds the model in memory (REQ-MDL-010; RFC 015 Amendment 1): Ollama's
//! `GET /api/ps` says yes or no, a llama-server always yes (it holds the one model it was started
//! with), any other engine says nothing and the answer is unknown. Against the mock server shaped
//! as each engine; every verdict is a state: what the probe returned, what the mock received.

use super::*;
use crate::mock::ToolsSupport;
use serde_json::json;

fn ollama(ps: serde_json::Value) -> MockConfig {
    MockConfig {
        tools: ToolsSupport::Accept,
        ps: Some(ps),
        ..MockConfig::default()
    }
}

fn probe_of(config: MockConfig) -> (Probe, MockServer) {
    let server = MockServer::start(config, vec![]);
    let backend = OpenAiBackend::new(config_for(server.url()));
    let probe = rt().block_on(backend.probe());
    (probe, server)
}

fn config_for(url: &str) -> OpenAiConfig {
    OpenAiConfig {
        model: "test-model".into(),
        ..config(url, Mode::Auto)
    }
}

/// REQ-MDL-010: Ollama lists the model among those in memory: resident; lists others, or none:
/// not resident (`no`, not `unknown`: the engine answered).
#[test]
fn req_mdl_010_ollama_says_whether_the_model_is_in_memory() {
    let loaded =
        json!({"models": [{"name": "test-model", "model": "test-model", "context_length": 8192}]});
    assert_eq!(probe_of(ollama(loaded)).0.resident, Some(true));
    // A name without a tag is the `latest` tag, as Ollama itself resolves it.
    let tagged = json!({"models": [{"name": "test-model:latest", "model": "test-model:latest"}]});
    assert_eq!(probe_of(ollama(tagged)).0.resident, Some(true));
    let other = json!({"models": [{"name": "other:7b", "model": "other:7b"}]});
    assert_eq!(probe_of(ollama(other)).0.resident, Some(false));
    assert_eq!(
        probe_of(ollama(json!({"models": []}))).0.resident,
        Some(false)
    );
}

/// REQ-MDL-010: a llama-server holds one model, the one it was started with: always resident.
#[test]
fn req_mdl_010_a_llama_server_is_always_resident() {
    let llama = MockConfig {
        tools: ToolsSupport::Accept,
        props: Some(json!({
            "default_generation_settings": {"id": 0, "n_ctx": 16384},
            "total_slots": 1,
        })),
        ..MockConfig::default()
    };
    let (probe, _) = probe_of(llama);
    assert_eq!(probe.context_engine, Some(context::Engine::LlamaCpp));
    assert_eq!(probe.resident, Some(true));
}

/// REQ-MDL-010: another engine says nothing about residence: unknown, never a guess; an answer
/// that is not shaped as Ollama's `/api/ps` (another server's 200) is not read as one; and an
/// engine that is not reachable has no answer either.
#[test]
fn req_mdl_010_another_engine_is_unknown() {
    let (probe, _) = probe_of(MockConfig::default());
    assert!(probe.reachable);
    assert_eq!(probe.resident, None);
    let (probe, _) = probe_of(ollama(json!({"status": "ok"})));
    assert!(probe.reachable);
    assert_eq!(probe.resident, None, "not shaped as Ollama's answer");
    let backend = OpenAiBackend::new(config_for("http://127.0.0.1:9/v1"));
    let probe = rt().block_on(backend.probe());
    assert!(!probe.reachable);
    assert_eq!(probe.resident, None);
}

/// REQ-MDL-010: whether the model is resident is read **before** the probe's own request, which
/// makes Ollama load the model: the first request the engine receives is `GET /api/ps`, and no
/// completion has been asked for when it is answered.
#[test]
fn req_mdl_010_residence_is_read_before_the_probe_loads_the_model() {
    let (_, server) = probe_of(ollama(json!({"models": []})));
    let lines: Vec<String> = server.requests().into_iter().map(|r| r.line).collect();
    let ps = lines
        .iter()
        .position(|l| l.starts_with("GET /api/ps"))
        .expect("the engine was asked");
    let completion = lines
        .iter()
        .position(|l| l.starts_with("POST") && l.contains("/chat/completions"))
        .expect("the probe asks the engine for a completion");
    assert!(ps < completion, "{lines:?}");
}
