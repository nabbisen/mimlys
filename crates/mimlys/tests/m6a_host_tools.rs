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
allowed_args = ["ok", "cwd", "read", "write", "connect", "env", "setsid-sleep", "fork", "alloc"]
arg_pattern = "[A-Za-z0-9_./:=-]+"
max_args = 3
effect = "workspace_write"
timeout_ms = 20000
env = {{ PROBE_OK = "1" }}
env_allow = ["MIMLYS_TEST_RUNTIME_PID", "MIMLYS_TEST_OWNER_PID"]
[host_tools.strict]
command = {tool:?}
allowed_args = ["ok"]
arg_pattern = "[a-z]+"
max_args = 3
effect = "workspace_write"
timeout_ms = 20000
[host_tools.fetch]
command = {tool:?}
allowed_args = ["connect"]
arg_pattern = "[A-Za-z0-9_.:]+"
max_args = 2
effect = "external_read"
timeout_ms = 20000
network = true
"#,
            data = data.path().join("data").to_string_lossy(),
            script = script_path.to_string_lossy(),
            builtin = BUILTIN,
            tool = probe_tool().to_string_lossy(),
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
        eprintln!("skipped: this machine is tier {tier} ({table:?})");
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
#[test]
fn m6a_cancel_kills_the_whole_tree() {
    cancel_kills_the_whole_tree(false, "1234", None);
}

/// The same, on the `subreaper` rung of clause K (the rung a runner without
/// a delegated cgroup gets): the worker's read set holds no `/proc`
/// (REQ-CONF-011), so the descendants a cancelled tool left re-parented to
/// the worker are swept by the runtime. A different sleep length keeps the
/// two tests' processes apart when they run side by side.
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
    let run = env
        .command(&[
            "--json",
            "run",
            "sleep",
            "--scope",
            scope.to_str().unwrap(),
            "--grant",
            "host_tool:probe",
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    // Wait until the sleepers exist, then stop the run twice: the first
    // signal asks for a pause at the next boundary, the second cancels the
    // running step (external design §19.2); the store lock admits one
    // runtime at a time, so this is how a user cancels from the terminal.
    let started = std::time::Instant::now();
    while sleepers(secs) < 2 && started.elapsed() < std::time::Duration::from_secs(20) {
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    assert!(
        sleepers(secs) >= 2,
        "the host tool and its setsid child run"
    );
    let cancel_at = std::time::Instant::now();
    let pid = run.id().to_string();
    for _ in 0..2 {
        let _ = Command::new("kill").args(["-INT", &pid]).status();
        std::thread::sleep(std::time::Duration::from_millis(300));
    }
    let out = run.wait_with_output().unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while sleepers(secs) > 0 && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert_eq!(
        sleepers(secs),
        0,
        "every sleeper, the setsid one included, is gone"
    );
    assert!(cancel_at.elapsed() < std::time::Duration::from_secs(10));
    let value: Value = serde_json::from_str(String::from_utf8_lossy(&out.stdout).trim())
        .unwrap_or_else(|_| panic!("{}", String::from_utf8_lossy(&out.stderr)));
    let id = value["task"]["task_id"].as_str().unwrap().to_owned();
    let (_, task) = env.json(&["task", "show", &id]);
    assert_eq!(task["task"]["state"], "cancelled");
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
        eprintln!("skipped: this machine is tier {tier}");
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
        eprintln!("skipped: tier {tier}");
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
        eprintln!("skipped: tier {tier}");
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
    std::thread::sleep(std::time::Duration::from_millis(300));
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
    let (_, _) = env.json(&["status"]);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while sleepers("4321") > 0 && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert_eq!(sleepers("4321"), 0, "the leftover tree is gone");
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
        eprintln!("skipped: no rustup toolchain under {}", home.display());
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
        eprintln!("skipped: tier {}", status["tier"]);
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
    std::thread::sleep(std::time::Duration::from_millis(100));
    assert!(
        listener.accept().is_ok(),
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
        "[host_tools.beside]\ncommand = {tool:?}\nallowed_args = [\"read\"]\narg_pattern = \"[A-Za-z0-9_./:=-]+\"\nmax_args = 2\neffect = \"workspace_write\"\ntimeout_ms = 20000\nread_paths = [{bin:?}]\n",
        tool = probe_tool().to_string_lossy(),
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
    let started = std::time::Instant::now();
    while sleepers("1236") < 2 && started.elapsed() < std::time::Duration::from_secs(20) {
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert!(sleepers("1236") >= 2, "the host tool runs");
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
