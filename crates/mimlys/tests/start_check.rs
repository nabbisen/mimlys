//! REQ-SEC-010 at every start of a task (RFC 015, Amendment 1; handoff 015a, item 3; external
//! design §20), through the binary against the scripted OpenAI-compatible mock server.
//!
//! The configuration is read when a process starts, so a task made while the backend was local
//! can meet it declared `remote` at its next start: the tests make the task, edit the file as a
//! user would (`locality = "remote"` on the same loopback URL, which is a tunnel; the permission
//! given), and start the task again by each way there is. The verdict is what the mock received:
//! nothing at all, not even the probe, which is a request to the remote machine too.
#![cfg(feature = "test-bins")]

use mimlys_model::mock::{MockConfig, MockServer, Reply};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

mod common;
use common::run_bounded;

/// Far longer than anything a test waits for.
const HOLD_MS: u64 = 600_000;

fn complete() -> Value {
    json!({"decision": "complete", "summary": "done"})
}

struct Rig {
    dir: tempfile::TempDir,
    config: PathBuf,
    registry: common::Registry,
    scope: PathBuf,
    url: String,
}

impl Rig {
    fn new(server: &MockServer) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let scope = dir.path().join("scope");
        std::fs::create_dir_all(&scope).unwrap();
        std::fs::write(scope.join("a.txt"), "a\n").unwrap();
        let rig = Self {
            config: dir.path().join("config.toml"),
            registry: common::Registry::new(),
            scope,
            url: server.url().to_owned(),
            dir,
        };
        rig.write(false);
        rig
    }

    /// The configuration: the one backend on the mock's loopback URL, declared `local`, or
    /// declared `remote` with the permission given (a tunnel); a recovered task goes on by itself.
    fn write(&self, remote: bool) {
        let (locality, permission) = if remote {
            ("remote", "remote_backend_allowed = true\n")
        } else {
            ("local", "")
        };
        std::fs::write(
            &self.config,
            format!(
                r#"config_version = 2
[runtime]
data_dir = {data:?}
[backends.default]
kind = "openai_compatible"
url = {url:?}
model = "test-model"
structured_output = "none"
decision_transport = "envelope"
locality = "{locality}"
[policy]
{permission}
[policy.recovery]
auto_resume = true
"#,
                data = self.dir.path().join("data").to_string_lossy(),
                url = self.url,
            ),
        )
        .unwrap();
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_mimlys"));
        cmd.arg("--config")
            .arg(&self.config)
            .args(args)
            .env("MIMLYS_CONFIG", "")
            .env("MIMLYS_RUN_DIR", self.registry.path())
            .env("MIMLYS_WORKER_BIN", env!("CARGO_BIN_EXE_mimlys-worker"))
            .stdin(Stdio::null());
        cmd
    }

    fn run(&self, args: &[&str]) -> (i32, String, String) {
        let out = run_bounded(&mut self.command(args), self.dir.path());
        (out.code(), out.stdout.clone(), out.stderr.clone())
    }

    fn json(&self, args: &[&str]) -> (i32, Value) {
        let mut all = vec!["--json"];
        all.extend_from_slice(args);
        let (code, stdout, stderr) = self.run(&all);
        let value = serde_json::from_str(stdout.trim())
            .unwrap_or_else(|e| panic!("not JSON ({e}): {stdout}\n{stderr}"));
        (code, value)
    }

    fn task_id(&self) -> String {
        let (_, list) = self.json(&["task", "list"]);
        let tasks = list.as_array().unwrap();
        assert_eq!(tasks.len(), 1, "{list}");
        tasks[0]["task_id"].as_str().unwrap().to_owned()
    }

    fn scope(&self) -> &str {
        self.scope.to_str().unwrap()
    }
}

/// The line `task resume` (and every command that drives the task) prints for the refusal.
fn refusal_line(stdout: &str) -> Option<&str> {
    stdout.lines().find(|l| l.starts_with("nothing was sent:"))
}

/// The rule of a halt `policy` for a remote backend under a profile that forbids it, as the task
/// and the event say it (`halt_detail`, external design §5.2).
fn forbidden_detail() -> Value {
    json!({"rule": "remote_backend_forbidden", "backend": "default", "profile": "workspace"})
}

