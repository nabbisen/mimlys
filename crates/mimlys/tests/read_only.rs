//! The read commands open the store without its lock (REQ-PERS-013, RFC 013,
//! handoff 013): while another process holds the store, `status`, `events`,
//! `task list`, `task show`, `metrics`, `toolchain list` and `promote --patch`
//! answer from what is committed, say who holds the store, and never say
//! "locked"; they run no recovery, reaping or migration and leave every
//! file of the data directory as it was. The commands that write keep the
//! exclusive lock and its wait (LIM-O3).
//!
//! The process that "holds the store" is this test, through the persistence
//! crate's own `open`; the commands are the real `mimlys` binary.

use mimlys_core::clock::{Clock, SystemClock};
use mimlys_core::event::{Actor, Event, EventRecord};
use mimlys_core::ids::{EventId, SystemIds};
use mimlys_core::spi::Persistence;
use mimlys_core::task::{ExtensionCeiling, Task, TaskInput};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

mod common;
use common::run_bounded;

struct Env {
    dir: tempfile::TempDir,
    config: PathBuf,
    data: PathBuf,
}

impl Env {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        let config = dir.path().join("config.toml");
        std::fs::write(
            &config,
            format!(
                "config_version = 1\n[runtime]\ndata_dir = {:?}\n",
                data.to_string_lossy()
            ),
        )
        .unwrap();
        Self { dir, config, data }
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_mimlys"));
        cmd.arg("--config").arg(&self.config).args(args);
        cmd.env("MIMLYS_CONFIG", "");
        cmd
    }

    /// Exit code, standard output, standard error.
    fn run(&self, args: &[&str]) -> (i32, String, String) {
        let out = run_bounded(&mut self.command(args), self.dir.path());
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

    /// A project to work on, with one file.
    fn scope(&self) -> PathBuf {
        let dir = self.dir.path().join("scope");
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/lib.rs"), "fn a() {}\n").unwrap();
        dir
    }

    /// A task held in `created` by the real binary: returns its id.
    fn new_task(&self) -> String {
        let scope = self.scope();
        let (code, value) = self.json(&[
            "task",
            "new",
            "read me",
            "--scope",
            scope.to_str().unwrap(),
            "--hold",
        ]);
        assert_eq!(code, 0, "{value}");
        value["task"]["task_id"].as_str().unwrap().to_owned()
    }

    /// The task as the store has it.
    fn task(&self, id: &str) -> Task {
        let (code, value) = self.json(&["task", "show", id]);
        assert_eq!(code, 0, "{value}");
        serde_json::from_value(value["task"].clone()).unwrap()
    }
}

/// Moves a task through `input` in `store`, as a runtime would, with its event.
fn apply(store: &dyn Persistence, task: &Task, input: TaskInput) -> Task {
    let now = SystemClock.now();
    let next = task.transition(input, now).unwrap();
    store
        .record_task_transition(
            &next,
            &EventRecord {
                event_id: EventId::new(&SystemIds),
                at: now,
                actor: Actor::Runtime,
                task_id: Some(next.task_id),
                step_id: None,
                action_id: None,
                policy_hash: task.versions.policy_hash.clone(),
                event: Event::TaskStateChanged {
                    from: task.state.name().to_owned(),
                    to: next.state.name().to_owned(),
                    reason: None,
                },
            },
        )
        .unwrap();
    next
}

fn resume() -> TaskInput {
    TaskInput::Resume {
        extend: None,
        ceiling: ExtensionCeiling {
            max_steps: 200,
            max_deadline: SystemClock.now().plus_millis(86_400_000).unwrap(),
        },
    }
}

/// Every file of the data directory with its length and a digest of its
/// bytes, except the shared-memory index SQLite keeps beside a WAL database
/// and an empty write-ahead log (a read-only open creates them; they hold no
/// data) and the holder record a live writer owns.
fn snapshot(dir: &Path) -> Vec<(String, u64, u64)> {
    use std::hash::{DefaultHasher, Hash, Hasher};
    fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, u64, u64)>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            if name.ends_with("-shm") || name == "mimlys.holder" {
                continue;
            }
            if path.is_dir() {
                walk(root, &path, out);
            } else {
                let bytes = std::fs::read(&path).unwrap_or_default();
                // SQLite creates an empty `-wal` when it opens a WAL database
                // read-only; it holds no data.
                if name.ends_with("-wal") && bytes.is_empty() {
                    continue;
                }
                let mut hasher = DefaultHasher::new();
                bytes.hash(&mut hasher);
                out.push((name, bytes.len() as u64, hasher.finish()));
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.sort();
    out
}

