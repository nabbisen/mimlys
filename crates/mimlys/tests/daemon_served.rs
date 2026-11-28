//! Commands served by the daemon (RFC 014, Amendment 1; external design §4, §19.5; REQ-ARCH-009,
//! REQ-PERS-014, REQ-SCHED-008, REQ-SEC-016, REQ-CLI-005, REQ-CLI-008): a second terminal, a
//! repeat after a broken connection, `stop` and `restart` with a task running, the line, and the
//! daemon's own tree.
//!
//! Each test owns one environment (one registry and so one socket, one data directory) and one
//! daemon, which it stops when it ends. Unix only: the daemon is Linux's and macOS's in this
//! release.
#![cfg(unix)]

use serde_json::{Value, json};
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

mod common;
use common::run_bounded;

const BUILTIN: &str = r#""read_file", "write_file", "replace_in_file", "list_dir", "search_files", "delete_path", "run_script", "run_wasm""#;

fn probe_tool() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_mimlys-probe-tool"))
}

fn testserver() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_mimlys-mcp-testserver"))
}

fn write_file() -> Value {
    json!({"decision": "act", "tool": "write_file", "arguments": {"path": "notes/out.txt", "content": "hello"}})
}

fn ask() -> Value {
    json!({"decision": "ask_user", "question": "Which branch?"})
}

/// A host tool that runs for `secs` seconds: on Linux it leaves a sleeper in a session of its own
/// too (the tree a cancel must end); macOS has no `setsid` command, and its tool only sleeps.
fn slow(secs: &str) -> Value {
    let mode = if cfg!(target_os = "linux") {
        "setsid-sleep"
    } else {
        "sleep"
    };
    probe(&[mode, secs])
}

fn probe(args: &[&str]) -> Value {
    json!({"decision": "act", "tool": "run_host_tool", "arguments": {"tool": "probe", "args": args}})
}

fn complete() -> Value {
    json!({"decision": "complete", "summary": "done"})
}

struct Env {
    registry: common::Registry,
    data: tempfile::TempDir,
    config: PathBuf,
    extra_env: Vec<(String, String)>,
}

