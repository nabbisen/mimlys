//! The stop signals against an external tool (handoff 010e; REQ-CLI-005,
//! REQ-TASK-006; external design §7.2, §16.3, §19.2), over the scripted MCP
//! test server's `slow` tool (`mimlys-mcp-testserver`: it waits the given
//! milliseconds, then answers, and ends at once when the call is cancelled).
//!
//! The first signal interrupts only what can be undone. `slow` declared
//! `external_write` cannot be undone: the process says that it waits for it and
//! does, and the task pauses after it. A second signal interrupts it anyway: its
//! outcome is recorded unknown and the task halts `unknown_outcome` with the
//! action listed. Declared `external_read` and idempotent it can be undone, and
//! the first signal interrupts it once the grace (LIM-O1 less one second) has
//! passed. `task cancel` after a process died in the middle of such a call ends
//! the task `cancelled` and names the action. Every verdict is read from the
//! state: the task, the actions and the events in the store, the process's exit;
//! the holds (20 s and 7 s) are longer than the 4 s grace and than anything
//! waited for, and no verdict is taken from a short time.
//!
//! Unix only: the signals are `SIGINT`.
#![cfg(unix)]

use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

mod common;
use common::run_bounded;

fn testserver() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_mimlys-mcp-testserver"))
}

const BUILTIN: &str = r#""read_file", "write_file", "replace_in_file", "list_dir", "search_files", "delete_path", "run_script", "run_wasm""#;

/// The exact line of REQ-CLI-005 for the tool under test.
const WAITING: &str = "waiting for rypsok.slow to finish: it changes something outside this machine and cannot be undone; press Ctrl-C again to interrupt it anyway";

struct Env {
    data: tempfile::TempDir,
    config: PathBuf,
}

impl Env {
    /// The test server as `rypsok`, its `slow` tool declared with `effect`
    /// (`external_write`, or `external_read` and idempotent), the model
    /// proposing `slow` for `ms` milliseconds and then completing.
    fn new(ms: u64, effect: &str) -> Self {
        let data = tempfile::tempdir().unwrap();
        let config = data.path().join("config.toml");
        let script = [
            json!({"decision": "act", "tool": "rypsok.slow", "arguments": {"ms": ms}}),
            json!({"decision": "complete", "summary": "done"}),
        ];
        let script_path = data.path().join("script.json");
        std::fs::write(&script_path, serde_json::to_string(&script).unwrap()).unwrap();
        let text = format!(
            r#"config_version = 1
[runtime]
data_dir = {data:?}
[backend]
kind = "fake"
script = {script:?}
[policy.profiles.workspace]
tools = [{builtin}, "rypsok.slow"]
[servers.rypsok]
command = {server:?}
env_allow = ["PATH"]
[servers.rypsok.tools.slow]
effect = {effect:?}
idempotent = {idempotent}
timeout_ms = 60000
"#,
            data = data.path().join("data").to_string_lossy(),
            script = script_path.to_string_lossy(),
            builtin = BUILTIN,
            server = testserver().to_string_lossy(),
            idempotent = effect == "external_read",
        );
        std::fs::write(&config, text).unwrap();
        Self { data, config }
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_mimlys"));
        cmd.arg("--config")
            .arg(&self.config)
            .args(args)
            .env("MIMLYS_CONFIG", "")
            .env("MIMLYS_WORKER_BIN", env!("CARGO_BIN_EXE_mimlys-worker"))
            .stdin(std::process::Stdio::null());
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

    /// The events of a task, as a second process reads them.
    fn events(&self, id: &str) -> Vec<Value> {
        let (_, events) = self.json(&["events", id]);
        events
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["record"].clone())
            .collect()
    }

    /// The `action.finished` statuses of the task, in order.
    fn finished(&self, id: &str) -> Vec<String> {
        self.events(id)
            .iter()
            .filter(|e| e["event"] == "action.finished")
            .map(|e| e["status"].as_str().unwrap().to_owned())
            .collect()
    }

