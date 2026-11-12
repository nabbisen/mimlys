//! `status` reads the engine's context length and warns when it cannot hold a
//! request (REQ-MDL-003; handoff 012c; limits LIM-C1, LIM-M2, LIM-M4), against
//! the mock server shaped as llama.cpp (`GET /props`) and as Ollama
//! (`POST /api/show`): the length is shown, `unknown` where the engine reports
//! none, a small one warns with the remedy for that engine and the run goes
//! on, a sufficient one does not warn.

use mimlys_model::mock::{MockConfig, MockServer, ToolsSupport};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::process::Command;

mod common;
use common::run_bounded;

/// The need under the default budget (LIM-C1 32 000 characters at 3.35 per
/// token, plus LIM-M2 4 096 tokens) in the `tool_calls` transport.
const NEED_TOOL_CALLS: u64 = 13_649;
/// The same budget at 4.32 characters per token in the `envelope` transport.
const NEED_ENVELOPE: u64 = 11_504;

struct Env {
    data: tempfile::TempDir,
    config: PathBuf,
}

impl Env {
    fn new(backend: &str) -> Self {
        let data = tempfile::tempdir().unwrap();
        let config = data.path().join("config.toml");
        std::fs::write(
            &config,
            format!(
                "config_version = 1\n[runtime]\ndata_dir = {:?}\n[backend]\n{backend}\n",
                data.path().join("data").to_string_lossy(),
            ),
        )
        .unwrap();
        Self { data, config }
    }

    fn run(&self, args: &[&str]) -> (i32, String, String) {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_mimlys"));
        cmd.arg("--config")
            .arg(&self.config)
            .args(args)
            .env("MIMLYS_CONFIG", "")
            .env("MIMLYS_WORKER_BIN", env!("CARGO_BIN_EXE_mimlys-worker"));
        let out = run_bounded(&mut cmd, self.data.path());
        (out.code(), out.stdout, out.stderr)
    }

    fn json(&self, args: &[&str]) -> (i32, Value) {
        let mut all = vec!["--json"];
        all.extend_from_slice(args);
        let (code, stdout, stderr) = self.run(&all);
        let value = serde_json::from_str(stdout.trim())
            .unwrap_or_else(|e| panic!("not JSON ({e}): {stdout}\n{stderr}"));
        (code, value)
    }
}

fn backend(server: &MockServer, transport: &str) -> String {
    format!(
        "kind = \"openai_compatible\"\nurl = {:?}\nmodel = \"test-model\"\ndecision_transport = \"{transport}\"",
        server.url()
    )
}

/// An engine that accepts `tools` (so `auto` is `tool_calls`) and answers as llama.cpp does.
fn llama_cpp(n_ctx: u64) -> MockConfig {
    MockConfig {
        tools: ToolsSupport::Accept,
        props: Some(json!({
            "default_generation_settings": {"id": 0, "n_ctx": n_ctx},
            "total_slots": 1,
            "model_path": "model.gguf",
        })),
        ..MockConfig::default()
    }
}

/// An engine that accepts `tools` and answers as Ollama does; `parameters` is
/// the text of the model's Modelfile parameters.
fn ollama(parameters: &str) -> MockConfig {
    MockConfig {
        tools: ToolsSupport::Accept,
        api_show: Some(json!({
            "modelfile": "FROM model.gguf",
            "parameters": parameters,
            "details": {"family": "qwen2"},
        })),
        ..MockConfig::default()
    }
}

/// An engine that reports its context nowhere.
fn silent() -> MockConfig {
    MockConfig {
        tools: ToolsSupport::Accept,
        ..MockConfig::default()
    }
}

fn warned(text: &str) -> bool {
    text.lines()
        .any(|l| l.starts_with("warning: the engine's context"))
}

/// REQ-MDL-003: llama.cpp-shaped `/props`: the per-slot `n_ctx` is read inside
/// the probe and shown, in JSON and in text, with the need; it is large
/// enough, so there is no warning.
#[test]
fn req_mdl_003_llama_cpp_props_context_is_shown() {
    let server = MockServer::start(llama_cpp(16_384), vec![]);
    let env = Env::new(&backend(&server, "auto"));
    let (code, status) = env.json(&["status"]);
    assert_eq!(code, 0, "{status}");
    assert_eq!(status["backend"]["context_tokens"], 16_384);
    assert_eq!(status["backend"]["context_engine"], "llama_cpp");
    assert_eq!(status["backend"]["context_needed_tokens"], NEED_TOOL_CALLS);
    let (code, text, _) = env.run(&["status"]);
    assert_eq!(code, 0, "{text}");
    assert!(
        text.contains(&format!(
            "context: 16384 tokens (needs about {NEED_TOOL_CALLS})"
        )),
        "{text}"
    );
    assert!(!warned(&text), "{text}");
}

/// REQ-MDL-003: Ollama-shaped `/api/show`: the model's `num_ctx` is read
/// and shown; a Modelfile that sets none gives Ollama's default of 4 096.
#[test]
fn req_mdl_003_ollama_show_context_is_shown() {
    let server = MockServer::start(
        ollama("stop \"<|im_end|>\"\nnum_ctx                        16384\n"),
        vec![],
    );
    let env = Env::new(&backend(&server, "auto"));
    let (code, status) = env.json(&["status"]);
    assert_eq!(code, 0, "{status}");
    assert_eq!(status["backend"]["context_tokens"], 16_384);
    assert_eq!(status["backend"]["context_engine"], "ollama");
    let (_, text, _) = env.run(&["status"]);
    assert!(
        text.contains(&format!(
            "context: 16384 tokens (needs about {NEED_TOOL_CALLS})"
        )),
        "{text}"
    );
    assert!(!warned(&text), "{text}");
    // The request carried the model's name.
    let shown: Vec<_> = server
        .requests()
        .into_iter()
        .filter(|r| r.line.starts_with("POST /api/show"))
        .collect();
    assert!(!shown.is_empty());
    assert_eq!(shown[0].body["model"], "test-model");

    // No `num_ctx` in the parameters: Ollama's default.
    let server = MockServer::start(ollama("stop \"<|im_end|>\""), vec![]);
    let env = Env::new(&backend(&server, "auto"));
    let (_, status) = env.json(&["status"]);
    assert_eq!(status["backend"]["context_tokens"], 4096);
    assert_eq!(status["backend"]["context_engine"], "ollama");
}

