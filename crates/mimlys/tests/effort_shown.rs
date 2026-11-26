//! What the user is shown of a level (handoff 015c, item 5; REQ-TASK-013, REQ-OBS-010; external
//! design §19.4, "Shown"): the level line of `run` and `task new`, what `task show` says a task
//! cost, the levels in `status` with `not configured` where one would be refused, and `by_effort`
//! in `metrics`; each in its human form and in `--json`, for a task at each level.

use mimlys_model::mock::{MockConfig, MockServer, Reply, UsageStyle};
use serde_json::{Value, json};

mod common;
use common::run_bounded;

struct Env {
    registry: common::Registry,
    dir: tempfile::TempDir,
    config: std::path::PathBuf,
}

impl Env {
    fn new(config: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        std::fs::write(
            &config_path,
            // The path is written into a TOML string: a Windows path's backslashes are escaped.
            config.replace(
                "@DATA@",
                &dir.path()
                    .join("data")
                    .to_string_lossy()
                    .replace('\\', "\\\\"),
            ),
        )
        .unwrap();
        Self {
            registry: common::Registry::new(),
            dir,
            config: config_path,
        }
    }

    fn run(&self, args: &[&str]) -> (i32, String, String) {
        let mut cmd = common::mimlys_in(&self.registry);
        cmd.arg("--config").arg(&self.config).args(args);
        cmd.env("MIMLYS_CONFIG", "");
        cmd.env("MIMLYS_WORKER_BIN", env!("CARGO_BIN_EXE_mimlys-worker"));
        let out = run_bounded(&mut cmd, self.dir.path());
        (out.code(), out.stdout, out.stderr)
    }

    fn json(&self, args: &[&str]) -> Value {
        let mut all = vec!["--json"];
        all.extend_from_slice(args);
        let (code, stdout, stderr) = self.run(&all);
        assert_eq!(code, 0, "{stderr}");
        serde_json::from_str(stdout.trim())
            .unwrap_or_else(|e| panic!("not JSON ({e}): {stdout}\n{stderr}"))
    }
}

fn scope(name: &str) -> tempfile::TempDir {
    let dir = tempfile::Builder::new().prefix(name).tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "a\n").unwrap();
    dir
}

/// An engine that serves `tasks` tasks: each reads `a.txt`, then completes.
fn engine(tasks: usize) -> MockServer {
    let task = || {
        vec![
            Reply::Decision(
                json!({"decision": "act", "tool": "read_file", "arguments": {"path": "a.txt"}}),
            ),
            Reply::Decision(json!({"decision": "complete", "summary": "read it"})),
        ]
    };
    MockServer::start(
        MockConfig {
            usage: UsageStyle::OpenAi,
            ..MockConfig::default()
        },
        (0..tasks).flat_map(|_| task()).collect(),
    )
}

fn config(default: &MockServer, deep: &MockServer) -> String {
    format!(
        "config_version = 2\n[runtime]\ndata_dir = \"@DATA@\"\n\
         [backends.default]\nkind = \"openai_compatible\"\nurl = {:?}\nmodel = \"small\"\n\
         [backends.big]\nkind = \"openai_compatible\"\nurl = {:?}\nmodel = \"large\"\n",
        default.url(),
        deep.url()
    )
}

