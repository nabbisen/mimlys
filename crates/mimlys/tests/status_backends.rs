//! `status` per backend (REQ-MDL-010; RFC 015 decision 3 and Amendment 1; external design §19.2;
//! limit LIM-M4; handoff 015a, item 4), through the binary against mock servers shaped as each
//! engine (llama.cpp's `/props`, Ollama's `/api/ps`, an engine that says nothing).
//!
//! The probes run at once, and that is decided by a state, not by a clock: the mock servers share
//! a rendezvous, and answer a model listing only when every server has received one. A `status`
//! that probed one backend after another would find its first listing unanswered (the rendezvous
//! never meets while it waits), and its first backend `unreachable`.

use mimlys_model::mock::{MockConfig, MockServer, Rendezvous, Reply, ToolsSupport};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::time::Duration;

mod common;
use common::run_bounded;

/// How long a listing waits for the others at most. Longer than LIM-M4 (5 s), so that a probe
/// that is run after another gives up (is `unreachable`) before the mock lets it go: what the
/// rendezvous decides is the state of the probes, not how long the mock waits.
const CAP: Duration = Duration::from_secs(20);

struct Rig {
    dir: tempfile::TempDir,
    config: PathBuf,
}

impl Rig {
    /// A configuration with the backends in `backends` (`(name, url, extra keys)`).
    fn new(backends: &[(&str, &str, &str)]) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let mut text = format!(
            "config_version = 2\n[runtime]\ndata_dir = {:?}\n",
            dir.path().join("data").to_string_lossy()
        );
        for (name, url, extra) in backends {
            text.push_str(&format!(
                "[backends.{name}]\nkind = \"openai_compatible\"\nurl = {url:?}\nmodel = \"test-model\"\nstructured_output = \"none\"\n{extra}\n"
            ));
        }
        let config = dir.path().join("config.toml");
        std::fs::write(&config, text).unwrap();
        Self { dir, config }
    }

    fn data(&self) -> PathBuf {
        self.dir.path().join("data")
    }

    fn run(&self, args: &[&str]) -> (i32, String, String) {
        let mut cmd = common::mimlys();
        cmd.arg("--config")
            .arg(&self.config)
            .args(args)
            .env("MIMLYS_CONFIG", "")
            .env("MIMLYS_WORKER_BIN", env!("CARGO_BIN_EXE_mimlys-worker"));
        let out = run_bounded(&mut cmd, self.dir.path());
        (out.code(), out.stdout.clone(), out.stderr.clone())
    }

    /// `status --json`, once.
    fn status_json(&self) -> Value {
        let (code, stdout, stderr) = self.run(&["--json", "status"]);
        assert_eq!(code, 0, "{stderr}");
        serde_json::from_str(stdout.trim())
            .unwrap_or_else(|e| panic!("not JSON ({e}): {stdout}\n{stderr}"))
    }

    /// `status`, once.
    fn status_text(&self) -> String {
        let (code, text, stderr) = self.run(&["status"]);
        assert_eq!(code, 0, "{stderr}");
        text
    }

    /// Both, the JSON first.
    fn status(&self) -> (Value, String) {
        let value = self.status_json();
        (value, self.status_text())
    }
}

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

fn ollama(loaded: &[&str]) -> MockConfig {
    let models: Vec<Value> = loaded
        .iter()
        .map(|m| json!({"name": m, "model": m, "context_length": 16384}))
        .collect();
    MockConfig {
        tools: ToolsSupport::Accept,
        ps: Some(json!({"models": models})),
        ..MockConfig::default()
    }
}

fn silent() -> MockConfig {
    MockConfig {
        tools: ToolsSupport::Accept,
        ..MockConfig::default()
    }
}

fn with(config: MockConfig, point: &Rendezvous) -> MockConfig {
    MockConfig {
        models_rendezvous: Some(point.clone()),
        ..config
    }
}

/// The line of `status` that begins `backend         <name>:`.
fn line_of<'a>(text: &'a str, name: &str) -> &'a str {
    let prefix = format!("backend         {name}:");
    text.lines()
        .find(|l| l.starts_with(&prefix))
        .unwrap_or_else(|| panic!("no line for {name}:\n{text}"))
}