/// REQ-MDL-003: an engine that reports nothing gives `context: unknown`, never
/// a guess, and no warning.
#[test]
fn req_mdl_003_unreported_context_is_unknown() {
    let server = MockServer::start(silent(), vec![]);
    let env = Env::new(&backend(&server, "auto"));
    let (code, status) = env.json(&["status"]);
    assert_eq!(code, 0, "{status}");
    assert_eq!(status["backend"]["context_tokens"], Value::Null);
    assert!(status["backend"].get("context_engine").is_none());
    assert_eq!(status["backend"]["context_needed_tokens"], NEED_TOOL_CALLS);
    let (_, text, _) = env.run(&["status"]);
    assert!(
        text.contains(&format!("context: unknown (needs about {NEED_TOOL_CALLS})")),
        "{text}"
    );
    assert!(!warned(&text), "{text}");

    // A body that is not an engine's answer is not read as one either.
    let server = MockServer::start(
        MockConfig {
            props: Some(json!({"status": "ok"})),
            api_show: Some(json!({"unrelated": true})),
            ..silent()
        },
        vec![],
    );
    let env = Env::new(&backend(&server, "auto"));
    let (_, status) = env.json(&["status"]);
    assert_eq!(status["backend"]["context_tokens"], Value::Null);
}

/// REQ-MDL-003: llama.cpp with a context below the need warns with one line
/// naming `-c`; the exit code stays 0 (the run goes on).
#[test]
fn req_mdl_003_small_llama_cpp_context_warns_with_the_remedy() {
    let server = MockServer::start(llama_cpp(4096), vec![]);
    let env = Env::new(&backend(&server, "auto"));
    let (code, text, _) = env.run(&["status"]);
    assert_eq!(code, 0, "{text}");
    assert!(
        text.contains(&format!(
            "context: 4096 tokens (needs about {NEED_TOOL_CALLS})"
        )),
        "{text}"
    );
    let warnings: Vec<&str> = text
        .lines()
        .filter(|l| l.starts_with("warning: the engine's context"))
        .collect();
    assert_eq!(warnings.len(), 1, "{text}");
    assert!(
        warnings[0].contains("4096 tokens") && warnings[0].contains(&NEED_TOOL_CALLS.to_string()),
        "{text}"
    );
    assert!(warnings[0].contains("`-c 16384`"), "{text}");
    assert!(!warnings[0].contains("num_ctx"), "{text}");
}

/// REQ-MDL-003: Ollama with a context below the need (its default of 4 096)
/// warns with one line naming `num_ctx` and the Modelfile; the exit code stays 0.
#[test]
fn req_mdl_003_small_ollama_context_warns_with_the_remedy() {
    let server = MockServer::start(ollama("stop \"<|im_end|>\""), vec![]);
    let env = Env::new(&backend(&server, "auto"));
    let (code, text, _) = env.run(&["status"]);
    assert_eq!(code, 0, "{text}");
    let warnings: Vec<&str> = text
        .lines()
        .filter(|l| l.starts_with("warning: the engine's context"))
        .collect();
    assert_eq!(warnings.len(), 1, "{text}");
    assert!(warnings[0].contains("PARAMETER num_ctx 16384"), "{text}");
    assert!(warnings[0].contains("Modelfile"), "{text}");
    assert!(!warnings[0].contains("`-c"), "{text}");
}

/// REQ-MDL-003: the need follows the transport in force: a context of 12 000
/// tokens holds a request of the `envelope` transport (4.32 characters per
/// token) and does not hold one of `tool_calls` (3.35); equal to the need
/// does not warn.
#[test]
fn req_mdl_003_the_need_follows_the_transport() {
    // `envelope` configured: the engine would accept `tools`, and is not asked.
    let server = MockServer::start(llama_cpp(12_000), vec![]);
    let env = Env::new(&backend(&server, "envelope"));
    let (_, status) = env.json(&["status"]);
    assert_eq!(status["backend"]["decision_transport"], "envelope");
    assert_eq!(status["backend"]["context_needed_tokens"], NEED_ENVELOPE);
    let (_, text, _) = env.run(&["status"]);
    assert!(!warned(&text), "{text}");

    let env = Env::new(&backend(&server, "tool_calls"));
    let (_, status) = env.json(&["status"]);
    assert_eq!(status["backend"]["decision_transport"], "tool_calls");
    assert_eq!(status["backend"]["context_needed_tokens"], NEED_TOOL_CALLS);
    let (code, text, _) = env.run(&["status"]);
    assert_eq!(code, 0, "{text}");
    assert!(warned(&text), "{text}");

    // Exactly the need: sufficient.
    let server = MockServer::start(llama_cpp(NEED_TOOL_CALLS), vec![]);
    let env = Env::new(&backend(&server, "auto"));
    let (_, text, _) = env.run(&["status"]);
    assert!(!warned(&text), "{text}");
    // One token short: warns.
    let server = MockServer::start(llama_cpp(NEED_TOOL_CALLS - 1), vec![]);
    let env = Env::new(&backend(&server, "auto"));
    let (_, text, _) = env.run(&["status"]);
    assert!(warned(&text), "{text}");
}
