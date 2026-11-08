//! M6c clause tests (handoff 011c; RFC 011 Amendment 2 and "Amendment 2,
//! settled"; REQ-TEST-013, REQ-CONF-001 … 003, REQ-CONF-005, REQ-CONF-007,
//! REQ-CONF-009, REQ-CONF-011) over the probe tool (`mimlys-probe-tool`)
//! under the Seatbelt profile. macOS only: the Linux suite is
//! `m6a_host_tools.rs`, the Windows one `m6b_host_tools.rs`.
//!
//! Every test that claims a denial has a CONTROL: the same thing done by the
//! probe tool outside any confinement, which shows that the thing is
//! possible on this machine, so that a denial inside the worker is the
//! profile's doing and not the machine's. Each rule these tests guard was
//! shown to fail without it (the mutation outputs are in the review
//! request).
//!
//! Clause K is `enforced (seatbelt_membership)` on macOS (the membership
//! kill loop), so the tier is `full` with no `accept_best_effort`, and these
//! tests run with none: the `MIMLYS_PROBE_BEST_EFFORT` stand-in of the first
//! slice is gone.
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
    /// The dependency step is declared (`with_fetch`): runs grant it too.
    fetch: bool,
    /// Held for the test's life: every runtime these tests start makes `xcrun`'s
    /// cache when it is absent, so the tests that delete it or look for it
    /// (which hold the lock exclusively, [`cache_lock`]) must be alone.
    _runtimes: Option<std::sync::RwLockReadGuard<'static, ()>>,
}

impl Env {
    /// The probe tool declared as `probe`; `run_host_tool` is visible through the tasks' `host_tool` grants.
    /// `extra` is appended to the configuration before the declarations.
    fn new(script: &[Value], extra: &str) -> Self {
        let guard = CACHE.read().unwrap_or_else(|e| e.into_inner());
        Self::build(script, extra, Some(guard))
    }

    /// As [`Env::new`], for a test that already holds [`cache_lock`].
    fn new_exclusive(script: &[Value], extra: &str, _held: &CacheGuard) -> Self {
        Self::build(script, extra, None)
    }

