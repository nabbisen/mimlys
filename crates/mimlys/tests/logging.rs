//! The logs on standard error (REQ-PRIV-004; external design §17.3): JSON
//! lines with a timestamp and a level, never the goal, a file's content, the
//! model's output or a configured secret, whatever the level; the debug option
//! that would log prompts warns at start and is silent when off. Against the
//! mock server, so that the secret really travels (as the bearer token) and
//! the model's reply really arrives.

use mimlys_model::mock::{MockConfig, MockServer, Reply};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::process::Command;

mod common;
use common::run_bounded;

const GOAL: &str = "PRIVGOALMARK-read-a-txt-and-report";
const FILE: &str = "PRIVFILEMARK-in-the-source";
const MODEL: &str = "PRIVMODELMARK-the-summary";
const SECRET: &str = "PRIVSECRETMARK-0123456789";

struct Env {
    dir: tempfile::TempDir,
    config: PathBuf,
    scope: PathBuf,
}

impl Env {
    /// A configuration for the mock server (which expects the secret as its
    /// bearer token) with the given `[logging]` body, and a project holding
    /// one file with the marker content.
    fn new(server: &MockServer, logging: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config.toml");
        std::fs::write(
            &config,
            format!(
                "config_version = 1\n[runtime]\ndata_dir = {:?}\n[backend]\nkind = \"openai_compatible\"\nurl = {:?}\nmodel = \"test-model\"\ndecision_transport = \"envelope\"\napi_key = \"env:MIMLYS_LOGGING_TEST_KEY\"\n[logging]\n{logging}\n",
                dir.path().join("data").to_string_lossy(),
                server.url()
            ),
        )
        .unwrap();
        let scope = dir.path().join("project");
        std::fs::create_dir_all(&scope).unwrap();
        std::fs::write(scope.join("a.txt"), format!("{FILE}\n")).unwrap();
        Self { dir, config, scope }
    }

    /// The stored events of a task, as JSON.
    fn events(&self, task: &str) -> Value {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_mimlys"));
        cmd.arg("--config")
            .arg(&self.config)
            .args(["--json", "events", task])
            .env("MIMLYS_CONFIG", "")
            .env("MIMLYS_LOGGING_TEST_KEY", SECRET);
        let out = run_bounded(&mut cmd, self.dir.path());
        assert_eq!(out.code(), 0, "{}", out.stderr);
        serde_json::from_str(out.stdout.trim()).unwrap_or_else(|e| panic!("{e}: {}", out.stdout))
    }

    /// The exit code, standard output and standard error of a run of the
    /// goal over the project.
    fn run(&self) -> (i32, String, String) {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_mimlys"));
        cmd.arg("--config")
            .arg(&self.config)
            .args(["--json", "run", GOAL, "--scope"])
            .arg(&self.scope)
            .env("MIMLYS_CONFIG", "")
            .env("MIMLYS_LOGGING_TEST_KEY", SECRET)
            .env("MIMLYS_WORKER_BIN", env!("CARGO_BIN_EXE_mimlys-worker"));
        let out = run_bounded(&mut cmd, self.dir.path());
        (out.code(), out.stdout, out.stderr)
    }
}

fn server() -> MockServer {
    MockServer::start(
        MockConfig {
            expect_bearer: Some(SECRET.to_owned()),
            ..MockConfig::default()
        },
        vec![
            Reply::Decision(
                json!({"decision": "act", "tool": "read_file", "arguments": {"path": "a.txt"}}),
            ),
            Reply::Decision(json!({"decision": "complete", "summary": MODEL})),
        ],
    )
}

/// REQ-PRIV-004 (external design §17.3): at the most verbose level, with
/// `debug_prompts` off, a whole run (a goal, a file read, a model's reply, a
/// secret sent as the bearer token) writes only structured lines to standard
/// error, each a JSON object with a `timestamp` and a `level`, and none of
/// them holds the goal, the file's content, the model's output or the
/// secret. The controls: the run completed, the server received the secret,
/// and the goal and the file's content were in the requests, so that the
/// absence from the log is not the absence of the thing.
#[test]
fn req_priv_004_logs_are_structured_and_hold_no_prompt_file_or_secret() {
    let server = server();
    let env = Env::new(&server, "level = \"trace\"");
    let (code, stdout, stderr) = env.run();
    assert_eq!(code, 0, "{stdout}\n{stderr}");
    let task: Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(task["task"]["state"], "completed", "{task}");
    let requests = server.decision_requests();
    assert_eq!(requests.len(), 2);
    assert!(requests[1].body.to_string().contains(GOAL));
    assert!(requests[1].body.to_string().contains(FILE));
    assert!(
        requests.iter().all(|r| r
            .headers
            .iter()
            .any(|(k, v)| k == "authorization" && v == &format!("Bearer {SECRET}"))),
        "the secret was used"
    );

    assert!(
        !stderr.contains("debug_prompts"),
        "no warning when the option is off: {stderr}"
    );
    for line in stderr.lines().filter(|l| !l.trim().is_empty()) {
        let value: Value = serde_json::from_str(line)
            .unwrap_or_else(|e| panic!("a log line that is not JSON ({e}): {line}"));
        assert!(value["timestamp"].is_string(), "no timestamp: {line}");
        assert!(value["level"].is_string(), "no level: {line}");
    }
    for (what, needle) in [
        ("the goal", GOAL),
        ("the file", FILE),
        ("the model's output", MODEL),
        ("the secret", SECRET),
    ] {
        assert!(
            !stderr.contains(needle),
            "{what} reached the log:\n{stderr}"
        );
    }
}