fn entry<'a>(status: &'a Value, name: &str) -> &'a Value {
    status["backends"]
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b["name"] == name)
        .unwrap_or_else(|| panic!("no entry for {name}: {status}"))
}

/// REQ-MDL-010, LIM-M4: four backends, one of each kind of engine, are probed **at once**: the
/// listings are answered only when all four have arrived, and every backend is reachable; each
/// line and entry says whether the model is listed and resident (Ollama: yes when `/api/ps` lists
/// it and no when it does not; llama.cpp: always yes; another engine: unknown), the transport in
/// force and the context, and the last probe (`not probed`).
#[test]
fn req_mdl_010_status_probes_every_backend_at_once_and_says_each() {
    let point = Rendezvous::new(4, CAP);
    let loaded = MockServer::start(with(ollama(&["test-model"]), &point), vec![]);
    let unloaded = MockServer::start(with(ollama(&["another:7b"]), &point), vec![]);
    let llama = MockServer::start(with(llama_cpp(4096), &point), vec![]);
    let plain = MockServer::start(with(silent(), &point), vec![]);
    let rig = Rig::new(&[
        ("default", loaded.url(), ""),
        ("big", llama.url(), ""),
        ("cold", unloaded.url(), ""),
        ("plain", plain.url(), ""),
    ]);
    // One run, and the verdict from it alone: all four listings arrived, so all four probes were
    // started before any finished. (A later run finds the rendezvous met and waits for nobody.)
    let status = rig.status_json();
    assert!(point.met(), "{} of 4 listings arrived", point.arrived());
    assert_eq!(point.arrived(), 4);
    let text = rig.status_text();

    // The entries: `default` first, then by name.
    let names: Vec<&str> = status["backends"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["default", "big", "cold", "plain"]);
    for name in &names {
        let b = entry(&status, name);
        assert_eq!(b["reachable"], true, "{name}: {b}");
        assert_eq!(b["model_listed"], true, "{name}: {b}");
        assert_eq!(b["decision_transport"], "tool_calls", "{name}: {b}");
        assert_eq!(b["locality"], "local", "{name}");
        assert_eq!(b["kind"], "openai_compatible", "{name}");
        assert_eq!(b["last_probe"], Value::Null, "{name}");
    }
    assert_eq!(entry(&status, "default")["resident"], true);
    assert_eq!(entry(&status, "big")["resident"], true, "llama.cpp: always");
    assert_eq!(entry(&status, "cold")["resident"], false);
    assert_eq!(entry(&status, "plain")["resident"], Value::Null, "unknown");
    // The context, per backend: llama.cpp's own, Ollama's loaded one, unknown for the others.
    assert_eq!(entry(&status, "big")["context_tokens"], 4096);
    assert_eq!(entry(&status, "default")["context_tokens"], 16384);
    assert_eq!(entry(&status, "plain")["context_tokens"], Value::Null);

    // The lines.
    let default = line_of(&text, "default");
    assert!(default.contains("reachable, model listed"), "{default}");
    assert!(default.contains("resident yes"), "{default}");
    assert!(
        default.contains("decision transport tool_calls"),
        "{default}"
    );
    assert!(
        default.contains("context: 16384 tokens per request"),
        "{default}"
    );
    assert!(default.ends_with("; last probe: not probed"), "{default}");
    assert!(line_of(&text, "big").contains("resident yes"));
    assert!(line_of(&text, "cold").contains("resident no"));
    assert!(line_of(&text, "plain").contains("resident unknown"));
    // The context warning is for the backend it is about, named.
    let warnings: Vec<&str> = text
        .lines()
        .filter(|l| l.starts_with("warning: backend"))
        .collect();
    assert_eq!(warnings.len(), 1, "{text}");
    assert!(warnings[0].contains("backend `big`"), "{warnings:?}");
    assert!(
        warnings[0].contains("4096 tokens per request"),
        "{warnings:?}"
    );

    // `backend` is the `default` entry's view, as it was before several backends; deprecated.
    assert_eq!(status["backend"], *entry(&status, "default"));
}