impl Env {
    /// A fake backend that plays `script`, the profile asking for `write_file`'s approval, the
    /// probe tool declared as `probe`, and the test server as `rypsok`.
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
tools = [{builtin}]
approval_tools = ["write_file"]
[host_tools.probe]
command = {tool:?}
read_paths = [{tooldir:?}]
allowed_args = ["ok", "setsid-sleep", "sleep"]
arg_pattern = "[0-9]+"
max_args = 2
effect = "workspace_write"
timeout_ms = 60000
[servers.rypsok]
command = {server:?}
env_allow = ["PATH", "MIMLYS_TESTSERVER_RUN_AT_START"]
"#,
            data = data.path().join("data").to_string_lossy(),
            script = script_path.to_string_lossy(),
            builtin = BUILTIN,
            tool = probe_tool().to_string_lossy(),
            tooldir = probe_tool().parent().unwrap().to_string_lossy(),
            server = testserver().to_string_lossy(),
        );
        std::fs::write(&config, text).unwrap();
        Self {
            registry: common::Registry::new(),
            data,
            config,
            extra_env: vec![],
        }
    }

    fn env(mut self, key: &str, value: &str) -> Self {
        self.extra_env.push((key.to_owned(), value.to_owned()));
        self
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut cmd = common::mimlys_in(&self.registry);
        cmd.arg("--config")
            .arg(&self.config)
            .args(args)
            .env("MIMLYS_CONFIG", "")
            .env("MIMLYS_WORKER_BIN", env!("CARGO_BIN_EXE_mimlys-worker"))
            .stdin(std::process::Stdio::null());
        for (k, v) in &self.extra_env {
            cmd.env(k, v);
        }
        cmd
    }

    fn run(&self, args: &[&str]) -> common::Bounded {
        run_bounded(&mut self.command(args), self.data.path())
    }

    fn json(&self, args: &[&str]) -> (i32, Value) {
        let mut all = vec!["--json"];
        all.extend_from_slice(args);
        let out = self.run(&all);
        let value = serde_json::from_str(out.stdout.trim()).unwrap_or_else(|e| {
            panic!(
                "not JSON ({e}): {}\n{}\n(args: {all:?})",
                out.stdout, out.stderr
            )
        });
        (out.code(), value)
    }

    fn start_daemon(&self) {
        let out = self.run(&["daemon", "start"]);
        assert_eq!(out.code(), 0, "{}\n{}", out.stdout, out.stderr);
    }

    fn scope(&self) -> PathBuf {
        let dir = self.data.path().join("scope");
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/lib.rs"), "fn a() {}\n").unwrap();
        dir
    }

    /// The task's state, as `task list` says it.
    fn state(&self, id: &str) -> String {
        let (_, tasks) = self.json(&["task", "list"]);
        tasks
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["task_id"] == id)
            .map(|t| t["state"].as_str().unwrap().to_owned())
            .unwrap_or_default()
    }

    fn wait_state(&self, id: &str, state: &str) {
        assert!(
            wait_for(Duration::from_secs(120), || self.state(id) == state),
            "task {id} did not reach {state}: it is {}",
            self.state(id)
        );
    }

    /// `run --detach`: the task is created and the daemon runs it; its id.
    fn run_detached(&self, extra: &[&str]) -> String {
        let scope = self.scope();
        let mut args = vec![
            "run",
            "goal",
            "--detach",
            "--scope",
            scope.to_str().unwrap(),
        ];
        args.extend_from_slice(extra);
        let (code, value) = self.json(&args);
        assert_eq!(code, 0, "{value}\n{}", self.daemon_log());
        value["task"]["task_id"].as_str().unwrap().to_owned()
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

    /// What the daemon's log says, for a failure's message.
    fn daemon_log(&self) -> String {
        std::fs::read_to_string(self.data.path().join("data/logs/daemon.log")).unwrap_or_default()
    }

    fn daemon_pid(&self) -> Option<i32> {
        let (_, status) = self.json(&["daemon", "status"]);
        status["daemon"]["pid"]
            .as_i64()
            .and_then(|p| i32::try_from(p).ok())
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let pid = self.daemon_pid_quietly();
        let _ = self.run(&["daemon", "stop"]);
        if let Some(pid) = pid.and_then(rustix::process::Pid::from_raw) {
            std::thread::sleep(Duration::from_millis(300));
            let _ = rustix::process::kill_process(pid, rustix::process::Signal::KILL);
        }
    }
}

impl Env {
    fn daemon_pid_quietly(&self) -> Option<i32> {
        let out = self.run(&["--json", "daemon", "status"]);
        serde_json::from_str::<Value>(out.stdout.trim())
            .ok()
            .and_then(|v| v["daemon"]["pid"].as_i64())
            .and_then(|p| i32::try_from(p).ok())
    }
}

/// Whether this machine runs host tools (the full tier); a test that needs one says that it did not
/// run its subject here, as the skip tier does.
fn full_tier(env: &Env, test: &str) -> bool {
    let (_, status) = env.json(&["status"]);
    if status["tier"] == "full" {
        return true;
    }
    common::skip(test, &format!("tier {}", status["tier"]));
    false
}

fn wait_for(limit: Duration, mut done: impl FnMut() -> bool) -> bool {
    let started = std::time::Instant::now();
    while started.elapsed() < limit {
        if done() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    done()
}

/// How many processes of a slow tool's tree run now: those whose command line ends with
/// `sleep <secs>` (the tool, its shell and its sleepers).
fn sleepers(secs: &str) -> usize {
    let out = Command::new("pgrep")
        .args(["-f", &format!("sleep {secs}$")])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| !l.trim().is_empty())
        .count()
}

/// The session an event names.
fn session_of(event: &Value) -> String {
    event["session"].as_str().unwrap_or_default().to_owned()
}

fn event_named<'a>(events: &'a [Value], name: &str) -> Option<&'a Value> {
    events.iter().find(|e| e["event"] == name)
}