/// Fails naming the files that differ.
fn assert_unchanged(before: &[(String, u64, u64)], after: &[(String, u64, u64)], what: &str) {
    let names = |v: &[(String, u64, u64)]| v.iter().map(|e| e.0.clone()).collect::<Vec<_>>();
    assert_eq!(
        names(before),
        names(after),
        "{what}: the set of files changed"
    );
    let changed: Vec<_> = before
        .iter()
        .zip(after)
        .filter(|(b, a)| b != a)
        .map(|(b, a)| format!("{} ({} -> {} bytes)", b.0, b.1, a.1))
        .collect();
    assert!(changed.is_empty(), "{what}: changed files: {changed:?}");
}

fn never_locked(what: &str, stdout: &str, stderr: &str) {
    for text in [stdout, stderr] {
        assert!(
            !text.to_lowercase().contains("locked"),
            "{what} must not say the store is locked:\n{stdout}\n{stderr}"
        );
    }
}

/// REQ-PERS-013: while another process holds the store and runs a task, the
/// read commands answer from what is committed, `status` names the holder, and
/// none says "locked". The commands that write keep the exclusive lock.
#[test]
fn req_pers_013_read_commands_answer_while_another_process_holds_the_store() {
    let env = Env::new();
    let id = env.new_task();
    let task = env.task(&id);
    let writer = mimlys_persist::open(&env.data).unwrap();
    let running = apply(&writer, &task, resume());

    let (code, stdout, stderr) = env.run(&["status"]);
    assert_eq!(code, 0, "{stdout}\n{stderr}");
    never_locked("status", &stdout, &stderr);
    let pid = std::process::id();
    let line = stdout
        .lines()
        .find(|l| l.starts_with("store:"))
        .unwrap_or_else(|| panic!("no store line:\n{stdout}"));
    assert!(
        line.starts_with(&format!("store: in use by runtime {pid} since ")),
        "{line}"
    );
    assert!(stdout.contains("running tasks   1"), "{stdout}");
    assert!(!stdout.contains("recovery pending"), "{stdout}");
    let (code, status) = env.json(&["status"]);
    assert_eq!(code, 0, "{status}");
    assert_eq!(status["store"]["state"], "in_use", "{status}");
    assert_eq!(status["store"]["holder"]["pid"], pid, "{status}");
    assert_eq!(status["running"][0], id.as_str(), "{status}");

    let (code, list) = env.json(&["task", "list"]);
    assert_eq!(code, 0, "{list}");
    assert_eq!(list[0]["state"], "running", "{list}");
    let (code, shown) = env.json(&["task", "show", &id]);
    assert_eq!(code, 0, "{shown}");
    assert_eq!(shown["task"]["state"], "running", "{shown}");
    let (code, events) = env.json(&["events", &id]);
    assert_eq!(code, 0, "{events}");
    assert!(
        events
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["record"]["to"] == "running"),
        "the running task's events are read: {events}"
    );
    let (code, metrics) = env.json(&["metrics"]);
    assert_eq!(code, 0, "{metrics}");
    assert_eq!(metrics["tasks_by_state"]["running"], 1, "{metrics}");
    let (code, stdout, stderr) = env.run(&["toolchain", "list"]);
    assert_eq!(code, 0, "{stdout}\n{stderr}");
    never_locked("toolchain list", &stdout, &stderr);
    let (code, stdout, stderr) = env.run(&["check"]);
    assert_eq!(code, 0, "{stdout}\n{stderr}");

    // The ones that write still take the lock, and still say why they cannot.
    for args in [
        vec!["task", "cancel", id.as_str()],
        vec!["rollback", id.as_str()],
        vec!["interpreters", "list"],
    ] {
        let (code, _, stderr) = env.run(&args);
        assert_eq!(code, 1, "{args:?}: {stderr}");
        assert!(
            stderr.to_lowercase().contains("locked"),
            "{args:?} keeps the exclusive open: {stderr}"
        );
    }
    drop((writer, running));
}