/// REQ-MDL-010, LIM-M4: one backend that accepts the connection and never answers does not hold
/// the others: the two that answer are probed at the same moment as it (the rendezvous of the
/// three meets) and are `reachable`; it is `unreachable`, with its reason, within the one bound.
#[test]
fn req_mdl_010_a_backend_that_does_not_answer_is_unreachable_and_holds_no_other() {
    let point = Rendezvous::new(3, CAP);
    let a = MockServer::start(with(ollama(&["test-model"]), &point), vec![]);
    let b = MockServer::start(with(llama_cpp(32768), &point), vec![]);
    let silent_one = MockServer::start(
        MockConfig {
            models_hang: true,
            ..with(silent(), &point)
        },
        vec![],
    );
    let rig = Rig::new(&[
        ("default", a.url(), ""),
        ("deaf", silent_one.url(), ""),
        ("fast", b.url(), ""),
    ]);
    let status = rig.status_json();
    assert!(point.met(), "{} of 3 listings arrived", point.arrived());
    assert_eq!(point.arrived(), 3);
    let text = rig.status_text();
    assert_eq!(entry(&status, "default")["reachable"], true);
    assert_eq!(entry(&status, "fast")["reachable"], true);
    let deaf = entry(&status, "deaf");
    assert_eq!(deaf["reachable"], false, "{deaf}");
    assert!(
        deaf["detail"].as_str().unwrap().starts_with("timeout"),
        "{deaf}"
    );
    assert_eq!(deaf["resident"], Value::Null);
    let line = line_of(&text, "deaf");
    assert!(line.contains("unreachable"), "{line}");
    assert!(
        !line.contains("resident"),
        "an engine that is not reachable says nothing: {line}"
    );
    assert!(line_of(&text, "default").contains("reachable, model listed"));
    assert!(line_of(&text, "fast").contains("resident yes"));
}

/// REQ-MDL-010: the last fitness probe of each backend, with its date, as the record under the data
/// directory holds it: `<class>, measured on this machine on <date>`, `not probed` for a backend
/// the record does not name, the detail of an `incomplete`, a class this release does not know
/// shown as it is and marked, a result measured with another model said so, and a record that
/// cannot be read said so and never read as "not probed".
#[test]
fn req_mdl_010_status_shows_the_last_probe_of_each_backend() {
    let a = MockServer::start(silent(), vec![]);
    let b = MockServer::start(silent(), vec![]);
    let c = MockServer::start(silent(), vec![]);
    let d = MockServer::start(silent(), vec![]);
    let rig = Rig::new(&[
        ("default", a.url(), ""),
        ("small", b.url(), ""),
        ("newer", c.url(), ""),
        ("never", d.url(), ""),
    ]);
    std::fs::create_dir_all(rig.data()).unwrap();
    let record = json!({
        "version": 1,
        "backends": {
            "default": {"class": "acts", "steps": 4, "model_ms_per_step": 820,
                        "transport": "tool_calls", "model": "test-model", "at": "2026-11-22T10:15:00Z"},
            "small": {"class": "incomplete", "detail": "asked_user", "steps": 3,
                      "model": "an-older-model", "at": "2026-11-21T08:00:00Z"},
            "newer": {"class": "reasons_aloud", "steps": 1, "at": "2026-11-20T00:00:00Z"},
        }
    });
    std::fs::write(rig.data().join("probe-results.json"), record.to_string()).unwrap();
    let (status, text) = rig.status();
    let default = line_of(&text, "default");
    assert!(
        default.ends_with("; last probe: acts, measured on this machine on 2026-11-22"),
        "{default}"
    );
    let small = line_of(&text, "small");
    assert!(
        small.contains("last probe: incomplete (asked_user), measured on this machine on 2026-11-21 with model an-older-model, which is not the model this backend names now (test-model)"),
        "{small}"
    );
    let newer = line_of(&text, "newer");
    assert!(
        newer.contains("last probe: reasons_aloud (unknown to this release), measured on this machine on 2026-11-20"),
        "{newer}"
    );
    assert!(line_of(&text, "never").ends_with("; last probe: not probed"));
    // The same in JSON.
    let last = &entry(&status, "default")["last_probe"];
    assert_eq!(last["class"], "acts");
    assert_eq!(last["steps"], 4);
    assert_eq!(last["at"], "2026-11-22T10:15:00Z");
    assert_eq!(
        entry(&status, "small")["last_probe"]["detail"],
        "asked_user"
    );
    assert_eq!(entry(&status, "never")["last_probe"], Value::Null);

    // A record that cannot be read is said, for every backend, and is not "not probed".
    std::fs::write(rig.data().join("probe-results.json"), "{not json").unwrap();
    let (_, text) = rig.status();
    let line = line_of(&text, "default");
    assert!(
        line.contains("last probe: unreadable (probe-results.json is not a record"),
        "{line}"
    );
    assert!(!line.contains("not probed"), "{line}");
    // A record of a newer version is not read either.
    std::fs::write(
        rig.data().join("probe-results.json"),
        r#"{"version": 2, "backends": {}}"#,
    )
    .unwrap();
    let (_, text) = rig.status();
    assert!(line_of(&text, "default").contains("version 2, which this release does not read"));
}