    /// The task as `task show` reports it.
    fn task(&self, id: &str) -> Value {
        self.json(&["task", "show", id]).1
    }

    /// The observations of the task, as the store holds them.
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

    /// `run`, which stops at the approval an `external_write` always needs
    /// (there is no terminal): returns the task's id.
    fn create(&self) -> String {
        let scope = self.scope();
        let (code, value) = self.json(&["run", "send it", "--scope", scope.to_str().unwrap()]);
        assert_eq!(code, 0, "{value}");
        value["task"]["task_id"].as_str().unwrap().to_owned()
    }
}

/// Polls `done` every 25 ms until it holds or `limit` has passed.
fn wait_for(limit: Duration, mut done: impl FnMut() -> bool) -> bool {
    let started = std::time::Instant::now();
    while started.elapsed() < limit {
        if done() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    done()
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

/// A `mimlys` command that follows a task, its streams in files.
struct Followed {
    run: std::process::Child,
    out_path: PathBuf,
    err_path: PathBuf,
}

impl Followed {
    /// Starts `args` and waits until the external call is in flight: the store
    /// has the action's start.
    fn start(env: &Env, args: &[&str]) -> Self {
        let mut all = vec!["--json"];
        all.extend_from_slice(args);
        let out_path = env.data.path().join("follow.stdout");
        let err_path = env.data.path().join("follow.stderr");
        let run = env
            .command(&all)
            .stdout(std::fs::File::create(&out_path).unwrap())
            .stderr(std::fs::File::create(&err_path).unwrap())
            .spawn()
            .unwrap();
        let mut followed = Self {
            run,
            out_path,
            err_path,
        };
        followed.await_state(env, "the external call never started", || {
            count_events(env.data.path(), "action.started") >= 1
        });
        followed
    }

    /// Waits until `reached` holds; a run that never gets there fails the
    /// test with its evidence (the bound is far longer than any start).
    fn await_state(&mut self, env: &Env, what: &str, reached: impl FnMut() -> bool) {
        if !wait_for(Duration::from_secs(120), reached) {
            self.give_up(env, what);
        }
    }

    /// Sends one SIGINT.
    fn interrupt(&self) {
        let _ = Command::new("kill")
            .args(["-INT", &self.run.id().to_string()])
            .status();
    }

    /// Waits for the exit, with a bound far longer than anything it waits for.
    fn exit(&mut self, env: &Env) -> std::process::ExitStatus {
        let mut status = None;
        wait_for(Duration::from_secs(120), || {
            status = self.run.try_wait().unwrap();
            status.is_some()
        });
        match status {
            Some(status) => status,
            None => self.give_up(env, "the run did not exit"),
        }
    }

    fn stderr(&self) -> String {
        std::fs::read_to_string(&self.err_path).unwrap_or_default()
    }

    fn stdout(&self) -> Value {
        let text = std::fs::read_to_string(&self.out_path).unwrap_or_default();
        serde_json::from_str(text.trim())
            .unwrap_or_else(|e| panic!("not JSON ({e}): {text}\n{}", self.stderr()))
    }

    /// Prints both streams and the store's events, kills the run, fails.
    fn give_up(&mut self, env: &Env, what: &str) -> ! {
        let events = rusqlite::Connection::open_with_flags(
            env.data.path().join("data/mimlys.db"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .and_then(|conn| {
            let mut stmt =
                conn.prepare("select at || ' ' || name from events order by position")?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
            Ok(rows.flatten().collect::<Vec<_>>().join("\n"))
        })
        .unwrap_or_default();
        let report = format!(
            "{what}\n--- stdout ---\n{}\n--- stderr ---\n{}\n--- events ---\n{events}",
            std::fs::read_to_string(&self.out_path).unwrap_or_default(),
            self.stderr(),
        );
        let _ = self.run.kill();
        let _ = self.run.wait();
        panic!("{report}");
    }
}

/// REQ-CLI-005 (first signal, an action that cannot be undone): `rypsok.slow`
/// declared `external_write` is held 7 s, longer than the grace of 4 s. One stop
/// signal while it runs: the process says that it waits for the tool and why,
/// once; it interrupts nothing (the action ends by itself, having waited its
/// full 7 s: `slept_ms` is in its observation, and it is `succeeded`, never
/// `cancelled`); the pause is taken at the step's end and the task is `paused`.
/// Judged by state: the exit, the task, the action's status and observation.
#[test]
fn req_cli_005_one_stop_signal_waits_for_an_external_write_to_finish() {
    let env = Env::new(7000, "external_write");
    let id = env.create();
    let mut run = Followed::start(&env, &["approve", &id]);
    run.interrupt();
    let err_path = run.err_path.clone();
    run.await_state(&env, "the wait was never announced", || {
        std::fs::read_to_string(&err_path).is_ok_and(|t| t.contains(WAITING))
    });
    let status = run.exit(&env);
    let stderr = run.stderr();
    // The runtime's own lines (the library's log lines are JSON): the one line
    // of the wait, nothing about an interruption.
    let said: Vec<&str> = stderr.lines().filter(|l| !l.starts_with('{')).collect();
    assert_eq!(
        said,
        [WAITING],
        "said once, nothing more: {status:?} {stderr}"
    );
    assert_eq!(env.task(&id)["task"]["state"], "paused", "{stderr}");
    assert_eq!(
        env.finished(&id),
        ["succeeded"],
        "the action ended by itself"
    );
    let observations = env.observations(&id);
    assert_eq!(observations.len(), 1, "{observations:?}");
    assert_eq!(
        observations[0]["status"], "succeeded",
        "it ran to its end: {}",
        observations[0]
    );
    assert!(
        observations[0].to_string().contains("slept_ms"),
        "the tool answered: {}",
        observations[0]
    );
    assert!(
        env.task(&id)["unknown_actions"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

/// REQ-CLI-005 (second signal, an action that cannot be undone): the same tool
/// held 20 s. The first signal makes the process wait; the second interrupts the
/// call anyway. The outcome is recorded as unknown: the action's status is
/// `unknown` (the status a crash gives it, not `cancelled`), the task is
/// `halted` with `unknown_outcome`, `task show` lists the action, and the
/// command's own JSON and text name it. Judged by state.
#[test]
fn req_cli_005_a_second_stop_signal_interrupts_it_anyway_and_the_task_halts_unknown_outcome() {
    let env = Env::new(20_000, "external_write");
    let id = env.create();
    let mut run = Followed::start(&env, &["approve", &id]);
    run.interrupt();
    let err_path = run.err_path.clone();
    run.await_state(&env, "the wait was never announced", || {
        std::fs::read_to_string(&err_path).is_ok_and(|t| t.contains(WAITING))
    });
    run.interrupt();
    let status = run.exit(&env);
    let stderr = run.stderr();
    assert!(
        stderr.contains("interrupting rypsok.slow although it cannot be undone"),
        "{status:?} {stderr}"
    );
    let task = env.task(&id);
    assert_eq!(task["task"]["state"], "halted", "{task}");
    assert_eq!(task["task"]["halt_reason"], "unknown_outcome", "{task}");
    assert_eq!(
        env.finished(&id),
        ["unknown"],
        "never `cancelled`: {stderr}"
    );
    assert!(
        env.events(&id)
            .iter()
            .any(|e| e["event"] == "task.state_changed"
                && e["to"] == "halted"
                && e["reason"] == "unknown_outcome"),
        "the halt is in the events"
    );
    let listed = task["unknown_actions"].as_array().unwrap();
    assert_eq!(listed.len(), 1, "{task}");
    assert_eq!(listed[0]["tool"], "rypsok.slow", "{task}");
    assert_eq!(listed[0]["effect"], "external_write", "{task}");
    // The run's own output names it too.
    let printed = run.stdout();
    assert_eq!(
        printed["unknown_actions"], task["unknown_actions"],
        "{printed}"
    );
    let observations = env.observations(&id);
    assert_eq!(observations[0]["status"], "unknown", "{}", observations[0]);
}

/// REQ-CLI-005 (first signal, what can be undone, through an external tool):
/// the same tool declared `external_read` and idempotent, held 20 s. The grace
/// passes and the call is interrupted, as before handoff 010e: the action is
/// `cancelled`, nothing is unknown, the pause is taken and the task is `paused`.
/// The rule is the action's rule after `unknown`, not the tool's origin.
#[test]
fn req_cli_005_one_stop_signal_interrupts_an_idempotent_external_read_at_the_grace() {
    let env = Env::new(20_000, "external_read");
    let scope = env.scope();
    let mut run = Followed::start(
        &env,
        &["run", "look it up", "--scope", scope.to_str().unwrap()],
    );
    run.interrupt();
    let status = run.exit(&env);
    let stderr = run.stderr();
    let id = run.stdout()["task"]["task_id"].as_str().unwrap().to_owned();
    assert!(
        !stderr.contains("waiting for"),
        "an idempotent read is not waited for: {status:?} {stderr}"
    );
    assert_eq!(env.task(&id)["task"]["state"], "paused", "{stderr}");
    assert_eq!(env.finished(&id), ["cancelled"], "{stderr}");
    assert!(
        env.task(&id)["unknown_actions"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

/// A task whose process died in the middle of its `external_write` call: the
/// call is in flight, the process is killed.
fn killed_in_flight(env: &Env) -> String {
    let id = env.create();
    let mut run = Followed::start(env, &["approve", &id]);
    let _ = run.run.kill();
    let _ = run.run.wait();
    id
}

/// REQ-TASK-006 (`task cancel` and an external write, JSON): the process that was
/// running the call dies in the middle of it (killed); the next command that
/// writes recovers the store, which marks the action unknown and halts the task
/// `unknown_outcome` (external design §16.3), and `task cancel` then ends the
/// task `cancelled` as asked. The command's JSON names the action with its
/// outcome unknown, and the action is `unknown`, not `cancelled`. (The store
/// admits one runtime at a time, so a second process cannot cancel a call that
/// a live one is making; `Runtime::cancel_task` on an action running in the
/// same runtime is covered in `mimlys-core`.)
#[test]
fn req_task_006_task_cancel_names_an_external_write_whose_outcome_is_unknown_json() {
    let env = Env::new(20_000, "external_write");
    let id = killed_in_flight(&env);
    let (code, value) = env.json(&["task", "cancel", &id]);
    assert_eq!(code, 0, "{value}");
    assert_eq!(value["task"]["state"], "cancelled", "{value}");
    let listed = value["unknown_actions"].as_array().unwrap();
    assert_eq!(listed.len(), 1, "{value}");
    assert_eq!(listed[0]["tool"], "rypsok.slow", "{value}");
    assert_eq!(listed[0]["effect"], "external_write", "{value}");
    assert_eq!(env.finished(&id), ["unknown"], "not `cancelled`");
}

/// REQ-TASK-006 (the same, text): the command's own text names the action.
#[test]
fn req_task_006_task_cancel_names_an_external_write_whose_outcome_is_unknown_text() {
    let env = Env::new(20_000, "external_write");
    let id = killed_in_flight(&env);
    let out = run_bounded(&mut env.command(&["task", "cancel", &id]), env.data.path());
    assert_eq!(out.code(), 0, "{}", out.stderr);
    assert!(
        out.stdout.contains("cancelled")
            && out.stdout.contains("outcome unknown: action")
            && out.stdout.contains("rypsok.slow"),
        "{}",
        out.stdout
    );
}
