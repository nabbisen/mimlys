//! M6a tests (RFC 011; REQ-TEST-013, REQ-CONF-004 … 009) over the probe
//! tool (`mimlys-probe-tool`) and, when a Rust toolchain is installed, the
//! demonstration of external design §23: `cargo test --offline` of a native
//! project under every mandatory clause. Linux only: the Windows suite is
//! `m6b_host_tools.rs`.
#![cfg(target_os = "linux")]

use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;

mod common;
use common::run_bounded;

fn probe_tool() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_mimlys-probe-tool"))
}

/// A directory outside every read set of the worker.
fn outside() -> PathBuf {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/m6a-outside");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("secret.txt"), "s").unwrap();
    dir.canonicalize().unwrap()
}

const BUILTIN: &str = r#""read_file", "write_file", "replace_in_file", "list_dir", "search_files", "delete_path", "run_script", "run_wasm""#;

struct Env {
    data: tempfile::TempDir,
    config: PathBuf,
    extra_env: Vec<(String, String)>,
}

impl Env {
    /// The probe tool declared as `probe` (offline) and `fetch` (the
    /// dependency step); `run_host_tool` is visible through the tasks' `host_tool` grants.
    fn new(script: &[Value], extra: &str) -> Self {
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
{extra}
[policy.profiles.workspace]
tools = [{builtin}]
[host_tools.probe]
command = {tool:?}
read_paths = [{tooldir:?}]
allowed_args = ["ok", "cwd", "read", "write", "connect", "env", "setsid-sleep", "fork", "alloc"]
arg_pattern = "[A-Za-z0-9_./:=-]+"
max_args = 3
effect = "workspace_write"
timeout_ms = 20000
env = {{ PROBE_OK = "1" }}
env_allow = ["MIMLYS_TEST_RUNTIME_PID", "MIMLYS_TEST_OWNER_PID"]
[host_tools.strict]
command = {tool:?}
read_paths = [{tooldir:?}]
allowed_args = ["ok"]
arg_pattern = "[a-z]+"
max_args = 3
effect = "workspace_write"
timeout_ms = 20000
[host_tools.fetch]
command = {tool:?}
read_paths = [{tooldir:?}]
allowed_args = ["connect", "read"]
arg_pattern = "[A-Za-z0-9_./:=-]+"
max_args = 2
effect = "external_read"
timeout_ms = 20000
network = true
"#,
            data = data.path().join("data").to_string_lossy(),
            script = script_path.to_string_lossy(),
            builtin = BUILTIN,
            tool = probe_tool().to_string_lossy(),
            tooldir = probe_tool().parent().unwrap().to_string_lossy(),
        );
        std::fs::write(&config, text).unwrap();
        Self {
            data,
            config,
            extra_env: vec![],
        }
    }

    fn env(mut self, key: &str, value: &str) -> Self {
        self.extra_env.push((key.into(), value.into()));
        self
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_mimlys"));
        cmd.arg("--config")
            .arg(&self.config)
            .args(args)
            .env("MIMLYS_CONFIG", "")
            .env("MIMLYS_WORKER_BIN", env!("CARGO_BIN_EXE_mimlys-worker"))
            .env("MIMLYS_TEST_SECRET", "operator-secret-value-1234")
            .stdin(std::process::Stdio::null());
        for (k, v) in &self.extra_env {
            cmd.env(k, v);
        }
        cmd
    }

    fn run(&self, args: &[&str]) -> (i32, String, String) {
        let out = run_bounded(&mut self.command(args), self.data.path());
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

    fn scope(&self) -> PathBuf {
        let dir = self.data.path().join("scope");
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/lib.rs"), "fn a() {}\n").unwrap();
        dir
    }

    fn tree(&self, id: &str) -> PathBuf {
        self.data
            .path()
            .join("data/workspaces")
            .join(id)
            .join("tree")
    }

    fn events(&self, id: &str) -> Vec<Value> {
        let (_, events) = self.json(&["events", id]);
        events
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["record"].clone())
            .collect()
    }

    fn observations(&self, id: &str) -> Vec<Value> {
        let conn = rusqlite::Connection::open(self.data.path().join("data/mimlys.db")).unwrap();
        let mut stmt = conn
            .prepare("select o.record from observations o join actions a on a.action_id = o.action_id where a.task_id = ?1 order by o.rowid")
            .unwrap();
        stmt.query_map([id], |r| r.get::<_, String>(0))
            .unwrap()
            .map(|r| serde_json::from_str(&r.unwrap()).unwrap())
            .collect()
    }
}

fn strict(args: &[&str]) -> Value {
    json!({"decision": "act", "tool": "run_host_tool", "arguments": {"tool": "strict", "args": args}})
}

fn probe(args: &[&str]) -> Value {
    json!({"decision": "act", "tool": "run_host_tool", "arguments": {"tool": "probe", "args": args}})
}

fn complete() -> Value {
    json!({"decision": "complete", "summary": "done"})
}

fn stdout_of(observation: &Value) -> String {
    observation["stdout"].as_str().unwrap_or("").to_owned()
}

fn exit_code_of(observation: &Value) -> i64 {
    observation["payload"]["exit_code"].as_i64().unwrap_or(-99)
}

