//! M6c clause tests, first slice (handoff 011c-1; RFC 011 Amendment 2
//! points 1 to 5; REQ-TEST-013, REQ-CONF-002, REQ-CONF-003, REQ-CONF-005,
//! REQ-CONF-011) over the probe tool (`mimlys-probe-tool`) under the
//! Seatbelt profile. macOS only: the Linux suite is `m6a_host_tools.rs`, the
//! Windows one `m6b_host_tools.rs`.
//!
//! Every test that claims a denial has a CONTROL: the same thing done by the
//! probe tool outside any confinement, which shows that the thing is
//! possible on this machine, so that a denial inside the worker is the
//! profile's doing and not the machine's. Each rule these tests guard was
//! shown to fail without it (the mutation outputs are in the review
//! request).
//!
//! Clause K is not enforced on macOS until RFC 011 Amendment 2 point 6 is
//! implemented, so the tier is `core` and `run_host_tool` is not offered.
//! The tests that run a host tool therefore start the worker with the
//! test-only stand-in `MIMLYS_PROBE_BEST_EFFORT=K` and
//! `policy.accept_best_effort = ["K"]` (the same hook `m6a` uses to test
//! RFC 011 §5). It changes what the PROBE REPORTS about K and nothing the
//! worker or the tool does: both still run under the real profile, which is
//! the subject. When K arrives (011c-2) the stand-in goes.
//!
//! A machine without `sandbox_init` (F reports `seatbelt_missing`) skips
//! through `common::skip`; any other state below `enforced` fails the test.
#![cfg(target_os = "macos")]

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
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/m6c-outside");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("secret.txt"), "s").unwrap();
    dir.canonicalize().unwrap()
}

const BUILTIN: &str = r#""read_file", "write_file", "replace_in_file", "list_dir", "search_files", "delete_path", "run_script", "run_wasm""#;

/// The operator's secret in the runtime's environment (clause E, REQ-CONF-011).
const SECRET: &str = "operator-secret-value-1234";

struct Env {
    data: tempfile::TempDir,
    config: PathBuf,
    extra_env: Vec<(String, String)>,
}