/// The `halt_detail` of the task's last `task.state_changed` event to `halted`, as `events --json`
/// shows it.
fn last_halt_detail(rig: &Rig, id: &str) -> Option<Value> {
    let (_, events) = rig.json(&["events", id]);
    events
        .as_array()
        .unwrap()
        .iter()
        .map(|e| &e["record"])
        .rfind(|r| r["event"] == "task.state_changed" && r["to"] == "halted")
        .map(|r| r["halt_detail"].clone())
        .filter(|d| !d.is_null())
}

/// REQ-SEC-010: a task made under a local backend, the file edited to declare the backend
/// remote, `task resume` under the shipped profile (`external = false`): the task halts `policy`,
/// the line names the backend and the profile, and the mock received no request at all.
#[test]
fn req_sec_010_task_resume_halts_policy_when_the_backend_became_remote() {
    let server = MockServer::start(MockConfig::default(), vec![Reply::Decision(complete())]);
    let rig = Rig::new(&server);
    let (code, created) = rig.json(&["task", "new", "go", "--scope", rig.scope(), "--hold"]);
    assert_eq!(code, 0, "{created}");
    let id = created["task"]["task_id"].as_str().unwrap().to_owned();
    let received = server.requests().len();

    rig.write(true);
    let (code, stdout, stderr) = rig.run(&["task", "resume", &id]);
    assert_eq!(code, 0, "{stderr}");
    assert!(stdout.contains("halted"), "{stdout}");
    let line = refusal_line(&stdout).unwrap_or_else(|| panic!("no refusal line: {stdout}"));
    assert!(line.contains("backend `default`"), "{line}");
    assert!(line.contains("profile `workspace`"), "{line}");
    assert!(line.contains("external = false"), "{line}");
    assert_eq!(
        server.requests().len(),
        received,
        "nothing was sent: not a decision request, not the probe"
    );
    let (_, shown) = rig.json(&["task", "show", &id]);
    assert_eq!(shown["task"]["state"], "halted");
    assert_eq!(shown["task"]["halt_reason"], "policy");
    // The halt names its rule (external design §5.2): in the task, and in the halt's event.
    assert_eq!(shown["task"]["halt_detail"], forbidden_detail(), "{shown}");
    assert_eq!(last_halt_detail(&rig, &id), Some(forbidden_detail()));
    // The same in JSON, and the same again at the next resume.
    let (code, value) = rig.json(&["task", "resume", &id]);
    assert_eq!(code, 0, "{value}");
    assert_eq!(value["task"]["halt_reason"], "policy");
    assert_eq!(value["task"]["halt_detail"], forbidden_detail(), "{value}");
    assert_eq!(server.requests().len(), received);

    // The user's remedy: the backend is local again; the same task goes on and sends.
    rig.write(false);
    let (code, stdout, stderr) = rig.run(&["task", "resume", &id]);
    assert_eq!(code, 0, "{stderr}");
    assert!(refusal_line(&stdout).is_none(), "{stdout}");
    assert!(!server.decision_requests().is_empty(), "the task ran");
}

/// REQ-SEC-010: a profile with `external = true` under a remote backend is not refused, at a
/// start either: the rule is the profile's.
#[test]
fn req_sec_010_a_profile_with_external_resumes_under_a_remote_backend() {
    let server = MockServer::start(MockConfig::default(), vec![Reply::Decision(complete())]);
    let rig = Rig::new(&server);
    let (code, created) = rig.json(&["task", "new", "go", "--scope", rig.scope(), "--hold"]);
    assert_eq!(code, 0, "{created}");
    let id = created["task"]["task_id"].as_str().unwrap().to_owned();
    rig.write(true);
    let mut text = std::fs::read_to_string(&rig.config).unwrap();
    text.push_str("[policy.profiles.workspace]\nexternal = true\n");
    std::fs::write(&rig.config, text).unwrap();
    let (code, stdout, stderr) = rig.run(&["task", "resume", &id]);
    assert_eq!(code, 0, "{stderr}");
    assert!(refusal_line(&stdout).is_none(), "{stdout}");
    assert!(!server.decision_requests().is_empty(), "the task ran");
}

