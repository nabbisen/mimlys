//! Effort levels through the binary (handoff 015c; REQ-TASK-013, REQ-CFG-007, REQ-MDL-010,
//! REQ-SEC-010; external design §19.4, §20): `--effort` and what it is refused with, the record of
//! the level, a task on a backend of its own that a later process finds by its name, the backend
//! that is removed from the file, and a command that runs no task opening no connection to a
//! backend. Two mock servers stand for two engines; each test decides by what each received.

use mimlys_model::mock::{MockConfig, MockServer, Reply};
use serde_json::{Value, json};

mod common;
use common::run_bounded;

struct Env {
    dir: tempfile::TempDir,
    config: std::path::PathBuf,
}

impl Env {
    fn new(config: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let env = Self {
            config: dir.path().join("config.toml"),
            dir,
        };
        env.write(config);
        env
    }

    /// Writes the file again: the edit of a user between two commands.
    fn write(&self, config: &str) {
        std::fs::write(
            &self.config,
            // The path is written into a TOML string: a Windows path's backslashes are escaped.
            config.replace(
                "@DATA@",
                &self
                    .dir
                    .path()
                    .join("data")
                    .to_string_lossy()
                    .replace('\\', "\\\\"),
            ),
        )
        .unwrap();
    }

    fn run(&self, args: &[&str]) -> (i32, String, String) {
        let mut cmd = common::mimlys();
        cmd.arg("--config").arg(&self.config).args(args);
        cmd.env("MIMLYS_CONFIG", "");
        cmd.env("MIMLYS_WORKER_BIN", env!("CARGO_BIN_EXE_mimlys-worker"));
        let out = run_bounded(&mut cmd, self.dir.path());
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

    fn task_count(&self) -> usize {
        let (_, tasks) = self.json(&["task", "list"]);
        tasks.as_array().unwrap().len()
    }

    fn created_events(&self) -> usize {
        let (_, events) = self.json(&["events"]);
        events
            .as_array()
            .map(|a| {
                a.iter()
                    .filter(|e| e["record"]["event"] == "task.created")
                    .count()
            })
            .unwrap_or(0)
    }
}

/// A scope with one file.
fn scope() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "a\n").unwrap();
    dir
}

/// Reads `a.txt`, then completes.
fn script() -> Vec<Reply> {
    vec![
        Reply::Decision(
            json!({"decision": "act", "tool": "read_file", "arguments": {"path": "a.txt"}}),
        ),
        Reply::Decision(json!({"decision": "complete", "summary": "read it"})),
    ]
}

fn engine() -> MockServer {
    MockServer::start(MockConfig::default(), script())
}

fn backend(name: &str, server: &MockServer, model: &str) -> String {
    format!(
        "[backends.{name}]\nkind = \"openai_compatible\"\nurl = {:?}\nmodel = {model:?}\n",
        server.url()
    )
}

/// A version-2 file: `default` on `default_engine`, `deep` on `deep_engine` and the level that
/// names it, then `extra`.
fn two_engines(default_engine: &MockServer, deep_engine: &MockServer, extra: &str) -> String {
    format!(
        "config_version = 2\n[runtime]\ndata_dir = \"@DATA@\"\n{}{}[effort.deep]\nbackend = \"deep\"\n{extra}",
        backend("default", default_engine, "small"),
        backend("deep", deep_engine, "large"),
    )
}

fn decisions(server: &MockServer) -> usize {
    server.decision_requests().len()
}

/// REQ-TASK-013: a name that is not a level is a usage error, exit 2, on `run` and on `task new`.
#[test]
fn req_task_013_an_unknown_level_is_a_usage_error() {
    let a = engine();
    let env = Env::new(&two_engines(&a, &a, ""));
    let scope = scope();
    let scope = scope.path().to_str().unwrap();
    for args in [
        vec!["run", "x", "--scope", scope, "--effort", "turbo"],
        vec!["task", "new", "x", "--scope", scope, "--effort", "Quick"],
    ] {
        let (code, _, stderr) = env.run(&args);
        assert_eq!(code, 2, "{stderr}");
        assert!(
            stderr.contains("quick") && stderr.contains("deep"),
            "{stderr}"
        );
    }
    assert_eq!(env.task_count(), 0);
}

