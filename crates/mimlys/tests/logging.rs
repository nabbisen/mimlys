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