    fn build(
        script: &[Value],
        extra: &str,
        guard: Option<std::sync::RwLockReadGuard<'static, ()>>,
    ) -> Self {
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
allowed_args = ["ok", "cwd", "read", "write", "connect", "connect-name", "getaddrinfo", "unix-socket", "unix-connect", "id", "sudo-id", "procargs", "procinfo", "signal0", "proclist", "env", "setsid-sleep", "orphan", "fork", "alloc-touch", "xcrun-find"]
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
            fetch: false,
            _runtimes: guard,
        }
    }

    /// Declares the dependency step (REQ-CONF-007): the probe tool again,
    /// as `fetch`, with `network = true`, so it runs in the dependency
    /// step's own worker.
    fn with_fetch(mut self) -> Self {
        let mut text = std::fs::read_to_string(&self.config).unwrap();
        text.push_str(&format!(
            r#"[host_tools.fetch]
command = {tool:?}
allowed_args = ["connect", "connect-name", "getaddrinfo", "read"]
arg_pattern = "[A-Za-z0-9_./:=-]+"
max_args = 2
effect = "external_read"
timeout_ms = 30000
network = true
"#,
            tool = probe_tool().to_string_lossy(),
        ));
        std::fs::write(&self.config, text).unwrap();
        self.fetch = true;
        self
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
    assert_eq!(
        tier, "full",
        "K is enforced on macOS, so the tier is full with no accepted clause: {table:?}"
    );
    let scope = env.scope();
    let mut args = vec![
        "--json",
        "run",
        "probe",
        "--scope",
        scope.to_str().unwrap(),
        "--grant",
        "host_tool:probe",
    ];
    if env.fetch {
        args.extend(["--grant", "host_tool:fetch"]);
    }
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

/// REQ-CONF-003 / RFC 011 Amendment 2 (settled): `status` shows the macOS
/// table as the worker observed it. F, N and P are `enforced (seatbelt)`, E
/// is the explicit environment, K is `enforced (seatbelt_membership)`, M and
/// C are the best-effort watchdogs; the tier is `full` with nothing accepted.
#[test]
fn req_conf_003_macos_status_table() {
    let test = "req_conf_003_macos_status_table";
    let env = Env::new(&[], "");
    let (tier, table) = clauses(&env);
    eprintln!("tier {tier}; clauses {table:?}");
    if !seatbelt_or_skip(test, &table) {
        return;
    }
    for (name, primitive) in [
        ("filesystem", "seatbelt"),
        ("network", "seatbelt"),
        ("privilege", "seatbelt"),
        ("environment", "explicit_environment"),
        ("termination", "seatbelt_membership"),
    ] {
        assert_eq!(table[name]["state"], "enforced", "{name}: {table:?}");
        assert_eq!(table[name]["primitive"], primitive, "{name}: {table:?}");
    }
    for (name, primitive) in [
        ("memory", "rss_watchdog"),
        ("count", "process_count_watchdog"),
    ] {
        assert_eq!(table[name]["state"], "best_effort", "{name}: {table:?}");
        assert_eq!(table[name]["primitive"], primitive, "{name}: {table:?}");
    }
    assert_eq!(
        tier, "full",
        "every mandatory clause is enforced: no accepted clause is needed"
    );
    let (_, status) = env.json(&["status"]);
    let lines: Vec<&str> = status["unavailable_clauses"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l.as_str().unwrap())
        .collect();
    for letter in ["M:", "C:"] {
        assert!(lines.iter().any(|l| l.starts_with(letter)), "{lines:?}");
    }
    for letter in ["F:", "N:", "P:", "E:", "K:"] {
        assert!(!lines.iter().any(|l| l.starts_with(letter)), "{lines:?}");
    }
}

/// REQ-CONF-003 / REQ-CONF-004: an injected failure of a mandatory clause
/// makes the tier `core` with the reason, and `run_host_tool` is neither
/// listed nor accepted. (The probe is honest about what it cannot do.)
#[test]
fn req_conf_003_probe_is_honest_and_fails_closed() {
    let env = Env::new(&[probe(&["ok"]), complete()], "").env("MIMLYS_PROBE_FAIL", "F");
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
    let env = Env::new(
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
    let env = Env::new(
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
    let env = Env::new(
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
    let env = Env::new(&[probe(&["connect", &addr]), complete()], "");
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

/// REQ-CONF-005 / clause N: a name that is not in `/etc/hosts` cannot be
/// resolved: resolution goes through a mach service and a Unix socket, an
/// egress channel (measured by S1). The control resolves the same name
/// outside the profile and skips where the machine cannot (no network).
/// (`localhost` is not the subject: the read set holds `/private/etc`, so it
/// resolves from the hosts file without any traffic.)
#[test]
fn req_conf_005_name_lookup_is_denied() {
    let test = "req_conf_005_name_lookup_is_denied";
    let (code, line) = control(&["getaddrinfo", "example.com"]);
    if code != 0 {
        common::skip(
            test,
            &format!("this machine cannot resolve example.com: {line}"),
        );
        return;
    }
    assert!(line.contains("getaddrinfo resolved"), "{line}");
    let env = Env::new(&[probe(&["getaddrinfo", "example.com"]), complete()], "");
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
    let env = Env::new(&[], "");
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
    let env = Env::new(
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
    let env = Env::new(
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
    let env = Env::new(
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
    let env = Env::new(&[probe(&["sudo-id"]), probe(&["id"]), complete()], "");
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

/// Live `/bin/sleep <secs>` processes of this user: the sleepers the probe
/// tool starts (`setsid-sleep`, `orphan`).
fn sleepers(secs: &str) -> Vec<u32> {
    let out = Command::new("pgrep")
        .args(["-f", &format!("^/bin/sleep {secs}$")])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.trim().parse().ok())
        .collect()
}

/// Live copies of the probe tool started as `<tool> sleep 5` (the children of `fork`).
fn fork_children() -> usize {
    let out = Command::new("pgrep")
        .args(["-f", &format!("^{} sleep 5$", probe_tool().display())])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| !l.trim().is_empty())
        .count()
}

/// The parent of a process, from `ps`.
fn ppid_of(pid: u32) -> Option<u32> {
    let out = Command::new("ps")
        .args(["-o", "ppid=", "-p", &pid.to_string()])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

/// Ends the sleepers of an earlier run of the same test that nobody reaped
/// (a failed run leaves them: they sleep for the length in their name), so
/// that a test starts from none.
fn clear_stale_sleepers(secs: &str) {
    let _ = Command::new("pkill")
        .args(["-f", &format!("^/bin/sleep {secs}$")])
        .status();
}

fn wait_until(limit: std::time::Duration, mut done: impl FnMut() -> bool) -> bool {
    let started = std::time::Instant::now();
    while started.elapsed() < limit {
        if done() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    done()
}

/// REQ-CONF-009 / clause K (Amendment 2, settled, point 6): a tool that
/// starts a sleeper in a session of its own, which its parent leaves
/// re-parented to `launchd`, and sleeps itself, is gone with its whole tree
/// when the task is cancelled, within LIM-O2, and the primitive in force is
/// the membership kill loop with NO accepted clause (the config names none).
/// The control inside the test: before the cancel the orphan exists and its
/// parent is `launchd` (a process group walk could not find it).
#[test]
fn req_conf_009_cancel_kills_the_whole_tree() {
    let test = "req_conf_009_cancel_kills_the_whole_tree";
    let secs = "2341";
    clear_stale_sleepers(secs);
    let env = Env::new(&[probe(&["setsid-sleep", secs]), complete()], "");
    let (tier, table) = clauses(&env);
    if !seatbelt_or_skip(test, &table) {
        return;
    }
    assert_eq!(table["termination"]["state"], "enforced", "{table:?}");
    assert_eq!(
        table["termination"]["primitive"], "seatbelt_membership",
        "{table:?}"
    );
    assert_eq!(tier, "full", "{table:?}");
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
    // Wait until the orphan exists, then stop the run twice: the first
    // signal asks for a pause at the next boundary, the second cancels the
    // running step (external design §19.2).
    assert!(
        wait_until(std::time::Duration::from_secs(30), || !sleepers(secs)
            .is_empty()),
        "the tool's sleeper runs"
    );
    let orphan = sleepers(secs)[0];
    assert_eq!(
        ppid_of(orphan),
        Some(1),
        "the sleeper is re-parented to launchd: outside the tool's tree by parentage"
    );
    let cancel_at = std::time::Instant::now();
    let pid = run.id().to_string();
    for _ in 0..2 {
        let _ = Command::new("kill").args(["-INT", &pid]).status();
        std::thread::sleep(std::time::Duration::from_millis(300));
    }
    let out = run.wait_with_output().unwrap();
    assert!(
        wait_until(std::time::Duration::from_secs(5), || sleepers(secs)
            .is_empty()),
        "the re-parented sleeper is gone"
    );
    assert!(cancel_at.elapsed() < std::time::Duration::from_secs(10));
    let value: Value = serde_json::from_str(String::from_utf8_lossy(&out.stdout).trim())
        .unwrap_or_else(|_| panic!("{}", String::from_utf8_lossy(&out.stderr)));
    let id = value["task"]["task_id"].as_str().unwrap().to_owned();
    let (_, task) = env.json(&["task", "show", &id]);
    assert_eq!(task["task"]["state"], "cancelled");
}

/// REQ-CONF-009 / clause K: what a tool leaves behind when it ends by itself
/// (an orphan re-parented to `launchd`, its parent gone, nobody to wait for
/// it) is a member of the sandbox and ends with the tool. The control: the
/// same tool outside the profile leaves its sleeper running.
#[test]
fn req_conf_009_the_tools_end_leaves_no_member() {
    let test = "req_conf_009_the_tools_end_leaves_no_member";
    for secs in ["2342", "2343"] {
        clear_stale_sleepers(secs);
    }
    let (code, line) = control(&["orphan", "2343"]);
    assert_eq!(code, 0, "{line}");
    assert!(
        wait_until(std::time::Duration::from_secs(5), || !sleepers("2343")
            .is_empty()),
        "the control leaves a sleeper behind"
    );
    let _ = Command::new("pkill")
        .args(["-f", "^/bin/sleep 2343$"])
        .status();
    let env = Env::new(&[probe(&["orphan", "2342"]), complete()], "");
    let Some(obs) = run_probe_script(test, &env) else {
        return;
    };
    assert_eq!(exit_code_of(&obs[0]), 0, "{}", stdout_of(&obs[0]));
    assert!(
        wait_until(std::time::Duration::from_secs(5), || sleepers("2342")
            .is_empty()),
        "the orphan the tool left is gone"
    );
}

/// REQ-CONF-001 / clause C (Amendment 2, settled, point 7): a tree of more
/// than LIM-H3 processes is stopped by the watchdog and ends `process_limit`
/// (REQ-SBX-003; `resource_exhausted{processes}`, never an exit); below it nothing
/// happens. The control: the same tool forks 20 children outside the profile.
#[test]
fn req_conf_001_count_watchdog_stops_a_fork_storm() {
    let test = "req_conf_001_count_watchdog_stops_a_fork_storm";
    let (code, line) = control(&["fork", "20", "50"]);
    assert_eq!(code, 0, "the control forks: {line}");
    let env = Env::new(
        &[
            probe(&["fork", "10", "200"]),
            probe(&["fork", "100", "4000"]),
            complete(),
        ],
        "",
    );
    let Some(obs) = run_probe_script(test, &env) else {
        return;
    };
    assert_eq!(
        exit_code_of(&obs[0]),
        0,
        "ten children are below LIM-H3: {}",
        stdout_of(&obs[0])
    );
    assert_eq!(
        obs[1]["payload"]["termination"], "process_limit",
        "a hundred children are stopped by the watchdog, and the model is told why: {}",
        obs[1]
    );
    assert!(
        obs[1]["payload"].get("exit_code").is_none(),
        "never an exit of the tool's own: {}",
        obs[1]
    );
    assert_eq!(
        obs[1]["cause"],
        serde_json::json!({"cause": "resource_exhausted", "limit": "processes"}),
        "{}",
        obs[1]
    );
    assert!(
        wait_until(std::time::Duration::from_secs(5), || fork_children() == 0),
        "the children are gone with the tree"
    );
}

/// REQ-CONF-001 / clause M (Amendment 2, settled, point 7): resident memory
/// of the tree above LIM-H2 ends the execution `memory_limit`; 200 MiB,
/// resident, is below it and passes. (Resident, not virtual: the tool writes
/// every page.) The control: the tool touches memory outside the profile.
#[test]
fn req_conf_001_memory_watchdog_ends_memory_limit() {
    let test = "req_conf_001_memory_watchdog_ends_memory_limit";
    let (code, line) = control(&["alloc-touch", "8"]);
    assert_eq!(code, 0, "the control touches memory: {line}");
    assert!(line.contains("touched 8 MiB"), "{line}");
    let env = Env::new(
        &[
            probe(&["alloc-touch", "200"]),
            probe(&["alloc-touch", "3000"]),
            complete(),
        ],
        "",
    );
    let Some(obs) = run_probe_script(test, &env) else {
        return;
    };
    assert_eq!(exit_code_of(&obs[0]), 0, "{}", stdout_of(&obs[0]));
    assert!(stdout_of(&obs[0]).contains("touched 200 MiB"));
    assert_eq!(
        obs[1]["payload"]["termination"], "memory_limit",
        "3000 MiB resident is above LIM-H2: {}",
        obs[1]
    );
}

/// REQ-CONF-011 / Amendment 2, settled, point 4: the process list with
/// names (`sysctl kern.proc.all`) is refused to a host tool; the control
/// reads it outside the profile.
#[test]
fn req_conf_011_the_process_list_is_refused() {
    let test = "req_conf_011_the_process_list_is_refused";
    let (code, line) = control(&["proclist"]);
    assert_eq!(code, 0, "the control reads the process list: {line}");
    let env = Env::new(&[probe(&["proclist"]), complete()], "");
    let Some(obs) = run_probe_script(test, &env) else {
        return;
    };
    assert_ne!(exit_code_of(&obs[0]), 0, "{}", stdout_of(&obs[0]));
    assert!(
        stdout_of(&obs[0]).contains("proclist failed: errno="),
        "{}",
        stdout_of(&obs[0])
    );
}

fn fetch(args: &[&str]) -> Value {
    json!({"decision": "act", "tool": "run_host_tool", "arguments": {"tool": "fetch", "args": args}})
}

/// REQ-CONF-007 / Amendment 2, settled, point 9: the dependency step's worker
/// resolves names and connects to port 443, and nothing else: a loopback
/// port that is not 443 is refused. The offline tool of the same task
/// resolves nothing and connects to nothing. The control: the machine
/// resolves and connects outside the profile (skipped where it cannot: no
/// network).
#[test]
fn req_conf_007_dependency_step_reaches_https_and_nothing_else() {
    let test = "req_conf_007_dependency_step_reaches_https_and_nothing_else";
    let (code, line) = control(&["connect-name", "example.com:443"]);
    if code != 0 {
        common::skip(
            test,
            &format!("this machine cannot reach example.com:443: {line}"),
        );
        return;
    }
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let (code, line) = control(&["connect", &addr]);
    assert_eq!(
        code, 0,
        "the control connects to the loopback listener: {line}"
    );
    let env = Env::new(
        &[
            fetch(&["getaddrinfo", "example.com"]),
            fetch(&["connect-name", "example.com:443"]),
            fetch(&["connect", &addr]),
            probe(&["getaddrinfo", "example.com"]),
            probe(&["connect-name", "example.com:443"]),
            complete(),
        ],
        "",
    )
    .with_fetch();
    let Some(obs) = run_probe_script(test, &env) else {
        return;
    };
    assert!(
        stdout_of(&obs[0]).contains("getaddrinfo resolved"),
        "the dependency step resolves names: {}",
        stdout_of(&obs[0])
    );
    assert_eq!(
        stdout_of(&obs[1]).trim(),
        "connected",
        "the dependency step reaches port 443"
    );
    assert!(
        stdout_of(&obs[2]).contains("connect failed: permission denied"),
        "only port 443: {}",
        stdout_of(&obs[2])
    );
    assert!(
        stdout_of(&obs[3]).contains("getaddrinfo failed"),
        "the offline tool resolves nothing: {}",
        stdout_of(&obs[3])
    );
    assert!(
        stdout_of(&obs[4]).contains("resolve failed"),
        "the offline tool connects to no name: {}",
        stdout_of(&obs[4])
    );
}

/// REQ-CONF-011 / Amendment 2, settled, point 8: the read set is the measured
/// minimum. `/private/etc` as a whole is not in it (the control reads
/// `/etc/hosts`), nor is `/System` (the control reads a file of an
/// application bundle there), nor the per-user temporary directory as a whole
/// (the control reads a marker the test puts there); the files the toolchain
/// needs are (`openssl.cnf`, `SystemVersion.plist`).
#[test]
fn req_conf_011_the_read_set_is_the_measured_minimum() {
    let test = "req_conf_011_the_read_set_is_the_measured_minimum";
    let user_temp = std::env::temp_dir().canonicalize().unwrap();
    let marker = user_temp.join(format!("mimlys-m6c-marker-{}", std::process::id()));
    std::fs::write(&marker, "m").unwrap();
    let denied = [
        "/etc/hosts",
        "/System/Library/CoreServices/Finder.app/Contents/Info.plist",
        marker.to_str().unwrap(),
    ];
    let allowed = [
        "/private/etc/ssl/openssl.cnf",
        "/System/Library/CoreServices/SystemVersion.plist",
        "/usr/bin/true",
    ];
    for path in denied.iter().chain(allowed.iter()) {
        let (code, line) = control(&["read", path]);
        assert_eq!(code, 0, "the control reads {path}: {line}");
    }
    let mut script: Vec<Value> = denied.iter().map(|p| probe(&["read", p])).collect();
    script.extend(allowed.iter().map(|p| probe(&["read", p])));
    script.push(complete());
    let env = Env::new(&script, "");
    let obs = run_probe_script(test, &env);
    let _ = std::fs::remove_file(&marker);
    let Some(obs) = obs else {
        return;
    };
    for (i, path) in denied.iter().enumerate() {
        assert!(
            stdout_of(&obs[i]).contains("read failed: permission denied"),
            "{path} is outside the read set: {}",
            stdout_of(&obs[i])
        );
    }
    for (i, path) in allowed.iter().enumerate() {
        assert_eq!(
            exit_code_of(&obs[denied.len() + i]),
            0,
            "{path} is in the read set: {}",
            stdout_of(&obs[denied.len() + i])
        );
    }
}

/// REQ-CONF-007 / Amendment 2, settled, point 9: the system's certificate
/// file is readable to the dependency step's worker and to nobody else (it
/// is not in the offline read set since `/private/etc` left it). The control
/// reads it outside the profile.
#[test]
fn req_conf_007_the_certificates_are_the_dependency_steps() {
    let test = "req_conf_007_the_certificates_are_the_dependency_steps";
    let cert = "/private/etc/ssl/cert.pem";
    let (code, line) = control(&["read", cert]);
    if code != 0 {
        common::skip(test, &format!("this machine has no {cert}: {line}"));
        return;
    }
    let env = Env::new(
        &[
            json!({"decision": "act", "tool": "run_host_tool", "arguments": {"tool": "fetch", "args": ["read", cert]}}),
            probe(&["read", cert]),
            complete(),
        ],
        "",
    )
    .with_fetch();
    let Some(obs) = run_probe_script(test, &env) else {
        return;
    };
    assert_eq!(
        exit_code_of(&obs[0]),
        0,
        "the dependency step reads the certificates: {}",
        stdout_of(&obs[0])
    );
    assert!(
        stdout_of(&obs[1]).contains("read failed: permission denied"),
        "the offline tool does not: {}",
        stdout_of(&obs[1])
    );
}

/// REQ-CONF-004 / external design §13.4: every host-tool approval carries the
/// tier line, and on macOS it reads plain `full`: no clause is accepted
/// (`accept_best_effort` is not in the configuration), since K is enforced.
#[test]
fn req_conf_004_host_tool_approval_shows_the_tier() {
    let test = "req_conf_004_host_tool_approval_shows_the_tier";
    let env = Env::new(
        &[probe(&["ok"]), complete()],
        "[policy.profiles.read_only]\nhost_tools = [\"probe\"]\napproval_tools = [\"run_host_tool\"]\n",
    );
    let (tier, table) = clauses(&env);
    if !seatbelt_or_skip(test, &table) {
        return;
    }
    assert_eq!(tier, "full", "{table:?}");
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
    let lines = value["pending_approval"]["rendered"]["lines"]
        .as_array()
        .expect("rendered lines")
        .clone();
    let tier_line = lines
        .iter()
        .find(|l| l["label"] == "Tier")
        .unwrap_or_else(|| panic!("no Tier line in {lines:?}"));
    assert_eq!(tier_line["value"], "full");
    let id = value["task"]["task_id"].as_str().unwrap().to_owned();
    let (code, value) = env.json(&["approve", &id]);
    assert_eq!(code, 0, "{value}");
    let obs = env.observations(&id);
    assert_eq!(exit_code_of(&obs[0]), 0, "{obs:?}");
    let (code, _) = env.json(&["task", "cancel", &id]);
    assert_eq!(code, 0);
}

/// A rustup toolchain's `bin` directory under the home directory (the
/// preferred one is `stable`), with the rustup home: the direct toolchain
/// `cargo`, as Homebrew's `rustup` shim links into `/opt/homebrew/Cellar`.
fn toolchain_bin() -> Option<(PathBuf, PathBuf)> {
    let home = directories::BaseDirs::new()?.home_dir().to_path_buf();
    let mut bins: Vec<PathBuf> = std::fs::read_dir(home.join(".rustup/toolchains"))
        .ok()?
        .flatten()
        .map(|e| e.path().join("bin"))
        .filter(|b| b.join("cargo").is_file() && b.join("rustc").is_file())
        .collect();
    bins.sort_by_key(|b| !b.to_string_lossy().contains("stable"));
    bins.into_iter().next().map(|b| (home.join(".rustup"), b))
}

/// External design §23, M6c: `cargo test --offline` of a native project, with
/// a path dependency, ends `completed{verified}` under every mandatory
/// clause, and WITHOUT `accept_best_effort` (the configuration names none):
/// the model fixes the failing test and the completion check verifies it.
/// The read set is the measured one; every entry that the run needed beyond
/// the minimum of point 8 is named in the review request with the failure
/// that asked for it.
#[test]
fn m6c_demonstration_cargo_test_offline() {
    let _serial = cache_lock();
    demonstration("m6c_demonstration_cargo_test_offline");
}

/// The tests that delete `xcrun`'s cache or look for it take turns with all
/// the others: the cache is one file in the per-user temporary directory, and
/// every runtime started makes it when it is absent.
static CACHE: std::sync::RwLock<()> = std::sync::RwLock::new(());

type CacheGuard = std::sync::RwLockWriteGuard<'static, ()>;

/// Exclusive: no other test runs a runtime meanwhile.
fn cache_lock() -> CacheGuard {
    CACHE.write().unwrap_or_else(|e| e.into_inner())
}

/// The demonstration: `cargo test --offline` of a native project, the model
/// fixes the failing test and the completion check verifies it.
fn demonstration(test: &str) {
    let Some((rustup_home, bin)) = toolchain_bin() else {
        common::skip(test, "no rustup toolchain under ~/.rustup");
        return;
    };
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
command = {cargo:?}
args = ["test", "--offline"]
allowed_args = ["--lib"]
max_args = 1
effect = "workspace_write"
timeout_ms = 600000
read_paths = [{bin:?}, {rustup:?}]
env = {{ PATH = {path:?} }}
"#,
            data = data.path().join("data").to_string_lossy(),
            script = script.to_string_lossy(),
            builtin = BUILTIN,
            cargo = bin.join("cargo").to_string_lossy(),
            bin = bin.to_string_lossy(),
            rustup = rustup_home.to_string_lossy(),
            path = format!("{}:/usr/bin:/bin", bin.display()),
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
    let (_, status, stderr) = run(&["status"]);
    eprintln!(
        "m6c demonstration: tier {} clauses {}",
        status["tier"], status["clauses"]
    );
    if status["clauses"]["filesystem"]["reason"] == "seatbelt_missing" {
        common::skip(test, "sandbox_init is not on this macOS (seatbelt_missing)");
        return;
    }
    assert_eq!(
        status["tier"], "full",
        "no accepted clause is in the configuration: {status}\n{stderr}"
    );
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
        "m6c demonstration: {:?} in {:?}",
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

/// REQ-CONF-009 / RFC 011 Amendment 2, implementation decisions, point 6: the
/// runtime killed with `SIGKILL` while a tool runs leaves no member alive. The
/// worker reads end of file on its input and ends; the tool and the sleeper it
/// left in a session of its own are swept (by the worker's last act or, when
/// the worker is gone first, by its sweeper) within LIM-O2 plus one poll. The
/// control: before the kill the sleeper's parent is `launchd`.
#[test]
fn req_conf_009_a_killed_runtime_leaves_no_member() {
    let test = "req_conf_009_a_killed_runtime_leaves_no_member";
    let secs = "2352";
    clear_stale_sleepers(secs);
    let env = Env::new(&[probe(&["setsid-sleep", secs]), complete()], "");
    let (_, table) = clauses(&env);
    if !seatbelt_or_skip(test, &table) {
        return;
    }
    let scope = env.scope();
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
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    assert!(
        wait_until(std::time::Duration::from_secs(30), || !sleepers(secs)
            .is_empty()),
        "the tool's sleeper runs"
    );
    assert_eq!(ppid_of(sleepers(secs)[0]), Some(1));
    let killed_at = std::time::Instant::now();
    run.kill().unwrap();
    let _ = run.wait();
    let bound = std::time::Duration::from_millis(
        mimlys_core::limits::LIM_O2_CANCEL_MS + mimlys_core::limits::LIM_H6_TRACK_INTERVAL_MS + 250,
    );
    let gone = wait_until(bound + std::time::Duration::from_secs(5), || {
        sleepers(secs).is_empty()
    });
    let took = killed_at.elapsed();
    assert!(
        gone,
        "a member is left after the runtime was killed ({took:?})"
    );
    assert!(
        took <= bound,
        "swept in {took:?}, more than LIM-O2 plus one poll ({bound:?})"
    );
}

/// REQ-CONF-005 / RFC 011 Amendment 2, implementation decisions, point 8
/// (external design §13.3, LIM-O8): on a Mac where `xcrun` never ran (here:
/// its cache deleted) with an Xcode selected, the runtime makes the cache itself,
/// outside the sandbox, before it starts the worker, and the demonstration
/// completes (011c-5 measured that it halts without). The control: the cache
/// is absent before and exists afterwards. With the Command Line Tools selected
/// the cache is not needed and the test says so.
#[test]
fn req_conf_005_the_demonstration_completes_with_the_xcrun_cache_deleted() {
    let test = "req_conf_005_the_demonstration_completes_with_the_xcrun_cache_deleted";
    let _serial = cache_lock();
    let Some(file) = mimlys_exec::confine::macos::xcrun_cache_file() else {
        common::skip(
            test,
            "the Command Line Tools are selected: no cache is needed",
        );
        return;
    };
    let _ = std::fs::remove_file(&file);
    assert!(!file.exists(), "the control: the cache is absent");
    demonstration(test);
    assert!(file.exists(), "the runtime made {}", file.display());
}

/// REQ-CONF-005 / external design §13.3 (the warming's list): after the
/// runtime's warming, a host tool inside the profile resolves EACH tool of the
/// list through the sandboxed `xcrun` (`xcrun --find <tool>`, run by the probe
/// tool as a build's shims run it). The cache is keyed by the tool asked for,
/// so a tool missing from the list fails here and fails a build the same way.
/// The control: the cache is absent before the run. With the Command Line
/// Tools selected the cache is not needed and the test says so.
#[test]
fn req_conf_005_every_listed_tool_resolves_through_the_sandboxed_xcrun() {
    // The tools a Rust link and the `cc` crate's builds ask for, written out here and
    // not read from the runtime's list: a test that took the list from the code under
    // test would stop asking for a tool the moment the code stopped warming it.
    const WARMED_TOOLS: [&str; 7] = ["clang", "ld", "ar", "ranlib", "strip", "libtool", "lipo"];
    let test = "req_conf_005_every_listed_tool_resolves_through_the_sandboxed_xcrun";
    let serial = cache_lock();
    let Some(file) = mimlys_exec::confine::macos::xcrun_cache_file() else {
        common::skip(
            test,
            "the Command Line Tools are selected: no cache is needed",
        );
        return;
    };
    let _ = std::fs::remove_file(&file);
    assert!(!file.exists(), "the control: the cache is absent");
    let mut script: Vec<Value> = WARMED_TOOLS
        .iter()
        .map(|tool| probe(&["xcrun-find", tool]))
        .collect();
    script.push(complete());
    let env = Env::new_exclusive(&script, "", &serial);
    let Some(obs) = run_probe_script(test, &env) else {
        return;
    };
    assert!(file.exists(), "the runtime made {}", file.display());
    for (tool, observation) in WARMED_TOOLS.iter().zip(&obs) {
        let out = stdout_of(observation);
        assert_eq!(
            exit_code_of(observation),
            0,
            "xcrun --find {tool} inside the profile: {out}"
        );
        assert!(
            out.trim_end().ends_with(&format!("/{tool}")),
            "xcrun --find {tool} resolved to: {out}"
        );
    }
}

/// REQ-CONF-003 / external design §13.3: if the cache cannot be made (the
/// test makes `xcrun` fail with the debug-only `MIMLYS_TEST_XCRUN`, which a
/// release build does not read), `status` and `check` each show one line
/// naming the file and the remedy, in the JSON as `xcrun_cache_missing`, and
/// the command goes on (exit 0). Without the hook the same commands show no
/// such line and the cache exists afterwards.
#[test]
fn req_conf_003_status_and_check_say_when_the_xcrun_cache_cannot_be_made() {
    let test = "req_conf_003_status_and_check_say_when_the_xcrun_cache_cannot_be_made";
    let _serial = cache_lock();
    let Some(file) = mimlys_exec::confine::macos::xcrun_cache_file() else {
        common::skip(
            test,
            "the Command Line Tools are selected: no cache is needed",
        );
        return;
    };
    let shown = file.to_string_lossy().into_owned();
    let _ = std::fs::remove_file(&file);
    let failing = Env::new_exclusive(&[], "", &_serial).env("MIMLYS_TEST_XCRUN", "/usr/bin/false");
    for command in ["status", "check"] {
        let (code, stdout, stderr) = failing.run(&[command]);
        assert_eq!(code, 0, "the run goes on: {stdout}\n{stderr}");
        let lines: Vec<&str> = stdout
            .lines()
            .filter(|l| l.contains("xcrun's cache"))
            .collect();
        assert_eq!(lines.len(), 1, "one line: {stdout}");
        assert!(lines[0].contains(&shown), "names the file: {}", lines[0]);
        assert!(
            lines[0].contains("run `env -i PATH=/usr/bin:/bin /usr/bin/xcrun --find clang` once"),
            "and the remedy: {}",
            lines[0]
        );
        let (_, json) = failing.json(&[command]);
        assert_eq!(json["xcrun_cache_missing"], shown.as_str(), "{json}");
        assert!(!file.exists(), "it could not be made");
    }
    let healthy = Env::new_exclusive(&[], "", &_serial);
    for command in ["status", "check"] {
        let (code, stdout, _) = healthy.run(&[command]);
        assert_eq!(code, 0);
        assert!(!stdout.contains("xcrun's cache"), "{stdout}");
        let (_, json) = healthy.json(&[command]);
        assert!(json.get("xcrun_cache_missing").is_none(), "{json}");
        assert!(file.exists(), "the runtime made the cache");
        let _ = std::fs::remove_file(&file);
    }
}
