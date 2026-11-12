//! `status` and `run` read the engine's context length and warn when it cannot
//! hold a request (REQ-MDL-003; handoff 012c; limits LIM-C1, LIM-M2, LIM-M4),
//! against the mock server shaped as llama.cpp (`GET /props`) and as Ollama
//! (`GET /api/ps`, `POST /api/show`): the length is shown, `unknown` where the
//! engine reports none, a small one warns with the remedy for that engine and
//! the run goes on, a sufficient one does not warn.

use mimlys_model::mock::{MockConfig, MockServer, Reply, ToolsSupport};
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

/// An engine that accepts `tools` (so `auto` is `tool_calls`) and answers as
/// llama.cpp does: the per-slot `n_ctx` and the number of slots.
fn llama_cpp(n_ctx: u64, slots: u64) -> MockConfig {
    MockConfig {
        tools: ToolsSupport::Accept,
        props: Some(json!({
            "default_generation_settings": {"id": 0, "n_ctx": n_ctx},
            "total_slots": slots,
            "model_path": "model.gguf",
        })),
        ..MockConfig::default()
    }
}

/// `/api/show` as Ollama answers it: `parameters` (the text of the model's
/// parameters) only where a Modelfile sets any; `model_info` holds the trained
/// maximum, which is not the context in force.
fn show(parameters: Option<&str>) -> Value {
    let mut body = json!({
        "modelfile": "FROM model.gguf",
        "details": {"family": "qwen2"},
        "model_info": {"qwen2.context_length": 131_072},
    });
    if let Some(parameters) = parameters {
        body["parameters"] = json!(parameters);
    }
    body
}

/// `/api/ps` as Ollama answers it: the models in memory, with the context in force.
fn ps(loaded: &[(&str, u64)]) -> Value {
    let models: Vec<Value> = loaded
        .iter()
        .map(|(name, n)| json!({"name": name, "model": name, "context_length": n}))
        .collect();
    json!({"models": models})
}

/// An Ollama that accepts `tools`, with the given `/api/ps` and `/api/show` bodies.
fn ollama(ps: Value, show: Value) -> MockConfig {
    MockConfig {
        tools: ToolsSupport::Accept,
        ps: Some(ps),
        api_show: Some(show),
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

fn warnings(text: &str) -> Vec<&str> {
    text.lines()
        .filter(|l| l.starts_with("warning: the engine's context"))
        .collect()
}

fn warned(text: &str) -> bool {
    !warnings(text).is_empty()
}

const GOAL: &str = "read a.txt and report";

fn scope() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "hello\n").unwrap();
    dir
}

fn complete() -> Reply {
    Reply::call("complete", &json!({"summary": "done"}))
}

/// REQ-MDL-003: llama.cpp-shaped `/props`: the per-slot `n_ctx` is read inside
/// the probe and shown, in JSON and in text, with the need; it is large
/// enough, so there is no warning.
#[test]
fn req_mdl_003_llama_cpp_props_context_is_shown() {
    let server = MockServer::start(llama_cpp(16_384, 1), vec![]);
    let env = Env::new(&backend(&server, "auto"));
    let (code, status) = env.json(&["status"]);
    assert_eq!(code, 0, "{status}");
    assert_eq!(status["backend"]["context_tokens"], 16_384);
    assert_eq!(status["backend"]["context_engine"], "llama_cpp");
    assert_eq!(status["backend"]["context_slots"], 1);
    assert_eq!(status["backend"]["context_needed_tokens"], NEED_TOOL_CALLS);
    let (code, text, _) = env.run(&["status"]);
    assert_eq!(code, 0, "{text}");
    assert!(
        text.contains(&format!(
            "context: 16384 tokens per request (needs about {NEED_TOOL_CALLS})"
        )),
        "{text}"
    );
    assert!(!warned(&text), "{text}");
}