/// REQ-MDL-010 (RFC 015 Amendment 1): `backend` is the `default` entry's view and `backends` has
/// every entry; with no backend named `default`, `backend` is null, the line says so, and the
/// others are listed.
#[test]
fn req_mdl_010_backend_is_the_default_entry_and_null_without_one() {
    let a = MockServer::start(silent(), vec![]);
    let b = MockServer::start(silent(), vec![]);
    let rig = Rig::new(&[("alpha", a.url(), ""), ("beta", b.url(), "")]);
    let (status, text) = rig.status();
    assert_eq!(status["backend"], Value::Null);
    assert_eq!(status["backends"].as_array().unwrap().len(), 2);
    assert!(text.contains("no backend is named `default`"), "{text}");
    assert!(text.contains("`alpha`, `beta`"), "{text}");
    // The old key is not the runtime's own flattened descriptor either.
    assert!(status.get("kind").is_none(), "{status}");
}

/// REQ-MDL-010: the other commands that start a task probe the backend that runs it, `default`,
/// and print its context warning only: a backend that is not `default` is not contacted at all.
#[test]
fn req_mdl_010_a_command_that_starts_a_task_uses_default_only() {
    let small = MockServer::start(llama_cpp(2048), vec![]);
    let big = MockServer::start(llama_cpp(32768), vec![]);
    let rig = Rig::new(&[("default", big.url(), ""), ("tiny", small.url(), "")]);
    let scope = tempfile::tempdir().unwrap();
    std::fs::write(scope.path().join("a.txt"), "a\n").unwrap();
    let scope = scope.path().to_str().unwrap();
    let (code, _, stderr) = rig.run(&["task", "new", "go", "--scope", scope, "--hold"]);
    assert_eq!(code, 0, "{stderr}");
    assert!(!stderr.contains("warning:"), "{stderr}");
    assert!(!big.requests().is_empty(), "default was probed");
    assert!(small.requests().is_empty(), "{:?}", small.requests());

    // And when `default` is the small one, its warning is printed, once.
    let rig = Rig::new(&[("default", small.url(), ""), ("big", big.url(), "")]);
    let before = big.requests().len();
    let (code, _, stderr) = rig.run(&["task", "new", "go", "--scope", scope, "--hold"]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(
        stderr
            .lines()
            .filter(|l| l.starts_with("warning: the engine's context"))
            .count(),
        1,
        "{stderr}"
    );
    assert_eq!(
        big.requests().len(),
        before,
        "a backend that is not default is not contacted"
    );
}

/// Keep `Reply` in use: a server with a scripted decision is not needed by these tests, and the
/// import documents that no decision is asked for.
#[allow(dead_code)]
fn _no_decisions() -> Vec<Reply> {
    vec![]
}