/// REQ-ARCH-009, external design §19.5: **a task started in one terminal is approved in another.**
/// The first client creates and starts it and goes (`--detach`); the daemon runs it to the
/// approval; a second client approves it and follows it to its end. The audit names each client's
/// own process, as the daemon read it from the system.
#[test]
fn req_arch_009_a_task_started_in_one_terminal_is_approved_from_another() {
    let env = Env::new(&[write_file(), complete()]);
    env.start_daemon();
    let id = env.run_detached(&[]);
    env.wait_state(&id, "waiting_user");
    let approved = env.run(&["approve", &id]);
    assert_eq!(
        approved.code(),
        0,
        "{}\n{}",
        approved.stdout,
        approved.stderr
    );
    assert!(
        approved.stdout.contains("completed"),
        "the second client follows the task to its end: {}",
        approved.stdout
    );
    assert_eq!(env.state(&id), "completed");
    let events = env.events(&id);
    let created = session_of(event_named(&events, "task.created").unwrap());
    let decided = session_of(event_named(&events, "approval.decided").unwrap());
    for session in [&created, &decided] {
        assert!(session.contains("pid:"), "{session} in {events:?}");
        assert!(!session.contains("pid:unknown"), "{session}");
    }
    assert_ne!(created, decided, "two clients, two sessions");
}

/// REQ-ARCH-009: a question the task asks is answered from a second client.
#[test]
fn req_arch_009_a_question_is_answered_from_another_terminal() {
    let env = Env::new(&[ask(), complete()]);
    env.start_daemon();
    let id = env.run_detached(&[]);
    env.wait_state(&id, "waiting_user");
    let answered = env.run(&["answer", &id, "main"]);
    assert_eq!(
        answered.code(),
        0,
        "{}\n{}",
        answered.stdout,
        answered.stderr
    );
    assert_eq!(env.state(&id), "completed");
    assert!(event_named(&env.events(&id), "question.answered").is_some());
}

/// REQ-ARCH-009: a task that waits is cancelled from a second client.
#[test]
fn req_arch_009_a_waiting_task_is_cancelled_from_another_terminal() {
    let env = Env::new(&[write_file(), complete()]);
    env.start_daemon();
    let id = env.run_detached(&[]);
    env.wait_state(&id, "waiting_user");
    let cancelled = env.run(&["task", "cancel", &id]);
    assert_eq!(
        cancelled.code(),
        0,
        "{}\n{}",
        cancelled.stdout,
        cancelled.stderr
    );
    assert_eq!(env.state(&id), "cancelled");
}

/// REQ-CLI-005, REQ-TASK-006 (014a's deferred test): **a running host tool is cancelled from a
/// second client, and its tree is gone within LIM-O2.** The tool and the sleeper it left in a
/// session of its own are running; a second client cancels the task; every sleeper ends.
#[test]
fn req_task_006_a_running_host_tool_is_cancelled_from_a_second_client() {
    let env = Env::new(&[slow("1236"), complete()]);
    if !full_tier(
        &env,
        "req_task_006_a_running_host_tool_is_cancelled_from_a_second_client",
    ) {
        return;
    }
    env.start_daemon();
    let id = env.run_detached(&["--grant", "host_tool:probe"]);
    assert!(
        wait_for(Duration::from_secs(60), || sleepers("1236") >= 1),
        "the host tool and its setsid child run"
    );
    let begun = std::time::Instant::now();
    let cancelled = env.run(&["task", "cancel", &id]);
    assert_eq!(
        cancelled.code(),
        0,
        "{}\n{}",
        cancelled.stdout,
        cancelled.stderr
    );
    assert!(
        wait_for(Duration::from_secs(60), || sleepers("1236") == 0),
        "every sleeper, the setsid one included, is gone"
    );
    // Within LIM-O2 and the runtime's own grace: a margin no stall of a slow machine reaches.
    assert!(
        begun.elapsed() < Duration::from_secs(15),
        "{:?}",
        begun.elapsed()
    );
    assert_eq!(env.state(&id), "cancelled");
}

