//! The `tool_calls` decision transport through the binary (RFC 012; REQ-MDL-009,
//! REQ-DEC-003, REQ-OBS-001), against the mock server: `status` shows the
//! transport in force, a task runs in it, text beside a call is counted and
//! kept out of the next request, and a configured `tool_calls` that the
//! engine refuses halts the task with no fallback.

use mimlys_model::mock::{MockConfig, MockServer, Reply, ToolsSupport};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::process::Command;

mod common;
use common::run_bounded;

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

fn accepts_tools() -> MockConfig {
    MockConfig {
        tools: ToolsSupport::Accept,
        ..MockConfig::default()
    }
}

fn scope() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "hello\n").unwrap();
    dir
}

const GOAL: &str = "read a.txt and report";

/// REQ-MDL-009: `status` shows the transport in force, in JSON and in text:
/// `tool_calls` where the engine accepts `tools`; `envelope` with the
/// engine's reason where it refuses them; nothing is resolved for a
/// configured `tool_calls` the engine refuses, and the reason is shown.
#[test]
fn req_mdl_009_status_shows_the_transport_in_force() {
    let accepting = MockServer::start(accepts_tools(), vec![]);
    let env = Env::new(&backend(&accepting, "auto"));
    let (code, status) = env.json(&["status"]);
    assert_eq!(code, 0, "{status}");
    assert_eq!(status["backend"]["decision_transport"], "tool_calls");
    assert_eq!(status["backend"]["parallel_tool_calls"], true);
    assert_eq!(status["backend"]["structured_output"], Value::Null);
    let (_, text, _) = env.run(&["status"]);
    assert!(text.contains("decision transport tool_calls"), "{text}");
    assert!(!text.contains("structured output"), "{text}");

    // The default mock refuses `tools` as llama.cpp without jinja does.
    let refusing = MockServer::start(MockConfig::default(), vec![]);
    let env = Env::new(&backend(&refusing, "auto"));
    let (_, status) = env.json(&["status"]);
    assert_eq!(status["backend"]["decision_transport"], "envelope");
    assert_eq!(
        status["backend"]["transport_reason"],
        "HTTP 500: tools param requires --jinja flag"
    );
    assert!(status["backend"]["structured_output"].is_string());
    let (_, text, _) = env.run(&["status"]);
    assert!(
        text.contains("decision transport envelope (HTTP 500: tools param requires --jinja flag)"),
        "{text}"
    );
    assert!(text.contains("structured output"), "{text}");

    // `envelope` configured: never asked.
    let env = Env::new(&backend(&accepting, "envelope"));
    let (_, status) = env.json(&["status"]);
    assert_eq!(status["backend"]["decision_transport"], "envelope");

    // `tool_calls` configured and refused: unresolved, with the reason, no fallback.
    let env = Env::new(&backend(&refusing, "tool_calls"));
    let (code, status) = env.json(&["status"]);
    assert_eq!(code, 0, "{status}");
    assert_eq!(status["backend"]["decision_transport"], Value::Null);
    assert_eq!(status["backend"]["detail"], "tool_calls_refused");
    assert_eq!(
        status["backend"]["transport_reason"],
        "HTTP 500: tools param requires --jinja flag"
    );
    let (_, text, _) = env.run(&["status"]);
    assert!(text.contains("decision transport unresolved"), "{text}");
}

/// REQ-MDL-009: `tool_calls` configured and refused by the engine is
/// `backend_unavailable` with the reason, never a silent fallback: the task
/// halts and the engine never receives a decision request in either transport.
#[test]
fn req_mdl_009_configured_tool_calls_refused_halts_the_task() {
    let server = MockServer::start(
        MockConfig::default(),
        vec![Reply::Decision(
            json!({"decision": "complete", "summary": "x"}),
        )],
    );
    let env = Env::new(&backend(&server, "tool_calls"));
    let scope = scope();
    let (code, stdout, _) = env.run(&["run", GOAL, "--scope", scope.path().to_str().unwrap()]);
    assert_eq!(code, 0, "{stdout}");
    assert!(
        stdout.contains("HTTP 500: tools param requires --jinja flag"),
        "the reason is told: {stdout}"
    );
    assert!(stdout.contains("backend_unavailable"), "{stdout}");
    assert!(
        server.decision_requests().is_empty(),
        "no decision request: no fallback to the envelope"
    );
    assert_eq!(server.remaining(), 1);
}