/// REQ-TASK-013 (external design §20): the three refusals at creation. Each exits 1, names what to
/// set, and makes nothing: no task, no `task.created` event.
#[test]
fn req_task_013_each_refusal_at_creation_makes_nothing() {
    let a = engine();
    let b = engine();
    let scope = scope();
    let scope = scope.path().to_str().unwrap();

    // `deep` with no key, shipped or configured.
    let env = Env::new(&format!(
        "config_version = 2\n[runtime]\ndata_dir = \"@DATA@\"\n{}",
        backend("default", &a, "small")
    ));
    for args in [
        vec!["run", "x", "--scope", scope, "--effort", "deep"],
        vec!["task", "new", "x", "--scope", scope, "--effort", "deep"],
    ] {
        let (code, _, stderr) = env.run(&args);
        assert_eq!(code, 1, "{stderr}");
        assert!(
            stderr.contains("[effort.deep]") && stderr.contains("backend"),
            "{stderr}"
        );
    }
    assert_eq!((env.task_count(), env.created_events()), (0, 0));
    assert_eq!(decisions(&a), 0);

    // A level above `effort_max`: the ceiling's, then the profile's.
    let env = Env::new(&two_engines(
        &a,
        &b,
        "[policy.ceiling]\neffort_max = \"standard\"\n",
    ));
    let (code, _, stderr) = env.run(&["run", "x", "--scope", scope, "--effort", "deep"]);
    assert_eq!(code, 1, "{stderr}");
    assert!(stderr.contains("[policy.ceiling] effort_max"), "{stderr}");
    let env = Env::new(&two_engines(
        &a,
        &b,
        "[policy.profiles.workspace]\neffort_max = \"quick\"\n",
    ));
    let (code, _, stderr) =
        env.run(&["task", "new", "x", "--scope", scope, "--effort", "standard"]);
    assert_eq!(code, 1, "{stderr}");
    assert!(
        stderr.contains("[policy.profiles.workspace] effort_max"),
        "{stderr}"
    );
    assert_eq!((env.task_count(), env.created_events()), (0, 0));

    // A level whose backend does not answer: nothing is listening on the port.
    let env = Env::new(&format!(
        "config_version = 2\n[runtime]\ndata_dir = \"@DATA@\"\n{}[backends.deep]\nkind = \"openai_compatible\"\nurl = \"http://127.0.0.1:9/v1\"\nmodel = \"large\"\n[effort.deep]\nbackend = \"deep\"\n",
        backend("default", &a, "small")
    ));
    let (code, _, stderr) = env.run(&["run", "x", "--scope", scope, "--effort", "deep"]);
    assert_eq!(code, 1, "{stderr}");
    assert!(
        stderr.contains("`deep`") && stderr.contains("does not answer"),
        "{stderr}"
    );
    assert_eq!((env.task_count(), env.created_events()), (0, 0));
    assert_eq!(decisions(&a), 0, "the default backend was not asked either");
}