/// REQ-PERS-013: the reader runs no recovery. A store whose runtime ended with
/// a task `running` is read as committed; `status` says `recovery pending`;
/// no read command changes a byte of the data directory. The next command that
/// writes recovers it.
#[test]
fn req_pers_013_read_commands_run_no_recovery_and_change_nothing() {
    let env = Env::new();
    let id = env.new_task();
    let task = env.task(&id);
    {
        let writer = mimlys_persist::open(&env.data).unwrap();
        apply(&writer, &task, resume());
    }
    let before = snapshot(&env.data);

    let (code, stdout, stderr) = env.run(&["status"]);
    assert_eq!(code, 0, "{stdout}\n{stderr}");
    never_locked("status", &stdout, &stderr);
    assert!(
        stdout.contains("recovery pending: the next command that writes recovers it"),
        "{stdout}"
    );
    assert!(stdout.contains("running tasks   1"), "{stdout}");
    let (_, status) = env.json(&["status"]);
    assert_eq!(status["store"]["state"], "recovery_pending", "{status}");
    assert!(status["store"].get("holder").is_none(), "{status}");
    let scope_file = env.scope().join("src/lib.rs");
    assert!(scope_file.is_file());
    for args in [
        vec!["task", "list"],
        vec!["task", "show", id.as_str()],
        vec!["events"],
        vec!["events", id.as_str()],
        vec!["metrics"],
        vec!["toolchain", "list"],
        vec!["promote", id.as_str(), "--patch"],
    ] {
        let (code, stdout, stderr) = env.run(&args);
        // A task `running` cannot be promoted (it may be changing); every other command answers.
        if args[0] == "promote" {
            assert_eq!(code, 1, "{args:?}: {stdout}\n{stderr}");
            assert!(stderr.contains("promote_workspace"), "{stderr}");
        } else {
            assert_eq!(code, 0, "{args:?}: {stdout}\n{stderr}");
        }
        never_locked(&format!("{args:?}"), &stdout, &stderr);
        if args == ["task", "list"] {
            assert!(stdout.contains(" running "), "read as committed: {stdout}");
        }
    }
    assert_unchanged(
        &before,
        &snapshot(&env.data),
        "a read command writes nothing, not a byte",
    );
    assert_eq!(env.task(&id).state.name(), "running", "still not recovered");

    // The next command that writes recovers it.
    let (code, stdout, stderr) = env.run(&["interpreters", "list"]);
    assert_eq!(code, 0, "{stdout}\n{stderr}");
    assert_ne!(env.task(&id).state.name(), "running", "recovered");
    let (_, status) = env.json(&["status"]);
    assert_eq!(status["store"]["state"], "free", "{status}");
    let (_, metrics) = env.json(&["metrics"]);
    assert_eq!(metrics["events_by_name"]["task.recovered"], 1, "{metrics}");
}

/// REQ-PERS-013: a store from a newer schema is refused with the reason and
/// is not migrated or touched; an older one says `migration pending`.
#[test]
fn req_pers_013_a_reader_refuses_a_newer_schema_and_does_not_migrate() {
    let env = Env::new();
    drop(mimlys_persist::open(&env.data).unwrap());
    let set_version = |v: i64| {
        let conn = rusqlite::Connection::open(env.data.join("mimlys.db")).unwrap();
        conn.execute("UPDATE schema_version SET version = ?1", [v])
            .unwrap();
    };
    set_version(99);
    let before = snapshot(&env.data);
    for command in [
        vec!["status"],
        vec!["task", "list"],
        vec!["events"],
        vec!["metrics"],
    ] {
        let (code, stdout, stderr) = env.run(&command);
        assert_eq!(code, 1, "{command:?}: {stdout}");
        assert!(
            stderr.contains("schema version 99 is newer than the supported"),
            "{command:?}: {stderr}"
        );
    }
    assert_unchanged(&before, &snapshot(&env.data), "nothing migrated or written");
    let (code, value) = env.json(&["status"]);
    assert_eq!(code, 1);
    assert_eq!(value["error"]["kind"], "internal", "{value}");
}

