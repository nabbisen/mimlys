//! The user's copy of a host tool's stream (handoff 004d; REQ-PERS-005), end to end through
//! `mimlys run` and the probe tool: a stream whose excerpt is a cut of it is stored whole as a
//! blob, within LIM-H4 (not LIM-S4, which is a script's bound), as the cleaned and redacted
//! text, with no secret the operator configured. Linux only: the host-tool suites of the other
//! platforms are `m6b_host_tools.rs` and `m6c_host_tools.rs`, and the choice of the bound is by
//! the tool's name, tested without a platform in `mimlys-core`.
#![cfg(target_os = "linux")]

use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;

mod common;
use common::run_bounded;

const SECRET: &str = "sk-live-0123456789abcdef";

fn probe_tool() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_mimlys-probe-tool"))
}

struct Env {
    registry: common::Registry,
    data: tempfile::TempDir,
    config: PathBuf,
}

impl Env {
    /// The probe tool declared as `probe`; a server that is never started and holds the secret
    /// the runtime must redact (a configured secret, as `env_secrets` makes one).
    fn new(script: &[Value]) -> Self {
        let data = tempfile::tempdir().unwrap();
        let config = data.path().join("config.toml");
        let script_path = data.path().join("script.json");
        std::fs::write(&script_path, serde_json::to_string(script).unwrap()).unwrap();
        let text = format!(
            r#"config_version = 1
[runtime]
data_dir = {data:?}
[backend]
kind = "fake"
script = {script:?}
[policy.profiles.workspace]
tools = ["read_file", "list_dir"]
[host_tools.probe]
command = {tool:?}
read_paths = [{tooldir:?}]
allowed_args = ["ok", "emit", "fill"]
arg_pattern = "[A-Za-z0-9_=:-]+"
max_args = 3
effect = "workspace_write"
timeout_ms = 60000
[servers.decoy]
command = {tool:?}
env_secrets = {{ TOKEN = "env:MIMLYS_TEST_TOKEN" }}
"#,
            data = data.path().join("data").to_string_lossy(),
            script = script_path.to_string_lossy(),
            tool = probe_tool().to_string_lossy(),
            tooldir = probe_tool().parent().unwrap().to_string_lossy(),
        );
        std::fs::write(&config, text).unwrap();
        Self {
            registry: common::Registry::new(),
            data,
            config,
        }
    }

    fn data_dir(&self) -> PathBuf {
        self.data.path().join("data")
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut cmd = common::mimlys_in(&self.registry);
        cmd.arg("--config")
            .arg(&self.config)
            .args(args)
            .env("MIMLYS_CONFIG", "")
            .env("MIMLYS_WORKER_BIN", env!("CARGO_BIN_EXE_mimlys-worker"))
            .env("MIMLYS_TEST_TOKEN", SECRET);
        cmd
    }

    fn json(&self, args: &[&str]) -> (i32, Value) {
        let mut all = vec!["--json"];
        all.extend_from_slice(args);
        let out = run_bounded(&mut self.command(&all), self.data.path());
        let value = serde_json::from_str(out.stdout.trim())
            .unwrap_or_else(|e| panic!("not JSON ({e}): {}\n{}", out.stdout, out.stderr));
        (out.code(), value)
    }

    fn scope(&self) -> PathBuf {
        let dir = self.data.path().join("scope");
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/lib.rs"), "fn a() {}\n").unwrap();
        dir
    }
}

fn emit(bytes: usize) -> Value {
    json!({"decision": "act", "tool": "run_host_tool",
           "arguments": {"tool": "probe", "args": ["emit", bytes.to_string(), SECRET]}})
}

/// The tier is `full` or the host-tool tests skip, as every suite of them does.
fn full_tier(env: &Env, test: &str) -> bool {
    let (_, status) = env.json(&["status"]);
    let tier = status["tier"].as_str().unwrap_or("").to_owned();
    if tier != "full" {
        common::skip(test, &format!("this machine is tier {tier}"));
        return false;
    }
    true
}