/// REQ-TASK-013, REQ-PERS-015: a task at `deep` sends its requests to the deep engine and none to
/// `default`'s; its record has the level, who chose it and the envelope, and the creation event
/// has the same; a task with no flag is `standard` by default on `default`.
#[test]
fn req_task_013_a_deep_task_runs_on_its_own_backend_and_records_its_envelope() {
    let default_engine = engine();
    let deep_engine = engine();
    let env = Env::new(&two_engines(&default_engine, &deep_engine, ""));
    let scope = scope();
    let scope = scope.path().to_str().unwrap();

    let (code, run) = env.json(&["run", "read it", "--scope", scope, "--effort", "deep"]);
    assert_eq!(code, 0, "{run}");
    assert_eq!(run["task"]["state"], "completed");
    assert!(decisions(&deep_engine) >= 2, "{}", decisions(&deep_engine));
    assert_eq!(decisions(&default_engine), 0, "none to the default backend");
    let effort = &run["task"]["effort"];
    assert_eq!(effort["level"], "deep", "{run}");
    assert_eq!(effort["chosen_by"], "user");
    assert_eq!(effort["envelope"]["backend"], "deep");
    assert_eq!(effort["envelope"]["steps"], 40);
    assert_eq!(effort["envelope"]["recent_steps"], 6);
    assert_eq!(run["task"]["versions"]["backend"]["model"], "large");
    let id = run["task"]["task_id"].as_str().unwrap().to_owned();
    let (_, events) = env.json(&["events", &id]);
    let created = events
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["record"]["event"] == "task.created")
        .unwrap();
    assert_eq!(created["record"]["effort"], *effort);

    // No flag: `standard`, by default, on the backend named `default`.
    let before = decisions(&deep_engine);
    let (code, run) = env.json(&["run", "read it", "--scope", scope]);
    assert_eq!(code, 0, "{run}");
    assert_eq!(run["task"]["effort"]["level"], "standard");
    assert_eq!(run["task"]["effort"]["chosen_by"], "default");
    assert_eq!(run["task"]["effort"]["envelope"]["backend"], "default");
    assert!(decisions(&default_engine) >= 2);
    assert_eq!(decisions(&deep_engine), before, "none to the deep backend");
}

/// REQ-TASK-013: `quick` ships its two keys: 22 steps and 10 minutes; the flags win over them.
#[test]
fn req_task_013_quick_ships_its_keys_and_a_flag_wins() {
    let a = engine();
    let env = Env::new(&two_engines(&a, &a, ""));
    let scope = scope();
    let scope = scope.path().to_str().unwrap();
    let (code, run) = env.json(&[
        "task", "new", "x", "--hold", "--scope", scope, "--effort", "quick",
    ]);
    assert_eq!(code, 0, "{run}");
    let envelope = &run["task"]["effort"]["envelope"];
    assert_eq!(envelope["steps"], 22);
    assert_eq!(envelope["deadline_ms"], 600_000);
    assert_eq!(run["task"]["budgets"]["steps"], 22);

    let scope2 = self::scope();
    let (code, run) = env.json(&[
        "task",
        "new",
        "x",
        "--hold",
        "--scope",
        scope2.path().to_str().unwrap(),
        "--effort",
        "quick",
        "--steps",
        "9",
        "--deadline",
        "3m",
    ]);
    assert_eq!(code, 0, "{run}");
    let envelope = &run["task"]["effort"]["envelope"];
    assert_eq!(envelope["steps"], 9, "the flag over the level");
    assert_eq!(envelope["deadline_ms"], 180_000);
    assert_eq!(run["task"]["budgets"]["steps"], 9);
}