/// REQ-CLI-005, external design §19.5: **Ctrl-C in a following client pauses the task, and
/// closing it leaves the task running.** The first signal asks for a pause at the boundary; a
/// client that is killed outright (its terminal closed) leaves the task running in the daemon,
/// which goes on to its end.
#[test]
fn req_cli_005_ctrl_c_in_a_following_client_pauses_and_closing_it_leaves_the_task_running() {
    // Ctrl-C: the pause.
    let env = Env::new(&[slow("1237"), complete()]);
    if !full_tier(
        &env,
        "req_cli_005_ctrl_c_in_a_following_client_pauses_and_closing_it_leaves_the_task_running",
    ) {
        return;
    }
    env.start_daemon();
    let scope = env.scope();
    let err_path = env.data.path().join("run.stderr");
    let mut run = env
        .command(&[
            "run",
            "sleep",
            "--scope",
            scope.to_str().unwrap(),
            "--grant",
            "host_tool:probe",
        ])
        .stdout(std::fs::File::create(env.data.path().join("run.stdout")).unwrap())
        .stderr(std::fs::File::create(&err_path).unwrap())
        .spawn()
        .unwrap();
    assert!(
        wait_for(Duration::from_secs(60), || sleepers("1237") >= 1),
        "the tool runs"
    );
    let _ = Command::new("kill")
        .args(["-INT", &run.id().to_string()])
        .status();
    assert!(
        wait_for(Duration::from_secs(60), || {
            std::fs::read_to_string(&err_path)
                .is_ok_and(|t| t.contains("the task pauses when the step in flight has finished"))
        }),
        "the first signal was never taken: {}",
        std::fs::read_to_string(&err_path).unwrap_or_default()
    );
    wait_for(common::BOUND, || run.try_wait().unwrap().is_some());
    let _ = run.kill();
    let _ = run.wait();
    let (_, tasks) = env.json(&["task", "list"]);
    let id = tasks[0]["task_id"].as_str().unwrap().to_owned();
    env.wait_state(&id, "paused");
}

/// REQ-CLI-005, external design §19.5: a client that goes away (killed, a closed terminal)
/// **leaves the task running**; the daemon goes on.
#[test]
fn req_cli_005_a_client_that_goes_away_leaves_the_task_running() {
    let env = Env::new(&[slow("1238"), complete()]);
    if !full_tier(
        &env,
        "req_cli_005_a_client_that_goes_away_leaves_the_task_running",
    ) {
        return;
    }
    env.start_daemon();
    let scope = env.scope();
    let mut run = env
        .command(&[
            "run",
            "sleep",
            "--scope",
            scope.to_str().unwrap(),
            "--grant",
            "host_tool:probe",
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    assert!(
        wait_for(Duration::from_secs(60), || sleepers("1238") >= 1),
        "the tool runs"
    );
    run.kill().unwrap();
    let _ = run.wait();
    let (_, tasks) = env.json(&["task", "list"]);
    let id = tasks[0]["task_id"].as_str().unwrap().to_owned();
    assert_eq!(
        env.state(&id),
        "running",
        "the daemon goes on without its client"
    );
    // `sleep 1238` is 1238 seconds: cancel it from another client, as a user would.
    assert_eq!(env.run(&["task", "cancel", &id]).code(), 0);
    env.wait_state(&id, "cancelled");
}

/// REQ-SEC-016, INV-13, THR-39, external design §20: **a request from the daemon's own tree is
/// refused and the refusal recorded.** An MCP test server runs, as a child in its own session,
/// `mimlys approve` for a task that waits for approval; the daemon refuses it, nothing is
/// approved, and the audit holds `daemon.refused`. The user's own client then approves it.
#[test]
fn req_sec_016_a_request_from_the_daemons_own_tree_is_refused_and_recorded() {
    let probe_dir = tempfile::tempdir().unwrap();
    let marker = probe_dir.path().join("task-id");
    let (code_file, err_file) = (probe_dir.path().join("code"), probe_dir.path().join("err"));
    let script = probe_dir.path().join("try-approve.sh");
    // Filled in after the environment exists (its registry and configuration are the script's).
    let mut env = Env::new(&[write_file(), complete()]);
    let run_dir = env.registry.path();
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\nwhile [ ! -s {marker:?} ]; do sleep 0.1; done\nMIMLYS_RUN_DIR={run:?} MIMLYS_CONFIG= {bin:?} --config {config:?} approve \"$(cat {marker:?})\" > /dev/null 2> {err:?}\necho $? > {code:?}\n",
            run = run_dir,
            bin = env!("CARGO_BIN_EXE_mimlys"),
            config = env.config,
            err = err_file,
            code = code_file,
        ),
    )
    .unwrap();
    env = env.env("MIMLYS_TESTSERVER_RUN_AT_START", script.to_str().unwrap());
    env.start_daemon();
    let id = env.run_detached(&[]);
    env.wait_state(&id, "waiting_user");
    std::fs::write(&marker, &id).unwrap();
    assert!(
        wait_for(Duration::from_secs(60), || code_file.exists()),
        "the server's child never answered"
    );
    let code = std::fs::read_to_string(&code_file).unwrap();
    let said = std::fs::read_to_string(&err_file).unwrap();
    assert_eq!(code.trim(), "1", "{said}");
    assert!(said.contains("daemon's own tree"), "{said}");
    // Nothing was applied: the task still waits for the user.
    assert_eq!(env.state(&id), "waiting_user");
    assert!(event_named(&env.events(&id), "approval.decided").is_none());
    // The refusal is in the audit (a runtime event, not the task's), and the user's own client
    // may approve.
    let (_, all) = env.json(&["events"]);
    assert!(
        all.as_array().unwrap().iter().any(
            |e| e["record"]["event"] == "daemon.refused" && e["record"]["reason"] == "own_tree"
        ),
        "{all}"
    );
    assert_eq!(env.run(&["approve", &id]).code(), 0);
    assert_eq!(env.state(&id), "completed");
}