/// REQ-PERS-013: `promote --patch` plans and prints and writes nothing, so it
/// reads while another process holds the store; `promote` itself, which
/// writes, does not. `rollback` has no dry run and always restores: it keeps
/// the exclusive open (see the first test).
#[test]
fn req_pers_013_promote_patch_reads_and_promote_writes() {
    let env = Env::new();
    let id = env.new_task();
    let tree_file = env
        .data
        .join("workspaces")
        .join(&id)
        .join("tree/src/lib.rs");
    std::fs::write(&tree_file, "fn a() -> i32 { 1 }\n").unwrap();
    let writer = mimlys_persist::open(&env.data).unwrap();
    let before = snapshot(&env.data);

    let (code, stdout, stderr) = env.run(&["promote", &id, "--patch"]);
    assert_eq!(code, 0, "{stdout}\n{stderr}");
    never_locked("promote --patch", &stdout, &stderr);
    assert!(
        stdout.contains("-fn a() {}") && stdout.contains("+fn a() -> i32 { 1 }"),
        "{stdout}"
    );
    assert_unchanged(&before, &snapshot(&env.data), "the plan wrote nothing");
    assert_eq!(
        std::fs::read_to_string(env.scope().join("src/lib.rs")).unwrap(),
        "fn a() {}\n",
        "and the project is untouched"
    );

    let (code, _, stderr) = env.run(&["promote", &id, "--yes"]);
    assert_eq!(code, 1, "{stderr}");
    assert!(stderr.to_lowercase().contains("locked"), "{stderr}");
    drop(writer);
}