/// REQ-MDL-010 (external design §20): a task made in one process is started by another, which
/// resolves its backend by its name again; with the backend removed from the file the start is
/// refused naming the backend and the configured ones, the task stays as it was, and nothing is
/// sent anywhere.
#[test]
fn req_mdl_010_a_later_process_finds_the_backend_by_name_and_a_removed_one_refuses() {
    let default_engine = engine();
    let deep_engine = engine();
    let config = two_engines(&default_engine, &deep_engine, "");
    let env = Env::new(&config);
    let scope = scope();
    let scope = scope.path().to_str().unwrap();
    let (code, made) = env.json(&[
        "task", "new", "x", "--hold", "--scope", scope, "--effort", "deep",
    ]);
    assert_eq!(code, 0, "{made}");
    let id = made["task"]["task_id"].as_str().unwrap().to_owned();
    assert_eq!(decisions(&deep_engine) + decisions(&default_engine), 0);

    // The file loses the backend and its level: the start is refused, and the task stays.
    let without = format!(
        "config_version = 2\n[runtime]\ndata_dir = \"@DATA@\"\n{}",
        backend("default", &default_engine, "small")
    );
    env.write(&without);
    let (_, events_before) = env.json(&["events", &id]);
    let (code, _, stderr) = env.run(&["task", "resume", &id]);
    assert_eq!(code, 1, "{stderr}");
    assert!(
        stderr.contains("`deep`") && stderr.contains("configured: `default`"),
        "{stderr}"
    );
    let (_, shown) = env.json(&["task", "show", &id]);
    assert_eq!(shown["task"]["state"], "created");
    let (_, events_after) = env.json(&["events", &id]);
    assert_eq!(events_after, events_before, "no event was written");
    assert_eq!(decisions(&deep_engine) + decisions(&default_engine), 0);

    // The file has it again: another process finds it by its name, and the task runs there.
    env.write(&config);
    let (code, run) = env.json(&["task", "resume", &id]);
    assert_eq!(code, 0, "{run}");
    assert_eq!(run["task"]["state"], "completed");
    assert!(decisions(&deep_engine) >= 2);
    assert_eq!(decisions(&default_engine), 0);
}

/// REQ-MDL-010: a command that runs no task opens no connection to a backend: the engines see
/// nothing from `task list`, `task show`, `events`, `metrics` and `check`.
#[test]
fn req_mdl_010_a_command_that_runs_no_task_contacts_no_backend() {
    let default_engine = engine();
    let deep_engine = engine();
    let env = Env::new(&two_engines(&default_engine, &deep_engine, ""));
    let scope = scope();
    let (code, made) = env.json(&[
        "task",
        "new",
        "x",
        "--hold",
        "--scope",
        scope.path().to_str().unwrap(),
        "--effort",
        "deep",
    ]);
    assert_eq!(code, 0, "{made}");
    // Creating asked the engine of the task's backend, to probe it, and no other.
    assert_eq!(default_engine.requests().len(), 0, "the default engine");
    let deep_before = deep_engine.requests().len();
    assert!(deep_before > 0, "the deep engine was probed");
    let id = made["task"]["task_id"].as_str().unwrap().to_owned();
    for args in [
        vec!["task", "list"],
        vec!["task", "show", id.as_str()],
        vec!["events"],
        vec!["metrics"],
        vec!["check"],
    ] {
        let (code, _, stderr) = env.run(&args);
        assert_eq!(code, 0, "{args:?}: {stderr}");
    }
    assert_eq!(
        default_engine.requests().len(),
        0,
        "no connection to default"
    );
    assert_eq!(deep_engine.requests().len(), deep_before, "none to deep");
}

/// REQ-SEC-010 (external design §20): a task whose backend is remote at its next start, under a
/// profile without `external`, halts `policy` naming that backend and the profile, and no request
/// is sent. A level is never allowed to change where a task's data goes: the file that makes it
/// so (every level remote, on one endpoint) is the user's edit.
#[test]
fn req_sec_010_a_task_whose_level_backend_turned_remote_halts_policy() {
    let default_engine = engine();
    let deep_engine = engine();
    let config = two_engines(&default_engine, &deep_engine, "");
    let env = Env::new(&config);
    let scope = scope();
    let (code, made) = env.json(&[
        "task",
        "new",
        "x",
        "--hold",
        "--scope",
        scope.path().to_str().unwrap(),
        "--effort",
        "deep",
    ]);
    assert_eq!(code, 0, "{made}");
    let id = made["task"]["task_id"].as_str().unwrap().to_owned();
    // The edit: both backends are one remote endpoint now (a tunnel), and the profile still has
    // `external = false`.
    let tunnel = |name: &str| {
        format!(
            "[backends.{name}]\nkind = \"openai_compatible\"\nurl = {:?}\nmodel = \"m\"\nlocality = \"remote\"\n",
            deep_engine.url()
        )
    };
    env.write(&format!(
        "config_version = 2\n[runtime]\ndata_dir = \"@DATA@\"\n[policy]\nremote_backend_allowed = true\n{}{}[effort.deep]\nbackend = \"deep\"\n",
        tunnel("default"),
        tunnel("deep")
    ));
    let before = decisions(&deep_engine);
    let (code, run) = env.json(&["task", "resume", &id]);
    assert_eq!(code, 0, "{run}");
    assert_eq!(run["task"]["state"], "halted", "{run}");
    assert_eq!(run["task"]["halt_reason"], "policy");
    assert_eq!(
        run["task"]["halt_detail"]["rule"],
        "remote_backend_forbidden"
    );
    assert_eq!(run["task"]["halt_detail"]["backend"], "deep");
    assert_eq!(run["task"]["halt_detail"]["profile"], "workspace");
    assert_eq!(decisions(&deep_engine), before, "nothing was sent");
    assert_eq!(decisions(&default_engine), 0);
}

