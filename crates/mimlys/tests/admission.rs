//! Several runtimes of one user (handoff 013d; REQ-SCHED-006, REQ-OBS-009, REQ-CLI-005; RFC 013
//! decisions 5 and 8; external design §19.3), through the binary and, for the stop request, through
//! the runtime over the real registry.
//!
//! Two runtimes are two data directories and one registry (`MIMLYS_RUN_DIR`), with
//! `admission_slots = 1` so that the second waits. The first holds its slot with the scripted
//! MCP test server's `slow` tool (an idempotent external read, held far longer than anything
//! waited for); the test ends it by a stop signal (Unix) or by killing the process. Every verdict
//! is read from a state: the store's events, the registry's entries, `task show`, `status`,
//! the exit. No verdict is taken from a short time, and the holds are long.

use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

mod common;
use common::run_bounded;

const BUILTIN: &str = r#""read_file", "write_file", "replace_in_file", "list_dir", "search_files", "delete_path", "run_script", "run_wasm""#;

/// Far longer than anything a test waits for; the holder is ended by the test.
const HOLD_MS: u64 = 60_000;

/// One runtime of the user: its own data directory and configuration, the user's registry.
struct Runtime {
    dir: tempfile::TempDir,
    config: PathBuf,
    registry: PathBuf,
}

impl Runtime {
    /// A runtime whose model proposes `rypsok.slow` for `ms` milliseconds, then completes.
    fn new(registry: &Path, ms: u64) -> Self {
        Self::with(registry, ms, 1)
    }

    fn with(registry: &Path, ms: u64, slots: usize) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let script = [
            json!({"decision": "act", "tool": "rypsok.slow", "arguments": {"ms": ms}}),
            json!({"decision": "complete", "summary": "done"}),
        ];
        let script_path = dir.path().join("script.json");
        std::fs::write(&script_path, serde_json::to_string(&script).unwrap()).unwrap();
        let config = dir.path().join("config.toml");
        let text = format!(
            r#"config_version = 1
[runtime]
data_dir = {data:?}
admission_slots = {slots}
[backend]
kind = "fake"
script = {script:?}
[policy.profiles.workspace]
tools = [{BUILTIN}, "rypsok.slow"]
[servers.rypsok]
command = {server:?}
env_allow = ["PATH"]
[servers.rypsok.tools.slow]
effect = "external_read"
idempotent = true
timeout_ms = 300000
"#,
            data = dir.path().join("data").to_string_lossy(),
            script = script_path.to_string_lossy(),
            server = env!("CARGO_BIN_EXE_mimlys-mcp-testserver"),
        );
        std::fs::write(&config, text).unwrap();
        Self {
            dir,
            config,
            registry: registry.to_owned(),
        }
    }

    fn data(&self) -> PathBuf {
        self.dir.path().join("data")
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_mimlys"));
        cmd.arg("--config")
            .arg(&self.config)
            .args(args)
            .env("MIMLYS_CONFIG", "")
            .env("MIMLYS_RUN_DIR", &self.registry)
            .env("MIMLYS_WORKER_BIN", env!("CARGO_BIN_EXE_mimlys-worker"))
            .stdin(Stdio::null());
        cmd
    }

    fn scope(&self) -> PathBuf {
        let dir = self.dir.path().join("scope");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "a\n").unwrap();
        dir
    }

    /// A command to its end, as JSON.
    fn json(&self, args: &[&str]) -> (i32, Value) {
        let mut all = vec!["--json"];
        all.extend_from_slice(args);
        let out = run_bounded(&mut self.command(&all), self.dir.path());
        let value = serde_json::from_str(out.stdout.trim())
            .unwrap_or_else(|e| panic!("not JSON ({e}): {}\n{}", out.stdout, out.stderr));
        (out.code(), value)
    }

    /// A command to its end, as text: (code, standard output, standard error).
    fn text(&self, args: &[&str]) -> (i32, String, String) {
        let out = run_bounded(&mut self.command(args), self.dir.path());
        (out.code(), out.stdout.clone(), out.stderr.clone())
    }

    /// `run` started and left running, its streams in files.
    fn start(&self) -> Started {
        let out = self.dir.path().join("run.stdout");
        let err = self.dir.path().join("run.stderr");
        let scope = self.scope();
        let child = self
            .command(&["--json", "run", "go", "--scope", scope.to_str().unwrap()])
            .stdout(std::fs::File::create(&out).unwrap())
            .stderr(std::fs::File::create(&err).unwrap())
            .spawn()
            .unwrap();
        Started {
            child,
            out,
            err,
            data: self.data(),
        }
    }

    /// The events of the task, as a second process reads them.
    fn events(&self, id: &str) -> Vec<Value> {
        let (_, events) = self.json(&["events", id]);
        events
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["record"].clone())
            .collect()
    }

    /// The id of the runtime's only task.
    fn task_id(&self) -> String {
        let (_, list) = self.json(&["task", "list"]);
        let tasks = list.as_array().unwrap();
        assert_eq!(tasks.len(), 1, "{list}");
        tasks[0]["task_id"].as_str().unwrap().to_owned()
    }

    /// The `task.admission_changed` events of the task: `(state, slots_taken, waited_ms)`.
    fn admission_events(&self, id: &str) -> Vec<(String, Option<u64>, Option<u64>)> {
        self.events(id)
            .iter()
            .filter(|e| e["event"] == "task.admission_changed")
            .map(|e| {
                (
                    e["state"].as_str().unwrap().to_owned(),
                    e["slots_taken"].as_u64(),
                    e["waited_ms"].as_u64(),
                )
            })
            .collect()
    }

    #[cfg(unix)]
    fn observations(&self, id: &str) -> Vec<Value> {
        let conn = rusqlite::Connection::open(self.data().join("mimlys.db")).unwrap();
        let mut stmt = conn
            .prepare("select o.record from observations o join actions a on a.action_id = o.action_id where a.task_id = ?1 order by o.rowid")
            .unwrap();
        stmt.query_map([id], |r| r.get::<_, String>(0))
            .unwrap()
            .map(|r| serde_json::from_str(&r.unwrap()).unwrap())
            .collect()
    }
}