/// REQ-ARCH-004, external design §4: **a connection cut after a request was applied and before its
/// answer was read**: the client makes the connection again, sends the same request once more
/// under the same id, and answers with what the store recorded. Exactly one task exists.
/// (`mutation: a repeat after a break applied twice`)
#[test]
fn req_arch_004_a_repeat_after_a_broken_connection_is_applied_once() {
    let env = Env::new(&[complete()]).env("MIMLYS_DAEMON_CUT_ANSWER", "create_task");
    env.start_daemon();
    let scope = env.scope();
    let created = env.run(&[
        "--json",
        "task",
        "new",
        "--hold",
        "goal",
        "--scope",
        scope.to_str().unwrap(),
    ]);
    assert_eq!(created.code(), 0, "{}\n{}", created.stdout, created.stderr);
    let value: Value = serde_json::from_str(created.stdout.trim()).unwrap();
    let id = value["task"]["task_id"].as_str().unwrap();
    assert!(
        created.stderr.contains("done earlier"),
        "the user is told that it was done earlier: {}",
        created.stderr
    );
    let (_, tasks) = env.json(&["task", "list"]);
    assert_eq!(tasks.as_array().unwrap().len(), 1, "{tasks}");
    assert_eq!(tasks[0]["task_id"], id);
}

/// REQ-CLI-008, REQ-CLI-005: **`stop` with a task running pauses it, lists it and resumes
/// nothing**; the tool's tree is gone; the daemon is gone. (`mutation: stop resuming what it
/// paused`)
#[test]
fn req_cli_008_stop_pauses_a_running_task_lists_it_and_resumes_nothing() {
    let env = Env::new(&[slow("1239"), complete()]);
    if !full_tier(
        &env,
        "req_cli_008_stop_pauses_a_running_task_lists_it_and_resumes_nothing",
    ) {
        return;
    }
    env.start_daemon();
    let id = env.run_detached(&["--grant", "host_tool:probe"]);
    assert!(
        wait_for(Duration::from_secs(60), || sleepers("1239") >= 1),
        "the tool runs"
    );
    let (code, stopped) = env.json(&["daemon", "stop"]);
    assert_eq!(code, 0, "{stopped}");
    assert_eq!(stopped["daemon"]["paused"], json!([id]), "{stopped}");
    // Nothing resumes by itself: the task stays paused, and there is no daemon to resume it.
    assert_eq!(env.state(&id), "paused");
    assert!(
        wait_for(Duration::from_secs(30), || sleepers("1239") == 0),
        "the tool's tree is gone"
    );
    std::thread::sleep(Duration::from_secs(2));
    assert_eq!(env.state(&id), "paused");
    assert_eq!(
        env.json(&["daemon", "status"]).1["daemon"]["running"],
        false
    );
}

/// REQ-CLI-008: **`restart` resumes exactly what its stop paused**, and says so.
#[test]
fn req_cli_008_restart_resumes_exactly_the_tasks_its_stop_paused() {
    let env = Env::new(&[slow("1240"), complete(), complete()]);
    if !full_tier(
        &env,
        "req_cli_008_restart_resumes_exactly_the_tasks_its_stop_paused",
    ) {
        return;
    }
    env.start_daemon();
    let id = env.run_detached(&["--grant", "host_tool:probe"]);
    assert!(
        wait_for(Duration::from_secs(60), || sleepers("1240") >= 1),
        "the tool runs"
    );
    let (code, restarted) = env.json(&["daemon", "restart"]);
    assert_eq!(code, 0, "{restarted}");
    assert_eq!(restarted["daemon"]["resumed"], json!([id]), "{restarted}");
    // The new daemon runs it again, from the script's start: the tool, then it completes (the
    // sleeper is ended with the task, which the test cancels from a second client).
    assert_eq!(env.run(&["task", "cancel", &id]).code(), 0);
    env.wait_state(&id, "cancelled");
}