/// REQ-TASK-013: `run` and `task new` print the level first: who chose it and what the task runs
/// with, the backend and its model included; a level that is the default only because
/// `effort_max` is below `standard` says what sets it.
#[test]
fn req_task_013_run_and_task_new_print_the_level_first() {
    let (a, b) = (engine(2), engine(2));
    let env = Env::new(&config(&a, &b));
    let scope = scope("shown-run");
    let scope = scope.path().to_str().unwrap();
    let (code, stdout, stderr) =
        env.run(&["run", "read it", "--scope", scope, "--effort", "quick"]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(
        stdout.lines().next().unwrap(),
        "effort    quick (you chose): steps 22, deadline 10m, backend default (small)",
        "{stdout}"
    );
    let scope2 = self::scope("shown-new");
    let (code, stdout, _) = env.run(&[
        "task",
        "new",
        "x",
        "--hold",
        "--scope",
        scope2.path().to_str().unwrap(),
    ]);
    assert_eq!(code, 0);
    assert_eq!(
        stdout.lines().next().unwrap(),
        "effort    standard (default): steps 40, deadline 2h, backend default (small)",
        "{stdout}"
    );

    // `effort_max` below `standard`: the default is the bound, and the line says what sets it.
    let env = Env::new(&format!(
        "{}[policy.profiles.workspace]\neffort_max = \"quick\"\n",
        config(&a, &b)
    ));
    let scope3 = self::scope("shown-max");
    let (code, stdout, stderr) = env.run(&[
        "task",
        "new",
        "x",
        "--hold",
        "--scope",
        scope3.path().to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(
        stdout.lines().next().unwrap(),
        "effort    quick (default, the most [policy.profiles.workspace] effort_max allows): steps 22, deadline 10m, backend default (small)",
        "{stdout}"
    );
}

/// REQ-OBS-010 (external design §19.4): `task show` answers what a task cost: the level, who
/// chose it, the steps of the envelope, the time against the deadline, the model's time, the
/// tokens where reported, and the backend and model of every step; at each level, human and JSON.
#[test]
fn req_obs_010_task_show_says_what_each_task_cost() {
    let (a, b) = (engine(2), engine(2));
    let env = Env::new(&format!(
        "{}[effort.deep]\nbackend = \"big\"\nsteps = 50\ndeadline = \"3h\"\n",
        config(&a, &b)
    ));
    let mut ids = Vec::new();
    for (level, backend, model, steps, deadline_ms) in [
        ("quick", "default", "small", 22, 600_000),
        ("standard", "default", "small", 40, 7_200_000),
        ("deep", "big", "large", 50, 10_800_000),
    ] {
        let scope = scope("shown-cost");
        let run = env.json(&[
            "run",
            "read it",
            "--scope",
            scope.path().to_str().unwrap(),
            "--effort",
            level,
        ]);
        assert_eq!(run["task"]["state"], "completed", "{run}");
        let id = run["task"]["task_id"].as_str().unwrap().to_owned();
        assert_eq!(run["effort"]["level"], level);

        let shown = env.json(&["task", "show", &id]);
        assert_eq!(shown["effort"]["level"], level);
        assert_eq!(shown["effort"]["chosen_by"], "user");
        assert_eq!(shown["effort"]["envelope"]["backend"], backend);
        assert_eq!(shown["effort"]["envelope"]["steps"], steps);
        assert_eq!(shown["effort"]["envelope"]["deadline_ms"], deadline_ms);
        let cost = &shown["cost"];
        assert_eq!(cost["steps_used"], 1, "{shown}");
        // The step that completes the task is decided and not counted against the budget.
        assert_eq!(cost["steps_decided"], 2, "{shown}");
        // Nothing of the level records a context budget; the task's is its backend's.
        assert!(
            shown["effort"]["envelope"].get("context_chars").is_none(),
            "{shown}"
        );
        assert_eq!(cost["steps_envelope"], steps);
        assert_eq!(cost["steps_limit"], steps);
        assert_eq!(cost["deadline_envelope_ms"], deadline_ms);
        assert!(cost["elapsed_ms"].as_i64().unwrap() >= 0);
        assert!(cost["prompt_tokens"].as_u64().unwrap() > 0, "{cost}");
        assert!(cost["completion_tokens"].as_u64().is_some(), "{cost}");
        assert_eq!(
            cost["backends"],
            json!([{"backend": backend, "model": model, "steps": 2}]),
            "{cost}"
        );

        // The words of the human form.
        let (code, stdout, _) = env.run(&["task", "show", &id]);
        assert_eq!(code, 0);
        assert!(
            stdout.contains(&format!(
                "effort    {level} (you chose); envelope: steps {steps}"
            )),
            "{stdout}"
        );
        assert!(
            stdout.contains(&format!(
                "cost      2 steps decided; 1 of {steps} counted against the budget; "
            )),
            "{stdout}"
        );
        assert!(
            stdout.contains("context   the backend's (32000 characters)"),
            "{stdout}"
        );
        assert!(stdout.contains("model time"), "{stdout}");
        assert!(stdout.contains("tokens    prompt "), "{stdout}");
        assert!(
            stdout.contains(&format!("backends  {backend} ({model}): 2 steps")),
            "{stdout}"
        );
        ids.push((level, id));
    }

    // `metrics`: by level, over every task, and for one task its own level.
    let all = env.json(&["metrics"]);
    for level in ["quick", "standard", "deep"] {
        let at = &all["by_effort"][level];
        assert_eq!(at["tasks_by_state"]["completed"], 1, "{level}: {all}");
        assert_eq!(at["steps"], 2, "{level}");
        assert!(at["prompt_tokens"].as_u64().unwrap() > 0, "{level}");
        assert!(at["model_ms"].as_u64().is_some(), "{level}");
    }
    let (level, id) = &ids[2];
    let one = env.json(&["metrics", "--task", id]);
    let by = one["by_effort"].as_object().unwrap();
    assert_eq!(by.keys().collect::<Vec<_>>(), [level], "{one}");
    let (_, stdout, _) = env.run(&["metrics"]);
    assert!(stdout.contains("by_effort.deep.steps"), "{stdout}");
    assert!(
        stdout.contains("by_effort.quick.tasks_by_state.completed"),
        "{stdout}"
    );
}

/// REQ-TASK-013 (external design §19.4): `status` lists the three levels, each with its backend and
/// its keys, and `not configured` for the level a task could not be created at: `deep`, until it
/// is configured; and the level that the default profile's `effort_max` forbids says so.
#[test]
fn req_task_013_status_lists_the_levels_and_what_would_be_refused() {
    let (a, b) = (engine(2), engine(2));
    let env = Env::new(&config(&a, &b));
    let (code, stdout, stderr) = env.run(&["status"]);
    assert_eq!(code, 0, "{stderr}");
    let line = |level: &str| {
        stdout
            .lines()
            .find(|l| l.starts_with(&format!("effort    {level}")))
            .unwrap_or_else(|| panic!("no line of {level}: {stdout}"))
            .to_owned()
    };
    assert_eq!(
        line("quick"),
        "effort    quick     backend default (small); steps 22, deadline 10m"
    );
    assert_eq!(
        line("standard"),
        "effort    standard  backend default (small); no key of its own: the profile's budgets"
    );
    assert!(line("deep").contains("not configured"), "{}", line("deep"));
    assert!(line("deep").contains("[effort.deep]"), "{}", line("deep"));
    let status = env.json(&["status"]);
    let levels = status["effort"].as_array().unwrap();
    assert_eq!(levels.len(), 3);
    assert_eq!(levels[0]["level"], "quick");
    assert_eq!(levels[0]["configured"], true);
    assert_eq!(levels[0]["steps"], 22);
    assert_eq!(levels[0]["deadline_ms"], 600_000);
    assert_eq!(levels[1]["configured"], true);
    assert_eq!(levels[2]["level"], "deep");
    assert_eq!(levels[2]["configured"], false);

    // Configured, and above the default profile's `effort_max`.
    let env = Env::new(&format!(
        "{}[effort.deep]\nbackend = \"big\"\nrecent_steps = 3\n[policy.ceiling]\neffort_max = \"standard\"\n",
        config(&a, &b)
    ));
    let (_, stdout, _) = env.run(&["status"]);
    let deep = stdout
        .lines()
        .find(|l| l.starts_with("effort    deep"))
        .unwrap();
    assert!(
        deep.contains("backend big (large)") && deep.contains("recent_steps 3"),
        "{deep}"
    );
    assert!(
        deep.contains("refused for the default profile")
            && deep.contains("[policy.ceiling] effort_max"),
        "{deep}"
    );
    let status = env.json(&["status"]);
    assert_eq!(status["effort"][2]["configured"], true);
    assert_eq!(status["effort"][2]["allowed"], false);
    assert_eq!(status["effort"][2]["backend"], "big");
    assert_eq!(status["effort"][2]["model"], "large");
}