/// REQ-PRIV-004 (external design §17.3, configuration `debug_prompts`):
/// turning the prompt option on prints a warning on standard error before
/// anything else; with it off there is none.
#[test]
fn req_priv_004_the_prompt_option_warns_at_start() {
    let server = server();
    let env = Env::new(&server, "level = \"info\"\ndebug_prompts = true");
    let (code, stdout, stderr) = env.run();
    assert_eq!(code, 0, "{stdout}\n{stderr}");
    let first = stderr.lines().next().unwrap_or_default();
    assert!(
        first.starts_with("warning:") && first.contains("debug_prompts"),
        "the warning comes first: {stderr}"
    );
    let server = self::server();
    let quiet = Env::new(&server, "level = \"info\"");
    let (code, stdout, stderr) = quiet.run();
    assert_eq!(code, 0, "{stdout}\n{stderr}");
    assert!(!stderr.contains("debug_prompts"), "{stderr}");
}

/// The server of the next tests: the model's final reply carries the secret
/// (a model repeating a key it should not have), so that the redaction of the
/// lines that hold a reply has something to remove.
fn leaking_server() -> MockServer {
    MockServer::start(
        MockConfig {
            expect_bearer: Some(SECRET.to_owned()),
            ..MockConfig::default()
        },
        vec![
            Reply::Decision(
                json!({"decision": "act", "tool": "read_file", "arguments": {"path": "a.txt"}}),
            ),
            Reply::Decision(
                json!({"decision": "complete", "summary": format!("{MODEL} {SECRET}")}),
            ),
        ],
    )
}

fn json_lines(stderr: &str) -> Vec<Value> {
    stderr
        .lines()
        .filter(|l| l.starts_with('{'))
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("not JSON ({e}): {l}")))
        .collect()
}

fn message(line: &Value) -> &str {
    line["fields"]["message"].as_str().unwrap_or_default()
}

/// Every string value stored under `key`, anywhere in `value`.
fn collect(value: &Value, key: &str, into: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (k, v) in map {
                match (k == key, v) {
                    (true, Value::String(s)) => into.push(s.clone()),
                    _ => collect(v, key, into),
                }
            }
        }
        Value::Array(items) => items.iter().for_each(|v| collect(v, key, into)),
        _ => {}
    }
}

/// REQ-OBS-004 (external design §17.3): a log line written while a task's
/// step runs carries the step's ids, `task_id` and `step_id` (and
/// `action_id` once the step has an action: the unit test next to the step
/// loop covers that), as the fields of its `span`; a line written before the
/// first step carries none. A whole run, `debug` level, with the prompt
/// option on only so that the model requests and replies are lines written
/// during the steps: the ids are those of the task and of the two steps in
/// the stored events.
#[test]
fn req_obs_004_a_line_during_a_step_has_the_ids_and_one_outside_has_none() {
    let server = server();
    let env = Env::new(&server, "level = \"debug\"\ndebug_prompts = true");
    let (code, stdout, stderr) = env.run();
    assert_eq!(code, 0, "{stdout}\n{stderr}");
    let task: Value = serde_json::from_str(stdout.trim()).unwrap();
    let task_id = task["task"]["task_id"].as_str().unwrap().to_owned();
    let mut stored_steps = Vec::new();
    collect(&env.events(&task_id), "step_id", &mut stored_steps);
    stored_steps.sort();
    stored_steps.dedup();
    assert_eq!(stored_steps.len(), 2, "two steps ran: {stored_steps:?}");

    let lines = json_lines(&stderr);
    let first_prompt = lines
        .iter()
        .position(|l| message(l) == "model request message")
        .expect("a request line");
    assert!(first_prompt > 0, "lines were written before the first step");
    for line in &lines[..first_prompt] {
        assert!(line.get("span").is_none(), "a line outside a step: {line}");
    }
    let mut seen = Vec::new();
    for line in lines
        .iter()
        .filter(|l| matches!(message(l), "model request message" | "model reply"))
    {
        let span = &line["span"];
        assert_eq!(span["name"], "step", "{line}");
        assert_eq!(span["task_id"], task_id.as_str(), "{line}");
        let step = span["step_id"].as_str().unwrap_or_else(|| panic!("{line}"));
        assert!(stored_steps.iter().any(|s| s == step), "{line}");
        assert!(
            span.get("action_id").is_none(),
            "no action during the decision: {line}"
        );
        seen.push(step.to_owned());
    }
    seen.sort();
    seen.dedup();
    assert_eq!(seen, stored_steps, "both steps wrote lines");
}