/// REQ-SCHED-008, external design §19.5: **one line under one budget.** A second task that
/// would run beside the first waits in line, says what it waits behind, and starts when the first
/// leaves `running`. A task that waits for the user keeps the daemon alive past its idle time.
#[test]
fn req_sched_008_a_second_task_waits_in_line_and_a_waiting_task_keeps_the_daemon_alive() {
    let env = Env::new(&[slow("3"), complete(), complete()]).env("MIMLYS_DAEMON_IDLE_MS", "400");
    env.start_daemon();
    let first = env.run_detached(&["--grant", "host_tool:probe"]);
    // The second, on a scope of its own (two tasks never share one), while the first runs.
    let other = env.data.path().join("other");
    std::fs::create_dir_all(&other).unwrap();
    std::fs::write(other.join("b.txt"), "b\n").unwrap();
    let second_run = env.run(&[
        "--json",
        "run",
        "second",
        "--detach",
        "--scope",
        other.to_str().unwrap(),
    ]);
    assert_eq!(
        second_run.code(),
        0,
        "{}\n{}",
        second_run.stdout,
        second_run.stderr
    );
    assert!(
        second_run
            .stderr
            .contains(&format!("queued behind task {first}, running since")),
        "{}",
        second_run.stderr
    );
    let second: Value = serde_json::from_str(second_run.stdout.trim()).unwrap();
    let second = second["task"]["task_id"].as_str().unwrap().to_owned();
    env.wait_state(&first, "completed");
    env.wait_state(&second, "completed");
}

/// REQ-CLI-008, LIM-O13: a task that waits for an approval keeps the daemon alive: nothing the
/// idle time would end while it waits.
#[test]
fn req_cli_008_a_task_that_waits_for_the_user_keeps_the_daemon_alive() {
    let env = Env::new(&[write_file(), complete()]).env("MIMLYS_DAEMON_IDLE_MS", "300");
    env.start_daemon();
    let id = env.run_detached(&[]);
    env.wait_state(&id, "waiting_user");
    std::thread::sleep(Duration::from_secs(3));
    assert!(
        env.daemon_pid().is_some(),
        "the daemon is alive past its idle time"
    );
    // Once it is cancelled the daemon has nothing to do, and ends.
    assert_eq!(env.run(&["task", "cancel", &id]).code(), 0);
    // (Without asking it: a client that connects is activity, and would keep it from ending.)
    assert!(wait_for(Duration::from_secs(60), || {
        std::fs::read_dir(env.registry.path())
            .map(|d| {
                !d.flatten()
                    .any(|e| e.path().extension().is_some_and(|x| x == "sock"))
            })
            .unwrap_or(true)
    }));
}

/// What a command that a repeat answers as "took effect, and its task is gone" prints (handoff
/// 014c, item 7; external design §4): the one line, the one JSON object, success, and on standard
/// error what the client says (that it was done earlier, and that the task is gone).
fn assert_gone(out: &common::Bounded, request: &str, id: &str) {
    assert_eq!(out.code(), 0, "{}\n{}", out.stdout, out.stderr);
    let value: Value = serde_json::from_str(out.stdout.trim())
        .unwrap_or_else(|e| panic!("not JSON ({e}): {}\n{}", out.stdout, out.stderr));
    assert_eq!(
        value,
        json!({"applied": true, "gone": true, "request": request, "task_id": id}),
        "{}",
        out.stdout
    );
    assert!(out.stderr.contains("done earlier"), "{}", out.stderr);
    assert!(
        out.stderr.contains(&format!("task {id} is gone")),
        "{}",
        out.stderr
    );
}