/// REQ-PERS-013: `events --follow` of a task another process runs polls the
/// store through the reader: it prints what the runtime commits, as it is
/// committed, and ends when the task does.
#[test]
fn req_pers_013_follow_reads_the_events_of_a_task_another_process_runs() {
    let env = Env::new();
    let id = env.new_task();
    let task = env.task(&id);
    let writer = mimlys_persist::open(&env.data).unwrap();
    let running = apply(&writer, &task, resume());

    let out_path = env.dir.path().join("follow.stdout");
    let err_path = env.dir.path().join("follow.stderr");
    let mut follow = env.command(&["events", &id, "--follow"]);
    follow
        .stdin(std::process::Stdio::null())
        .stdout(std::fs::File::create(&out_path).unwrap())
        .stderr(std::fs::File::create(&err_path).unwrap());
    let mut child = follow.spawn().unwrap();

    // What the follower prints is state, read from its output file; wait for it
    // (bounded, and no decision depends on how long it takes).
    let mut wait_for = |needle: &str| {
        let started = std::time::Instant::now();
        loop {
            let text = std::fs::read_to_string(&out_path).unwrap_or_default();
            if text.contains(needle) {
                return text;
            }
            assert!(
                started.elapsed() < common::BOUND,
                "the follower never printed {needle:?}: {text}\n{}",
                std::fs::read_to_string(&err_path).unwrap_or_default()
            );
            if let Some(status) = child.try_wait().unwrap() {
                panic!("the follower ended early ({status}): {text}");
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    };
    wait_for("to=running");
    // A state change the runtime commits is printed by a reader that holds nothing.
    let cancelled = apply(&writer, &running, TaskInput::Cancel);
    wait_for("to=cancelled");
    let started = std::time::Instant::now();
    let status = loop {
        if let Some(s) = child.try_wait().unwrap() {
            break s;
        }
        assert!(
            started.elapsed() < common::BOUND,
            "the follower did not end"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    assert!(status.success(), "{status}");
    assert!(cancelled.state.is_terminal());
    drop(writer);
}

/// REQ-PERS-013: following a task whose runtime is gone ends with the reason
/// instead of waiting for ever: nothing moves a task `running` that nobody runs
/// until a command that writes recovers the store.
#[test]
fn req_pers_013_follow_of_a_task_nobody_runs_says_recovery_pending() {
    let env = Env::new();
    let id = env.new_task();
    let task = env.task(&id);
    {
        let writer = mimlys_persist::open(&env.data).unwrap();
        apply(&writer, &task, resume());
    }
    let (code, stdout, stderr) = env.run(&["events", &id, "--follow"]);
    assert_eq!(code, 0, "{stdout}\n{stderr}");
    assert!(
        stdout.contains("recovery pending: the next command that writes recovers it"),
        "{stdout}"
    );
}

/// REQ-PERS-013: a data directory with no store yet is read as empty and
/// nothing is created; `status` says the store is not created yet.
#[test]
fn req_pers_013_reading_an_absent_store_creates_nothing() {
    let env = Env::new();
    let (code, stdout, stderr) = env.run(&["status"]);
    assert_eq!(code, 0, "{stdout}\n{stderr}");
    assert!(
        stdout.contains("store: not created yet (the first command that writes creates it)"),
        "{stdout}"
    );
    let (_, list) = env.json(&["task", "list"]);
    assert_eq!(list.as_array().unwrap().len(), 0);
    let (code, stdout, _) = env.run(&["events"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("no events"), "{stdout}");
    let (_, metrics) = env.json(&["metrics"]);
    assert_eq!(metrics["tasks_by_state"], serde_json::json!({}));
    let (code, _, _) = env.run(&["task", "show", "00000000-0000-0000-0000-000000000000"]);
    assert_eq!(code, 1, "no such task");
    assert!(!env.data.exists(), "a reader creates no data directory");
}

/// REQ-PERS-013: the lock is still exclusive between writers, and a reader
/// beside them does not change that.
#[test]
fn req_pers_013_two_writers_still_exclude_each_other() {
    let env = Env::new();
    let writer = mimlys_persist::open(&env.data).unwrap();
    let (code, _, stderr) = env.run(&["task", "new", "x", "--hold"]);
    assert_eq!(code, 1, "{stderr}");
    assert!(stderr.to_lowercase().contains("locked"), "{stderr}");
    let (code, _, stderr) = env.run(&["status"]);
    assert_eq!(code, 0, "{stderr}");
    drop(writer);
}

/// An environment with one configured MCP server (the test server) that
/// declares one tool.
fn env_with_a_server() -> Env {
    let env = Env::new();
    let mut text = std::fs::read_to_string(&env.config).unwrap();
    text.push_str(&format!(
        "[servers.probe]\ncommand = {:?}\n[servers.probe.tools.web_search]\neffect = \"external_read\"\nidempotent = true\n",
        env!("CARGO_BIN_EXE_mimlys-mcp-testserver")
    ));
    std::fs::write(&env.config, text).unwrap();
    env
}

/// REQ-PERS-013: `status` does not start the MCP servers when the data
/// directory does not exist yet. A server's working directory is the data
/// directory, a reader creates none, and starting one anywhere else (the
/// command's current directory) would be a side effect of a read command.
/// It says so and lists the declared tools as declared.
#[test]
fn req_pers_013_status_starts_no_server_without_a_data_directory() {
    let env = env_with_a_server();
    let (code, stdout, stderr) = env.run(&["status"]);
    assert_eq!(code, 0, "{stdout}\n{stderr}");
    assert!(
        stdout.contains("servers: not started (no data directory yet)"),
        "{stdout}"
    );
    assert!(
        stdout.contains("probe (not started)")
            && stdout.contains("declared tools: probe.web_search"),
        "{stdout}"
    );
    assert!(!stdout.contains("(running)"), "{stdout}");
    let (code, status) = env.json(&["status"]);
    assert_eq!(code, 0, "{status}");
    let server = &status["servers"][0];
    assert_eq!(server["name"], "probe");
    assert_eq!(server["state"], "not_started", "{server}");
    assert_eq!(server["detail"], "no data directory yet");
    assert_eq!(server["declared"], serde_json::json!(["probe.web_search"]));
    assert_eq!(server["tools"], serde_json::json!([]));
    assert!(!env.data.exists(), "a reader creates no data directory");
}

/// The other side: once the data directory exists, `status` starts the
/// servers to list their tools, as before.
#[test]
fn req_pers_013_status_starts_the_servers_once_the_data_directory_exists() {
    let env = env_with_a_server();
    std::fs::create_dir_all(&env.data).unwrap();
    let (code, status) = env.json(&["status"]);
    assert_eq!(code, 0, "{status}");
    let server = &status["servers"][0];
    assert_eq!(server["state"], "running", "{server}");
    assert_eq!(server["declared"], serde_json::Value::Null);
    let tools = server["tools"].as_array().unwrap();
    assert!(tools.iter().any(|t| t == "probe.web_search"), "{server}");
    let (_, stdout, _) = env.run(&["status"]);
    assert!(!stdout.contains("servers: not started"), "{stdout}");
}