/// REQ-PRIV-004 (external design §17.3): with `debug_prompts` on, at
/// `debug`, each request's messages and the model's reply are lines of their
/// own: the goal, the file's content and the reply appear; the secret, which
/// the model's reply here repeats and which travels as the bearer token,
/// appears nowhere, the redaction marker in its place.
#[test]
fn req_priv_004_with_the_prompt_option_on_prompts_and_replies_are_logged_redacted() {
    let server = leaking_server();
    let env = Env::new(&server, "level = \"debug\"\ndebug_prompts = true");
    let (code, stdout, stderr) = env.run();
    assert_eq!(code, 0, "{stdout}\n{stderr}");
    assert!(
        stderr
            .lines()
            .next()
            .unwrap_or_default()
            .starts_with("warning:"),
        "the warning is the first line"
    );
    let lines = json_lines(&stderr);
    let in_lines = |what: &str, needle: &str| {
        lines
            .iter()
            .any(|l| message(l) == what && l["fields"].to_string().contains(needle))
    };
    assert!(
        in_lines("model request message", GOAL),
        "the goal: {stderr}"
    );
    assert!(
        in_lines("model request message", FILE),
        "the file: {stderr}"
    );
    assert!(in_lines("model reply", MODEL), "the reply: {stderr}");
    assert!(
        in_lines("model reply", "[redacted:"),
        "the secret in the reply is redacted: {stderr}"
    );
    assert!(
        !stderr.contains(SECRET),
        "the secret reached the log:\n{stderr}"
    );
    assert!(
        server.decision_requests().iter().all(|r| r
            .headers
            .iter()
            .any(|(k, v)| k == "authorization" && v == &format!("Bearer {SECRET}"))),
        "the secret travelled as the bearer token"
    );
}

/// REQ-PRIV-004: with the option off, nothing of a prompt, a file or a
/// reply is written at the most verbose level, though the model's reply
/// repeats the secret and the other lines are many.
#[test]
fn req_priv_004_with_the_prompt_option_off_no_line_holds_a_prompt_or_a_reply() {
    let server = leaking_server();
    let env = Env::new(&server, "level = \"trace\"");
    let (code, stdout, stderr) = env.run();
    assert_eq!(code, 0, "{stdout}\n{stderr}");
    for line in json_lines(&stderr) {
        assert!(
            !matches!(message(&line), "model request message" | "model reply"),
            "{line}"
        );
    }
    for needle in [GOAL, FILE, MODEL, SECRET] {
        assert!(
            !stderr.contains(needle),
            "{needle} reached the log:\n{stderr}"
        );
    }
}

// The lines of a thread or a task that logs for a step (REQ-OBS-004): the
// worker's standard-error reader and the MCP client's. They run on their own
// and write each line in the span of the call or execution in flight when
// they read it (`mimlys_core::log_span`).

const BUILTIN: &str = r#""read_file", "write_file", "replace_in_file", "list_dir", "search_files", "delete_path", "run_script", "run_wasm""#;

/// The line the test server writes on standard error during a call.
const SERVER_LINE: &str = "SERVERSTDERRMARK-a-line-during-the-call";

/// A configuration for the fake backend (a script of decisions) with
/// `level = "debug"`, `extra` appended, and a project with one file.
struct Fake {
    dir: tempfile::TempDir,
    config: PathBuf,
    scope: PathBuf,
}