/// External design §4, handoff 014c item 7: **a rollback that took effect and whose task was
/// deleted since** (the answer was cut, and the task deleted before the client asked again) is
/// answered as that: a success that says so, in `--json` too, and never `not_found`.
#[test]
fn req_arch_004_a_rollback_whose_task_is_gone_is_answered_as_applied_and_gone() {
    let env = Env::new(&[complete()])
        .env("MIMLYS_DAEMON_CUT_ANSWER", "rollback_workspace")
        .env("MIMLYS_DAEMON_DELETE_AFTER_CUT", "1");
    env.start_daemon();
    let scope = env.scope();
    let (code, ran) = env.json(&["run", "goal", "--scope", scope.to_str().unwrap()]);
    assert_eq!(code, 0, "{ran}");
    let id = ran["task"]["task_id"].as_str().unwrap().to_owned();
    let out = env.run(&["--json", "rollback", &id]);
    assert_gone(&out, "rollback_workspace", &id);
}

/// The same for a promotion (`promote --yes`) and for the reversal of one (`rollback --promotion`):
/// each of the five arms prints the one line and the one object.
#[test]
fn req_arch_004_a_promotion_and_its_reversal_whose_task_is_gone_are_answered_as_applied_and_gone() {
    // The promotion: the cut and the deletion fall on it.
    let env = Env::new(&[write_file(), complete()])
        .env("MIMLYS_DAEMON_CUT_ANSWER", "promote_workspace")
        .env("MIMLYS_DAEMON_DELETE_AFTER_CUT", "1");
    env.start_daemon();
    let id = env.run_detached(&[]);
    env.wait_state(&id, "waiting_user");
    assert_eq!(env.run(&["approve", &id]).code(), 0);
    let out = env.run(&["--json", "promote", &id, "--yes"]);
    assert_gone(&out, "promote_workspace", &id);
    drop(env);
    // The reversal: a promotion made as usual, then the cut and the deletion fall on its reversal.
    let env = Env::new(&[write_file(), complete()])
        .env("MIMLYS_DAEMON_CUT_ANSWER", "revert_promotion")
        .env("MIMLYS_DAEMON_DELETE_AFTER_CUT", "1");
    env.start_daemon();
    let id = env.run_detached(&[]);
    env.wait_state(&id, "waiting_user");
    assert_eq!(env.run(&["approve", &id]).code(), 0);
    let (code, promoted) = env.json(&["promote", &id, "--yes"]);
    assert_eq!(code, 0, "{promoted}");
    let promotion = promoted["promotion"]["promotion_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let out = env.run(&[
        "--json",
        "rollback",
        &id,
        "--promotion",
        &promotion,
        "--yes",
    ]);
    assert_gone(&out, "revert_promotion", &id);
}

/// REQ-ARCH-009, external design §19.5: **`status` says which mode is in force**, and, when
/// served, the daemon first.
#[test]
fn req_arch_009_status_says_the_mode_in_force() {
    let env = Env::new(&[complete()]);
    let (_, embedded) = env.json(&["status"]);
    assert_eq!(embedded["mode"], "embedded", "{embedded}");
    assert!(embedded.get("daemon").is_none());
    let text = env.run(&["status"]);
    assert!(text.stdout.contains("mode: embedded"), "{}", text.stdout);

    env.start_daemon();
    let pid = env.daemon_pid().unwrap();
    let (_, served) = env.json(&["status"]);
    assert_eq!(served["mode"], "served", "{served}");
    assert_eq!(served["daemon"]["pid"], pid);
    let text = env.run(&["status"]);
    assert!(
        text.stdout.contains("mode: served by the daemon")
            && text.stdout.contains(&format!("daemon: pid {pid}")),
        "{}",
        text.stdout
    );
}

/// External design §19.5: `--detach` needs a daemon, and in embedded mode is refused, saying so,
/// before anything is created.
#[test]
fn req_cli_008_detach_without_a_daemon_is_refused_before_anything_is_created() {
    let env = Env::new(&[complete()]);
    let scope = env.scope();
    let refused = env.run(&[
        "run",
        "goal",
        "--detach",
        "--scope",
        scope.to_str().unwrap(),
    ]);
    assert_eq!(refused.code(), 1, "{}\n{}", refused.stdout, refused.stderr);
    assert!(
        refused.stderr.contains("--detach needs a daemon"),
        "{}",
        refused.stderr
    );
    let (_, tasks) = env.json(&["task", "list"]);
    assert_eq!(tasks.as_array().map_or(0, Vec::len), 0, "{tasks}");
}