/// REQ-MDL-003: Ollama-shaped answers, in the order `/api/ps` (the context in
/// force of the configured model, when it is loaded), then `/api/show`
/// (`num_ctx` of its parameters), then unknown: the loaded value wins over the
/// Modelfile's; another model's entry is not read; the trained maximum in
/// `model_info` is never taken for the context.
#[test]
fn req_mdl_003_ollama_reads_ps_before_show() {
    // Loaded: `/api/ps` is the value in force, whatever the parameters say.
    let server = MockServer::start(
        ollama(
            ps(&[("other:latest", 2048), ("test-model:latest", 4096)]),
            show(Some("num_ctx                        16384\n")),
        ),
        vec![],
    );
    let env = Env::new(&backend(&server, "auto"));
    let (_, status) = env.json(&["status"]);
    assert_eq!(status["backend"]["context_tokens"], 4096);
    assert_eq!(status["backend"]["context_engine"], "ollama");
    assert!(status["backend"].get("context_slots").is_none(), "{status}");
    let shown: Vec<_> = server
        .requests()
        .into_iter()
        .filter(|r| r.line.starts_with("POST /api/show"))
        .collect();
    assert!(
        shown.is_empty(),
        "a loaded model is not asked about its parameters"
    );

    // Not loaded: `num_ctx` from the parameters; the request carries the model's name.
    let server = MockServer::start(
        ollama(
            ps(&[("other:latest", 2048)]),
            show(Some(
                "stop \"<|im_end|>\"\nnum_ctx                        16384\n",
            )),
        ),
        vec![],
    );
    let env = Env::new(&backend(&server, "auto"));
    let (_, status) = env.json(&["status"]);
    assert_eq!(status["backend"]["context_tokens"], 16_384);
    assert_eq!(status["backend"]["context_engine"], "ollama");
    let (_, text, _) = env.run(&["status"]);
    assert!(
        text.contains(&format!(
            "context: 16384 tokens per request (needs about {NEED_TOOL_CALLS})"
        )),
        "{text}"
    );
    assert!(!warned(&text), "{text}");
    let shown: Vec<_> = server
        .requests()
        .into_iter()
        .filter(|r| r.line.starts_with("POST /api/show"))
        .collect();
    assert!(!shown.is_empty());
    assert_eq!(shown[0].body["model"], "test-model");
}

/// REQ-MDL-003: an Ollama that has the model neither loaded nor given a
/// `num_ctx` says nothing about the context (its `model_info` holds the
/// trained maximum): `unknown`, worded for Ollama, with the need; no assumed
/// default and no warning.
#[test]
fn req_mdl_003_ollama_without_num_ctx_and_not_loaded_is_unknown() {
    let server = MockServer::start(ollama(ps(&[]), show(None)), vec![]);
    let env = Env::new(&backend(&server, "auto"));
    let (code, status) = env.json(&["status"]);
    assert_eq!(code, 0, "{status}");
    assert_eq!(status["backend"]["context_tokens"], Value::Null);
    assert_eq!(status["backend"]["context_engine"], "ollama");
    assert_eq!(status["backend"]["context_needed_tokens"], NEED_TOOL_CALLS);
    let (_, text, _) = env.run(&["status"]);
    assert!(
        text.contains(&format!(
            "context: unknown (Ollama sets it when it loads the model; needs about {NEED_TOOL_CALLS})"
        )),
        "{text}"
    );
    assert!(!warned(&text), "{text}");
}

/// REQ-MDL-003: an engine that reports nothing gives `context: unknown`, never
/// a guess, and no warning; a body that is not an engine's answer is not read
/// as one either.
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

    let server = MockServer::start(
        MockConfig {
            props: Some(json!({"status": "ok"})),
            ps: Some(json!({"status": "ok"})),
            api_show: Some(json!({"unrelated": true})),
            ..silent()
        },
        vec![],
    );
    let env = Env::new(&backend(&server, "auto"));
    let (_, status) = env.json(&["status"]);
    assert_eq!(status["backend"]["context_tokens"], Value::Null);
    assert!(status["backend"].get("context_engine").is_none());
}