/// REQ-SEC-010: the creation is refused by the same rule and the backend is not contacted for it
/// either (the probe before a command that runs steps is skipped when the start is refused).
#[test]
fn req_sec_010_a_creation_that_is_refused_contacts_nothing() {
    let server = MockServer::start(MockConfig::default(), vec![]);
    let rig = Rig::new(&server);
    rig.write(true);
    for args in [
        vec!["run", "go", "--scope", rig.scope()],
        vec!["task", "new", "go", "--scope", rig.scope()],
        vec!["task", "new", "go", "--scope", rig.scope(), "--hold"],
    ] {
        let (code, _, stderr) = rig.run(&args);
        assert_eq!(code, 1, "{args:?}: {stderr}");
        assert!(
            stderr.contains("profile workspace forbids external communication"),
            "{stderr}"
        );
    }
    assert!(server.requests().is_empty(), "{:?}", server.requests());
}

/// REQ-SEC-010: a recovered task under `recovery.auto_resume` is a start: a run that was killed
/// with its first request in flight leaves a task the next start recovers to `running`, and
/// when the backend has become remote meanwhile, the task halts `policy` at its first step and
/// the mock receives nothing more.
#[test]
fn req_sec_010_a_recovered_task_under_auto_resume_is_checked() {
    // The first request is held for far longer than the test waits.
    let server = MockServer::start(
        MockConfig::default(),
        vec![Reply::Slow(HOLD_MS, Box::new(Reply::Decision(complete())))],
    );
    let rig = Rig::new(&server);
    let mut child = rig
        .command(&["--json", "run", "go", "--scope", rig.scope()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let started = std::time::Instant::now();
    while server.decision_requests().is_empty() {
        assert!(
            started.elapsed() < Duration::from_secs(120),
            "the first request never came"
        );
        std::thread::sleep(Duration::from_millis(25));
    }
    // The runtime dies with the request in flight: the task is `running`.
    child.kill().unwrap();
    child.wait().unwrap();
    let id = rig.task_id();
    let (_, shown) = rig.json(&["task", "show", &id]);
    assert_eq!(shown["task"]["state"], "running", "{shown}");
    let received = server.requests().len();

    rig.write(true);
    // `revoke` is a command that drives a task that is running: the next start recovers it
    // (`auto_resume`: back to `running`), then drives it. It has no probe of its own.
    let grant = shown["grants"][0]["grant_id"].as_str().unwrap().to_owned();
    let (code, stdout, stderr) = rig.run(&["revoke", &id, &grant]);
    assert_eq!(code, 0, "{stderr}\n{stdout}");
    let (_, after) = rig.json(&["task", "show", &id]);
    assert_eq!(after["task"]["state"], "halted", "{stdout}");
    assert_eq!(after["task"]["halt_reason"], "policy", "{after}");
    assert_eq!(after["task"]["halt_detail"], forbidden_detail(), "{after}");
    assert_eq!(
        server.requests().len(),
        received,
        "nothing was sent after the backend became remote"
    );
    let (_, events) = rig.json(&["events", &id]);
    let names: Vec<&str> = events
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["record"]["event"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"task.recovered"), "{names:?}");
}

/// REQ-TASK-007, external design §5.2: the other cause of a halt `policy`, a completion check that
/// the validator refuses (a tool the profile does not offer), is told from the first by its
/// rule, `completion_check_refused`, in the task and in the halt's event, through `--json`; the
/// text says it too, and does not blame the backend.
#[test]
fn req_task_007_a_refused_completion_check_halts_policy_naming_its_rule() {
    let server = MockServer::start(MockConfig::default(), vec![Reply::Decision(complete())]);
    let rig = Rig::new(&server);
    let check = json!({"tool": "no_such_tool", "arguments": {}}).to_string();
    let args = [
        "run",
        "go",
        "--scope",
        rig.scope(),
        "--completion-check",
        &check,
    ];
    let (code, stdout, stderr) = rig.run(&args);
    assert_eq!(code, 0, "{stderr}");
    assert!(stdout.contains("completion check was refused"), "{stdout}");
    assert!(refusal_line(&stdout).is_none(), "{stdout}");
    let id = rig.task_id();
    let (_, shown) = rig.json(&["task", "show", &id]);
    assert_eq!(shown["task"]["state"], "halted", "{shown}");
    assert_eq!(shown["task"]["halt_reason"], "policy", "{shown}");
    assert_eq!(
        shown["task"]["halt_detail"],
        json!({"rule": "completion_check_refused"}),
        "{shown}"
    );
    assert_eq!(
        last_halt_detail(&rig, &id),
        Some(json!({"rule": "completion_check_refused"}))
    );
}
