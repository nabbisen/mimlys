//! A rejection reaches the model with its reason and detail, across a restart, and is
//! counted (handoff 002c; REQ-DEC-008, REQ-DEC-009, REQ-OBS-008), through the binary against
//! the mock server.

use mimlys_model::mock::{MockConfig, MockServer, Reply};
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
    fn new(server: &MockServer) -> Self {
        let data = tempfile::tempdir().unwrap();
        let config = data.path().join("config.toml");
        std::fs::write(
            &config,
            format!(
                "config_version = 1\n[runtime]\ndata_dir = {:?}\n[backend]\nkind = \"openai_compatible\"\nurl = {:?}\nmodel = \"test-model\"\ndecision_transport = \"envelope\"\n",
                data.path().join("data").to_string_lossy(),
                server.url(),
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

fn scope() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "hello\n").unwrap();
    dir
}

/// The text of every message of a recorded request.
fn messages(body: &Value) -> Vec<String> {
    body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|m| m["content"].as_str().map(str::to_owned))
        .collect()
}

/// The body of the last rejection segment of a request.
fn last_rejection(body: &Value) -> Value {
    let segment = messages(body)
        .into_iter()
        .rfind(|m| m.starts_with("<<feedback kind=\"rejection\""))
        .expect("a rejection segment in the request");
    serde_json::from_str(segment.lines().nth(1).unwrap()).unwrap()
}

/// REQ-DEC-009, REQ-DEC-008: the model is told what the rejection says (the relative-path hint
/// and the reason), the same after the process is restarted; REQ-OBS-008: the denial is in the
/// event with its reason and counted by reason in `metrics`, in text and JSON.
#[test]
fn req_dec_009_a_denied_path_is_told_and_counted_and_told_again_after_a_restart() {
    let server = MockServer::start(
        MockConfig::default(),
        vec![Reply::Decision(
            json!({"decision": "act", "tool": "read_file", "arguments": {"path": "../outside"}}),
        )],
    );
    let env = Env::new(&server);
    let scope = scope();
    // The script runs out after the first decision: the task halts at the second step.
    let (code, value) = env.json(&[
        "run",
        "read a.txt",
        "--scope",
        scope.path().to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "{value}");
    assert_eq!(value["task"]["state"], "halted", "{value}");
    let id = value["task"]["task_id"].as_str().unwrap().to_owned();

    // The live request: the first process told the model at its second step.
    let live = server.decision_requests();
    assert!(live.len() >= 2, "{} requests", live.len());
    let told = last_rejection(&live[1].body);
    assert_eq!(told["code"], "denied");
    assert_eq!(told["detail"], json!({"reason": "path_outside_workspace"}));
    assert!(
        told["hint"]
            .as_str()
            .unwrap()
            .contains("relative to the workspace root"),
        "{told}"
    );
    let seen = live.len();

    // A second process resumes the task: the segment is read from the store.
    server.push(Reply::Decision(
        json!({"decision": "complete", "summary": "done"}),
    ));
    let (code, value) = env.json(&["task", "resume", &id]);
    assert_eq!(code, 0, "{value}");
    assert_eq!(value["task"]["state"], "completed", "{value}");
    let after = server.decision_requests();
    let again = last_rejection(&after[seen].body);
    assert_eq!(
        again, told,
        "the same decision gets the same segment after a restart"
    );

    // The event and the counters.
    let (_, events) = env.json(&["events", &id]);
    let rejected: Vec<&Value> = events
        .as_array()
        .unwrap()
        .iter()
        .map(|e| &e["record"])
        .filter(|r| r["event"] == "decision.rejected")
        .collect();
    assert_eq!(rejected.len(), 1, "{events}");
    assert_eq!(rejected[0]["code"], "denied");
    assert_eq!(rejected[0]["reason"], "path_outside_workspace");
    assert!(rejected[0].get("grant_kind").is_none_or(Value::is_null));
    let (_, metrics) = env.json(&["metrics", "--task", &id]);
    assert_eq!(metrics["rejections_by_code"]["denied"], 1);
    assert_eq!(metrics["denials_by_reason"]["path_outside_workspace"], 1);
    let (_, text, _) = env.run(&["metrics", "--task", &id]);
    assert!(
        text.lines().any(
            |l| l.starts_with("denials_by_reason.path_outside_workspace") && l.ends_with(" 1")
        ),
        "{text}"
    );
}