/// REQ-DEC-003 (amended), REQ-OBS-001: a task runs in the `tool_calls`
/// transport; `decision.received` records the transport, the hash of the
/// mapped envelope and the length of text beside the call; that text is not in
/// the next request.
#[test]
fn req_dec_003_a_task_runs_in_tool_calls_and_text_beside_a_call_stays_out() {
    let beside = "Sure, I will read the file with ZEBRA-MARKER first.";
    let server = MockServer::start(
        accepts_tools(),
        vec![
            Reply::call_with_text(beside, "read_file", &json!({"path": "a.txt"})),
            Reply::call("complete", &json!({"summary": "read it"})),
        ],
    );
    let env = Env::new(&backend(&server, "auto"));
    let scope = scope();
    let (code, value) = env.json(&["run", GOAL, "--scope", scope.path().to_str().unwrap()]);
    assert_eq!(code, 0, "{value}");
    assert_eq!(value["task"]["state"], "completed", "{value}");
    let id = value["task"]["task_id"].as_str().unwrap().to_owned();

    let requests = server.decision_requests();
    assert_eq!(requests.len(), 2);
    for r in &requests {
        assert!(r.body["tools"].is_array(), "every request offers functions");
        assert_eq!(r.body["parallel_tool_calls"], false);
        assert!(r.body.get("tool_choice").is_none());
        assert!(r.body.get("response_format").is_none());
    }
    assert!(
        !requests[1].body.to_string().contains("ZEBRA-MARKER"),
        "text beside a call is not placed in the context"
    );

    let (_, events) = env.json(&["events", &id]);
    let received: Vec<&Value> = events
        .as_array()
        .unwrap()
        .iter()
        .map(|e| &e["record"])
        .filter(|r| r["event"] == "decision.received")
        .collect();
    assert_eq!(received.len(), 2, "{events}");
    assert_eq!(received[0]["transport"], "tool_calls");
    assert_eq!(
        received[0]["text_chars"].as_u64(),
        Some(beside.chars().count() as u64)
    );
    assert_eq!(received[1]["transport"], "tool_calls");
    assert!(received[1].get("text_chars").is_none_or(Value::is_null));
    let expected = mimlys_core::hash::hash_canonical(
        &json!({"decision": "act", "tool": "read_file", "arguments": {"path": "a.txt"}}),
    );
    assert_eq!(
        received[0]["hash"],
        serde_json::to_value(expected).unwrap(),
        "the hash is the canonical hash of the mapped envelope"
    );
}

/// REQ-CFG-001, REQ-MDL-009: `decision_transport` takes the three values and
/// nothing else; the `fake` backend has no `tool_calls` transport.
#[test]
fn req_mdl_009_configuration_values() {
    for value in ["auto", "tool_calls", "envelope"] {
        let env = Env::new(&format!(
            "kind = \"openai_compatible\"\nurl = \"http://127.0.0.1:9/v1\"\nmodel = \"m\"\ndecision_transport = \"{value}\""
        ));
        let (code, v) = env.json(&["check"]);
        assert_eq!(code, 0, "{value}: {v}");
        assert_eq!(v["config"]["backend"]["decision_transport"], value);
    }
    let env =
        Env::new("kind = \"openai_compatible\"\nurl = \"http://127.0.0.1:9/v1\"\nmodel = \"m\"");
    let (_, v) = env.json(&["check"]);
    assert_eq!(
        v["config"]["backend"]["decision_transport"], "auto",
        "the default"
    );
    let env = Env::new("decision_transport = \"native\"");
    let (code, _, stderr) = env.run(&["check"]);
    assert_eq!(code, 1);
    assert!(stderr.contains("unknown variant `native`"), "{stderr}");
    let env = Env::new("kind = \"fake\"\ndecision_transport = \"tool_calls\"");
    let (code, _, stderr) = env.run(&["check"]);
    assert_eq!(code, 1);
    assert!(stderr.contains("backend.decision_transport"), "{stderr}");
}

/// REQ-MDL-009 (handoff 012, correction 1): in the `tool_calls` transport,
/// `status` tells the user which MCP tools the model cannot see for their
/// function name, with the reason, in the server's `unavailable` list (JSON and
/// text), and no longer lists them as available. The case: a tool name that is
/// valid at 64 characters (`<server>.web_search`, 53 + 1 + 10) is 65 as a
/// function name, where the dot is written `__`; its neighbour one character
/// shorter (`web_fetch`) stays. Under the envelope transport there are no
/// function names and both stay; a host tool is never affected.
#[test]
fn req_mdl_009_status_names_the_tools_the_model_cannot_see() {
    let server = format!("s{}", "x".repeat(52)); // 53 characters
    let extra = format!(
        "\n[servers.{server}]\ncommand = {:?}\nenv_allow = [\"PATH\"]\n",
        env!("CARGO_BIN_EXE_mimlys-mcp-testserver")
    );
    let names = |status: &Value, key: &str| -> Vec<String> {
        status["servers"][0][key]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| {
                v.as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| v[0].as_str().unwrap().to_owned())
            })
            .collect()
    };
    let search = format!("{server}.web_search");
    let fetch = format!("{server}.web_fetch");

    let accepting = MockServer::start(accepts_tools(), vec![]);
    let env = Env::new(&format!("{}{extra}", backend(&accepting, "auto")));
    let (code, status) = env.json(&["status"]);
    assert_eq!(code, 0, "{status}");
    assert_eq!(status["backend"]["decision_transport"], "tool_calls");
    assert_eq!(status["servers"][0]["state"], "running", "{status}");
    let unavailable = status["servers"][0]["unavailable"].as_array().unwrap();
    let entry = unavailable
        .iter()
        .find(|e| e[0] == search.as_str())
        .unwrap_or_else(|| panic!("{search} is left out: {unavailable:?}"));
    assert_eq!(entry[1], "function_name_invalid", "{entry}");
    let available = names(&status, "tools");
    assert!(!available.contains(&search), "{available:?}");
    assert!(available.contains(&fetch), "{available:?}");
    let (_, text, _) = env.run(&["status"]);
    assert!(text.contains("function_name_invalid"), "{text}");
    // Host tools are not functions of their own.
    for host_tool in status["host_tools"].as_array().unwrap() {
        assert!(host_tool.get("unavailable").is_none(), "{host_tool}");
    }

    let envelope = MockServer::start(accepts_tools(), vec![]);
    let env = Env::new(&format!("{}{extra}", backend(&envelope, "envelope")));
    let (_, status) = env.json(&["status"]);
    assert_eq!(status["backend"]["decision_transport"], "envelope");
    assert!(
        !names(&status, "unavailable").contains(&search),
        "no function names in the envelope transport: {status}"
    );
    assert!(names(&status, "tools").contains(&search), "{status}");
}
