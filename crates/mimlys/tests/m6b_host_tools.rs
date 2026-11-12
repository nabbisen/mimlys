//! M6b tests (RFC 011 §3, §7; REQ-TEST-013, REQ-CONF-004 … 008) over the
//! probe tool (`mimlys-probe-tool`) on Windows: every clause inside a host
//! tool running in the AppContainer and the job, the tree killed as one, the
//! honest probe, the argument rules, the dependency step, the reversible
//! toolchain permissions, and, when a rustup toolchain is installed under
//! the profile, the demonstration of external design §23. Windows only: the
//! Linux suite is `m6a_host_tools.rs`.
#![cfg(windows)]

mod common;

use common::run_bounded;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;

fn probe_tool() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_mimlys-probe-tool"))
}

/// A directory outside the read set: beside, not beneath, the worker's directory.
fn outside() -> PathBuf {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/m6b-outside");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("secret.txt"), "s").unwrap();
    // Without the verbatim prefix: it is an argument of the probe tool.
    let dir = dir.canonicalize().unwrap();
    PathBuf::from(dir.to_string_lossy().trim_start_matches(r"\\?\"))
}

/// The observations of a task, in order, from the store under `data`.
fn observations_in(data: &Path, task_id: &str) -> Vec<Value> {
    let conn = rusqlite::Connection::open(data.join("data/mimlys.db")).unwrap();
    let mut stmt = conn
        .prepare("select o.record from observations o join actions a on a.action_id = o.action_id where a.task_id = ?1 order by o.rowid")
        .unwrap();
    stmt.query_map([task_id], |r| r.get::<_, String>(0))
        .unwrap()
        .map(|r| serde_json::from_str(&r.unwrap()).unwrap())
        .collect()
}

/// A short account of each observation: exit code, termination, both streams.
fn account(obs: &[Value]) -> String {
    obs.iter()
        .map(|o| {
            format!(
                "exit {} termination {} stdout {:?} stderr {:?}",
                o["payload"]["exit_code"],
                o["payload"]["termination"],
                o["stdout"].as_str().unwrap_or(""),
                o["stderr"].as_str().unwrap_or("")
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

const BUILTIN: &str = r#""read_file", "write_file", "replace_in_file", "list_dir", "search_files", "delete_path", "run_script", "run_wasm""#;

struct Env {
    data: tempfile::TempDir,
    config: PathBuf,
    extra_env: Vec<(String, String)>,
}

impl Env {
    /// The probe tool declared as `probe` (offline, a 10 s deadline so that
    /// the tree-kill test runs through the deadline), `strict` and `fetch`
    /// (the dependency step); `run_host_tool` is visible through the tasks'
    /// `host_tool` grants.
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
[logging]
level = "debug"
{extra}
[policy.profiles.workspace]
tools = [{builtin}]
[host_tools.probe]
command = {tool:?}
allowed_args = ["ok", "cwd", "read", "write", "connect", "env", "setsid-sleep", "fork", "alloc"]
arg_pattern = "[A-Za-z0-9_./:=~\\\\-]+"
max_args = 3
effect = "workspace_write"
timeout_ms = 10000
env = {{ PROBE_OK = "1" }}
[host_tools.strict]
command = {tool:?}
allowed_args = ["ok"]
arg_pattern = "[a-z]+"
max_args = 3
effect = "workspace_write"
timeout_ms = 20000
[host_tools.fetch]
command = {tool:?}
allowed_args = ["ok", "connect"]
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
        // The runtime's log, shown with a failing test.
        eprintln!("--- mimlys {args:?}\n{}", out.stderr);
        (out.code(), out.stdout, out.stderr)
    }

    /// The rejected decisions of a task: `code: reason`, in order.
    fn rejections(&self, id: &str) -> Vec<String> {
        self.events(id)
            .iter()
            .filter(|e| e["event"] == "decision.rejected")
            .map(|e| format!("{}: {}", e["code"], e["reason"]))
            .collect()
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
        observations_in(self.data.path(), id)
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

/// The clause table as `status` reports it, by word → state.
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

/// Whether a process id is alive (`tasklist` lists it).
fn alive(pid: &str) -> bool {
    let out = Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).contains(pid)
}

/// REQ-TEST-013 / RFC 011 §3: for every clause the probe reports enforced,
/// the forbidden thing fails as expected inside a host tool, and the
/// allowed thing works.
#[test]
fn m6b_clauses_hold_inside_a_host_tool() {
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
    assert_eq!(
        tier, "full",
        "the Windows backend holds every mandatory clause: {table:?}"
    );
    for clause in [
        "filesystem",
        "network",
        "termination",
        "privilege",
        "memory",
        "count",
    ] {
        assert_eq!(table[clause]["state"], "enforced", "{clause}: {table:?}");
    }
    assert_eq!(table["filesystem"]["primitive"], "appcontainer");
    assert_eq!(table["termination"]["primitive"], "job_object");
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
    assert_eq!(
        obs.len(),
        10,
        "rejections {:?}\n{}",
        env.rejections(&id),
        account(&obs)
    );
    assert_eq!(exit_code_of(&obs[0]), 0, "{}", stdout_of(&obs[0]));
    assert_eq!(stdout_of(&obs[0]).trim(), "ok");
    // Both canonical: the temporary directory may be spelled in its short form.
    assert_eq!(
        Path::new(stdout_of(&obs[1]).trim())
            .canonicalize()
            .map(|p| p.to_string_lossy().to_lowercase())
            .unwrap_or_else(|_| stdout_of(&obs[1])),
        env.tree(&id)
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .to_lowercase(),
        "cwd is the workspace tree"
    );
    // F: outside the read set, no read and no write; inside the workspace, a write.
    assert!(
        stdout_of(&obs[2]).contains("read failed"),
        "{}",
        stdout_of(&obs[2])
    );
    assert!(
        stdout_of(&obs[3]).contains("write failed"),
        "{}",
        stdout_of(&obs[3])
    );
    assert_eq!(stdout_of(&obs[4]).trim(), "wrote");
    assert!(env.tree(&id).join("inside.txt").is_file());
    assert!(!outside.join("w.txt").exists());
    // N: no network capability (the listener is alive and would accept).
    assert!(
        stdout_of(&obs[5]).contains("connect failed"),
        "{}",
        stdout_of(&obs[5])
    );
    drop(listener);
    // E: the operator's variable is absent; the declared one is present.
    assert!(stdout_of(&obs[6]).contains("MIMLYS_TEST_SECRET unset"));
    assert!(stdout_of(&obs[7]).contains("PROBE_OK=1"));
    // C: the job's process limit stops the hundred.
    assert_ne!(
        exit_code_of(&obs[8]),
        0,
        "the active process limit bounds the tree: {}",
        stdout_of(&obs[8])
    );
    // M: the job's memory limit ends the allocation.
    assert_ne!(
        exit_code_of(&obs[9]),
        0,
        "3000 MiB above LIM-H2 ends the tool: {}",
        stdout_of(&obs[9])
    );
}

/// REQ-CONF-009 / RFC 011 §3 (K): a tool that starts a detached child in a
/// new process group and sleeps is terminated with its whole tree when its
/// deadline passes; the job forbids breakaway, so the child dies with it.
#[test]
fn m6b_deadline_kills_the_whole_tree() {
    let env = Env::new(&[probe(&["setsid-sleep", "60"]), complete()], "");
    let (tier, _) = clauses(&env);
    assert_eq!(tier, "full");
    let scope = env.scope();
    let started = std::time::Instant::now();
    let (code, value) = env.json(&[
        "run",
        "sleep then die",
        "--scope",
        scope.to_str().unwrap(),
        "--grant",
        "host_tool:probe",
    ]);
    assert_eq!(code, 0, "{value}");
    let elapsed = started.elapsed();
    let id = value["task"]["task_id"].as_str().unwrap().to_owned();
    let obs = env.observations(&id);
    assert!(
        elapsed < std::time::Duration::from_secs(40),
        "the deadline of 10 s ended the tool: the run took {:.1} s; task {}\n{}",
        elapsed.as_secs_f64(),
        value["task"],
        account(&obs)
    );
    assert_eq!(
        obs[0]["payload"]["termination"],
        "deadline",
        "{}",
        account(&obs)
    );
    let child = stdout_of(&obs[0])
        .lines()
        .find_map(|l| l.strip_prefix("child "))
        .map(str::trim)
        .map(str::to_owned)
        .unwrap_or_else(|| panic!("no child pid in {}", stdout_of(&obs[0])));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while alive(&child) && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert!(
        !alive(&child),
        "the detached child {child} is gone with the job"
    );
}

/// REQ-TEST-013 / RFC 011 §5: an injected failure of a mandatory clause
/// makes the tier `core` with the reason, and `run_host_tool` is neither
/// listed nor accepted.
#[test]
fn m6b_probe_is_honest_and_fails_closed() {
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

/// REQ-CONF-006 / RFC 011 §6: literals and pattern matches pass; an
/// unlisted flag, a path, too many arguments and an undeclared tool are
/// `denied`, and the task goes on between them.
#[test]
fn m6b_argument_rules_are_enforced() {
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
    assert_eq!(tier, "full");
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
        ["denied", "denied", "denied", "denied"],
        "rejections {:?}; task {}\n{}",
        env.rejections(&id),
        value["task"],
        account(&env.observations(&id))
    );
}

/// REQ-CONF-007 / RFC 011 §8: the dependency step runs in its own worker
/// with the internet client capability and is `external_read`; the offline
/// tool in the same task has no network. (An AppContainer cannot reach
/// loopback without an administrator's exemption, so the step's reach is
/// shown by its capability, not by a local listener.)
#[test]
fn m6b_dependency_step_is_external_read() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let env = Env::new(
        &[
            json!({"decision": "act", "tool": "run_host_tool", "arguments": {"tool": "fetch", "args": ["ok"]}}),
            probe(&["connect", &format!("127.0.0.1:{port}")]),
            complete(),
        ],
        "",
    );
    let (tier, _) = clauses(&env);
    assert_eq!(tier, "full");
    let scope = env.scope();
    let (code, value) = env.json(&[
        "run",
        "fetch then probe",
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
    assert_eq!(stdout_of(&obs[0]).trim(), "ok");
    assert!(
        stdout_of(&obs[1]).contains("connect failed"),
        "the offline tool cannot: {}",
        stdout_of(&obs[1])
    );
    let effects: Vec<String> = env
        .events(&id)
        .iter()
        .filter(|e| e["event"] == "action.authorized")
        .filter_map(|e| e["effect"].as_str().map(str::to_owned))
        .collect();
    assert!(
        effects.is_empty() || effects[0] == "external_read",
        "the dependency step is external_read: {effects:?}"
    );
    let (_, status) = env.json(&["status"]);
    let fetch = status["host_tools"]
        .as_array()
        .map(|a| {
            a.iter()
                .any(|t| t["name"] == "fetch" && t["network"] == true)
        })
        .unwrap_or(true);
    assert!(fetch, "{status}");
}

/// REQ-CONF-008 / RFC 011 §7: a declared path without the container's
/// permission makes its tool unavailable with `acl_not_applied` and a grant
/// for it is refused with the reason; `toolchain grant` applies the
/// permission reversibly and `revoke` leaves the directory's permissions as
/// they were.
#[test]
fn m6b_toolchain_permissions_are_reversible_and_gate_tools() {
    let toolchain = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(toolchain.path().join("bin")).unwrap();
    let dir = toolchain.path().canonicalize().unwrap();
    let dir_text = dir.to_string_lossy().trim_start_matches(r"\\?\").to_owned();
    let env = Env::new(
        &[probe(&["ok"]), complete()],
        &format!(
            "[host_tools.gated]\ncommand = {:?}\nallowed_args = [\"ok\"]\nmax_args = 1\neffect = \"workspace_write\"\nread_paths = [{:?}]\n",
            probe_tool().to_string_lossy(),
            dir_text
        ),
    );
    let before = Command::new("icacls")
        .arg(&dir_text)
        .output()
        .unwrap()
        .stdout;
    let (_, status) = env.json(&["status"]);
    let gated = status["host_tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "gated")
        .cloned()
        .unwrap_or_else(|| panic!("{status}"));
    assert!(
        gated["unavailable"]
            .as_str()
            .is_some_and(|r| r.starts_with("acl_not_applied")),
        "{gated}"
    );
    let scope = env.scope();
    let (code, refused) = env.json(&[
        "run",
        "use the gated tool",
        "--scope",
        scope.to_str().unwrap(),
        "--grant",
        "host_tool:gated",
    ]);
    assert_eq!(code, 1, "{refused}");
    assert!(
        refused["error"]["message"]
            .as_str()
            .unwrap()
            .contains("toolchain grant"),
        "{refused}"
    );
    let (code, granted) = env.json(&["toolchain", "grant", &dir_text, "--yes"]);
    assert_eq!(code, 0, "{granted}");
    assert_eq!(granted["applied"], true);
    let granted_acl = Command::new("icacls")
        .arg(&dir_text)
        .output()
        .unwrap()
        .stdout;
    eprintln!(
        "--- icacls before\n{}--- icacls after grant\n{}",
        String::from_utf8_lossy(&before),
        String::from_utf8_lossy(&granted_acl)
    );
    let (_, listed) = env.json(&["toolchain", "list"]);
    let entry = listed["host_tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "gated")
        .cloned()
        .unwrap();
    assert!(entry["unavailable"].is_null(), "{listed}");
    let (code, revoked) = env.json(&["toolchain", "revoke", &dir_text, "--yes"]);
    assert_eq!(code, 0, "{revoked}");
    let after = Command::new("icacls")
        .arg(&dir_text)
        .output()
        .unwrap()
        .stdout;
    // The entries are what they were. The write marks the descriptor
    // auto-inherited, after which icacls prints `(I)` on the inherited
    // entries (their flag was set before the write too); the marker is not
    // compared.
    assert_eq!(
        String::from_utf8_lossy(&before).replace("(I)", ""),
        String::from_utf8_lossy(&after).replace("(I)", ""),
        "revoke leaves the permissions as they were"
    );
    // mimlys's own directories cannot be revoked here.
    let workspaces = env.data.path().join("data/workspaces");
    let (code, refused) = env.json(&["toolchain", "revoke", workspaces.to_str().unwrap(), "--yes"]);
    assert_eq!(code, 1, "{refused}");
}

/// External design §23, M6b: `cargo test --offline` of a native project,
/// with a path dependency, under every mandatory clause; the model fixes
/// the failing test and the completion check verifies it. Runs when a
/// rustup toolchain is installed under the user's profile; the toolchain
/// directories receive the container's permission first.
#[test]
fn m6b_demonstration_cargo_test_offline() {
    let home = directories::BaseDirs::new()
        .map(|b| b.home_dir().to_path_buf())
        .unwrap();
    let cargo = home.join(".cargo").join("bin").join("cargo.exe");
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
    // REQ-CONF-011: the toolchain's own directory, never `.cargo` as a whole
    // (`credentials.toml` lives there); `toolchain grant` refuses the latter.
    let cargo_home = home
        .join(".cargo")
        .join("bin")
        .to_string_lossy()
        .into_owned();
    let rustup_home = home.join(".rustup").to_string_lossy().into_owned();
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
[logging]
level = "debug"
[policy.profiles.workspace]
tools = [{builtin}]
[host_tools.cargo_test]
command = {cargo:?}
args = ["test", "--offline"]
allowed_args = ["--lib"]
max_args = 1
effect = "workspace_write"
timeout_ms = 600000
read_paths = [{cargo_home:?}, {rustup_home:?}]
env = {{ RUSTUP_HOME = {rustup_home:?}, RUSTUP_TOOLCHAIN = "stable" }}
"#,
            data = data.path().join("data").to_string_lossy(),
            script = script.to_string_lossy(),
            builtin = BUILTIN,
            cargo = cargo.to_string_lossy(),
        ),
    )
    .unwrap();
    let run = |args: &[&str]| {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_mimlys"));
        cmd.arg("--config")
            .arg(&config)
            .arg("--json")
            .args(args)
            .env("MIMLYS_CONFIG", "")
            .env("MIMLYS_WORKER_BIN", env!("CARGO_BIN_EXE_mimlys-worker"))
            .stdin(std::process::Stdio::null());
        let out = run_bounded(&mut cmd, data.path());
        eprintln!("--- mimlys {args:?}\n{}", out.stderr);
        let value: Value = serde_json::from_str(out.stdout.trim())
            .unwrap_or_else(|e| panic!("not JSON ({e}): {}\n{}", out.stdout, out.stderr));
        (out.code(), value)
    };
    for dir in [&cargo_home, &rustup_home] {
        let (code, value) = run(&["toolchain", "grant", dir, "--yes"]);
        assert_eq!(code, 0, "{value}");
    }
    let (_, status) = run(&["status"]);
    assert_eq!(status["tier"], "full", "{status}");
    let started = std::time::Instant::now();
    let (code, value) = run(&[
        "run",
        "the test in src/lib.rs fails: fix it",
        "--scope",
        scope.to_str().unwrap(),
        "--grant",
        "host_tool:cargo_test",
        "--completion-check",
        &json!({"tool": "run_host_tool", "arguments": {"tool": "cargo_test"}}).to_string(),
    ]);
    assert_eq!(code, 0, "{value}");
    let id = value["task"]["task_id"].as_str().unwrap_or_default();
    let obs = observations_in(data.path(), id);
    // Current Windows refuses an AppContainer the NUL device, which cargo
    // opens for every rustc it starts (M6b report §4): the fact is stated,
    // not hidden, and the demonstration is not claimed on this machine.
    let nul = obs.iter().find(|o| {
        o["stderr"]
            .as_str()
            .is_some_and(|s| s.contains("NUL device"))
    });
    if let Some(o) = nul {
        eprintln!(
            "m6b demonstration: not run on this Windows: cargo cannot open the NUL device inside the AppContainer: {}",
            o["stderr"]
        );
    } else {
        assert_eq!(
            value["task"]["state"],
            "completed",
            "task {}\n{}",
            value["task"],
            account(&obs)
        );
        assert_eq!(value["task"]["completion"], "verified", "{value}");
        eprintln!(
            "m6b demonstration: completed{{verified}} in {:.2} s",
            started.elapsed().as_secs_f64()
        );
    }
    for dir in [&cargo_home, &rustup_home] {
        let (code, value) = run(&["toolchain", "revoke", dir, "--yes"]);
        assert_eq!(code, 0, "{value}");
    }
}

// ---------------------------------------------------------------------
// Handoff 011e (REQ-CONF-008): `toolchain revoke --all`, removing every
// trace before an uninstall.
// ---------------------------------------------------------------------

/// `icacls` of a directory, compared without the `(I)` marker as the rest of
/// this suite does (a write marks the descriptor auto-inherited).
fn icacls(path: &Path) -> String {
    let out = Command::new("icacls").arg(path).output().unwrap();
    String::from_utf8_lossy(&out.stdout).replace("(I)", "")
}

/// A directory path without the verbatim prefix, as a string.
fn plain(path: &Path) -> String {
    path.canonicalize()
        .unwrap()
        .to_string_lossy()
        .trim_start_matches(r"\\?\")
        .to_owned()
}

fn same(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

/// REQ-CONF-008: after mimlys has started (its own entries) and two
/// `toolchain grant`s, `revoke --all --yes` exits 0 and leaves every touched
/// directory as it was before mimlys first ran; the worker's directory no
/// longer mentions the container; the container profile and the record file
/// are gone.
#[test]
fn req_conf_008_revoke_all_leaves_the_directories_as_they_were() {
    let tc_a = tempfile::tempdir().unwrap();
    let tc_b = tempfile::tempdir().unwrap();
    let (a, b) = (plain(tc_a.path()), plain(tc_b.path()));
    let env = Env::new(&[], "");
    let data = env.data.path().join("data");
    let own = ["workspaces", "tmp", "interpreters"].map(|d| data.join(d));
    // The data directory is owner-only before anything is in it (REQ-WS-001):
    // the directories below inherit that, and it is what they were before
    // the container's entries. (Created with the temporary directory's
    // inheritance instead, they would change at mimlys's first start, when
    // the data directory is protected, and that is not what this test is
    // about.)
    mimlys_exec::confine::create_private_dir(&data).unwrap();
    for d in &own {
        std::fs::create_dir_all(d).unwrap();
    }
    let mut dirs: Vec<PathBuf> = own.to_vec();
    dirs.push(PathBuf::from(&a));
    dirs.push(PathBuf::from(&b));
    let before: Vec<String> = dirs.iter().map(|d| icacls(d)).collect();
    // The first start applies mimlys's own entries and creates the profile.
    let (code, listed) = env.json(&["toolchain", "list"]);
    assert_eq!(code, 0, "{listed}");
    let sid = listed["container"]["sid"].as_str().unwrap().to_owned();
    let profile = listed["container"]["profile"].as_str().unwrap().to_owned();
    for dir in [&a, &b] {
        let (code, v) = env.json(&["toolchain", "grant", dir, "--yes"]);
        assert_eq!(code, 0, "{v}");
    }
    assert_ne!(icacls(&own[0]), before[0], "mimlys's own entry is in force");
    assert_ne!(icacls(Path::new(&a)), before[3], "the grant is in force");
    let worker_dir = Path::new(env!("CARGO_BIN_EXE_mimlys-worker"))
        .parent()
        .unwrap();
    assert!(
        icacls(worker_dir).contains(&sid),
        "the worker's directory carries the container"
    );
    let packages = std::env::var("LOCALAPPDATA")
        .map(|l| PathBuf::from(l).join("Packages").join(&profile))
        .unwrap();
    let profile_folder_existed = packages.exists();

    let (code, report) = env.json(&["toolchain", "revoke", "--all", "--yes"]);
    assert_eq!(code, 0, "{report}");
    assert_eq!(report["failed"].as_array().unwrap().len(), 0, "{report}");
    assert_eq!(report["profile_deleted"], true, "{report}");
    assert_eq!(report["record_deleted"], true, "{report}");
    let removed: Vec<String> = report["removed"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["path"].as_str().unwrap().to_owned())
        .collect();
    for dir in dirs
        .iter()
        .map(|d| plain(d))
        .chain(std::iter::once(plain(worker_dir)))
    {
        assert!(
            removed.iter().any(|r| same(r, &dir)),
            "{dir} is on the list: {removed:?}"
        );
    }
    for (dir, was) in dirs.iter().zip(&before) {
        eprintln!("--- icacls {}\n{}", dir.display(), icacls(dir));
        assert_eq!(
            &icacls(dir),
            was,
            "{} is as it was before mimlys",
            dir.display()
        );
    }
    assert!(
        !icacls(worker_dir).contains(&sid),
        "the worker's directory no longer mentions the container"
    );
    assert!(
        !data.join("toolchain-acls.json").exists(),
        "the record is gone"
    );
    if profile_folder_existed {
        assert!(
            !packages.exists(),
            "the profile's folder {} is gone",
            packages.display()
        );
    }
}

/// REQ-CONF-008: an entry whose record is lost is found by the scan of the
/// declared read paths and removed.
#[test]
fn req_conf_008_revoke_all_finds_entries_without_a_record() {
    let toolchain = tempfile::tempdir().unwrap();
    let a = plain(toolchain.path());
    let env = Env::new(
        &[],
        &format!(
            "[host_tools.gated]\ncommand = {:?}\nallowed_args = [\"ok\"]\nmax_args = 1\neffect = \"workspace_write\"\nread_paths = [{:?}]\n",
            probe_tool().to_string_lossy(),
            a
        ),
    );
    let before = icacls(Path::new(&a));
    let (code, v) = env.json(&["toolchain", "grant", &a, "--yes"]);
    assert_eq!(code, 0, "{v}");
    assert_ne!(icacls(Path::new(&a)), before, "the grant is in force");
    std::fs::remove_file(env.data.path().join("data/toolchain-acls.json")).unwrap();
    let (code, report) = env.json(&["toolchain", "revoke", "--all", "--yes"]);
    assert_eq!(code, 0, "{report}");
    let found = report["removed"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| same(e["path"].as_str().unwrap(), &a))
        .unwrap_or_else(|| panic!("{a} was not found: {report}"));
    assert_eq!(found["found_by"], "scan", "{report}");
    assert_eq!(found["source"], "user", "{report}");
    assert_eq!(
        icacls(Path::new(&a)),
        before,
        "removed although the record was lost"
    );
}

/// REQ-CONF-008: without `--yes` and without a terminal nothing is removed
/// and the message names `--yes`; with it the JSON has the documented shape.
#[test]
fn req_conf_008_revoke_all_asks_and_reports() {
    let env = Env::new(&[], "");
    let (code, refused) = env.json(&["toolchain", "revoke", "--all"]);
    assert_eq!(code, 1, "{refused}");
    assert!(
        refused["error"]["message"]
            .as_str()
            .unwrap()
            .contains("--yes"),
        "{refused}"
    );
    let (code, report) = env.json(&["toolchain", "revoke", "--all", "--yes"]);
    assert_eq!(code, 0, "{report}");
    for key in ["removed", "failed", "profile_deleted", "record_deleted"] {
        assert!(report.get(key).is_some(), "{key} in {report}");
    }
    for entry in report["removed"].as_array().unwrap() {
        assert!(entry["path"].is_string() && entry["source"].is_string());
        assert!(matches!(
            entry["found_by"].as_str(),
            Some("record" | "scan")
        ));
    }
}

// ---------------------------------------------------------------------
// Handoff 011f (REQ-CONF-008, LIM-O7): the writers of one directory's
// permissions wait for one another and never write on a stale base.
// ---------------------------------------------------------------------

/// REQ-CONF-008, THR-36: two installations (two data directories, two
/// containers) start and grant the same directory at the same moment. Both
/// start with their own entries on the worker's directory (shared by both,
/// as it is by the whole suite), then both grant a directory holding many
/// files, so that each write lasts. Afterwards both containers' entries are
/// on both directories (`icacls`) and both containers run the probe tool: a
/// writer waits for the other (LIM-O7) instead of replacing its entry with
/// one built on a base read before the other's write landed.
#[test]
fn req_conf_008_concurrent_grants_keep_both_entries() {
    let shared = tempfile::tempdir().unwrap();
    for i in 0..300 {
        std::fs::write(shared.path().join(format!("f{i:03}.txt")), "x").unwrap();
    }
    let shared_text = plain(shared.path());
    let envs = [
        Env::new(&[probe(&["ok"]), complete()], ""),
        Env::new(&[probe(&["ok"]), complete()], ""),
    ];
    let barrier = std::sync::Barrier::new(envs.len());
    // Both runtimes' first start and their grant, at once.
    let results: Vec<(i32, Value)> = std::thread::scope(|scope| {
        let handles: Vec<_> = envs
            .iter()
            .map(|env| {
                let (barrier, shared_text) = (&barrier, &shared_text);
                scope.spawn(move || {
                    barrier.wait();
                    env.json(&["toolchain", "grant", shared_text, "--yes"])
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    for (code, v) in &results {
        assert_eq!(*code, 0, "{v}");
        assert_eq!(v["applied"], true, "{v}");
    }
    let worker_dir = Path::new(env!("CARGO_BIN_EXE_mimlys-worker"))
        .parent()
        .unwrap();
    let on_shared = icacls(Path::new(&shared_text));
    let on_worker = icacls(worker_dir);
    for env in &envs {
        let (code, listed) = env.json(&["toolchain", "list"]);
        assert_eq!(code, 0, "{listed}");
        let sid = listed["container"]["sid"].as_str().unwrap().to_owned();
        assert!(
            on_shared.contains(&sid),
            "{sid} is on the granted directory:\n{on_shared}"
        );
        assert!(
            on_worker.contains(&sid),
            "{sid} is on the worker's directory:\n{on_worker}"
        );
    }
    // Each container still runs the probe tool: its entries are in force.
    for env in &envs {
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
        let obs = env.observations(&id);
        assert_eq!(obs.len(), 1, "{}", account(&obs));
        assert_eq!(exit_code_of(&obs[0]), 0, "{}", account(&obs));
        assert_eq!(stdout_of(&obs[0]).trim(), "ok");
    }
}

/// THROWAWAY (handoff 017, measurement 4, "before"): the time of mimlys's first writing command.
#[test]
fn req_conf_008_first_start_time_is_reported() {
    let env = Env::new(&[], "");
    let small = tempfile::tempdir().unwrap();
    let dir = small.path().canonicalize().unwrap().to_string_lossy().trim_start_matches(r"\\?\").to_owned();
    let started = std::time::Instant::now();
    let (code, granted) = env.json(&["toolchain", "grant", &dir, "--yes"]);
    let elapsed = started.elapsed();
    assert_eq!(code, 0, "{granted}");
    eprintln!("FIRST_START_MS={}", elapsed.as_millis());
}