/// REQ-TASK-013: a valid configuration of 0.12 (version 1, one `[backend]`, no level) runs at
/// `standard` exactly as before: the same requests, byte for byte, with the flag and without it.
#[test]
fn req_task_013_a_configuration_of_0_12_runs_at_standard_as_before() {
    let requests = |flag: &[&str]| {
        let server = engine();
        let env = Env::new(&format!(
            "config_version = 1\n[runtime]\ndata_dir = \"@DATA@\"\n[backend]\nkind = \"openai_compatible\"\nurl = {:?}\nmodel = \"small\"\n",
            server.url()
        ));
        let scope = scope();
        let mut args = vec!["run", "read it", "--scope", scope.path().to_str().unwrap()];
        args.extend_from_slice(flag);
        let (code, run) = env.json(&args);
        assert_eq!(code, 0, "{run}");
        assert_eq!(run["task"]["effort"]["level"], "standard");
        let id = run["task"]["task_id"].as_str().unwrap().to_owned();
        let scope_root = std::fs::canonicalize(scope.path()).unwrap();
        server
            .decision_requests()
            .into_iter()
            .map(|r| {
                // The task's own id and workspace are the only words that differ between runs.
                r.body
                    .to_string()
                    .replace(&id, "<task>")
                    .replace(&scope_root.to_string_lossy().to_string(), "<scope>")
            })
            .collect::<Vec<_>>()
    };
    let without = requests(&[]);
    assert!(without.len() >= 2);
    assert_eq!(requests(&["--effort", "standard"]), without);
}

/// REQ-MDL-010 (external design §20): a backend that serves another model under the same name
/// goes on, with a notice at the start that names both models; each step records the model it was
/// decided with.
#[test]
fn req_mdl_010_a_model_changed_under_the_same_name_is_told_at_the_start() {
    let default_engine = engine();
    let deep_engine = engine();
    let env = Env::new(&two_engines(&default_engine, &deep_engine, ""));
    let scope = scope();
    let (code, made) = env.json(&[
        "task",
        "new",
        "x",
        "--hold",
        "--scope",
        scope.path().to_str().unwrap(),
        "--effort",
        "deep",
    ]);
    assert_eq!(code, 0, "{made}");
    let id = made["task"]["task_id"].as_str().unwrap().to_owned();
    // The file names another model for the backend: `large` is `large-2` now.
    env.write(&two_engines(&default_engine, &deep_engine, "").replace("large", "large-2"));
    let (code, _, stderr) = env.run(&["task", "resume", &id]);
    assert_eq!(code, 0, "{stderr}");
    assert!(
        stderr.contains("`large-2`") && stderr.contains("`large`") && stderr.contains("notice"),
        "{stderr}"
    );
    let (_, shown) = env.json(&["task", "show", &id]);
    assert_eq!(shown["task"]["state"], "completed");
}
