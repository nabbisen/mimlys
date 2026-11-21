//! The engine's gate among the user's runtimes (handoff 013d; REQ-SCHED-007, REQ-OBS-009; RFC 013
//! decision 7 with Amendment 1; external design §19.3), through the binary against the scripted
//! OpenAI-compatible mock server.
//!
//! Two runtimes are two data directories, one registry (`MIMLYS_RUN_DIR`) and one endpoint (one
//! mock server). The mock holds the first decision request for far longer than anything a test
//! waits for (`Reply::Slow`), so the first runtime's request is in flight until the test ends it
//! (a stop signal on Unix, whose first Ctrl-C interrupts the request at the grace; killing the
//! process elsewhere). Every verdict is a state: what the mock received, the registry's entries,
//! `task show`, `status`, `metrics`, the store's events, the exit.
#![cfg(feature = "test-bins")]

use mimlys_model::mock::{MockConfig, MockServer, Reply};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

mod common;
use common::run_bounded;

/// Far longer than anything a test waits for.
const HOLD_MS: u64 = 600_000;

fn complete() -> Value {
    json!({"decision": "complete", "summary": "done"})
}

/// A mock server whose first decision request is held, and whose later ones are answered at once.
fn server() -> MockServer {
    MockServer::start(
        MockConfig::default(),
        vec![
            Reply::Slow(HOLD_MS, Box::new(Reply::Decision(complete()))),
            Reply::Decision(complete()),
            Reply::Decision(complete()),
        ],
    )
}

struct Runtime {
    dir: tempfile::TempDir,
    config: PathBuf,
    registry: PathBuf,
}

impl Runtime {
    fn new(registry: &Path, server: &MockServer, width: usize) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config.toml");
        std::fs::write(
            &config,
            format!(
                r#"config_version = 1
[runtime]
data_dir = {data:?}
[backend]
kind = "openai_compatible"
url = {url:?}
model = "test-model"
structured_output = "none"
decision_transport = "envelope"
max_in_flight = {width}
"#,
                data = dir.path().join("data").to_string_lossy(),
                url = server.url(),
            ),
        )
        .unwrap();
        Self {
            dir,
            config,
            registry: registry.to_owned(),
        }
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

    fn json(&self, args: &[&str]) -> (i32, Value) {
        let mut all = vec!["--json"];
        all.extend_from_slice(args);
        let out = run_bounded(&mut self.command(&all), self.dir.path());
        let value = serde_json::from_str(out.stdout.trim())
            .unwrap_or_else(|e| panic!("not JSON ({e}): {}\n{}", out.stdout, out.stderr));
        (out.code(), value)
    }

    #[cfg(unix)]
    fn text(&self, args: &[&str]) -> (i32, String, String) {
        let out = run_bounded(&mut self.command(args), self.dir.path());
        (out.code(), out.stdout.clone(), out.stderr.clone())
    }

    fn start(&self) -> Started {
        let out = self.dir.path().join("run.stdout");
        let err = self.dir.path().join("run.stderr");
        let scope = self.dir.path().join("scope");
        std::fs::create_dir_all(&scope).unwrap();
        std::fs::write(scope.join("a.txt"), "a\n").unwrap();
        let child = self
            .command(&["--json", "run", "go", "--scope", scope.to_str().unwrap()])
            .stdout(std::fs::File::create(&out).unwrap())
            .stderr(std::fs::File::create(&err).unwrap())
            .spawn()
            .unwrap();
        Started { child, out, err }
    }

    fn task_id(&self) -> String {
        let (_, list) = self.json(&["task", "list"]);
        let tasks = list.as_array().unwrap();
        assert_eq!(tasks.len(), 1, "{list}");
        tasks[0]["task_id"].as_str().unwrap().to_owned()
    }

    #[cfg(unix)]
    fn event_names(&self, id: &str) -> Vec<String> {
        let (_, events) = self.json(&["events", id]);
        events
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["record"]["event"].as_str().unwrap().to_owned())
            .collect()
    }
}

struct Started {
    child: Child,
    out: PathBuf,
    err: PathBuf,
}

impl Started {
    fn pid(&self) -> u32 {
        self.child.id()
    }