/// REQ-MDL-003: llama.cpp with a context below the need warns with one line
/// naming `-c`; the exit code stays 0 (the run goes on). With one slot the
/// remedy is `-c` of the figure; the figure is per request.
#[test]
fn req_mdl_003_small_llama_cpp_context_warns_with_the_remedy() {
    let server = MockServer::start(llama_cpp(4096, 1), vec![]);
    let env = Env::new(&backend(&server, "auto"));
    let (code, text, _) = env.run(&["status"]);
    assert_eq!(code, 0, "{text}");
    assert!(
        text.contains(&format!(
            "context: 4096 tokens per request (needs about {NEED_TOOL_CALLS})"
        )),
        "{text}"
    );
    let found = warnings(&text);
    assert_eq!(found.len(), 1, "{text}");
    assert!(
        found[0].contains("4096 tokens per request")
            && found[0].contains(&NEED_TOOL_CALLS.to_string()),
        "{text}"
    );
    assert!(found[0].contains("`-c 16384`"), "{text}");
    assert!(!found[0].contains("--parallel"), "{text}");
    assert!(!found[0].contains("num_ctx"), "{text}");
}

/// REQ-MDL-003: with more than one slot `-c` is shared among them, so the
/// remedy names the whole (`-c` of the figure times the slots, with the
/// per-slot figure) or `--parallel 1` with the figure; `-c 16384` alone would
/// leave 8192 per slot and the warning would stay.
#[test]
fn req_mdl_003_llama_cpp_slots_are_in_the_remedy() {
    // Measured on llama-server build 12110: `-c 8192 --parallel 2` gives 4096 per slot.
    let server = MockServer::start(llama_cpp(4096, 2), vec![]);
    let env = Env::new(&backend(&server, "auto"));
    let (_, status) = env.json(&["status"]);
    assert_eq!(status["backend"]["context_tokens"], 4096);
    assert_eq!(status["backend"]["context_slots"], 2);
    let (code, text, _) = env.run(&["status"]);
    assert_eq!(code, 0, "{text}");
    let found = warnings(&text);
    assert_eq!(found.len(), 1, "{text}");
    assert!(found[0].contains("`-c 32768`"), "{text}");
    assert!(
        found[0].contains("16384 for each of its 2 parallel slots"),
        "{text}"
    );
    assert!(found[0].contains("`--parallel 1 -c 16384`"), "{text}");

    // The remedy works: 32768 over two slots is 16384 per request.
    let server = MockServer::start(llama_cpp(16_384, 2), vec![]);
    let env = Env::new(&backend(&server, "auto"));
    let (_, text, _) = env.run(&["status"]);
    assert!(!warned(&text), "{text}");
}

/// REQ-MDL-003: Ollama with a context below the need warns with one line
/// naming `num_ctx` and the Modelfile; the exit code stays 0.
#[test]
fn req_mdl_003_small_ollama_context_warns_with_the_remedy() {
    let server = MockServer::start(
        ollama(ps(&[("test-model:latest", 4096)]), show(None)),
        vec![],
    );
    let env = Env::new(&backend(&server, "auto"));
    let (code, text, _) = env.run(&["status"]);
    assert_eq!(code, 0, "{text}");
    let found = warnings(&text);
    assert_eq!(found.len(), 1, "{text}");
    assert!(found[0].contains("PARAMETER num_ctx 16384"), "{text}");
    assert!(found[0].contains("Modelfile"), "{text}");
    assert!(!found[0].contains("`-c"), "{text}");
}

/// REQ-MDL-003: the need follows the transport in force: a context of 12 000
/// tokens holds a request of the `envelope` transport (4.32 characters per
/// token) and does not hold one of `tool_calls` (3.35); equal to the need
/// does not warn.
#[test]
fn req_mdl_003_the_need_follows_the_transport() {
    // `envelope` configured: the engine would accept `tools`, and is not asked.
    let server = MockServer::start(llama_cpp(12_000, 1), vec![]);
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

    // Exactly the need: sufficient. One token short: warns.
    let server = MockServer::start(llama_cpp(NEED_TOOL_CALLS, 1), vec![]);
    let env = Env::new(&backend(&server, "auto"));
    let (_, text, _) = env.run(&["status"]);
    assert!(!warned(&text), "{text}");
    let server = MockServer::start(llama_cpp(NEED_TOOL_CALLS - 1, 1), vec![]);
    let env = Env::new(&backend(&server, "auto"));
    let (_, text, _) = env.run(&["status"]);
    assert!(warned(&text), "{text}");
}