impl Fake {
    /// `profile_tools` are added to the workspace profile's tools; `extra` is
    /// appended to the configuration.
    fn new(script: &[Value], profile_tools: &str, extra: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        // Short and inside the write set, as the host-tool suites do.
        std::fs::create_dir_all(dir.path().join("data/tmp")).unwrap();
        let script_path = dir.path().join("script.json");
        std::fs::write(&script_path, serde_json::to_string(script).unwrap()).unwrap();
        let config = dir.path().join("config.toml");
        std::fs::write(
            &config,
            format!(
                "config_version = 1\n[runtime]\ndata_dir = {:?}\n[backend]\nkind = \"fake\"\nscript = {:?}\n[logging]\nlevel = \"debug\"\n[policy.profiles.workspace]\ntools = [{BUILTIN}{profile_tools}]\n{extra}",
                dir.path().join("data").to_string_lossy(),
                script_path.to_string_lossy(),
            ),
        )
        .unwrap();
        let scope = dir.path().join("project");
        std::fs::create_dir_all(&scope).unwrap();
        std::fs::write(scope.join("a.txt"), "x\n").unwrap();
        Self { dir, config, scope }
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_mimlys"));
        cmd.arg("--config")
            .arg(&self.config)
            .args(args)
            .env("MIMLYS_CONFIG", "")
            .env("MIMLYS_WORKER_BIN", env!("CARGO_BIN_EXE_mimlys-worker"))
            .env("MIMLYS_TESTSERVER_STDERR_LINE", SERVER_LINE);
        cmd
    }

    fn json(&self, args: &[&str]) -> Value {
        let mut all = vec!["--json"];
        all.extend_from_slice(args);
        let out = run_bounded(&mut self.command(&all), self.dir.path());
        assert_eq!(out.code(), 0, "{}\n{}", out.stdout, out.stderr);
        serde_json::from_str(out.stdout.trim())
            .unwrap_or_else(|e| panic!("{e}: {}\n{}", out.stdout, out.stderr))
    }

    /// A run of a goal to its end: the task's id, the ids of its steps and of
    /// its actions in the stored events, and the log.
    fn run(&self, grant: Option<&str>) -> Ran {
        let scope = self.scope.to_string_lossy().into_owned();
        let mut all = vec!["--json", "run", "do it", "--scope", &scope];
        if let Some(grant) = grant {
            all.extend(["--grant", grant]);
        }
        let out = run_bounded(&mut self.command(&all), self.dir.path());
        assert_eq!(out.code(), 0, "{}\n{}", out.stdout, out.stderr);
        let task: Value = serde_json::from_str(out.stdout.trim()).unwrap();
        assert_eq!(task["task"]["state"], "completed", "{task}");
        let task_id = task["task"]["task_id"].as_str().unwrap().to_owned();
        let events = self.json(&["events", &task_id]);
        let (mut steps, mut actions) = (Vec::new(), Vec::new());
        collect(&events, "step_id", &mut steps);
        collect(&events, "action_id", &mut actions);
        Ran {
            task_id,
            steps,
            actions,
            lines: json_lines(&out.stderr),
            stderr: out.stderr,
        }
    }
}

struct Ran {
    task_id: String,
    steps: Vec<String>,
    actions: Vec<String>,
    lines: Vec<Value>,
    stderr: String,
}

impl Ran {
    /// Asserts that the line carries the ids of this task, of one of its steps and of one of its actions.
    fn assert_ids(&self, line: &Value) {
        let span = &line["span"];
        assert_eq!(
            span["name"], "step",
            "no step span on {line}\n{}",
            self.stderr
        );
        assert_eq!(span["task_id"], self.task_id.as_str(), "{line}");
        let step = span["step_id"].as_str().unwrap_or_else(|| panic!("{line}"));
        assert!(
            self.steps.iter().any(|s| s == step),
            "{line}: {:?}",
            self.steps
        );
        let action = span["action_id"]
            .as_str()
            .unwrap_or_else(|| panic!("no action id on {line}"));
        assert!(
            self.actions.iter().any(|a| a == action),
            "{line}: {:?}",
            self.actions
        );
    }
}

fn act(tool: &str, arguments: Value) -> Value {
    json!({"decision": "act", "tool": tool, "arguments": arguments})
}

fn complete() -> Value {
    json!({"decision": "complete", "summary": "done"})
}