    /// Waits until `reached` holds; fails with the evidence when the bound passes or the run
    /// ends first.
    fn until(&mut self, what: &str, mut reached: impl FnMut() -> bool) {
        let started = std::time::Instant::now();
        while started.elapsed() < Duration::from_secs(120) {
            if reached() {
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

    fn stderr(&self) -> String {
        std::fs::read_to_string(&self.err).unwrap_or_default()
    }

    fn stdout(&self) -> Value {
        let text = std::fs::read_to_string(&self.out).unwrap_or_default();
        serde_json::from_str(text.trim())
            .unwrap_or_else(|e| panic!("not JSON ({e}): {text}\n{}", self.stderr()))
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
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The registry as a reader sees it.
fn listing(registry: &Path) -> mimlys::registry::Scan {
    mimlys::registry::Registry::new(
        registry.to_owned(),
        mimlys::registry::Bounds::default(),
        std::sync::Arc::new(mimlys_core::clock::SystemClock),
    )
    .list()
    .unwrap()
}

fn gate_entries(registry: &Path, state: mimlys::registry::State) -> usize {
    listing(registry)
        .live
        .iter()
        .filter(|s| s.entry.kind == mimlys::registry::Kind::Gate && s.entry.state == state)
        .count()
}

/// The first runtime's request is in flight: the mock has it, and the registry has its gate.
fn holder(registry: &Path, server: &MockServer, width: usize) -> (Runtime, Started) {
    let first = Runtime::new(registry, server, width);
    let mut run = first.start();
    run.until("the first request never reached the engine", || {
        server.decision_requests().len() == 1
            && gate_entries(registry, mimlys::registry::State::Held) == 1
    });
    (first, run)
}

fn registry() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

/// REQ-SCHED-007, REQ-OBS-009, RFC 013 decision 7: two runtimes against one endpoint with
/// `max_in_flight = 1`. The second runtime's request waits: the engine receives only the first
/// (the second is not sent), `task show` and `status` say why with the holder's pid, and when the
/// first request ends, the second is sent and answered; the wait is the step's `engine_wait_ms`,
/// which `metrics` counts, for the second task and not for the first. Unix: the first request
/// ends by a stop signal (interrupted at the grace).
#[cfg(unix)]
#[test]
fn req_sched_007_a_second_request_waits_for_the_gate_and_the_metric_counts_it() {
    let registry = registry();
    let server = server();
    let (first, mut holding) = holder(registry.path(), &server, 1);
    let second = Runtime::new(registry.path(), &server, 1);
    let mut waiting = second.start();
    waiting.until("the second runtime never waited", || {
        gate_entries(registry.path(), mimlys::registry::State::Waiting) == 1
    });
    let holder_pid = holding.pid();
    assert_eq!(
        server.decision_requests().len(),
        1,
        "the second request was not sent"
    );
    let id = second.task_id();
    let url = server.url().trim_end_matches("/v1").to_owned();
    // `task show` says why.
    let (_, shown, _) = second.text(&["task", "show", &id]);
    assert!(
        shown.contains(&format!(
            "waiting: engine (max_in_flight = 1; 1 request in flight to {url}, held by runtime {holder_pid})"
        )),
        "{shown}"
    );
    let (_, view) = second.json(&["task", "show", &id]);
    assert_eq!(view["task"]["state"], "running");
    assert_eq!(view["waiting"]["reason"], "engine");
    assert_eq!(view["waiting"]["holders"], json!([holder_pid]));
    assert_eq!(view["waiting"]["url"], url.as_str());
    // `status` lists the gate, the holder and the waiter.
    let (_, status) = first.json(&["status"]);
    let machine = &status["machine"];
    assert_eq!(
        machine["engine_gates"],
        json!([{
            "url": url, "in_flight": 1, "limit": 1,
            "holders": [holder_pid], "waiting": [waiting.pid()]
        }])
    );
    let runtimes = machine["runtimes"].as_array().unwrap();
    let row = |pid: u32| runtimes.iter().find(|r| r["pid"] == pid).unwrap();
    assert_eq!(row(holder_pid)["state"], "running");
    assert_eq!(row(holder_pid)["holds"], "engine_gate");
    assert_eq!(row(waiting.pid())["state"], "waiting");
    assert_eq!(row(waiting.pid())["waiting_for"], "engine");
    assert_eq!(row(waiting.pid())["behind"], json!([holder_pid]));
    let (_, text, _) = first.text(&["status"]);
    assert!(
        text.contains(&format!(
            "engine gate {url}: 1 of 1 in flight (runtime {holder_pid}); waiting: runtime {}",
            waiting.pid()
        )),
        "{text}"
    );
    // The first request ends; the second is sent and answered.
    holding.interrupt();
    let status = waiting.exit();
    assert!(status.success(), "{status:?} {}", waiting.stderr());
    assert_eq!(waiting.stdout()["task"]["state"], "completed");
    assert_eq!(server.decision_requests().len(), 2);
    let (_, metrics) = second.json(&["metrics", "--task", &id]);
    assert!(
        metrics["engine_wait_ms"].as_u64().unwrap() > 0,
        "the wait is a metric: {metrics}"
    );
    assert_eq!(metrics["engine_waits"], 1, "{metrics}");
    assert_eq!(metrics["admission_waits"], 0);
    let first_id = first.task_id();
    let (_, metrics) = first.json(&["metrics", "--task", &first_id]);
    assert_eq!(metrics["engine_wait_ms"], 0, "the first did not wait");
    assert_eq!(metrics["engine_waits"], 0);
    let _ = holding.exit();
    // Nothing is left in the registry.
    assert!(listing(registry.path()).live.is_empty());
}

/// REQ-SCHED-007, REQ-CLI-005, RFC 013 Amendment 1: a first Ctrl-C during the wait for the gate
/// ends it with the request not sent: the engine never receives it, the step ends cancelled, the
/// task is paused (the process exits 0), no decision was received, and the waiting entry is gone.
#[cfg(unix)]
#[test]
fn req_sched_007_a_first_ctrl_c_during_the_wait_for_the_gate_sends_nothing() {
    let registry = registry();
    let server = server();
    let (_first, mut holding) = holder(registry.path(), &server, 1);
    let second = Runtime::new(registry.path(), &server, 1);
    let mut waiting = second.start();
    waiting.until("the second runtime never waited", || {
        gate_entries(registry.path(), mimlys::registry::State::Waiting) == 1
    });
    waiting.interrupt();
    let status = waiting.exit();
    assert!(status.success(), "{status:?} {}", waiting.stderr());
    let id = second.task_id();
    let (_, view) = second.json(&["task", "show", &id]);
    assert_eq!(view["task"]["state"], "paused", "{view}");
    assert!(view.get("waiting").is_none());
    assert_eq!(server.decision_requests().len(), 1, "nothing was sent");
    let names = second.event_names(&id);
    assert!(
        !names.contains(&"decision.received".to_owned()),
        "{names:?}"
    );
    assert_eq!(
        gate_entries(registry.path(), mimlys::registry::State::Waiting),
        0
    );
    assert_eq!(
        gate_entries(registry.path(), mimlys::registry::State::Held),
        1
    );
    holding.kill();
}

/// REQ-SCHED-007, LIM-O12: the gate's width is `max_in_flight`: with 2, a second request is sent
/// while the first is in flight, and neither waits. Every platform.
#[test]
fn req_sched_007_the_width_is_max_in_flight() {
    let registry = registry();
    let server = server();
    let (_first, mut holding) = holder(registry.path(), &server, 2);
    let second = Runtime::new(registry.path(), &server, 2);
    let mut run = second.start();
    let status = run.exit();
    assert!(status.success(), "{status:?} {}", run.stderr());
    assert_eq!(run.stdout()["task"]["state"], "completed");
    assert_eq!(server.decision_requests().len(), 2, "both were sent");
    let id = second.task_id();
    let (_, metrics) = second.json(&["metrics", "--task", &id]);
    assert_eq!(metrics["engine_waits"], 0, "{metrics}");
    holding.kill();
}

/// REQ-SCHED-007: `status`'s probe takes no gate: with the only place taken by a request in flight,
/// `status` answers at once, reaching the engine. Every platform.
#[test]
fn req_sched_007_the_probe_of_status_takes_no_gate() {
    let registry = registry();
    let server = server();
    let (first, mut holding) = holder(registry.path(), &server, 1);
    let (code, status) = first.json(&["status"]);
    assert_eq!(code, 0);
    assert_eq!(status["backend"]["reachable"], true, "{status}");
    assert_eq!(
        gate_entries(registry.path(), mimlys::registry::State::Waiting),
        0
    );
    assert!(
        holding.child.try_wait().unwrap().is_none(),
        "the first request is still in flight"
    );
    holding.kill();
}

/// THR-38, REQ-SCHED-007: a runtime killed while its request is in flight leaves a stale gate: the
/// next runtime's request is not kept waiting (the entry is reaped at its open), and `runtime reap`
/// lists a gate that is left. Every platform.
#[test]
fn thr_38_a_killed_runtimes_gate_is_stale_and_does_not_keep_the_next_request_waiting() {
    let registry = registry();
    // Two requests are held, one after the other; the third is answered at once.
    let server = MockServer::start(
        MockConfig::default(),
        vec![
            Reply::Slow(HOLD_MS, Box::new(Reply::Decision(complete()))),
            Reply::Slow(HOLD_MS, Box::new(Reply::Decision(complete()))),
            Reply::Decision(complete()),
        ],
    );
    let (first, mut holding) = holder(registry.path(), &server, 1);
    let holder_pid = holding.pid();
    holding.kill();
    let (_, reaped) = first.json(&["runtime", "reap"]);
    let reaped = reaped["reaped"].as_array().unwrap();
    assert_eq!(reaped.len(), 1, "{reaped:?}");
    assert_eq!(reaped[0]["kind"], "gate");
    assert_eq!(reaped[0]["pid"], holder_pid);
    // A second killed holder, reaped by the next runtime's own open.
    let (_third, mut holding) = holder_again(registry.path(), &server);
    holding.kill();
    let next = Runtime::new(registry.path(), &server, 1);
    let mut run = next.start();
    let status = run.exit();
    assert!(status.success(), "{status:?} {}", run.stderr());
    assert_eq!(run.stdout()["task"]["state"], "completed");
    let id = next.task_id();
    let (_, metrics) = next.json(&["metrics", "--task", &id]);
    assert_eq!(metrics["engine_waits"], 0, "{metrics}");
}

/// A request in flight again: the script held the first request, so the next is queued as held.
fn holder_again(registry: &Path, server: &MockServer) -> (Runtime, Started) {
    let before = server.decision_requests().len();
    let again = Runtime::new(registry, server, 1);
    let mut run = again.start();
    run.until("the request never reached the engine", || {
        server.decision_requests().len() > before
            && gate_entries(registry, mimlys::registry::State::Held) == 1
    });
    (again, run)
}