/// REQ-MDL-003: `run` warns too, once, on standard error after the probe, and
/// goes on: the task completes and the exit code is 0; standard output (the
/// JSON value) carries no warning. A sufficient context prints none.
#[test]
fn req_mdl_003_run_warns_once_on_stderr_and_goes_on() {
    let server = MockServer::start(llama_cpp(4096, 2), vec![complete()]);
    let env = Env::new(&backend(&server, "auto"));
    let scope = scope();
    let scope = scope.path().to_str().unwrap();
    let (code, stdout, stderr) = env.run(&["--json", "run", GOAL, "--scope", scope]);
    assert_eq!(code, 0, "{stdout}\n{stderr}");
    let value: Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(value["task"]["state"], "completed", "{value}");
    assert_eq!(warnings(&stderr).len(), 1, "{stderr}");
    assert!(stderr.contains("--parallel 1 -c 16384"), "{stderr}");
    assert!(!stdout.contains("warning"), "{stdout}");

    let server = MockServer::start(llama_cpp(16_384, 1), vec![complete()]);
    let env = Env::new(&backend(&server, "auto"));
    let (code, stdout, stderr) = env.run(&["--json", "run", GOAL, "--scope", scope]);
    assert_eq!(code, 0, "{stdout}\n{stderr}");
    assert!(!warned(&stderr), "{stderr}");
}

/// REQ-MDL-003: when the probe could not read an Ollama's context (the model
/// is not loaded and its Modelfile sets no `num_ctx`), `run` reads `/api/ps`
/// once more after the first model reply, when the model is loaded, and warns
/// then if the context is too small (and only then); the reply is not lost.
#[test]
fn req_mdl_003_run_reads_ollama_again_after_the_first_reply() {
    let scope = scope();
    let scope = scope.path().to_str().unwrap();
    let small = MockConfig {
        ps_after_decision: Some(ps(&[("test-model:latest", 4096)])),
        ..ollama(ps(&[]), show(None))
    };
    let server = MockServer::start(small, vec![complete()]);
    let env = Env::new(&backend(&server, "auto"));
    let (code, stdout, stderr) = env.run(&["--json", "run", GOAL, "--scope", scope]);
    assert_eq!(code, 0, "{stdout}\n{stderr}");
    let value: Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(value["task"]["state"], "completed", "{value}");
    let found = warnings(&stderr);
    assert_eq!(found.len(), 1, "{stderr}");
    assert!(
        found[0].contains("4096 tokens per request") && found[0].contains("PARAMETER num_ctx"),
        "{stderr}"
    );

    // Loaded with enough: nothing.
    let large = MockConfig {
        ps_after_decision: Some(ps(&[("test-model:latest", 16_384)])),
        ..ollama(ps(&[]), show(None))
    };
    let server = MockServer::start(large, vec![complete()]);
    let env = Env::new(&backend(&server, "auto"));
    let (code, stdout, stderr) = env.run(&["--json", "run", GOAL, "--scope", scope]);
    assert_eq!(code, 0, "{stdout}\n{stderr}");
    assert!(!warned(&stderr), "{stderr}");

    // The probe read the context: no second read, and one warning.
    let known = MockConfig {
        ps_after_decision: Some(ps(&[("test-model:latest", 2048)])),
        ..ollama(ps(&[("test-model:latest", 4096)]), show(None))
    };
    let server = MockServer::start(known, vec![complete()]);
    let env = Env::new(&backend(&server, "auto"));
    let (_, _, stderr) = env.run(&["--json", "run", GOAL, "--scope", scope]);
    let found = warnings(&stderr);
    assert_eq!(found.len(), 1, "{stderr}");
    assert!(found[0].contains("4096 tokens"), "{stderr}");
}