/// REQ-OBS-004: a line that an MCP server writes on its standard error during
/// a call is logged by the client's reader task, which does not inherit the
/// step's span. It carries the ids of the task, the step and the action of
/// the call in flight when the reader read it. (The test server writes the
/// line and waits 300 ms before it answers, so that the line is read while
/// the call is in flight; a line read after the call ended has no ids, which
/// the unit test of `mimlys_core::log_span` covers.)
#[test]
fn req_obs_004_a_servers_standard_error_line_during_a_call_carries_the_ids() {
    let server = PathBuf::from(env!("CARGO_BIN_EXE_mimlys-mcp-testserver"));
    let extra = format!(
        "[policy.ceiling]\ntools = [{BUILTIN}, \"rypsok.echo_env\"]\n[servers.rypsok]\ncommand = {:?}\nenv_allow = [\"PATH\", \"MIMLYS_TESTSERVER_STDERR_LINE\"]\n[servers.rypsok.tools.echo_env]\neffect = \"external_read\"\nidempotent = true\n",
        server.to_string_lossy()
    );
    let fake = Fake::new(
        &[act("rypsok.echo_env", json!({})), complete()],
        ", \"rypsok.echo_env\"",
        &extra,
    );
    let ran = fake.run(None);
    let lines: Vec<&Value> = ran
        .lines
        .iter()
        .filter(|l| message(l) == SERVER_LINE)
        .collect();
    assert_eq!(lines.len(), 1, "the server's line, once:\n{}", ran.stderr);
    assert_eq!(lines[0]["fields"]["server"], "rypsok", "{}", lines[0]);
    ran.assert_ids(lines[0]);
}

/// REQ-OBS-004: the worker's standard-error reader is a thread of its own,
/// and the worker serves many steps. Its lines ("worker trace", written at
/// `debug`) about a host tool's execution carry the ids of the task, step and
/// action of the execution in flight when the thread read them. A line read
/// after the execution ended has no ids, so the test asks that the "received"
/// line (written when the execution starts, long before it ends) has them and
/// that every trace line that has `span` has the right ones. Needs the full
/// tier (the host tool is not offered below it), as the host-tool suites do,
/// and is skipped, counted, where there is not. Unix only: the Windows host
/// tools are `m6b_host_tools.rs`'s.
#[cfg(unix)]
#[test]
fn req_obs_004_a_host_tools_worker_line_carries_the_ids() {
    let tool = PathBuf::from(env!("CARGO_BIN_EXE_mimlys-probe-tool"));
    let extra = format!(
        "[host_tools.probe]\ncommand = {tool:?}\nread_paths = [{dir:?}]\nallowed_args = [\"ok\"]\narg_pattern = \"[a-z]+\"\nmax_args = 1\neffect = \"workspace_write\"\ntimeout_ms = 20000\n",
        dir = tool.parent().unwrap().to_string_lossy(),
    );
    let fake = Fake::new(
        &[
            act("run_host_tool", json!({"tool": "probe", "args": ["ok"]})),
            complete(),
        ],
        "",
        &extra,
    );
    let status = fake.json(&["status"]);
    if status["tier"] != "full" {
        common::skip(
            "req_obs_004_a_host_tools_worker_line_carries_the_ids",
            &format!("this machine is tier {}", status["tier"]),
        );
        return;
    }
    let ran = fake.run(Some("host_tool:probe"));
    let trace: Vec<&Value> = ran
        .lines
        .iter()
        .filter(|l| message(l).starts_with("worker trace: "))
        .collect();
    let received = trace
        .iter()
        .find(|l| message(l).contains("run_host_tool received"))
        .unwrap_or_else(|| panic!("no worker trace line of the execution:\n{}", ran.stderr));
    ran.assert_ids(received);
    for line in trace.iter().filter(|l| l.get("span").is_some()) {
        ran.assert_ids(line);
    }
}

/// REQ-PRIV-004 (external design §17.3): the switch is the request. With
/// `debug_prompts` on at the default level, `info`, the request and reply
/// lines are written all the same, redacted: the goal, the file's content
/// and the reply appear and the secret, which the reply repeats, never.
#[test]
fn req_priv_004_with_the_prompt_option_on_the_level_does_not_matter() {
    let server = leaking_server();
    let env = Env::new(&server, "level = \"info\"\ndebug_prompts = true");
    let (code, stdout, stderr) = env.run();
    assert_eq!(code, 0, "{stdout}\n{stderr}");
    let lines = json_lines(&stderr);
    let in_lines = |what: &str, needle: &str| {
        lines
            .iter()
            .any(|l| message(l) == what && l["fields"].to_string().contains(needle))
    };
    assert!(in_lines("model request message", GOAL), "{stderr}");
    assert!(in_lines("model request message", FILE), "{stderr}");
    assert!(in_lines("model reply", MODEL), "{stderr}");
    assert!(in_lines("model reply", "[redacted:"), "{stderr}");
    assert!(
        !stderr.contains(SECRET),
        "the secret reached the log:\n{stderr}"
    );
    assert!(
        lines.iter().all(|l| l["level"] != "DEBUG"
            || matches!(message(l), "model request message" | "model reply")),
        "only the prompt target is let through at debug: {stderr}"
    );
}