impl Env {
    /// The probe tool declared as `probe`; `run_host_tool` is visible through the tasks' `host_tool` grants.
    /// `extra` is appended to the configuration before the declarations.
    fn new(script: &[Value], extra: &str) -> Self {
        let data = tempfile::tempdir().unwrap();
        // The directory is in the write set (`<data>/tmp`) and short enough for a Unix socket path.
        std::fs::create_dir_all(data.path().join("data/tmp")).unwrap();
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
allowed_args = ["ok", "cwd", "read", "write", "connect", "getaddrinfo", "unix-socket", "unix-connect", "id", "sudo-id", "procargs", "procinfo", "signal0", "env"]
arg_pattern = "[A-Za-z0-9_./:=-]+"
max_args = 3
effect = "workspace_write"
timeout_ms = 20000
env = {{ PROBE_OK = "1" }}
env_allow = ["MIMLYS_TEST_RUNTIME_PID", "MIMLYS_TEST_OWNER_PID"]
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

    /// The same, with the stand-in for clause K (see the module comment).
    fn with_k_stand_in(script: &[Value], extra: &str) -> Self {
        Self::new(
            script,
            &format!("[policy]\naccept_best_effort = [\"K\"]\n{extra}"),
        )
        .env("MIMLYS_PROBE_BEST_EFFORT", "K")
    }

    /// Replaces the fake backend's script (for a test whose script names a
    /// path that exists only once the environment does).
    fn set_script(&self, script: &[Value]) {
        std::fs::write(
            self.data.path().join("script.json"),
            serde_json::to_string(script).unwrap(),
        )
        .unwrap();
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
            .env("MIMLYS_TEST_SECRET", SECRET)
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

    /// The write set's short directory (`<data>/tmp`), canonical.
    fn short_dir(&self) -> PathBuf {
        self.data
            .path()
            .join("data/tmp")
            .canonicalize()
            .expect("created by Env::new")
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

/// The clause table as `status` reports it, by name → description.
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

/// Whether this machine has `sandbox_init` at all; records the skip when it does not.
fn seatbelt_or_skip(test: &str, table: &std::collections::BTreeMap<String, Value>) -> bool {
    if table["filesystem"]["reason"] == "seatbelt_missing" {
        common::skip(test, "sandbox_init is not on this macOS (seatbelt_missing)");
        return false;
    }
    true
}

/// Runs the script's host-tool calls under the worker (the K stand-in
/// makes the tier `full`) and returns the observations. F, N and P must be
/// `enforced (seatbelt)` here: anything else fails the test, never skips it.
fn run_probe_script(test: &str, env: &Env) -> Option<Vec<Value>> {
    run_script(test, env, None)
}

/// As [`run_probe_script`], with the runtime's command wrapped (see [`pid_aware`]).
fn run_script(
    test: &str,
    env: &Env,
    wrapper: Option<fn(Command) -> Command>,
) -> Option<Vec<Value>> {
    let (tier, table) = clauses(env);
    if !seatbelt_or_skip(test, &table) {
        return None;
    }
    for letter in ["filesystem", "network", "privilege"] {
        assert!(
            table[letter]["state"] == "enforced" && table[letter]["primitive"] == "seatbelt",
            "{letter} must be enforced (seatbelt) on macOS: {table:?}"
        );
    }
    assert!(tier.starts_with("full"), "tier {tier}: {table:?}");
    let scope = env.scope();
    let args = [
        "--json",
        "run",
        "probe",
        "--scope",
        scope.to_str().unwrap(),
        "--grant",
        "host_tool:probe",
    ];
    let mut cmd = env.command(&args);
    if let Some(wrap) = wrapper {
        cmd = wrap(cmd);
    }
    let out = run_bounded(&mut cmd, env.data.path());
    let value: Value = serde_json::from_str(out.stdout.trim())
        .unwrap_or_else(|e| panic!("not JSON ({e}): {}\n{}", out.stdout, out.stderr));
    assert_eq!(
        value["task"]["state"], "completed",
        "{value}\n{}",
        out.stderr
    );
    let id = value["task"]["task_id"].as_str().unwrap().to_owned();
    Some(env.observations(&id))
}

/// The probe tool run directly, outside every confinement: the control.
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

/// The runtime as a child that knows its own process id: `sh` sets
/// `MIMLYS_TEST_RUNTIME_PID` to its pid and `exec`s the runtime, which keeps
/// the pid.
fn pid_aware(cmd: Command) -> Command {
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
    wrapped
}

/// A same-user process that holds the operator's secret in its environment
/// and sleeps: what the control reads with `KERN_PROCARGS2`.
struct Holder(std::process::Child);

impl Holder {
    fn start() -> Self {
        let child = Command::new("sleep")
            .arg("60")
            .env("MIMLYS_TEST_SECRET", SECRET)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));
        Self(child)
    }
}

impl Drop for Holder {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// REQ-CONF-003 / RFC 011 Amendment 2 points 2 to 5: `status` shows the
/// macOS table as the worker observed it. F, N and P are `enforced
/// (seatbelt)`, E is the explicit environment, K, M and C say which point of
/// the Amendment they wait for, and the tier stays `core` while K is not
/// enforced, so `run_host_tool` is not offered.
#[test]
fn req_conf_003_macos_status_table() {
    let test = "req_conf_003_macos_status_table";
    let env = Env::new(&[], "");
    let (tier, table) = clauses(&env);
    eprintln!("tier {tier}; clauses {table:?}");
    if !seatbelt_or_skip(test, &table) {
        return;
    }
    for (name, state) in [
        ("filesystem", "seatbelt"),
        ("network", "seatbelt"),
        ("privilege", "seatbelt"),
    ] {
        assert_eq!(table[name]["state"], "enforced", "{name}: {table:?}");
        assert_eq!(table[name]["primitive"], state, "{name}: {table:?}");
    }
    assert_eq!(table["environment"]["state"], "enforced", "{table:?}");
    assert_eq!(table["environment"]["primitive"], "explicit_environment");
    assert_eq!(table["termination"]["state"], "unavailable", "{table:?}");
    assert!(
        table["termination"]["reason"]
            .as_str()
            .unwrap()
            .contains("point 6"),
        "{table:?}"
    );
    for name in ["memory", "count"] {
        assert_eq!(table[name]["state"], "unavailable", "{name}: {table:?}");
        assert!(
            table[name]["reason"].as_str().unwrap().contains("point 7"),
            "{name}: {table:?}"
        );
    }
    assert_eq!(tier, "core", "K is not enforced, so the tier is core");
    let (_, status) = env.json(&["status"]);
    let lines: Vec<&str> = status["unavailable_clauses"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l.as_str().unwrap())
        .collect();
    for letter in ["K:", "M:", "C:"] {
        assert!(lines.iter().any(|l| l.starts_with(letter)), "{lines:?}");
    }
    for letter in ["F:", "N:", "P:", "E:"] {
        assert!(!lines.iter().any(|l| l.starts_with(letter)), "{lines:?}");
    }
}

/// REQ-CONF-003 / RFC 011 §5: the K stand-in the other tests use does what
/// it says and no more: with it K is best-effort and the tier is `core`
/// unless `accept_best_effort` names K; M and C stay unavailable.
#[test]
fn req_conf_003_k_stand_in_is_explicit() {
    let env = Env::new(&[], "").env("MIMLYS_PROBE_BEST_EFFORT", "K");
    let (tier, table) = clauses(&env);
    assert_eq!(table["termination"]["state"], "best_effort");
    assert_eq!(tier, "core");
    let env = Env::with_k_stand_in(&[], "");
    let (tier, table) = clauses(&env);
    if !seatbelt_or_skip("req_conf_003_k_stand_in_is_explicit", &table) {
        return;
    }
    assert_eq!(
        tier,
        "full (K best-effort, accepted by policy.accept_best_effort)"
    );
    assert_eq!(table["memory"]["state"], "unavailable");
}

/// REQ-CONF-003 / REQ-CONF-004: an injected failure of a mandatory clause
/// makes the tier `core` with the reason, and `run_host_tool` is neither
/// listed nor accepted. (The probe is honest about what it cannot do.)
#[test]
fn req_conf_003_probe_is_honest_and_fails_closed() {
    let env = Env::with_k_stand_in(&[probe(&["ok"]), complete()], "").env("MIMLYS_PROBE_FAIL", "F");
    let (tier, table) = clauses(&env);
    assert_eq!(tier, "core", "{table:?}");
    assert_eq!(table["filesystem"]["state"], "unavailable");
    assert!(
        table["filesystem"]["reason"]
            .as_str()
            .unwrap()
            .contains("injected"),
        "{table:?}"
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

/// REQ-CONF-002: the host tool runs in the workspace and its allowed
/// counterparts work: `cwd` is the workspace tree (canonical), a write
/// inside it succeeds and leaves the file there, and a file of the project
/// copy is readable. (The positive half of every denial below.)
#[test]
fn req_conf_002_the_workspace_is_usable() {
    let test = "req_conf_002_the_workspace_is_usable";
    let env = Env::with_k_stand_in(
        &[
            probe(&["ok"]),
            probe(&["cwd"]),
            probe(&["write", "inside.txt"]),
            probe(&["read", "src/lib.rs"]),
            complete(),
        ],
        "",
    );
    let Some(obs) = run_probe_script(test, &env) else {
        return;
    };
    assert_eq!(obs.len(), 4);
    assert_eq!(exit_code_of(&obs[0]), 0);
    assert_eq!(stdout_of(&obs[0]).trim(), "ok");
    let tree = Path::new(stdout_of(&obs[1]).trim()).to_path_buf();
    assert!(
        tree.ends_with("tree")
            && tree.starts_with(env.data.path().join("data").canonicalize().unwrap()),
        "cwd is the workspace tree: {tree:?}"
    );
    assert_eq!(stdout_of(&obs[2]).trim(), "wrote");
    assert!(tree.join("inside.txt").is_file());
    assert!(
        stdout_of(&obs[3]).contains("read 10 bytes"),
        "{}",
        stdout_of(&obs[3])
    );
}

/// REQ-CONF-002 / clause F: a file outside the read set cannot be read,
/// and neither can a file in the runtime's data directory beside its store
/// (only its `interpreters` and `tmp` are in the set); the control reads the
/// same files outside the profile.
#[test]
fn req_conf_002_read_outside_the_set_is_denied() {
    let test = "req_conf_002_read_outside_the_set_is_denied";
    let outside = outside().join("secret.txt");
    let (code, line) = control(&["read", outside.to_str().unwrap()]);
    assert_eq!(code, 0, "the control reads it: {line}");
    let env = Env::with_k_stand_in(
        &[
            probe(&["read", outside.to_str().unwrap()]),
            // From the workspace tree (`<data>/workspaces/<id>/tree`): the data directory itself.
            probe(&["read", "../../../store-marker.txt"]),
            complete(),
        ],
        "",
    );
    let marker = env.data.path().join("data/store-marker.txt");
    std::fs::write(&marker, "m").unwrap();
    let (code, line) = control(&["read", marker.to_str().unwrap()]);
    assert_eq!(code, 0, "the control reads the marker: {line}");
    let Some(obs) = run_probe_script(test, &env) else {
        return;
    };
    assert_ne!(exit_code_of(&obs[0]), 0, "{}", stdout_of(&obs[0]));
    assert!(
        stdout_of(&obs[0]).contains("read failed: permission denied"),
        "{}",
        stdout_of(&obs[0])
    );
    assert!(
        stdout_of(&obs[1]).contains("read failed: permission denied"),
        "the data directory is outside the read set: {}",
        stdout_of(&obs[1])
    );
}

/// REQ-CONF-002 / clause F: nothing outside the workspace can be written;
/// the control writes the same place outside the profile.
#[test]
fn req_conf_002_write_outside_the_workspace_is_denied() {
    let test = "req_conf_002_write_outside_the_workspace_is_denied";
    let outside = outside();
    let target = outside.join("w.txt");
    let (code, line) = control(&["write", target.to_str().unwrap()]);
    assert_eq!(code, 0, "the control writes it: {line}");
    assert!(target.exists());
    std::fs::remove_file(&target).unwrap();
    let env = Env::with_k_stand_in(
        &[probe(&["write", target.to_str().unwrap()]), complete()],
        "",
    );
    let Some(obs) = run_probe_script(test, &env) else {
        return;
    };
    assert_ne!(exit_code_of(&obs[0]), 0, "{}", stdout_of(&obs[0]));
    assert!(
        stdout_of(&obs[0]).contains("write failed: permission denied"),
        "{}",
        stdout_of(&obs[0])
    );
    assert!(
        !target.exists(),
        "nothing was written outside the workspace"
    );
}

/// REQ-CONF-005 / clause N: no connection to a live loopback listener; the
/// control connects, the listener sees it from the control and not from the
/// host tool.
#[test]
fn req_conf_005_tcp_to_loopback_is_denied() {
    let test = "req_conf_005_tcp_to_loopback_is_denied";
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let (code, line) = control(&["connect", &addr]);
    assert_eq!(code, 0, "the control connects: {line}");
    std::thread::sleep(std::time::Duration::from_millis(100));
    assert!(
        listener.accept().is_ok(),
        "the control reached the listener"
    );
    let env = Env::with_k_stand_in(&[probe(&["connect", &addr]), complete()], "");
    let Some(obs) = run_probe_script(test, &env) else {
        return;
    };
    assert_ne!(exit_code_of(&obs[0]), 0, "{}", stdout_of(&obs[0]));
    assert!(
        stdout_of(&obs[0]).contains("connect failed: permission denied"),
        "{}",
        stdout_of(&obs[0])
    );
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert!(
        listener.accept().is_err(),
        "the listener saw no connection from the host tool"
    );
}

/// REQ-CONF-005 / clause N: a name cannot be resolved (it goes through a
/// mach service and a Unix socket, an egress channel: measured by S1); the
/// control resolves `localhost` outside the profile.
#[test]
fn req_conf_005_name_lookup_is_denied() {
    let test = "req_conf_005_name_lookup_is_denied";
    let (code, line) = control(&["getaddrinfo", "localhost"]);
    if code != 0 {
        common::skip(
            test,
            &format!("this machine cannot resolve localhost: {line}"),
        );
        return;
    }
    assert!(line.contains("getaddrinfo resolved"), "{line}");
    let env = Env::with_k_stand_in(&[probe(&["getaddrinfo", "localhost"]), complete()], "");
    let Some(obs) = run_probe_script(test, &env) else {
        return;
    };
    assert_ne!(exit_code_of(&obs[0]), 0, "{}", stdout_of(&obs[0]));
    assert!(
        stdout_of(&obs[0]).contains("getaddrinfo failed"),
        "{}",
        stdout_of(&obs[0])
    );
}

/// REQ-CONF-005 / clause N: Unix sockets work under the write set (bind,
/// listen, connect) and a socket that exists outside it cannot be reached;
/// the control reaches it.
#[test]
fn req_conf_005_unix_sockets_only_under_the_write_set() {
    let test = "req_conf_005_unix_sockets_only_under_the_write_set";
    // A short absolute path (a Unix socket path holds about a hundred bytes), outside every set.
    let outside_socket = PathBuf::from(format!(
        "/private/tmp/mimlys-m6c-{}.sock",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&outside_socket);
    let listener = std::os::unix::net::UnixListener::bind(&outside_socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let (code, line) = control(&["unix-connect", outside_socket.to_str().unwrap()]);
    assert_eq!(code, 0, "the control connects: {line}");
    std::thread::sleep(std::time::Duration::from_millis(100));
    assert!(listener.accept().is_ok(), "the control reached the socket");
    let env = Env::with_k_stand_in(&[], "");
    let inside = env.short_dir().join("m6c.sock");
    assert!(inside.to_str().unwrap().len() < 100, "{inside:?}");
    env.set_script(&[
        probe(&["unix-socket", inside.to_str().unwrap()]),
        probe(&["unix-connect", outside_socket.to_str().unwrap()]),
        complete(),
    ]);
    let obs = run_probe_script(test, &env);
    let _ = std::fs::remove_file(&outside_socket);
    let Some(obs) = obs else {
        return;
    };
    assert!(
        stdout_of(&obs[0]).contains("unix-socket bound and connected"),
        "a Unix socket under the write set works: {}",
        stdout_of(&obs[0])
    );
    assert_ne!(exit_code_of(&obs[1]), 0, "{}", stdout_of(&obs[1]));
    assert!(
        stdout_of(&obs[1]).contains("unix-connect failed: permission denied"),
        "{}",
        stdout_of(&obs[1])
    );
    assert!(
        listener.accept().is_err(),
        "the listener saw no connection from the host tool"
    );
}

/// REQ-CONF-002 / clause E: the operator's variable is absent from the
/// tool's environment, the declared one is present; the control shows the
/// tool sees the variable when it is given one.
#[test]
fn req_conf_002_the_operators_secret_is_not_in_the_tools_environment() {
    let test = "req_conf_002_the_operators_secret_is_not_in_the_tools_environment";
    let out = Command::new(probe_tool())
        .args(["env", "MIMLYS_TEST_SECRET"])
        .env_clear()
        .env("MIMLYS_TEST_SECRET", SECRET)
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&out.stdout).contains(SECRET),
        "the control sees a variable it is given"
    );
    let env = Env::with_k_stand_in(
        &[
            probe(&["env", "MIMLYS_TEST_SECRET"]),
            probe(&["env", "PROBE_OK"]),
            complete(),
        ],
        "",
    );
    let Some(obs) = run_probe_script(test, &env) else {
        return;
    };
    assert!(
        stdout_of(&obs[0]).contains("MIMLYS_TEST_SECRET unset"),
        "{}",
        stdout_of(&obs[0])
    );
    assert!(!stdout_of(&obs[0]).contains(SECRET));
    assert!(stdout_of(&obs[1]).contains("PROBE_OK=1"));
}

/// REQ-CONF-011 / Amendment 2 point 4: the arguments and environment of
/// the runtime (which holds the operator's `env:` secrets) cannot be read
/// with `sysctl KERN_PROCARGS2`. The control reads the same kind of
/// environment from a same-user process outside the profile (and shows the
/// secret in it), so the refusal inside is the profile's.
#[test]
fn req_conf_011_procargs_of_the_runtime_is_refused() {
    let test = "req_conf_011_procargs_of_the_runtime_is_refused";
    let holder = Holder::start();
    let out = Command::new(probe_tool())
        .args([
            "procargs",
            "MIMLYS_TEST_OWNER_PID",
            &format!("MIMLYS_TEST_SECRET={SECRET}"),
        ])
        .env_clear()
        .env("MIMLYS_TEST_OWNER_PID", holder.0.id().to_string())
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("the needle is visible: yes"),
        "the control reads a same-user process's environment: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    let needle = format!("MIMLYS_TEST_SECRET={SECRET}");
    let env = Env::with_k_stand_in(
        &[
            probe(&["procargs", "MIMLYS_TEST_RUNTIME_PID", &needle]),
            complete(),
        ],
        "",
    );
    let Some(obs) = run_script(test, &env, Some(pid_aware)) else {
        return;
    };
    assert_ne!(exit_code_of(&obs[0]), 0, "{}", stdout_of(&obs[0]));
    assert!(
        stdout_of(&obs[0]).contains("procargs failed: errno="),
        "{}",
        stdout_of(&obs[0])
    );
    assert!(!stdout_of(&obs[0]).contains("the needle is visible"));
}

/// REQ-CONF-011 / Amendment 2 point 4: other processes are invisible: no
/// process information and no signal for the runtime; the control does both
/// to a same-user process outside the profile.
#[test]
fn req_conf_011_other_processes_are_invisible() {
    let test = "req_conf_011_other_processes_are_invisible";
    let holder = Holder::start();
    for command in ["procinfo", "signal0"] {
        let out = Command::new(probe_tool())
            .args([command, "MIMLYS_TEST_OWNER_PID"])
            .env_clear()
            .env("MIMLYS_TEST_OWNER_PID", holder.0.id().to_string())
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "the control: {command}: {}",
            String::from_utf8_lossy(&out.stdout)
        );
    }
    let env = Env::with_k_stand_in(
        &[
            probe(&["procinfo", "MIMLYS_TEST_RUNTIME_PID"]),
            probe(&["signal0", "MIMLYS_TEST_RUNTIME_PID"]),
            complete(),
        ],
        "",
    );
    let Some(obs) = run_script(test, &env, Some(pid_aware)) else {
        return;
    };
    assert!(
        stdout_of(&obs[0]).contains("procinfo failed: errno="),
        "{}",
        stdout_of(&obs[0])
    );
    assert!(
        stdout_of(&obs[1]).contains("signal0 failed: errno="),
        "{}",
        stdout_of(&obs[1])
    );
}

/// REQ-CONF-005 / clause P (Amendment 2 point 5): the kernel refuses a
/// setuid `execve` from a sandboxed process. The control: outside the
/// profile `sudo -n id -u` prints `0` (the runner has passwordless sudo;
/// elsewhere the test skips). Inside, the exec fails and no `0` is printed,
/// while an ordinary `exec` of `id` works (so the refusal is the setuid
/// bit's).
#[test]
fn req_conf_005_setuid_exec_is_refused() {
    let test = "req_conf_005_setuid_exec_is_refused";
    let (code, line) = control(&["sudo-id"]);
    if code != 0 || !line.ends_with("ran: 0") {
        common::skip(test, &format!("no passwordless sudo here: {line}"));
        return;
    }
    let env = Env::with_k_stand_in(&[probe(&["sudo-id"]), probe(&["id"]), complete()], "");
    let Some(obs) = run_probe_script(test, &env) else {
        return;
    };
    assert_ne!(exit_code_of(&obs[0]), 0, "{}", stdout_of(&obs[0]));
    assert!(
        stdout_of(&obs[0]).contains("sudo-id failed: permission denied"),
        "{}",
        stdout_of(&obs[0])
    );
    assert!(!stdout_of(&obs[0]).contains("ran: 0"));
    assert_eq!(exit_code_of(&obs[1]), 0, "{}", stdout_of(&obs[1]));
    assert!(stdout_of(&obs[1]).contains("id ran: "));
    assert!(
        !stdout_of(&obs[1]).contains("id ran: 0"),
        "{}",
        stdout_of(&obs[1])
    );
}

/// RFC 011 Amendment 2 point 9 (the dependency step's profile) and the
/// measured read set (point 8) wait for spike S1b: recorded as skipped so
/// that the count says so.
#[test]
fn m6c_dependency_step_waits_for_s1b() {
    common::skip(
        "m6c_dependency_step_waits_for_s1b",
        "the dependency step's profile is RFC 011 Amendment 2 point 9 (spike S1b)",
    );
}