/// A `run` in progress.
struct Started {
    child: Child,
    out: PathBuf,
    err: PathBuf,
    data: PathBuf,
}

impl Started {
    fn pid(&self) -> u32 {
        self.child.id()
    }

    /// How many events named `name` the store holds, as a second process reads it.
    fn count(&self, name: &str) -> usize {
        count_events(&self.data, name)
    }

    /// Waits until `reached` holds; fails with the evidence when the bound passes.
    fn until(&mut self, what: &str, mut reached: impl FnMut(&Self) -> bool) {
        let started = std::time::Instant::now();
        while started.elapsed() < Duration::from_secs(120) {
            if reached(self) {
                return;
            }
            if let Some(status) = self.child.try_wait().unwrap() {
                self.give_up(&format!("{what}: the run ended first ({status})"));
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        self.give_up(what);
    }

    #[cfg(unix)]
    fn interrupt(&self) {
        let _ = Command::new("kill")
            .args(["-INT", &self.child.id().to_string()])
            .status();
    }

    /// Ends the process as a crash does: nothing is returned or closed by it.
    fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    fn exit(&mut self) -> std::process::ExitStatus {
        let started = std::time::Instant::now();
        while started.elapsed() < Duration::from_secs(120) {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        self.give_up("the run did not exit")
    }

    fn stdout(&self) -> Value {
        let text = std::fs::read_to_string(&self.out).unwrap_or_default();
        serde_json::from_str(text.trim())
            .unwrap_or_else(|e| panic!("not JSON ({e}): {text}\n{}", self.stderr()))
    }

    fn stderr(&self) -> String {
        std::fs::read_to_string(&self.err).unwrap_or_default()
    }

    fn give_up(&mut self, what: &str) -> ! {
        let cut = |text: String| text.chars().take(1500).collect::<String>();
        let report = format!(
            "{what}\n--- stdout ---\n{}\n--- stderr ---\n{}",
            cut(std::fs::read_to_string(&self.out).unwrap_or_default()),
            cut(self.stderr()),
        );
        self.kill();
        panic!("{report}");
    }
}

impl Drop for Started {
    fn drop(&mut self) {
        // A test that failed leaves nothing running.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn count_events(data: &Path, name: &str) -> usize {
    rusqlite::Connection::open_with_flags(
        data.join("mimlys.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .and_then(|conn| {
        conn.query_row("select count(*) from events where name = ?1", [name], |r| {
            r.get::<_, i64>(0)
        })
    })
    .map_or(0, |n| usize::try_from(n).unwrap_or(0))
}

/// A registry directory of its own, outside both runtimes' data directories.
fn registry() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

/// The first runtime holds the only slot: its execution has begun.
fn holder(registry: &Path) -> (Runtime, Started) {
    let first = Runtime::new(registry, HOLD_MS);
    let mut run = first.start();
    run.until("the first runtime's action never started", |r| {
        r.count("action.started") >= 1
    });
    (first, run)
}

/// REQ-SCHED-006, REQ-OBS-009, RFC 013 decisions 5 and 8: two runtimes of one user with one slot.
/// The second runtime's execution waits, and `task show` and `status` say why with the holder's
/// pid; its task stays `running`; the first runtime's `status` lists both; when the first's
/// execution ends, the second is admitted and its execution runs; the events record both changes.
/// Unix: the first ends by a stop signal (a clean end of its execution).
#[cfg(unix)]
#[test]
fn req_sched_006_a_second_runtime_waits_for_the_slot_and_is_admitted_when_the_first_ends() {
    let registry = registry();
    let (first, mut holding) = holder(registry.path());
    let second = Runtime::new(registry.path(), 50);
    let mut waiting = second.start();
    waiting.until("the second runtime never waited", |r| {
        r.count("task.admission_changed") >= 1
    });
    let holder_pid = holding.pid();
    let id = second.task_id();
    // It is waiting: nothing of its action began, and the first has no admission event.
    assert_eq!(waiting.count("action.started"), 0);
    assert_eq!(holding.count("task.admission_changed"), 0);
    // `task show` says why, with the holder's pid, and the task is still running.
    let (_, shown, _) = second.text(&["task", "show", &id]);
    assert!(shown.contains("state     running"), "{shown}");
    assert!(
        shown.contains(&format!(
            "waiting: admission (LIM-X5 = 1 slot taken by runtime {holder_pid})"
        )),
        "{shown}"
    );
    let (_, view) = second.json(&["task", "show", &id]);
    assert_eq!(view["task"]["state"], "running");
    assert_eq!(view["waiting"]["reason"], "admission");
    assert_eq!(view["waiting"]["holders"], json!([holder_pid]));
    assert_eq!(view["waiting"]["limit"], 1);
    // `status` of either runtime lists both.
    let (_, status, _) = second.text(&["status"]);
    for expected in [
        "admission: ready (1 of 1 slots taken)".to_owned(),
        format!("runtime {holder_pid}: running, holds a slot;"),
        format!(
            "runtime {}: waiting: admission (LIM-X5 = 1 slot taken by runtime {holder_pid});",
            waiting.pid()
        ),
    ] {
        assert!(status.contains(&expected), "{expected}\n{status}");
    }
    let (_, machine) = first.json(&["status"]);
    let machine = &machine["machine"];
    assert_eq!(machine["slots"], json!({"limit": 1, "taken": 1}));
    let runtimes = machine["runtimes"].as_array().unwrap();
    let row = |pid: u32| {
        runtimes
            .iter()
            .find(|r| r["pid"] == pid)
            .unwrap_or_else(|| panic!("runtime {pid} not listed: {machine}"))
    };
    assert_eq!(row(holder_pid)["state"], "running");
    assert_eq!(row(holder_pid)["holds"], "slot");
    assert_eq!(
        row(holder_pid)["data_dir"],
        first.data().to_string_lossy().as_ref()
    );
    assert_eq!(row(waiting.pid())["state"], "waiting");
    assert_eq!(row(waiting.pid())["waiting_for"], "admission");
    assert_eq!(row(waiting.pid())["behind"], json!([holder_pid]));
    assert_eq!(row(waiting.pid())["task_id"], id.as_str());
    // The first's execution ends (the stop signal interrupts the call at the grace), and the
    // second is admitted and runs.
    holding.interrupt();
    let status = waiting.exit();
    assert!(status.success(), "{status:?} {}", waiting.stderr());
    assert_eq!(waiting.stdout()["task"]["state"], "completed");
    let changes = second.admission_events(&id);
    assert_eq!(changes.len(), 2, "{changes:?}");
    assert_eq!(changes[0], ("waiting".to_owned(), Some(1), None));
    assert_eq!(changes[1].0, "admitted");
    assert_eq!(changes[1].1, Some(1));
    assert!(changes[1].2.is_some(), "how long it waited: {changes:?}");
    // In order: waiting, admitted, then the action begins.
    let names: Vec<String> = second
        .events(&id)
        .iter()
        .map(|e| e["event"].as_str().unwrap().to_owned())
        .collect();
    let at = |name: &str, nth: usize| {
        names
            .iter()
            .enumerate()
            .filter(|(_, n)| *n == name)
            .nth(nth)
            .unwrap_or_else(|| panic!("{name} #{nth} in {names:?}"))
            .0
    };
    assert!(at("task.admission_changed", 0) < at("task.admission_changed", 1));
    assert!(at("task.admission_changed", 1) < at("action.started", 0));
    let _ = holding.exit();
}

/// REQ-SCHED-006, REQ-CLI-005, RFC 013 Amendment 1: a first Ctrl-C during the admission wait leaves
/// the action not begun (`stopped_before_start`, nothing recorded `running`, no `action.started`),
/// the task paused, and the process exits 0 within the bound; its waiting entry is gone and the
/// holder's slot untouched. (Unix signals; the same through the runtime on every platform in
/// `req_sched_006_a_stop_request_ends_a_wait_over_the_real_registry`.)
#[cfg(unix)]
#[test]
fn req_sched_006_a_first_ctrl_c_during_the_wait_leaves_the_action_not_begun() {
    let registry = registry();
    let (first, mut holding) = holder(registry.path());
    let second = Runtime::new(registry.path(), 50);
    let mut waiting = second.start();
    waiting.until("the second runtime never waited", |r| {
        r.count("task.admission_changed") >= 1
    });
    waiting.interrupt();
    let status = waiting.exit();
    assert!(status.success(), "{status:?} {}", waiting.stderr());
    let id = second.task_id();
    let (_, view) = second.json(&["task", "show", &id]);
    assert_eq!(view["task"]["state"], "paused", "{view}");
    assert!(view.get("waiting").is_none(), "nothing is waited for now");
    assert_eq!(waiting.count("action.started"), 0, "the action never began");
    assert_eq!(
        second.admission_events(&id).len(),
        1,
        "waiting, no admission"
    );
    let observations = second.observations(&id);
    assert_eq!(observations.len(), 1, "{observations:?}");
    assert_eq!(observations[0]["status"], "cancelled");
    assert_eq!(
        observations[0]["notices"],
        json!([{"code": "stopped_before_start"}])
    );
    // The holder is untouched, and the waiter's entry is gone.
    let (_, machine) = first.json(&["status"]);
    let runtimes = machine["machine"]["runtimes"].as_array().unwrap();
    assert_eq!(runtimes.len(), 1, "{machine}");
    assert_eq!(runtimes[0]["pid"], holding.pid());
    holding.kill();
}

/// THR-38, REQ-SCHED-006: a holder that is killed (a crash: it returns nothing) leaves a stale
/// entry; `status` counts it, `runtime reap` removes it and lists it with the holder's pid, and a
/// second `runtime reap` finds nothing. Every platform.
#[test]
fn thr_38_runtime_reap_lists_the_entry_a_killed_holder_left() {
    let registry = registry();
    let (first, mut holding) = holder(registry.path());
    let holder_pid = holding.pid();
    // Alive, it is not stale: nothing to reap.
    let (code, none) = first.json(&["runtime", "reap"]);
    assert_eq!(code, 0);
    assert_eq!(none["reaped"], json!([]), "{none}");
    holding.kill();
    let (_, machine) = first.json(&["status"]);
    assert_eq!(machine["machine"]["stale_entries"], 1, "{machine}");
    assert_eq!(machine["machine"]["runtimes"], json!([]));
    let (_, text, _) = first.text(&["runtime", "reap"]);
    assert!(text.contains("reaped slot of runtime"), "{text}");
    assert!(text.contains(&format!("runtime {holder_pid}")), "{text}");
    assert!(text.contains("its holder is gone"), "{text}");
    let (_, again) = first.json(&["runtime", "reap"]);
    assert_eq!(again["reaped"], json!([]), "{again}");
    let (_, text, _) = first.text(&["runtime", "reap"]);
    assert!(text.contains("nothing to reap"), "{text}");
    let (_, machine) = first.json(&["status"]);
    assert_eq!(machine["machine"]["stale_entries"], 0, "{machine}");
}

/// THR-38, REQ-SCHED-006: a stale entry is reaped at the next open: after a holder is killed, the
/// next runtime takes the slot at once, with no wait and no admission event, and the stale entry
/// is gone afterwards. Every platform.
#[test]
fn thr_38_a_stale_entry_is_reaped_at_the_next_open_and_the_next_runtime_is_not_kept_waiting() {
    let registry = registry();
    let (_first, mut holding) = holder(registry.path());
    holding.kill();
    let second = Runtime::new(registry.path(), 50);
    let mut next = second.start();
    let status = next.exit();
    assert!(status.success(), "{status:?} {}", next.stderr());
    assert_eq!(next.stdout()["task"]["state"], "completed");
    let id = second.task_id();
    assert!(
        second.admission_events(&id).is_empty(),
        "it did not wait: {:?}",
        second.admission_events(&id)
    );
    let (_, again) = second.json(&["runtime", "reap"]);
    assert_eq!(again["reaped"], json!([]), "reaped by the open: {again}");
}

/// REQ-SCHED-006, THR-37, external design §19.3: a registry that cannot be used (here: a file where
/// its directory should be) lets the runtime go on as if alone, and says so, visibly: a `warn`
/// line, `task.admission_changed{unadmitted, reason}` in the task's events, and `admission:
/// unavailable (<reason>)` in `status` (text and JSON). The task completes. Every platform.
#[test]
fn req_sched_006_a_registry_that_cannot_be_used_runs_unadmitted_and_says_so() {
    let somewhere = registry();
    let file = somewhere.path().join("not-a-directory");
    std::fs::write(&file, b"").unwrap();
    let runtime = Runtime::new(&file, 50);
    let mut run = runtime.start();
    let status = run.exit();
    assert!(status.success(), "{status:?} {}", run.stderr());
    assert_eq!(run.stdout()["task"]["state"], "completed");
    let id = runtime.task_id();
    let events = runtime.events(&id);
    let unadmitted: Vec<&Value> = events
        .iter()
        .filter(|e| e["event"] == "task.admission_changed")
        .collect();
    assert_eq!(unadmitted.len(), 1, "{events:?}");
    assert_eq!(unadmitted[0]["state"], "unadmitted");
    assert!(
        unadmitted[0]["reason"]
            .as_str()
            .unwrap()
            .contains("is not a directory of its own"),
        "{}",
        unadmitted[0]
    );
    let stderr = run.stderr();
    assert!(
        stderr.contains("\"level\":\"WARN\"") && stderr.contains("unadmitted"),
        "said at warn: {stderr}"
    );
    let (_, text, _) = runtime.text(&["status"]);
    assert!(
        text.contains("admission: unavailable ("),
        "status says so: {text}"
    );
    let (_, status) = runtime.json(&["status"]);
    assert_eq!(status["machine"]["admission"]["state"], "unavailable");
    assert!(
        status["machine"]["admission"]["reason"]
            .as_str()
            .unwrap()
            .contains("is not a directory of its own"),
        "{status}"
    );
}

/// REQ-OBS-009: `status` of a registry with nothing in it says so, in both forms, and
/// `runtime reap` of a registry that is not there yet finds nothing and creates nothing.
#[test]
fn req_obs_009_an_idle_registry_lists_nothing() {
    let registry = registry();
    let absent = registry.path().join("absent");
    let runtime = Runtime::new(&absent, 50);
    let (_, text, _) = runtime.text(&["runtime", "reap"]);
    assert!(text.contains("nothing to reap"), "{text}");
    assert!(!absent.exists(), "reaping creates nothing");
    let (_, machine) = runtime.json(&["status"]);
    let machine = &machine["machine"];
    assert_eq!(machine["admission"]["state"], "ready", "{machine}");
    assert_eq!(machine["slots"], json!({"limit": 1, "taken": 0}));
    assert_eq!(machine["runtimes"], json!([]));
    assert_eq!(machine["engine_gates"], json!([]));
    assert_eq!(machine["stale_entries"], 0);
    let (_, text, _) = runtime.text(&["status"]);
    assert!(
        text.contains("runtimes: none executing or waiting"),
        "{text}"
    );
}