/// The rows of the store, read as a second process would.
fn query(data: &Path, sql: &str) -> Vec<String> {
    let conn = rusqlite::Connection::open(data.join("mimlys.db")).unwrap();
    let mut stmt = conn.prepare(sql).unwrap();
    stmt.query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

fn run_emit(test: &str, bytes: usize) -> Option<(Env, Vec<Value>)> {
    let env = Env::new(&[
        emit(bytes),
        json!({"decision": "complete", "summary": "done"}),
    ]);
    if !full_tier(&env, test) {
        return None;
    }
    let scope = env.scope();
    let (code, value) = env.json(&[
        "run",
        "emit",
        "--scope",
        scope.to_str().unwrap(),
        "--grant",
        "host_tool:probe",
    ]);
    assert_eq!(code, 0, "{value}");
    let executions: Vec<Value> = query(
        &env.data_dir(),
        "select record from executions order by rowid",
    )
    .iter()
    .map(|r| serde_json::from_str(r).unwrap())
    .collect();
    Some((env, executions))
}

/// What `mimlys-probe-tool emit <bytes> <word>` prints to standard output: the same lines, so
/// that the test compares the whole text and not a part of it.
fn emitted(want: usize, word: &str) -> String {
    let mut text = String::new();
    let mut n = 0u64;
    while text.len() < want {
        match n {
            400 => text.push_str(&format!("the word is {word} here\n")),
            401 => text.push_str("progress 50%\rprogress 100%\n"),
            _ => text.push_str(&format!("line {n:07} of the tool's output\n")),
        }
        n += 1;
    }
    text
}

/// REQ-PERS-005: a host tool's stream whose excerpt is a cut of it is stored whole, as a blob
/// that its execution names: the text the tool printed, cleaned and redacted (a carriage return as
/// printed, the operator's secret replaced by its marker), where the observation holds the
/// excerpt. Standard error, which fits, has none. The size, 3 MB, is above what the worker's frame
/// could carry in 0.11 and earlier (about 600 KB of text) and below LIM-H4.
#[test]
fn req_pers_005_a_host_tools_cut_stream_is_stored_whole_without_the_operators_secret() {
    let bytes = 3_000_000;
    let Some((env, executions)) = run_emit(
        "req_pers_005_a_host_tools_cut_stream_is_stored_whole_without_the_operators_secret",
        bytes,
    ) else {
        return;
    };
    let blob = executions[0]["stdout_blob"]
        .as_str()
        .expect("a blob is named");
    assert!(
        executions[0]["stderr_blob"].is_null(),
        "a stream that fits writes none"
    );
    let stored = std::fs::read_to_string(env.data_dir().join("blobs").join(blob)).unwrap();
    let expected = emitted(bytes, SECRET).replace(SECRET, "[redacted:servers.decoy.env.TOKEN]");
    assert_eq!(stored, expected);
    assert!(!stored.contains(SECRET), "a secret never reaches the disk");
    assert!(stored.contains("progress 50%\rprogress 100%"));
    let observation: Value = serde_json::from_str(
        &query(
            &env.data_dir(),
            "select record from observations order by rowid",
        )[0],
    )
    .unwrap();
    let excerpt = observation["stdout"].as_str().unwrap();
    assert!(excerpt.chars().count() < 4100 && !excerpt.contains("line 0000200"));
    assert!(!excerpt.contains(SECRET));
    assert_eq!(
        observation["stderr"],
        format!("emitted {} lines\ndone\n", emitted_lines(bytes))
    );
    assert_eq!(
        query(&env.data_dir(), "select blob_id from blobs"),
        [blob.to_owned()]
    );
}

fn emitted_lines(want: usize) -> u64 {
    let mut len = 0usize;
    let mut n = 0u64;
    while len < want {
        len += match n {
            400 => format!("the word is {SECRET} here\n").len(),
            401 => "progress 50%\rprogress 100%\n".len(),
            _ => format!("line {n:07} of the tool's output\n").len(),
        };
        n += 1;
    }
    n
}

/// REQ-PERS-005: a host tool whose output fits its excerpt writes no blob.
#[test]
fn req_pers_005_a_host_tools_output_that_fits_writes_no_blob() {
    let Some((env, executions)) = run_emit(
        "req_pers_005_a_host_tools_output_that_fits_writes_no_blob",
        200,
    ) else {
        return;
    };
    assert!(executions[0]["stdout_blob"].is_null() && executions[0]["stderr_blob"].is_null());
    assert!(query(&env.data_dir(), "select blob_id from blobs").is_empty());
}

/// The run of the probe tool's `fill <bytes> <hex>`, through `mimlys run` and the real worker.
fn run_fill(test: &str, bytes: usize, hex: &str) -> Option<(Env, Vec<Value>)> {
    let env = Env::new(&[
        json!({"decision": "act", "tool": "run_host_tool",
               "arguments": {"tool": "probe", "args": ["fill", bytes.to_string(), hex]}}),
        json!({"decision": "complete", "summary": "done"}),
    ]);
    if !full_tier(&env, test) {
        return None;
    }
    let scope = env.scope();
    let (code, value) = env.json(&[
        "run",
        "fill",
        "--scope",
        scope.to_str().unwrap(),
        "--grant",
        "host_tool:probe",
    ]);
    assert_eq!(code, 0, "{value}");
    let executions = query(
        &env.data_dir(),
        "select record from executions order by rowid",
    )
    .iter()
    .map(|r| serde_json::from_str(r).unwrap())
    .collect();
    Some((env, executions))
}

/// `mimlys --json task output <task>` over the store the run made: the index.
fn index_of(env: &Env) -> Value {
    let task = query(&env.data_dir(), "select task_id from tasks").remove(0);
    let (code, index) = env.json(&["task", "output", &task]);
    assert_eq!(code, 0, "{index}");
    index
}

fn blob_of(env: &Env, executions: &[Value]) -> Vec<u8> {
    let blob = executions[0]["stdout_blob"]
        .as_str()
        .expect("the execution names a blob");
    std::fs::read(env.data_dir().join("blobs").join(blob)).unwrap()
}

/// REQ-PERS-005, LIM-H4 (handoff 004d-4): a host tool that prints exactly LIM-H4 (4 MiB) is
/// stored whole, through the real worker: the record is carried (the frame limit follows the
/// bounds in force), the execution ends `exited`, the task completes, the blob holds every byte,
/// and the index says the capture was not cut. A worker that could not send more than about
/// 600 KB of text ended the execution instead (0.11 and earlier).
#[test]
fn req_pers_005_a_host_tool_that_prints_exactly_its_bound_is_stored_whole() {
    let bound = mimlys_core::limits::LIM_H4_HOST_OUTPUT_BYTES;
    let Some((env, executions)) = run_fill(
        "req_pers_005_a_host_tool_that_prints_exactly_its_bound_is_stored_whole",
        bound,
        "78",
    ) else {
        return;
    };
    assert_eq!(executions[0]["termination"], "exited", "{}", executions[0]);
    let stored = blob_of(&env, &executions);
    assert_eq!(stored.len(), bound);
    assert!(stored.iter().all(|b| *b == b'x'));
    let stdout = &index_of(&env)["copies"][1];
    assert_eq!(stdout["bytes"], bound);
    assert_eq!(stdout["capture_cut"], false, "{stdout}");
    assert!(stdout.get("captured_bytes").is_none(), "{stdout}");
}

/// REQ-PERS-005, LIM-H4 (handoff 004d-4): a host tool that prints past its bound is not ended (it
/// is drained); the copy holds the bound, the first bytes it printed, and the index says the
/// capture was cut and how many bytes it kept.
#[test]
fn req_pers_005_a_host_tool_that_prints_past_its_bound_has_a_copy_that_says_the_capture_was_cut() {
    let bound = mimlys_core::limits::LIM_H4_HOST_OUTPUT_BYTES;
    let Some((env, executions)) = run_fill(
        "req_pers_005_a_host_tool_that_prints_past_its_bound_has_a_copy_that_says_the_capture_was_cut",
        bound + 5000,
        "78",
    ) else {
        return;
    };
    assert_eq!(executions[0]["termination"], "exited");
    assert_eq!(blob_of(&env, &executions).len(), bound);
    let stdout = &index_of(&env)["copies"][1];
    assert_eq!(stdout["capture_cut"], true, "{stdout}");
    assert_eq!(stdout["captured_bytes"], bound, "{stdout}");
    assert_eq!(stdout["bytes"], bound, "{stdout}");
}

/// REQ-PERS-005 (handoff 004d-4): a host tool's capture of bytes that are no text, up to LIM-H4,
/// is stored whole: three times the bound (U+FFFD for each byte), never cut again.
#[test]
fn req_pers_005_a_host_tools_invalid_bytes_up_to_the_bound_are_stored_whole() {
    let bound = mimlys_core::limits::LIM_H4_HOST_OUTPUT_BYTES;
    let Some((env, executions)) = run_fill(
        "req_pers_005_a_host_tools_invalid_bytes_up_to_the_bound_are_stored_whole",
        bound,
        "ff",
    ) else {
        return;
    };
    let stored = blob_of(&env, &executions);
    assert_eq!(stored.len(), 3 * bound);
    assert!(stored.chunks(3).all(|c| c == [0xef, 0xbf, 0xbd]));
    let stdout = &index_of(&env)["copies"][1];
    assert_eq!(stdout["bytes"], 3 * bound);
    assert_eq!(stdout["capture_cut"], false, "{stdout}");
}