/// The clause table as `status` reports it, by letter → description.
fn clauses(env: &Env) -> (String, std::collections::BTreeMap<String, Value>) {
    let (_, status) = env.json(&["status"]);
    let tier = status["tier"]
        .as_str()
        .unwrap_or_else(|| panic!("no tier in {status}"))
        .to_owned();
    let map = status["clauses"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    (tier, map)
}

/// REQ-TEST-013 / RFC 011 §2: for every clause the probe reports enforced,
/// the forbidden thing fails as expected inside a host tool, and the
/// allowed thing works.
#[test]
fn m6a_clauses_hold_inside_a_host_tool() {
    let outside = outside();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let env = Env::new(
        &[
            probe(&["ok"]),
            probe(&["cwd"]),
            probe(&["read", outside.join("secret.txt").to_str().unwrap()]),
            probe(&["write", outside.join("w.txt").to_str().unwrap()]),
            probe(&["write", "inside.txt"]),
            probe(&["connect", &format!("127.0.0.1:{port}")]),
            probe(&["env", "MIMLYS_TEST_SECRET"]),
            probe(&["env", "PROBE_OK"]),
            probe(&["fork", "100"]),
            probe(&["alloc", "3000"]),
            complete(),
        ],
        "",
    );
    let (tier, table) = clauses(&env);
    eprintln!("tier {tier}; clauses {table:?}");
    if tier != "full" {
        common::skip(
            "m6a_clauses_hold_inside_a_host_tool",
            &format!("this machine is tier {tier} ({table:?})"),
        );
        return;
    }
    let scope = env.scope();
    let (code, value) = env.json(&[
        "run",
        "probe the clauses",
        "--scope",
        scope.to_str().unwrap(),
        "--grant",
        "host_tool:probe",
    ]);
    assert_eq!(code, 0, "{value}");
    assert_eq!(value["task"]["state"], "completed", "{value}");
    let id = value["task"]["task_id"].as_str().unwrap().to_owned();
    let obs = env.observations(&id);
    assert_eq!(obs.len(), 10, "{}", obs.len());
    assert_eq!(exit_code_of(&obs[0]), 0);
    assert_eq!(stdout_of(&obs[0]).trim(), "ok");
    assert_eq!(
        stdout_of(&obs[1]).trim(),
        env.tree(&id).canonicalize().unwrap().to_string_lossy(),
        "cwd is the workspace tree"
    );
    // F: outside the read set, no read and no write; inside the workspace, a write.
    assert!(
        stdout_of(&obs[2]).contains("read failed: permission denied"),
        "{}",
        stdout_of(&obs[2])
    );
    assert!(
        stdout_of(&obs[3]).contains("write failed: permission denied"),
        "{}",
        stdout_of(&obs[3])
    );
    assert_eq!(stdout_of(&obs[4]).trim(), "wrote");
    assert!(env.tree(&id).join("inside.txt").is_file());
    assert!(!outside.join("w.txt").exists());
    // N: no socket at all, loopback included (the listener is alive and would accept).
    assert!(
        stdout_of(&obs[5]).contains("connect failed: permission denied"),
        "{}",
        stdout_of(&obs[5])
    );
    drop(listener);
    // E: the operator's variable is absent; the declared one is present.
    assert!(stdout_of(&obs[6]).contains("MIMLYS_TEST_SECRET unset"));
    assert!(stdout_of(&obs[7]).contains("PROBE_OK=1"));
    // C and M: per the primitive in force.
    let c = table["count"]["state"].as_str().unwrap_or("");
    if c == "enforced" {
        assert_ne!(
            exit_code_of(&obs[8]),
            0,
            "pids.max bounds the tree: {}",
            stdout_of(&obs[8])
        );
    }
    assert_ne!(
        exit_code_of(&obs[9]),
        0,
        "3000 MiB above LIM-H2 ends the tool: {}",
        stdout_of(&obs[9])
    );
    let finished: Vec<String> = env
        .events(&id)
        .iter()
        .filter(|e| e["event"] == "action.finished")
        .map(|e| e["status"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(finished.len(), 10);
}

/// REQ-CONF-009 / RFC 011 §2 (K): a tool that `setsid`s a child and sleeps
/// is terminated with its whole tree when the task is cancelled, within LIM-O2.
/// REQ-CLI-005 (second signal): two stop signals to `mimlys run` cancel the task
/// (`cancelled`), the process exits and no process of the worker's tree is left.
#[test]
fn m6a_cancel_kills_the_whole_tree() {
    cancel_kills_the_whole_tree(false, "1234", None);
}

/// The same, on the `subreaper` rung of clause K (the rung a runner without
/// a delegated cgroup gets): the worker's read set holds no `/proc`
/// (REQ-CONF-011), so the descendants a cancelled tool left re-parented to
/// the worker are swept by the runtime. A different sleep length keeps the
/// two tests' processes apart when they run side by side.
/// REQ-CLI-005 (second signal): as above, on this rung.
#[test]
fn m6a_cancel_kills_the_whole_tree_on_the_subreaper_rung() {
    cancel_kills_the_whole_tree(true, "1235", Some("subreaper"));
}

fn cancel_kills_the_whole_tree(no_cgroup: bool, secs: &str, primitive: Option<&str>) {
    let name = if no_cgroup {
        "m6a_cancel_kills_the_whole_tree_on_the_subreaper_rung"
    } else {
        "m6a_cancel_kills_the_whole_tree"
    };
    let mut env = Env::new(&[probe(&["setsid-sleep", secs]), complete()], "");
    if no_cgroup {
        env = env.env("MIMLYS_PROBE_NO_CGROUP", "1");
    }
    let (tier, table) = clauses(&env);
    if tier != "full" {
        common::skip(name, &format!("tier {tier}"));
        return;
    }
    if let Some(primitive) = primitive {
        assert_eq!(
            table["termination"]["primitive"], primitive,
            "the knob selects the rung under test: {table:?}"
        );
    }
    let scope = env.scope();
    let err_path = env.data.path().join("run.stderr");
    let out_path = env.data.path().join("run.stdout");
    let mut run = env
        .command(&[
            "--json",
            "run",
            "sleep",
            "--scope",
            scope.to_str().unwrap(),
            "--grant",
            "host_tool:probe",
        ])
        .stdout(std::fs::File::create(&out_path).unwrap())
        .stderr(std::fs::File::create(&err_path).unwrap())
        .spawn()
        .unwrap();
    // Wait until the sleepers exist, then stop the run twice: the first
    // signal asks for a pause at the next boundary, the second cancels the
    // running step (external design §19.2); the store lock admits one
    // runtime at a time, so this is how a user cancels from the terminal.
    assert!(
        wait_for(std::time::Duration::from_secs(60), || sleepers(secs) >= 2),
        "the host tool and its setsid child run"
    );
    let pid = run.id().to_string();
    let signal = || {
        let _ = Command::new("kill").args(["-INT", &pid]).status();
    };
    // The second signal is sent when the first has been taken (the runtime says
    // so on its standard error), not after a pause that a slow machine could
    // outlast: two signals taken as one would ask for a pause and never cancel.
    signal();
    assert!(
        wait_for(std::time::Duration::from_secs(60), || {
            std::fs::read_to_string(&err_path).is_ok_and(|t| t.contains("stop signal: pausing"))
        }),
        "the first signal was never taken: {}",
        std::fs::read_to_string(&err_path).unwrap_or_default()
    );
    let cancel_at = std::time::Instant::now();
    signal();
    // The runtime's exit is waited for with a bound, as `run_bounded` does,
    // and the bound is far longer than the cancellation it waits for (the
    // stream files are read after, whatever a grandchild still holds): a hung
    // runtime fails this test with its output instead of hanging the binary.
    let mut exited = false;
    wait_for(common::BOUND, || {
        exited = run.try_wait().unwrap().is_some();
        exited
    });
    if !exited {
        let _ = run.kill();
        let _ = run.wait();
        panic!(
            "the run did not exit within {} s of the second signal\nstdout: {}\nstderr: {}",
            common::BOUND.as_secs(),
            std::fs::read_to_string(&out_path).unwrap_or_default(),
            std::fs::read_to_string(&err_path).unwrap_or_default()
        );
    }
    assert!(
        wait_for(std::time::Duration::from_secs(60), || sleepers(secs) == 0),
        "every sleeper, the setsid one included, is gone"
    );
    // Within LIM-O2 and the runtime's own grace (five times the limit): a
    // margin no stall of a slow machine reaches.
    assert!(cancel_at.elapsed() < std::time::Duration::from_secs(10));
    let stdout = std::fs::read_to_string(&out_path).unwrap_or_default();
    let value: Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|_| panic!("{}", std::fs::read_to_string(&err_path).unwrap_or_default()));
    let id = value["task"]["task_id"].as_str().unwrap().to_owned();
    let (_, task) = env.json(&["task", "show", &id]);
    assert_eq!(task["task"]["state"], "cancelled");
}

/// Polls `done` every 25 ms until it holds or `limit` has passed (the tests
/// decide by the state this waits for, never by how long it took; the limits
/// they give are far longer than anything they wait for, handoff 011i).
fn wait_for(limit: std::time::Duration, mut done: impl FnMut() -> bool) -> bool {
    let started = std::time::Instant::now();
    while started.elapsed() < limit {
        if done() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    done()
}

/// Whether the non-blocking `accept` finds a connection within 60 s.
fn accepted<T>(mut accept: impl FnMut() -> std::io::Result<T>) -> bool {
    wait_for(std::time::Duration::from_secs(60), || accept().is_ok())
}

/// REQ-CLI-005 (first signal): one stop signal to `mimlys run` while a step runs pauses
/// the task at the next boundary. The running step is not interrupted (its action
/// finishes), the next step does not start, the process exits by itself with the task
/// `paused`, and the signal is acknowledged on standard error. The step takes about
/// three seconds, less than the grace after which a pause that has not taken effect
/// becomes a cancellation (LIM-O1 less one second), so the pause is the outcome; the
/// test decides by the task's recorded state and the process's exit, and the one wait
/// it bounds is far longer than anything it waits for.
#[test]
fn req_cli_005_one_stop_signal_pauses_at_the_next_boundary() {
    let secs = "2.718";
    let env = Env::new(
        &[probe(&["setsid-sleep", secs]), probe(&["ok"]), complete()],
        "",
    );
    let (tier, _) = clauses(&env);
    if tier != "full" {
        common::skip(
            "req_cli_005_one_stop_signal_pauses_at_the_next_boundary",
            &format!("tier {tier}"),
        );
        return;
    }
    let mut run = Followed::start(&env, "pause me", secs, 1);
    run.interrupt();
    let status = run.exit(&env, secs);
    let stderr = run.stderr();
    assert!(
        stderr.contains("pausing at the next step"),
        "{status:?} {stderr}"
    );
    let id = run.task_id();
    let (_, task) = env.json(&["task", "show", &id]);
    assert_eq!(task["task"]["state"], "paused", "{task}");
    let finished: Vec<String> = env
        .events(&id)
        .iter()
        .filter(|e| e["event"] == "action.finished")
        .map(|e| e["status"].as_str().unwrap().to_owned())
        .collect();
    // Not asserted (§3c: no verdict by a short time); shown with `--nocapture`:
    // how long the step took from its action's start, against the grace of
    // 4000 ms after which a pause not yet applied cancels the execution.
    let events = env.events(&id);
    let at = |name: &str| {
        let e = events.iter().find(|e| e["event"] == name)?;
        let t = e["at"].as_str()?.get(11..23)?.to_owned();
        let n = |a: usize, b: usize| t.get(a..b)?.parse::<u64>().ok();
        Some(((n(0, 2)? * 60 + n(3, 5)?) * 60 + n(6, 8)?) * 1000 + n(9, 12)?)
    };
    if let (Some(from), Some(to)) = (at("action.started"), at("step.finished")) {
        eprintln!("the step took {} ms (the grace is 4000 ms)", to - from);
    }
    // The step is about a second shorter than the grace, but a machine that
    // stalls for that second would make it the other case (the watchdog said so on
    // standard error): the test does not decide by that margin. Either way the
    // run exited, the task is paused, and one action ran, no further one began.
    let expected = if stderr.contains("its execution was cancelled") {
        "cancelled"
    } else {
        "succeeded"
    };
    assert_eq!(
        finished,
        [expected],
        "the running step ended (finished, or cancelled at the grace) and no further action began"
    );
}

/// REQ-CLI-005 (first signal, during the model request): the pause waits for the
/// request in flight and for the step it belongs to; the request is held 3 s (less
/// than the grace of 4 s), so the step goes on to its action, finishes it, and the task
/// pauses at the boundary after it. Judged by state: the signal is sent when the
/// store has the step's start and not yet the decision, the run exits, the task is
/// `paused`, and the one action that ran is not followed by another. (A stall of the
/// machine past the grace would cancel the request instead: the verdict
/// follows what standard error says, as in the test of the running tool.)
#[test]
fn req_cli_005_one_stop_signal_during_the_model_request_pauses_after_the_step() {
    let env = Env::new(
        &[
            json!({"raw": probe(&["ok"]).to_string(), "delay_ms": 3000}),
            probe(&["ok"]),
            complete(),
        ],
        "",
    );
    let (tier, _) = clauses(&env);
    if tier != "full" {
        common::skip(
            "req_cli_005_one_stop_signal_during_the_model_request_pauses_after_the_step",
            &format!("tier {tier}"),
        );
        return;
    }
    let mut run = Followed::spawn(&env, "pause me early");
    run.await_state(&env, "the step started", "", || {
        count_events(env.data.path(), "step.started") >= 1
    });
    assert_eq!(
        count_events(env.data.path(), "decision.received"),
        0,
        "the model request is still in flight when the signal is sent"
    );
    run.interrupt();
    let status = run.exit(&env, "");
    let stderr = run.stderr();
    assert!(
        stderr.contains("pausing at the next step"),
        "{status:?} {stderr}"
    );
    let id = run.task_id();
    let (_, task) = env.json(&["task", "show", &id]);
    assert_eq!(task["task"]["state"], "paused", "{task}");
    let finished = finished_statuses(&env, &id);
    assert!(
        finished.len() <= 1
            && finished
                .iter()
                .all(|s| s == "succeeded" || s == "cancelled"),
        "no action began after the pause: {finished:?}"
    );
}

/// REQ-CLI-005 / LIM-O1 (first signal, a model request that outlasts the grace): the
/// request is held 6 s. The signal asks for a pause; the grace passes and the request
/// is cancelled, the step ends `cancelled` before any action began, the pause is
/// taken at that boundary, and the run exits with the task `paused`. Before handoff
/// 010d the cancellation outlived the step and the run went round its loop for good.
/// Judged by state: the exit, the task `paused`, no action in the store.
#[test]
fn req_cli_005_one_stop_signal_cancels_a_model_request_that_outlasts_the_grace() {
    let env = Env::new(
        &[
            json!({"raw": probe(&["ok"]).to_string(), "delay_ms": 6000}),
            probe(&["ok"]),
            complete(),
        ],
        "",
    );
    let mut run = Followed::spawn(&env, "pause me early and late");
    run.await_state(&env, "the step started", "", || {
        count_events(env.data.path(), "step.started") >= 1
    });
    run.interrupt();
    let status = run.exit(&env, "");
    let stderr = run.stderr();
    assert!(
        stderr.contains("pausing at the next step")
            && stderr.contains("its execution was cancelled"),
        "{status:?} {stderr}"
    );
    let id = run.task_id();
    let (_, task) = env.json(&["task", "show", &id]);
    assert_eq!(task["task"]["state"], "paused", "{task}");
    assert_eq!(
        count_events(env.data.path(), "action.started"),
        0,
        "the request was cancelled before the step had an action"
    );
    let steps: Vec<String> = env
        .events(&id)
        .iter()
        .filter(|e| e["event"] == "step.finished")
        .map(|e| e["outcome"].as_str().unwrap_or("").to_owned())
        .collect();
    assert_eq!(steps, ["cancelled"], "the one step ended cancelled");
}

/// The `action.finished` statuses of the task, in order.
fn finished_statuses(env: &Env, id: &str) -> Vec<String> {
    env.events(id)
        .iter()
        .filter(|e| e["event"] == "action.finished")
        .map(|e| e["status"].as_str().unwrap().to_owned())
        .collect()
}

/// How many events named `name` the store holds, as a second process reads it
/// (read-only); 0 while the store does not exist yet.
fn count_events(data: &Path, name: &str) -> usize {
    rusqlite::Connection::open_with_flags(
        data.join("data/mimlys.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .and_then(|conn| {
        conn.query_row("select count(*) from events where name = ?1", [name], |r| {
            r.get::<_, i64>(0)
        })
    })
    .map_or(0, |n| usize::try_from(n).unwrap_or(0))
}

/// REQ-CLI-005 / LIM-O1 (first signal, a step that outlasts the grace): one stop
/// signal while a step runs that would take two minutes. The pause cannot be taken
/// at a boundary within LIM-O1, so the step's execution is cancelled when the grace
/// (LIM-O1 less one second) has passed; the cancelled step is a boundary, the pause
/// is taken there, and the process exits with the task `paused`. Judged by state: the
/// process exits (the bound is far longer than the grace), the action is `cancelled`
/// (the sleeper did not run its two minutes), the task is `paused`, and nothing of
/// the worker's tree is left. Main's CI run 43338280616 showed a run alive 120 s
/// after one signal; a cancelled step left the cancellation set for good and the
/// run went round `drive`'s loop without ever taking the pause (handoff 010d).
/// The action is a workspace write (the probe tool's effect), which can be
/// undone: it is interrupted and rolled back, as before handoff 010e.
#[test]
fn req_cli_005_one_stop_signal_cancels_a_step_that_outlasts_the_grace() {
    let secs = "123.45";
    let env = Env::new(
        &[probe(&["setsid-sleep", secs]), probe(&["ok"]), complete()],
        "",
    );
    let (tier, _) = clauses(&env);
    if tier != "full" {
        common::skip(
            "req_cli_005_one_stop_signal_cancels_a_step_that_outlasts_the_grace",
            &format!("tier {tier}"),
        );
        return;
    }
    let mut run = Followed::start(&env, "pause me late", secs, 2);
    run.interrupt();
    let status = run.exit(&env, secs);
    let stderr = run.stderr();
    assert!(
        stderr.contains("pausing at the next step")
            && stderr.contains("its execution was cancelled"),
        "{status:?} {stderr}"
    );
    assert!(
        wait_for(std::time::Duration::from_secs(60), || sleepers(secs) == 0),
        "every sleeper, the setsid one included, is gone"
    );
    let id = run.task_id();
    let (_, task) = env.json(&["task", "show", &id]);
    assert_eq!(task["task"]["state"], "paused", "{task}");
    let finished: Vec<String> = env
        .events(&id)
        .iter()
        .filter(|e| e["event"] == "action.finished")
        .map(|e| e["status"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(
        finished,
        ["cancelled"],
        "the step was cancelled and no further action began"
    );
    // REQ-CLI-005 (handoff 010e): a workspace write is what a stop signal may
    // interrupt, because it is undone: the interrupted action's workspace is
    // rolled back, and its outcome is not unknown.
    assert_eq!(
        count_events(env.data.path(), "workspace.rollback"),
        1,
        "the interrupted workspace write was rolled back"
    );
    let observations = env.observations(&id);
    assert_eq!(observations.len(), 1, "{observations:?}");
    assert_eq!(observations[0]["rolled_back"], true, "{}", observations[0]);
    assert!(task["unknown_actions"].as_array().unwrap().is_empty());
}

/// A `mimlys run` started for a stop-signal test: its streams go to files
/// (as `run_bounded` does and as the cancel test's do), so that whatever the run
/// did is still there when the test gives up.
struct Followed {
    run: std::process::Child,
    out_path: PathBuf,
    err_path: PathBuf,
}

impl Followed {
    /// Starts `run <goal>` of the probe tool and waits until `sleepers_up`
    /// sleepers of `secs` (the host tool's step is running) are there.
    fn start(env: &Env, goal: &str, secs: &str, sleepers_up: usize) -> Self {
        let mut followed = Self::spawn(env, goal);
        followed.await_state(env, "the host tool's step never started", secs, || {
            sleepers(secs) >= sleepers_up
        });
        followed
    }

    /// Waits until `reached` holds; a run that never gets there fails the
    /// test with its evidence (the bound is far longer than any start).
    fn await_state(&mut self, env: &Env, what: &str, secs: &str, reached: impl FnMut() -> bool) {
        if !wait_for(std::time::Duration::from_secs(120), reached) {
            self.give_up(env, what, secs);
        }
    }

    /// Starts `run <goal>` of the probe tool.
    fn spawn(env: &Env, goal: &str) -> Self {
        let scope = env.scope();
        let out_path = env.data.path().join("run.stdout");
        let err_path = env.data.path().join("run.stderr");
        let run = env
            .command(&[
                "--json",
                "run",
                goal,
                "--scope",
                scope.to_str().unwrap(),
                "--grant",
                "host_tool:probe",
            ])
            .stdout(std::fs::File::create(&out_path).unwrap())
            .stderr(std::fs::File::create(&err_path).unwrap())
            .spawn()
            .unwrap();
        Self {
            run,
            out_path,
            err_path,
        }
    }

    /// Sends one SIGINT to the run.
    fn interrupt(&self) {
        let _ = Command::new("kill")
            .args(["-INT", &self.run.id().to_string()])
            .status();
    }

    /// Waits for the run's exit by its state, with a bound far longer than
    /// anything it waits for (the longest step of these tests is under ten
    /// seconds, LIM-O1 five): a run still alive after it fails the test with
    /// its evidence, before the run is killed.
    fn exit(&mut self, env: &Env, secs: &str) -> std::process::ExitStatus {
        let mut status = None;
        wait_for(std::time::Duration::from_secs(120), || {
            status = self.run.try_wait().unwrap();
            status.is_some()
        });
        match status {
            Some(status) => status,
            None => self.give_up(env, "the run did not exit after the stop signal", secs),
        }
    }

    /// Prints the run's two streams, the process tree below it and the
    /// store's events, then kills the run and fails. The evidence comes
    /// first: a run killed before it is read leaves nothing to explain it
    /// (main's CI run 43338280616).
    fn give_up(&mut self, env: &Env, what: &str, secs: &str) -> ! {
        let report = format!(
            "{what}\n--- stdout ---\n{}\n--- stderr ---\n{}\n--- process tree ---\n{}\n--- store ---\n{}",
            std::fs::read_to_string(&self.out_path).unwrap_or_default(),
            self.stderr(),
            process_tree(self.run.id(), secs),
            store_report(env.data.path()),
        );
        let _ = self.run.kill();
        let _ = self.run.wait();
        // Whatever the host tool left sleeping, by pid: the next run of the
        // suite is not met by it.
        if let Ok(found) = Command::new("pgrep")
            .args(["-f", &format!("^sleep {secs}$")])
            .output()
        {
            for pid in String::from_utf8_lossy(&found.stdout).lines() {
                let _ = Command::new("kill").args(["-KILL", pid.trim()]).status();
            }
        }
        panic!("{report}");
    }

    fn stderr(&self) -> String {
        std::fs::read_to_string(&self.err_path).unwrap_or_default()
    }

    /// The task id the run printed (`--json`).
    fn task_id(&self) -> String {
        let stdout = std::fs::read_to_string(&self.out_path).unwrap_or_default();
        let value: Value = serde_json::from_str(stdout.trim())
            .unwrap_or_else(|e| panic!("not JSON ({e}): {stdout}\n{}", self.stderr()));
        value["task"]["task_id"].as_str().unwrap().to_owned()
    }
}

/// The process `root`, its descendants and the `sleep <secs>` processes, one
/// line each: pid, parent, state, CPU ticks (a run spinning shows ticks that
/// grow, one blocked does not), threads, command line; for `root` also the
/// signal masks and each thread's state and wait channel.
fn process_tree(root: u32, secs: &str) -> String {
    let mut text = String::new();
    let stat = |pid: u32| -> Option<(u32, char, u64, u64)> {
        let s = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let rest = &s[s.rfind(')')? + 2..];
        let f: Vec<&str> = rest.split(' ').collect();
        Some((
            f.get(1)?.parse().ok()?,
            f.first()?.chars().next()?,
            f.get(11)?.parse().ok()?,
            f.get(12)?.parse().ok()?,
        ))
    };
    let cmdline = |pid: u32| {
        std::fs::read(format!("/proc/{pid}/cmdline"))
            .map(|b| String::from_utf8_lossy(&b).replace('\0', " "))
            .unwrap_or_default()
    };
    let all: Vec<u32> = std::fs::read_dir("/proc")
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().to_str()?.parse().ok())
        .collect();
    let mut wanted = vec![root];
    let mut i = 0;
    while i < wanted.len() {
        let parent = wanted[i];
        i += 1;
        for &pid in &all {
            if stat(pid).is_some_and(|s| s.0 == parent) && !wanted.contains(&pid) {
                wanted.push(pid);
            }
        }
    }
    for &pid in &all {
        if !wanted.contains(&pid) && cmdline(pid).trim() == format!("sleep {secs}") {
            wanted.push(pid);
        }
    }
    for &pid in &wanted {
        match stat(pid) {
            Some((ppid, state, utime, stime)) => {
                let threads = std::fs::read_dir(format!("/proc/{pid}/task"))
                    .map(|d| d.count())
                    .unwrap_or(0);
                text.push_str(&format!(
                    "pid {pid} ppid {ppid} state {state} ticks {utime}+{stime} threads {threads}: {}\n",
                    cmdline(pid).trim()
                ));
            }
            None => text.push_str(&format!("pid {pid}: gone\n")),
        }
    }
    if let Ok(status) = std::fs::read_to_string(format!("/proc/{root}/status")) {
        for line in status.lines() {
            if ["SigPnd", "ShdPnd", "SigBlk", "SigIgn", "SigCgt"]
                .iter()
                .any(|k| line.starts_with(k))
            {
                text.push_str(&format!("run {line}\n"));
            }
        }
    }
    if let Ok(tasks) = std::fs::read_dir(format!("/proc/{root}/task")) {
        for t in tasks.flatten() {
            let dir = t.path();
            let read = |name: &str| {
                std::fs::read_to_string(dir.join(name))
                    .unwrap_or_default()
                    .trim()
                    .to_owned()
            };
            let state = read("stat");
            let state = state.rsplit(") ").next().unwrap_or("").chars().next();
            text.push_str(&format!(
                "run thread {} {} state {state:?} wchan {}\n",
                t.file_name().to_string_lossy(),
                read("comm"),
                read("wchan")
            ));
        }
    }
    text
}

/// The store as a second process reads it, read-only: each task's state and
/// the events in order (all of them up to 80, else the first 20 and the last
/// 60 with the count between).
fn store_report(data: &Path) -> String {
    let path = data.join("data/mimlys.db");
    let conn = match rusqlite::Connection::open_with_flags(
        &path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    ) {
        Ok(conn) => conn,
        Err(e) => return format!("cannot open {}: {e}", path.display()),
    };
    let mut text = String::new();
    let query = |sql: &str, text: &mut String| -> Vec<(String, String)> {
        let Ok(mut stmt) = conn.prepare(sql) else {
            text.push_str(&format!("cannot prepare {sql}\n"));
            return vec![];
        };
        stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .map(|rows| rows.flatten().collect())
            .unwrap_or_default()
    };
    for (id, state) in query("select task_id, state from tasks", &mut text) {
        text.push_str(&format!("task {id} state {state}\n"));
    }
    let events = query(
        "select at || ' ' || name, record from events order by position",
        &mut text,
    );
    let total = events.len();
    for (i, (name, record)) in events.iter().enumerate() {
        if total <= 80 || i < 20 || i >= total - 60 {
            let record: String = record.chars().take(300).collect();
            text.push_str(&format!("event {i}: {name} {record}\n"));
        } else if i == 20 {
            text.push_str(&format!("... {} events omitted ...\n", total - 80));
        }
    }
    text.push_str(&format!("{total} events\n"));
    text
}

/// Live `sleep <secs>` processes of this user.
fn sleepers(secs: &str) -> usize {
    let out = Command::new("pgrep")
        .args(["-f", &format!("^sleep {secs}$")])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| !l.trim().is_empty())
        .count()
}

/// REQ-CONF-003 / REQ-CONF-004: an injected failure of a mandatory clause
/// makes the tier `core` with the reason, and `run_host_tool` is neither
/// listed nor accepted.
/// REQ-CLI-006: `status` shows the tier (`core`) and names the clause the platform cannot
/// enforce, in `clauses` (state `unavailable`, with the reason) and in `unavailable_clauses`.
/// REQ-SEC-003 (external design §13.4, §21), the confinement clause: a
/// confinement error (clause F failing) results in no execution: the model's
/// proposal of the host tool, granted at creation, is rejected `unknown_tool`
/// and nothing runs.
#[test]
fn m6a_probe_is_honest_and_fails_closed() {
    let env = Env::new(&[probe(&["ok"]), complete()], "").env("MIMLYS_PROBE_FAIL", "F");
    let (tier, table) = clauses(&env);
    assert_eq!(tier, "core");
    assert_eq!(table["filesystem"]["state"], "unavailable");
    assert!(
        table["filesystem"]["reason"]
            .as_str()
            .unwrap()
            .contains("injected")
    );
    let (_, status) = env.json(&["status"]);
    assert!(
        status["unavailable_clauses"]
            .as_array()
            .unwrap()
            .iter()
            .any(|l| l.as_str().unwrap().starts_with("F:"))
    );
    let scope = env.scope();
    let (code, value) = env.json(&[
        "run",
        "try",
        "--scope",
        scope.to_str().unwrap(),
        "--grant",
        "host_tool:probe",
    ]);
    assert_eq!(code, 0, "{value}");
    let id = value["task"]["task_id"].as_str().unwrap().to_owned();
    let rejected: Vec<String> = env
        .events(&id)
        .iter()
        .filter(|e| e["event"] == "decision.rejected")
        .map(|e| e["code"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(rejected, ["unknown_tool"]);
}

/// RFC 011 §5: a best-effort mandatory clause gives `core` unless the
/// administrator accepted it; then the tier says so.
#[test]
fn m6a_accepted_best_effort_is_explicit() {
    let env = Env::new(&[], "").env("MIMLYS_PROBE_BEST_EFFORT", "K");
    let (tier, table) = clauses(&env);
    assert_eq!(table["termination"]["state"], "best_effort");
    assert_eq!(tier, "core");
    let env = Env::new(&[], "[policy]\naccept_best_effort = [\"K\"]\n")
        .env("MIMLYS_PROBE_BEST_EFFORT", "K");
    let (tier, _) = clauses(&env);
    assert_eq!(
        tier,
        "full (K best-effort, accepted by policy.accept_best_effort)"
    );
    // An unavailable clause can never be accepted; a non-mandatory letter is refused.
    let env = Env::new(&[], "[policy]\naccept_best_effort = [\"M\"]\n");
    let (code, _, stderr) = env.run(&["check"]);
    assert_eq!(code, 1);
    assert!(stderr.contains("accept_best_effort"), "{stderr}");
    let env =
        Env::new(&[], "[policy]\naccept_best_effort = [\"F\"]\n").env("MIMLYS_PROBE_FAIL", "F");
    let (tier, _) = clauses(&env);
    assert_eq!(tier, "core");
}

/// REQ-CONF-004 / ED §13.4: every host-tool approval carries the tier line,
/// with the accepted clause; and a profile's `host_tools` grants at creation
/// make `run_host_tool` visible without a `--grant` (ED §9.1).
#[test]
fn m6a_host_tool_approval_shows_the_tier() {
    let env = Env::new(
        &[probe(&["ok"]), complete()],
        "[policy]\naccept_best_effort = [\"K\"]\n[policy.profiles.read_only]\nhost_tools = [\"probe\"]\napproval_tools = [\"run_host_tool\"]\n",
    )
    .env("MIMLYS_PROBE_BEST_EFFORT", "K");
    let (tier, _) = clauses(&env);
    if !tier.starts_with("full") {
        common::skip(
            "m6a_host_tool_approval_shows_the_tier",
            &format!("this machine is tier {tier}"),
        );
        return;
    }
    let scope = env.scope();
    let (code, value) = env.json(&[
        "run",
        "approve a host tool",
        "--scope",
        scope.to_str().unwrap(),
        "--profile",
        "read_only",
    ]);
    assert_eq!(code, 0, "{value}");
    assert_eq!(value["task"]["state"], "waiting_user", "{value}");
    let grants: Vec<&str> = value["grants"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|g| g["kind"]["kind"].as_str())
        .collect();
    assert!(grants.contains(&"host_tool"), "{value}");
    let lines = value["pending_approval"]["rendered"]["lines"]
        .as_array()
        .expect("rendered lines")
        .clone();
    let tier_line = lines
        .iter()
        .find(|l| l["label"] == "Tier")
        .unwrap_or_else(|| panic!("no Tier line in {lines:?}"));
    assert_eq!(
        tier_line["value"],
        "full (K best-effort, accepted by policy.accept_best_effort)"
    );
    assert!(
        lines
            .iter()
            .any(|l| l["label"] == "Grants used" && l["value"] == "host_tool:probe"),
        "{lines:?}"
    );
    let id = value["task"]["task_id"].as_str().unwrap().to_owned();
    // Approving from another process runs the tool; the fake backend of that
    // process starts its script again, so the task asks once more and is cancelled here.
    let (code, value) = env.json(&["approve", &id]);
    assert_eq!(code, 0, "{value}");
    let obs = env.observations(&id);
    assert_eq!(obs.len(), 1, "{obs:?}");
    assert_eq!(exit_code_of(&obs[0]), 0, "{obs:?}");
    // The gateway is never granted by name.
    let (code, refused) = env.json(&["grant", &id, "tool", "run_host_tool"]);
    assert_eq!(code, 1, "{refused}");
    assert!(
        refused["error"]["message"]
            .as_str()
            .unwrap()
            .contains("host_tool:<name>"),
        "{refused}"
    );
    let (code, _) = env.json(&["task", "cancel", &id]);
    assert_eq!(code, 0);
}

/// REQ-CONF-006 / RFC 011 §6: literals and pattern matches pass; an
/// unlisted flag, a path, too many arguments and an undeclared tool are
/// `denied`, and the task goes on between them.
#[test]
fn m6a_argument_rules_are_enforced() {
    let env = Env::new(
        &[
            strict(&["ok"]),
            strict(&["--all"]),
            strict(&["cwd"]),
            strict(&["read", "../x"]),
            strict(&["env"]),
            strict(&["ok", "a", "b", "c"]),
            strict(&["env", "home"]),
            json!({"decision": "act", "tool": "run_host_tool", "arguments": {"tool": "undeclared"}}),
            complete(),
        ],
        "",
    );
    let (tier, _) = clauses(&env);
    if tier != "full" {
        common::skip("m6a_argument_rules_are_enforced", &format!("tier {tier}"));
        return;
    }
    let scope = env.scope();
    let (code, value) = env.json(&[
        "run",
        "args",
        "--scope",
        scope.to_str().unwrap(),
        "--grant",
        "host_tool:strict",
    ]);
    assert_eq!(code, 0, "{value}");
    let id = value["task"]["task_id"].as_str().unwrap().to_owned();
    let rejected: Vec<String> = env
        .events(&id)
        .iter()
        .filter(|e| e["event"] == "decision.rejected")
        .map(|e| e["code"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(
        rejected,
        // An undeclared name fails the grant stage (7) before the bounds stage (8): `denied`.
        ["denied", "denied", "denied", "denied"],
        "{rejected:?} / {:?}",
        env.events(&id)
            .iter()
            .filter(|e| e["event"] == "decision.rejected")
            .collect::<Vec<_>>()
    );
}

/// REQ-CONF-007 / RFC 011 §8: the dependency step runs in a worker without
/// clause N, is `external_read` (approval where a profile says so), and reaches the network;
/// the offline tool in the same task cannot.
/// INV-13 (external design §21.2): the worker has no network except the
/// declared dependency step. A loopback listener is connected to from the
/// dependency step and refused to the offline host tool.
#[test]
fn m6a_dependency_step_has_network() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || for _ in listener.incoming().take(2) {});
    let addr = format!("127.0.0.1:{port}");
    let env = Env::new(
        &[
            json!({"decision": "act", "tool": "run_host_tool", "arguments": {"tool": "fetch", "args": ["connect", addr]}}),
            probe(&["connect", &addr]),
            complete(),
        ],
        "",
    );
    let (tier, _) = clauses(&env);
    if tier != "full" {
        common::skip("m6a_dependency_step_has_network", &format!("tier {tier}"));
        return;
    }
    let scope = env.scope();
    let (code, value) = env.json(&[
        "run",
        "fetch",
        "--scope",
        scope.to_str().unwrap(),
        "--grant",
        "host_tool:probe",
        "--grant",
        "host_tool:fetch",
    ]);
    assert_eq!(code, 0, "{value}");
    assert_eq!(value["task"]["state"], "completed", "{value}");
    let id = value["task"]["task_id"].as_str().unwrap().to_owned();
    let obs = env.observations(&id);
    assert_eq!(
        stdout_of(&obs[0]).trim(),
        "connected",
        "the dependency step reaches the network"
    );
    assert!(
        stdout_of(&obs[1]).contains("connect failed: permission denied"),
        "the offline tool cannot"
    );
}

/// REQ-CONF-009 / RFC 011 §9: a process tree a previous runtime left
/// behind is terminated at startup and the event recorded.
#[test]
fn m6a_orphans_are_reaped_at_startup() {
    let env = Env::new(&[], "");
    let data = env.data.path().join("data");
    std::fs::create_dir_all(&data).unwrap();
    let mut cmd = Command::new("sh");
    cmd.args(["-c", "sleep 4321 & sleep 4321"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let mut orphan = cmd.spawn().unwrap();
    // Both sleepers exist (a state, not a pause), up to 60 s.
    assert!(
        wait_for(std::time::Duration::from_secs(60), || sleepers("4321") >= 2),
        "the leftover tree starts"
    );
    let pid = orphan.id();
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
    let start_time: u64 = stat
        .rsplit_once(')')
        .unwrap()
        .1
        .split_whitespace()
        .nth(19)
        .unwrap()
        .parse()
        .unwrap();
    std::fs::write(
        data.join("worker.json"),
        json!({"pid": pid, "start_time": start_time}).to_string(),
    )
    .unwrap();
    assert!(sleepers("4321") >= 1);
    // REQ-PERS-013: `status` only reads; it reaps nothing. The leftover tree and
    // its record are as they were.
    let (_, _) = env.json(&["status"]);
    assert!(sleepers("4321") >= 1, "a reader does not reap a worker");
    assert!(data.join("worker.json").exists());
    // A command that writes reaps it at startup (REQ-CONF-009).
    let (_, _) = env.json(&["interpreters", "list"]);
    assert!(
        wait_for(std::time::Duration::from_secs(60), || sleepers("4321") == 0),
        "the leftover tree is gone"
    );
    let _ = orphan.wait();
    assert!(!data.join("worker.json").exists());
    let (_, metrics) = env.json(&["metrics"]);
    assert_eq!(metrics["events_by_name"]["runtime.orphans_killed"], 1);
}

/// External design §23, M6a: `cargo test --offline` of a native project,
/// with a path dependency, under every mandatory clause; the model fixes
/// the failing test and the completion check verifies it. Runs when a
/// rustup toolchain is installed under the home directory.
#[test]
fn m6a_demonstration_cargo_test_offline() {
    let home = directories::BaseDirs::new()
        .map(|b| b.home_dir().to_path_buf())
        .unwrap();
    let cargo = home.join(".cargo/bin/cargo");
    if !cargo.is_file() || !home.join(".rustup").is_dir() {
        common::skip(
            "m6a_demonstration_cargo_test_offline",
            &format!("no rustup toolchain under {}", home.display()),
        );
        return;
    }
    let data = tempfile::tempdir().unwrap();
    let scope = data.path().join("project");
    std::fs::create_dir_all(scope.join("src")).unwrap();
    std::fs::create_dir_all(scope.join("arith/src")).unwrap();
    std::fs::write(scope.join("Cargo.toml"), "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\narith = { path = \"arith\" }\n\n[workspace]\nmembers = [\"arith\"]\n").unwrap();
    std::fs::write(
        scope.join("arith/Cargo.toml"),
        "[package]\nname = \"arith\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(
        scope.join("arith/src/lib.rs"),
        "pub fn add(a: i32, b: i32) -> i32 { a + b }\n",
    )
    .unwrap();
    std::fs::write(scope.join("src/lib.rs"), "pub fn total(xs: &[i32]) -> i32 { xs.iter().fold(0, |acc, x| arith::add(acc, *x)) - 1 }\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn sums() { assert_eq!(super::total(&[1, 2, 3]), 6); }\n}\n").unwrap();
    let script = data.path().join("script.json");
    let act = |tool: &str, args: Value| json!({"decision": "act", "tool": tool, "arguments": args});
    std::fs::write(&script, serde_json::to_string(&[
        act("run_host_tool", json!({"tool": "cargo_test"})),
        act("replace_in_file", json!({"path": "src/lib.rs", "find": ") - 1 }", "replace": ") }"})),
        act("run_host_tool", json!({"tool": "cargo_test"})),
        json!({"decision": "complete", "summary": "the off-by-one is gone and the test passes"}),
    ]).unwrap()).unwrap();
    let config = data.path().join("config.toml");
    std::fs::write(
        &config,
        format!(
            r#"config_version = 1
[runtime]
data_dir = {data:?}
[backend]
kind = "fake"
script = {script:?}
[policy.profiles.workspace]
tools = [{builtin}]
[host_tools.cargo_test]
command = "~/.cargo/bin/cargo"
args = ["test", "--offline"]
allowed_args = ["--lib"]
max_args = 1
effect = "workspace_write"
timeout_ms = 600000
read_paths = ["~/.cargo/bin", "~/.rustup"]
env = {{ RUSTUP_HOME = "~/.rustup", RUSTUP_TOOLCHAIN = "stable" }}
"#,
            data = data.path().join("data").to_string_lossy(),
            script = script.to_string_lossy(),
            builtin = BUILTIN
        ),
    )
    .unwrap();
    let run = |args: &[&str]| -> (i32, Value, String) {
        let mut all = vec!["--config", config.to_str().unwrap(), "--json"];
        all.extend_from_slice(args);
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_mimlys"));
        cmd.args(&all)
            .env("MIMLYS_CONFIG", "")
            .env("MIMLYS_WORKER_BIN", env!("CARGO_BIN_EXE_mimlys-worker"));
        let out = run_bounded(&mut cmd, data.path());
        (
            out.code(),
            serde_json::from_str(out.stdout.trim()).unwrap_or(Value::Null),
            out.stderr,
        )
    };
    let (_, status, _) = run(&["status"]);
    eprintln!(
        "m6a demonstration: tier {} clauses {}",
        status["tier"], status["clauses"]
    );
    if status["tier"] != "full" {
        common::skip(
            "m6a_demonstration_cargo_test_offline",
            &format!("tier {}", status["tier"]),
        );
        return;
    }
    let started = std::time::Instant::now();
    let (code, value, stderr) = run(&[
        "run",
        "the test in src/lib.rs fails: fix total",
        "--scope",
        scope.to_str().unwrap(),
        "--grant",
        "host_tool:cargo_test",
        "--completion-check",
        &json!({"tool": "run_host_tool", "arguments": {"tool": "cargo_test"}}).to_string(),
    ]);
    eprintln!(
        "m6a demonstration: {:?} in {:?}",
        value["task"]["state"],
        started.elapsed()
    );
    if value["task"]["state"] != "completed" {
        // Evidence first: what each action printed, so that a failure on a
        // machine we cannot log into (a CI runner) explains itself.
        let conn = rusqlite::Connection::open(data.path().join("data/mimlys.db")).unwrap();
        let mut stmt = conn
            .prepare("select o.record from observations o order by o.rowid")
            .unwrap();
        for record in stmt.query_map([], |r| r.get::<_, String>(0)).unwrap() {
            let o: Value = serde_json::from_str(&record.unwrap()).unwrap();
            eprintln!(
                "observation: status {} cause {} payload {} stdout {:?} stderr {:?} notices {}",
                o["status"], o["cause"], o["payload"], o["stdout"], o["stderr"], o["notices"]
            );
        }
    }
    assert_eq!(code, 0, "{value}\n{stderr}");
    assert_eq!(value["task"]["state"], "completed", "{value}\n{stderr}");
    assert_eq!(value["task"]["completion"], "verified", "{value}");
    let id = value["task"]["task_id"].as_str().unwrap().to_owned();
    let (_, events, _) = run(&["events", &id]);
    let finished: Vec<String> = events
        .as_array()
        .unwrap()
        .iter()
        .map(|e| &e["record"])
        .filter(|e| e["event"] == "action.finished")
        .map(|e| e["status"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(
        finished,
        ["failed", "succeeded", "succeeded", "succeeded"],
        "{finished:?}"
    );
    assert!(
        data.path()
            .join("data/workspaces")
            .join(&id)
            .join("tree/target")
            .is_dir(),
        "the build happened inside the workspace"
    );
}

// ---------------------------------------------------------------------
// 0.7.1 (handoff 011d; REQ-CONF-011, REQ-CONF-012): the clause tests of
// the hardening. Each guards one rule and was shown to fail without it
// (the mutation outputs are in the review request). Where a test needs
// a kernel feature or a tier the machine lacks, it says so through
// `common::skip` and the review request counts it apart from the passes.
// ---------------------------------------------------------------------

/// The probe tool run directly, outside every confinement: the control that
/// shows the thing is possible on this machine, so that a denial inside the
/// worker is the confinement's doing and not the kernel's.
fn control(args: &[&str]) -> (i32, String) {
    let out = Command::new(probe_tool())
        .args(args)
        .env_clear()
        .output()
        .unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).trim().to_owned(),
    )
}

/// Runs the script's host-tool calls under a full-tier worker and returns the
/// observations, or `None` (recorded as a skip) below tier `full`.
fn run_probe_script(test: &str, env: &Env) -> Option<Vec<Value>> {
    let (tier, table) = clauses(env);
    if tier != "full" {
        common::skip(test, &format!("this machine is tier {tier} ({table:?})"));
        return None;
    }
    let scope = env.scope();
    let (code, value) = env.json(&[
        "run",
        "probe",
        "--scope",
        scope.to_str().unwrap(),
        "--grant",
        "host_tool:probe",
    ]);
    assert_eq!(code, 0, "{value}");
    assert_eq!(value["task"]["state"], "completed", "{value}");
    let id = value["task"]["task_id"].as_str().unwrap().to_owned();
    Some(env.observations(&id))
}

/// REQ-CONF-012: a socket created through `io_uring` never passes the
/// `socket` rule of the filter, so the ring itself is denied. Inside the host
/// tool `io_uring_setup` answers `EPERM`; the same command outside any
/// confinement creates the socket (the control).
#[test]
fn req_conf_012_io_uring_socket_is_denied() {
    let test = "req_conf_012_io_uring_socket_is_denied";
    let (code, line) = control(&["uring-socket"]);
    if code != 0 {
        common::skip(test, &format!("io_uring is not usable here: {line}"));
        return;
    }
    let env = Env::new(&[probe(&["uring-socket"]), complete()], "");
    let Some(obs) = run_probe_script(test, &env) else {
        return;
    };
    assert_ne!(exit_code_of(&obs[0]), 0, "{}", stdout_of(&obs[0]));
    assert!(
        stdout_of(&obs[0]).contains("uring-socket failed: io_uring_setup errno=1"),
        "the filter answers EPERM at the ring's creation: {}",
        stdout_of(&obs[0])
    );
}

/// REQ-CONF-012: nor can `io_uring` connect: a live loopback listener sees
/// no connection from the host tool, and sees one from the control.
#[test]
fn req_conf_012_io_uring_connect_is_denied() {
    let test = "req_conf_012_io_uring_connect_is_denied";
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let (code, line) = control(&["uring-connect", &addr]);
    if code != 0 {
        common::skip(test, &format!("io_uring is not usable here: {line}"));
        return;
    }
    assert!(
        accepted(|| listener.accept()),
        "the control reached the listener"
    );
    let env = Env::new(&[probe(&["uring-connect", &addr]), complete()], "");
    let Some(obs) = run_probe_script(test, &env) else {
        return;
    };
    assert_ne!(exit_code_of(&obs[0]), 0, "{}", stdout_of(&obs[0]));
    assert!(
        stdout_of(&obs[0]).contains("uring-connect failed: io_uring_setup errno=1"),
        "{}",
        stdout_of(&obs[0])
    );
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert!(
        listener.accept().is_err(),
        "the listener saw no connection from the host tool"
    );
}

/// REQ-CONF-012: `process_vm_readv` is denied. The tool reads a string held
/// by a descendant of its own (which the kernel's own checks allow: the
/// control does it), so a failure inside the worker is the filter's `EPERM`
/// and nothing else.
#[test]
fn req_conf_012_cross_process_memory_is_denied() {
    let test = "req_conf_012_cross_process_memory_is_denied";
    let (code, line) = control(&["vm-read", "child"]);
    if code != 0 {
        common::skip(
            test,
            &format!("process_vm_readv is not usable here: {line}"),
        );
        return;
    }
    assert!(line.contains("mimlys-vm-read-probe"), "{line}");
    let env = Env::new(&[probe(&["vm-read", "child"]), complete()], "");
    let Some(obs) = run_probe_script(test, &env) else {
        return;
    };
    assert_ne!(exit_code_of(&obs[0]), 0, "{}", stdout_of(&obs[0]));
    assert!(
        stdout_of(&obs[0]).contains("vm-read failed: errno=1"),
        "{}",
        stdout_of(&obs[0])
    );
    assert!(!stdout_of(&obs[0]).contains("mimlys-vm-read-probe"));
}

/// The runtime as a child that knows its own process id: `sh` sets
/// `MIMLYS_TEST_RUNTIME_PID` to its pid and `exec`s the runtime, which keeps
/// the pid.
fn pid_aware(mut cmd: Command) -> Command {
    let mut wrapped = Command::new("sh");
    wrapped
        .arg("-c")
        .arg("MIMLYS_TEST_RUNTIME_PID=$$ exec \"$0\" \"$@\"")
        .arg(cmd.get_program())
        .args(cmd.get_args());
    for (k, v) in cmd.get_envs() {
        match v {
            Some(v) => wrapped.env(k, v),
            None => wrapped.env_remove(k),
        };
    }
    wrapped.stdin(std::process::Stdio::null());
    let _ = &mut cmd;
    wrapped
}

/// REQ-CONF-011: another process's `/proc` files are not in the host tool's
/// read set. Three reads, each with a control from the unconfined tool:
/// the runtime's `environ` (also non-dumpable, REQ-CONF-012), a same-user
/// dumpable process's `environ`, and that process's `cmdline`.
///
/// What each pins: Landlock's own ptrace scoping already refuses `environ`
/// (and `maps`, `fd`, `mem`) of a process outside the sandbox's domain even
/// when `/proc` is readable (shown by the mutation run in the review
/// request), so the two `environ` reads hold whichever way the read set
/// goes. `cmdline` has no such check: it is world-readable, so only the read
/// set keeps it out, and that third read is the one that fails when `/proc`
/// is put back.
#[test]
fn req_conf_011_proc_environ_is_unreadable() {
    let test = "req_conf_011_proc_environ_is_unreadable";
    let me = std::process::id().to_string();
    let out = Command::new(probe_tool())
        .args(["read-proc", "MIMLYS_TEST_OWNER_PID", "environ"])
        .env_clear()
        .env("MIMLYS_TEST_OWNER_PID", &me)
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("read-proc read"),
        "the control reads a same-user process's environment: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    let env = Env::new(
        &[
            probe(&["read-proc", "MIMLYS_TEST_RUNTIME_PID", "environ"]),
            probe(&["read-proc", "MIMLYS_TEST_OWNER_PID", "environ"]),
            probe(&["read-proc", "MIMLYS_TEST_OWNER_PID", "cmdline"]),
            complete(),
        ],
        "",
    )
    .env("MIMLYS_TEST_OWNER_PID", &me);
    let (tier, table) = clauses(&env);
    if tier != "full" {
        common::skip(test, &format!("this machine is tier {tier} ({table:?})"));
        return;
    }
    let scope = env.scope();
    let cmd = pid_aware(env.command(&[
        "--json",
        "run",
        "probe",
        "--scope",
        scope.to_str().unwrap(),
        "--grant",
        "host_tool:probe",
    ]));
    let mut cmd = cmd;
    let out = run_bounded(&mut cmd, env.data.path());
    let value: Value = serde_json::from_str(out.stdout.trim())
        .unwrap_or_else(|e| panic!("not JSON ({e}): {}\n{}", out.stdout, out.stderr));
    assert_eq!(value["task"]["state"], "completed", "{value}");
    let id = value["task"]["task_id"].as_str().unwrap().to_owned();
    let obs = env.observations(&id);
    for (i, what) in [
        (0, "the runtime's environment"),
        (1, "a same-user process's environment"),
        (2, "a same-user process's command line"),
    ] {
        assert_ne!(exit_code_of(&obs[i]), 0, "{what}: {}", stdout_of(&obs[i]));
        assert!(
            stdout_of(&obs[i]).contains("read-proc failed: permission denied"),
            "{what}: {}",
            stdout_of(&obs[i])
        );
    }
}

/// REQ-CONF-011: a credentials file beside the declared toolchain directory
/// cannot be read; the declared directory itself can. The declaration is
/// `<toolchain>/bin`, the file is `<toolchain>/credentials.toml`: the read
/// set holds the former and not its parent.
#[test]
fn req_conf_011_credentials_beside_toolchain_are_unreadable() {
    let test = "req_conf_011_credentials_beside_toolchain_are_unreadable";
    let toolchain = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(toolchain.path().join("bin")).unwrap();
    std::fs::write(toolchain.path().join("bin/marker"), "m").unwrap();
    std::fs::write(
        toolchain.path().join("credentials.toml"),
        "token = \"secret\"\n",
    )
    .unwrap();
    let beside = toolchain.path().join("credentials.toml");
    let inside = toolchain.path().join("bin/marker");
    let extra = format!(
        "[host_tools.beside]\ncommand = {tool:?}\nallowed_args = [\"read\"]\narg_pattern = \"[A-Za-z0-9_./:=-]+\"\nmax_args = 2\neffect = \"workspace_write\"\ntimeout_ms = 20000\nread_paths = [{bin:?}, {tooldir:?}]\n",
        tool = probe_tool().to_string_lossy(),
        tooldir = probe_tool().parent().unwrap().to_string_lossy(),
        bin = toolchain.path().join("bin").to_string_lossy(),
    );
    let beside_call = |path: &Path| json!({"decision": "act", "tool": "run_host_tool", "arguments": {"tool": "beside", "args": ["read", path.to_str().unwrap()]}});
    let env = Env::new(
        &[beside_call(&beside), beside_call(&inside), complete()],
        &extra,
    );
    let (tier, table) = clauses(&env);
    if tier != "full" {
        common::skip(test, &format!("this machine is tier {tier} ({table:?})"));
        return;
    }
    let scope = env.scope();
    let (code, value) = env.json(&[
        "run",
        "probe",
        "--scope",
        scope.to_str().unwrap(),
        "--grant",
        "host_tool:beside",
    ]);
    assert_eq!(code, 0, "{value}");
    let id = value["task"]["task_id"].as_str().unwrap().to_owned();
    let obs = env.observations(&id);
    assert!(
        stdout_of(&obs[0]).contains("read failed: permission denied"),
        "the credentials file beside the toolchain: {}",
        stdout_of(&obs[0])
    );
    assert!(
        stdout_of(&obs[1]).contains("read 1 bytes"),
        "the declared directory is readable: {}",
        stdout_of(&obs[1])
    );
}

/// REQ-CONF-012: the runtime and the worker are non-dumpable. Seen from the
/// test process (the same user): the files inside `/proc/<pid>` of each are
/// owned by root, the kernel's mark of a non-dumpable process (the directory
/// itself keeps its owner), and `environ` cannot be opened. (The kernel's
/// `status` file has no `Dumpable` field, so the ownership is the evidence.)
#[test]
fn req_conf_012_runtime_is_not_dumpable() {
    use std::os::unix::fs::MetadataExt;
    let test = "req_conf_012_runtime_is_not_dumpable";
    let env = Env::new(&[probe(&["setsid-sleep", "1236"]), complete()], "");
    let (tier, table) = clauses(&env);
    if tier != "full" {
        common::skip(test, &format!("this machine is tier {tier} ({table:?})"));
        return;
    }
    let scope = env.scope();
    let mut run = env
        .command(&[
            "--json",
            "run",
            "dumpable",
            "--scope",
            scope.to_str().unwrap(),
            "--grant",
            "host_tool:probe",
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    // The host tool is running when its sleepers exist: runtime and worker are both up.
    assert!(
        wait_for(std::time::Duration::from_secs(60), || sleepers("1236") >= 2),
        "the host tool runs"
    );
    let runtime = run.id();
    let worker = child_of(runtime, "mimlys-worker")
        .unwrap_or_else(|| panic!("no mimlys-worker under pid {runtime}"));
    for (what, pid) in [("the runtime", runtime), ("the worker", worker)] {
        let dir = format!("/proc/{pid}");
        assert_eq!(
            std::fs::metadata(format!("{dir}/status")).unwrap().uid(),
            0,
            "{what} ({pid}) is non-dumpable: its /proc files belong to root"
        );
        let opened = std::fs::File::open(format!("{dir}/environ"));
        assert!(
            opened.is_err(),
            "{what} ({pid}): the environment cannot be opened by the same user"
        );
    }
    // The control: this test process is dumpable and the same user's.
    assert_ne!(
        std::fs::metadata(format!("/proc/{}/status", std::process::id()))
            .unwrap()
            .uid(),
        0
    );
    for _ in 0..2 {
        let _ = Command::new("kill")
            .args(["-INT", &runtime.to_string()])
            .status();
        std::thread::sleep(std::time::Duration::from_millis(300));
    }
    let _ = run.wait();
}

/// The first live child of `parent` whose command name is `comm`.
fn child_of(parent: u32, comm: &str) -> Option<u32> {
    for entry in std::fs::read_dir("/proc").ok()?.flatten() {
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        let Ok(stat) = std::fs::read_to_string(entry.path().join("stat")) else {
            continue;
        };
        let Some((head, rest)) = stat.rsplit_once(')') else {
            continue;
        };
        let name = head.split_once('(').map(|(_, n)| n).unwrap_or("");
        let ppid: u32 = rest
            .split_whitespace()
            .nth(1)
            .and_then(|p| p.parse().ok())
            .unwrap_or(0);
        if ppid == parent && name == comm {
            return Some(pid);
        }
    }
    None
}

/// REQ-CONF-012: where the kernel's Landlock has the TCP layer (ABI 4), the
/// N clause reports both layers; below it, the filter alone (still
/// enforced, because it denies the `io_uring` route).
#[test]
fn req_conf_012_landlock_net_layer_is_reported() {
    let test = "req_conf_012_landlock_net_layer_is_reported";
    let env = Env::new(&[], "");
    let (tier, table) = clauses(&env);
    if tier != "full" {
        common::skip(test, &format!("this machine is tier {tier} ({table:?})"));
        return;
    }
    let abi4 = mimlys_exec::confine::linux::landlock_net_supported();
    assert_eq!(table["network"]["state"], "enforced", "{table:?}");
    let expected = if abi4 {
        "seccomp+landlock_net"
    } else {
        "seccomp"
    };
    assert_eq!(table["network"]["primitive"], expected, "{table:?}");
    if !abi4 {
        common::skip(
            test,
            "kernel Landlock below ABI 4: the TCP layer is not testable here",
        );
    }
}

/// The files of `/etc` the dependency step's worker may read. The first two
/// are the list of handoff 011j: each is one the step failed without (measured
/// with `cargo fetch` and `curl`). The last two (handoff 011l) are the
/// certificate bundles of the Fedora/RHEL and openSUSE families, **not
/// measured**: from the families' documented layout. Each is admitted only
/// where it exists.
const NETWORK_ETC: [&str; 4] = [
    "/etc/resolv.conf",
    "/etc/ssl/certs/ca-certificates.crt",
    "/etc/pki/tls/certs/ca-bundle.crt",
    "/etc/ssl/ca-bundle.pem",
];

/// The files of `NETWORK_ETC` that exist on this machine, sorted as a read set is.
fn network_etc_present() -> Vec<String> {
    let mut present: Vec<String> = NETWORK_ETC
        .iter()
        .filter(|f| Path::new(f).exists())
        .map(|f| (*f).to_owned())
        .collect();
    present.sort();
    present
}

/// A path is under `dir` as a path component, not as a string prefix
/// (`/etcetera` is not under `/etc`).
fn under(path: &Path, dir: &str) -> bool {
    path.starts_with(dir)
}

/// REQ-CONF-011 (handoff 011j): the Linux read set holds neither `/etc` nor
/// `/opt`. The worker's offline set has no entry under either; the dependency
/// step's worker adds exactly the files of `NETWORK_ETC` that exist on the
/// machine and nothing else; a toolchain under `/opt` is in the set only when a declaration names
/// it.
#[test]
fn req_conf_011_the_read_set_holds_no_etc_and_no_opt() {
    let data = tempfile::tempdir().unwrap();
    let offline = mimlys::app::confinement_spec(data.path(), &[], &[], true).read_paths;
    for p in &offline {
        assert!(!under(p, "/etc") && !under(p, "/opt"), "{}", p.display());
    }
    for must in ["/usr", "/lib", "/bin", "/dev/urandom"] {
        assert!(offline.iter().any(|p| p == Path::new(must)), "{must} stays");
    }
    let online = mimlys::app::confinement_spec(data.path(), &[], &[], false).read_paths;
    let added: Vec<String> = online
        .iter()
        .filter(|p| !offline.contains(p))
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        added,
        network_etc_present(),
        "the dependency step's worker adds exactly the listed files that exist here"
    );
    assert!(
        !offline.iter().any(|p| p.starts_with("/opt")),
        "no /opt unless declared"
    );
    let mut declaration = mimlys_core::host_tool::HostToolDeclaration {
        name: "opt-tool".into(),
        command: "/opt/tool/bin/tool".into(),
        args: vec![],
        allowed_args: vec![],
        arg_pattern: None,
        max_args: 0,
        effect: mimlys_core::action::EffectKind::WorkspaceWrite,
        reversible: true,
        timeout_ms: 1000,
        read_paths: vec!["/opt/tool".into()],
        env: Default::default(),
        env_allow: vec![],
        network: false,
    };
    let declared =
        mimlys::app::confinement_spec(data.path(), &[], std::slice::from_ref(&declaration), true)
            .read_paths;
    assert!(
        declared.iter().any(|p| p == Path::new("/opt/tool")),
        "a declaration names it"
    );
    declaration.read_paths.clear();
    let undeclared =
        mimlys::app::confinement_spec(data.path(), &[], &[declaration], true).read_paths;
    assert!(!undeclared.iter().any(|p| p.starts_with("/opt")));
}

/// REQ-CONF-011 (handoff 011j): inside a confined host tool `/etc` is not
/// readable (a file that is world-readable on the machine is refused), and the
/// dependency step's worker reads the files of `NETWORK_ETC` that exist on
/// the machine and nothing else of `/etc`. The offline tool is refused those
/// files too.
#[test]
fn req_conf_011_etc_is_unreadable_but_for_the_dependency_steps_files() {
    let present: Vec<&str> = NETWORK_ETC
        .iter()
        .copied()
        .filter(|f| std::fs::File::open(f).is_ok())
        .collect();
    let fetch = |path: &str| json!({"decision": "act", "tool": "run_host_tool", "arguments": {"tool": "fetch", "args": ["read", path]}});
    let mut script = vec![
        probe(&["read", "/etc/passwd"]),
        fetch("/etc/passwd"),
        fetch("/etc/hosts"),
    ];
    for f in &present {
        script.push(probe(&["read", f]));
        script.push(fetch(f));
    }
    script.push(complete());
    let env = Env::new(&script, "");
    let (tier, _) = clauses(&env);
    if tier != "full" {
        common::skip(
            "req_conf_011_etc_is_unreadable_but_for_the_dependency_steps_files",
            &format!("tier {tier}"),
        );
        return;
    }
    let scope = env.scope();
    let (code, value) = env.json(&[
        "run",
        "read etc",
        "--scope",
        scope.to_str().unwrap(),
        "--grant",
        "host_tool:probe",
        "--grant",
        "host_tool:fetch",
    ]);
    assert_eq!(code, 0, "{value}");
    let id = value["task"]["task_id"].as_str().unwrap().to_owned();
    let obs = env.observations(&id);
    assert_eq!(obs.len(), 3 + 2 * present.len(), "{obs:?}");
    for (i, what) in ["probe /etc/passwd", "fetch /etc/passwd", "fetch /etc/hosts"]
        .iter()
        .enumerate()
    {
        assert!(
            stdout_of(&obs[i]).contains("read failed: permission denied"),
            "{what}: {}",
            stdout_of(&obs[i])
        );
    }
    for (n, f) in present.iter().enumerate() {
        assert!(
            stdout_of(&obs[3 + 2 * n]).contains("read failed: permission denied"),
            "the offline tool must not read {f}: {}",
            stdout_of(&obs[3 + 2 * n])
        );
        assert!(
            stdout_of(&obs[4 + 2 * n]).starts_with("read ")
                && !stdout_of(&obs[4 + 2 * n]).contains("failed"),
            "the dependency step must read {f}: {}",
            stdout_of(&obs[4 + 2 * n])
        );
    }
}

/// REQ-CONF-011 (handoff 011l): the step's files are admitted only where they
/// exist, and the bundles of the Fedora/RHEL and openSUSE families are among
/// them. Run with a fake test of existence, so that it holds on every machine.
#[test]
fn req_conf_011_the_other_families_bundles_are_admitted_where_they_exist() {
    use mimlys::app::network_read_paths_where;
    let all = network_read_paths_where(|_| true);
    let all: Vec<&str> = all.iter().map(|p| p.to_str().unwrap()).collect();
    assert_eq!(
        all, NETWORK_ETC,
        "every candidate, in order, when all exist"
    );
    let fedora = network_read_paths_where(|p| p == Path::new("/etc/pki/tls/certs/ca-bundle.crt"));
    assert_eq!(fedora, [PathBuf::from("/etc/pki/tls/certs/ca-bundle.crt")]);
    let suse = network_read_paths_where(|p| p == Path::new("/etc/ssl/ca-bundle.pem"));
    assert_eq!(suse, [PathBuf::from("/etc/ssl/ca-bundle.pem")]);
    assert!(
        network_read_paths_where(|_| false).is_empty(),
        "none where none exists"
    );
}

/// REQ-CONF-011 (handoff 011l): the offline set never holds a bundle, and
/// `status` names both read sets: `read_set` (the offline worker's) and
/// `dependency_step_read_set_extra` (what the step's worker adds), in JSON and
/// in text. The extra set holds exactly the step's files that exist here.
#[test]
fn req_conf_011_status_names_the_dependency_steps_read_set() {
    let env = Env::new(&[], "");
    let (code, status) = env.json(&["status"]);
    assert_eq!(code, 0, "{status}");
    let extra: Vec<String> = status["dependency_step_read_set_extra"]
        .as_array()
        .unwrap_or_else(|| panic!("no dependency_step_read_set_extra in {status}"))
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect();
    let mut sorted = extra.clone();
    sorted.sort();
    assert_eq!(sorted, network_etc_present());
    let read_set: Vec<&str> = status["read_set"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    for f in NETWORK_ETC {
        assert!(!read_set.contains(&f), "{f} is the step's alone");
    }
    assert!(read_set.contains(&"/usr"));
    let (code, text, _) = env.run(&["status"]);
    assert_eq!(code, 0);
    if extra.is_empty() {
        assert!(!text.contains("dependency step also reads"), "{text}");
    } else {
        let line = text
            .lines()
            .find(|l| l.starts_with("dependency step also reads"))
            .unwrap_or_else(|| panic!("no line for the step's files in {text}"));
        for f in &extra {
            assert!(line.contains(f.as_str()), "{line}");
        }
    }
}

/// REQ-CONF-011, RFC 017 (handoff 017): inside Landlock only the worker's and
/// the module's **files** are admitted, not the folders that hold them. A
/// worker installed in a folder with another file, and an interpreter module
/// in a folder with another file: a host tool reads the two files and is
/// refused both neighbours. (Before RFC 017 the folders were in the read set
/// and the neighbours were readable.)
#[test]
fn req_conf_011_only_the_workers_and_the_modules_files_are_admitted() {
    let test = "req_conf_011_only_the_workers_and_the_modules_files_are_admitted";
    let install = tempfile::tempdir().unwrap();
    let bin = install.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let worker = bin.join("mimlys-worker");
    std::fs::copy(env!("CARGO_BIN_EXE_mimlys-worker"), &worker).unwrap();
    let worker_neighbour = bin.join("neighbour.txt");
    std::fs::write(&worker_neighbour, "another file beside the worker").unwrap();
    let modules = tempfile::tempdir().unwrap();
    let module = modules.path().join("extra.wasm");
    std::fs::write(&module, "a module").unwrap();
    let module_neighbour = modules.path().join("neighbour.txt");
    std::fs::write(&module_neighbour, "another file beside the module").unwrap();
    let read = |path: &Path| probe(&["read", path.to_str().unwrap()]);
    let env = Env::new(
        &[
            read(&worker),
            read(&worker_neighbour),
            read(&module),
            read(&module_neighbour),
            complete(),
        ],
        &format!(
            "[interpreters.extra]\nmodule = {:?}\nsha256 = {:?}\n",
            module.to_string_lossy(),
            "0".repeat(64)
        ),
    )
    .env("MIMLYS_WORKER_BIN", worker.to_str().unwrap());
    let (tier, table) = clauses(&env);
    if tier != "full" {
        common::skip(test, &format!("this machine is tier {tier} ({table:?})"));
        return;
    }
    let scope = env.scope();
    let (code, value) = env.json(&[
        "run",
        "read the neighbours",
        "--scope",
        scope.to_str().unwrap(),
        "--grant",
        "host_tool:probe",
    ]);
    assert_eq!(code, 0, "{value}");
    assert_eq!(value["task"]["state"], "completed", "{value}");
    let id = value["task"]["task_id"].as_str().unwrap().to_owned();
    let obs = env.observations(&id);
    assert_eq!(obs.len(), 4, "{obs:?}");
    for (i, (what, allowed)) in [
        ("the worker's file", true),
        ("a file beside the worker", false),
        ("the module's file", true),
        ("a file beside the module", false),
    ]
    .iter()
    .enumerate()
    {
        let out = stdout_of(&obs[i]);
        if *allowed {
            assert!(
                out.starts_with("read ") && !out.contains("failed"),
                "{what} is readable: {out}"
            );
        } else {
            assert!(
                out.contains("read failed: permission denied"),
                "{what} is refused: {out}"
            );
        }
    }
}
