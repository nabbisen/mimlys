//! Plan first through the binaries (REQ-TASK-015; handoff 015b): `run --plan-first` and `task new
//! --plan-first`, the plan waited for and shown, `approve [--note]` and `reject`, the gate across
//! processes (each command is a process, and the fake backend's script starts again in each),
//! `task show` of a plan-first task and of an ordinary one, the `run` prompt over the test
//! harness, and the list of the commands that meet a plan wait, derived from the command line's own
//! help so that a command that is added fails here until it is classified.

use serde_json::Value;
use std::path::{Path, PathBuf};

mod common;
use common::run_bounded;

fn script(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/plan_first")
        .join(name)
}

struct Env {
    registry: common::Registry,
    dir: tempfile::TempDir,
    scope: PathBuf,
}

impl Env {
    /// A store and a project of its own; the model says what `script` says, from its start, in
    /// every process.
    fn new(script_name: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let scope = dir.path().join("project");
        std::fs::create_dir_all(scope.join("src")).unwrap();
        std::fs::write(scope.join("src/lib.rs"), "fn a() {}\n").unwrap();
        let config = format!(
            "config_version = 1\n[runtime]\ndata_dir = {:?}\n[backend]\nkind = \"fake\"\nscript = {:?}\n",
            dir.path().join("data").to_string_lossy(),
            script(script_name).to_string_lossy()
        );
        std::fs::write(dir.path().join("config.toml"), config).unwrap();
        Self {
            registry: common::Registry::new(),
            dir,
            scope,
        }
    }

    fn run(&self, args: &[&str]) -> (i32, String, String) {
        let mut cmd = common::mimlys_in(&self.registry);
        cmd.arg("--config")
            .arg(self.dir.path().join("config.toml"))
            .args(args)
            .env("MIMLYS_CONFIG", "")
            .env("MIMLYS_WORKER_BIN", env!("CARGO_BIN_EXE_mimlys-worker"));
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

    /// `run --plan-first`: the task waits for its plan (no terminal here). Its id.
    fn plan_wait(&self) -> String {
        let scope = self.scope.to_string_lossy().into_owned();
        let (code, stdout, stderr) =
            self.run(&["run", "add a file", "--scope", &scope, "--plan-first"]);
        assert_eq!(code, 0, "{stdout}\n{stderr}");
        let (_, list) = self.json(&["task", "list"]);
        list[0]["task_id"].as_str().unwrap().to_owned()
    }

    fn show(&self, task: &str) -> Value {
        self.json(&["task", "show", task]).1
    }

    fn event_names(&self, task: &str) -> Vec<String> {
        let (_, events) = self.json(&["events", task]);
        events
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["record"]["event"].as_str().unwrap().to_owned())
            .collect()
    }
}

fn state_of(shown: &Value) -> String {
    let t = &shown["task"];
    match t["wait_kind"].as_str() {
        Some(kind) => format!("{}:{kind}", t["state"].as_str().unwrap()),
        None => t["state"].as_str().unwrap().to_owned(),
    }
}

/// REQ-TASK-015: `run --plan-first` creates the task, shows the plan as an approval is shown and
/// says how to decide, and exits as for a pending approval; nothing acted (the project is as it
/// was, no action is recorded); the record holds `plan_first` and no approved version.
#[test]
fn req_task_015_run_plan_first_waits_shows_the_plan_and_says_how_to_decide() {
    let env = Env::new("plan_then_edit.json");
    let scope = env.scope.to_string_lossy().into_owned();
    let (code, stdout, _) = env.run(&["run", "add a file", "--scope", &scope, "--plan-first"]);
    assert_eq!(code, 0, "exit as for a pending approval");
    assert!(stdout.contains("Plan to approve · task "), "{stdout}");
    assert!(stdout.contains("1. \"read src/lib.rs\""), "{stdout}");
    assert!(stdout.contains("2. \"write a new file\""), "{stdout}");
    assert!(
        stdout.contains("is waiting for your approval of its plan; run `mimlys task show"),
        "{stdout}"
    );
    assert!(stdout.contains("mimlys approve") && stdout.contains("[--note TEXT]"));
    assert!(stdout.contains("mimlys reject") && stdout.contains("--note TEXT"));
    assert!(
        stdout.contains("waiting for your approval of its plan"),
        "the state line"
    );
    let (_, list) = env.json(&["task", "list"]);
    let task = list[0]["task_id"].as_str().unwrap();
    let shown = env.show(task);
    assert_eq!(state_of(&shown), "waiting_user:plan");
    assert_eq!(shown["task"]["plan_first"], true);
    assert!(shown["task"].get("plan_approved").is_none());
    assert_eq!(
        shown["task"]["pending"],
        serde_json::json!({"pending": "plan", "version": 1})
    );
    assert_eq!(shown["plan"]["version"], 1);
    assert_eq!(shown["plan"]["steps"].as_array().unwrap().len(), 2);
    assert!(shown["plan"]["created_at"].is_string());
    let names = env.event_names(task);
    assert!(names.contains(&"plan.approval_requested".to_owned()));
    assert!(
        !names.iter().any(|n| n.starts_with("action.")),
        "nothing acted: {names:?}"
    );
    assert_eq!(
        std::fs::read_to_string(env.scope.join("src/lib.rs")).unwrap(),
        "fn a() {}\n"
    );
    assert!(!env.scope.join("src/new.rs").exists());
}

/// REQ-TASK-015: `task show` of a plan-first task, before and after the approval, in text and in
/// JSON; of an ordinary task it shows its plan, with no gate line and no `plan_first` key.
#[test]
fn req_task_015_task_show_before_and_after_the_approval() {
    let env = Env::new("plan_then_edit.json");
    let task = env.plan_wait();
    let (code, before, _) = env.run(&["task", "show", &task]);
    assert_eq!(code, 0);
    assert!(before.contains("plan      version 1, made "), "{before}");
    assert!(
        before.contains("gate      plan first; no plan is approved yet"),
        "{before}"
    );
    assert!(
        before.contains("pending   the plan above, version 1: `mimlys approve"),
        "{before}"
    );
    assert!(before.contains("state     waiting for your approval of its plan"));
    let (code, _, stderr) = env.run(&["approve", &task, "--note", "keep it small"]);
    assert_eq!(code, 0, "{stderr}");
    let (_, after_text, _) = env.run(&["task", "show", &task]);
    assert!(
        after_text.contains("gate      plan first; version 1 approved"),
        "{after_text}"
    );
    assert!(!after_text.contains("pending   the plan above"));
    let after = env.show(&task);
    assert_eq!(after["task"]["plan_first"], true);
    assert_eq!(after["task"]["plan_approved"], 1);
    assert_eq!(state_of(&after), "completed");
    // Plans after the approval are recorded and never wait: the fake's script began again in the
    // second process (a plan, a plan, the edit, the end), the latest plan is the third version.
    assert_eq!(after["plan"]["version"], 3);
    // The decision is counted, apart from the actions' approvals; the waits are events by name.
    let (_, metrics) = env.json(&["metrics", "--task", &task]);
    assert_eq!(
        metrics["plan_approvals_by_decision"],
        serde_json::json!({"approved": 1})
    );
    assert!(
        metrics["approvals_by_decision"]
            .as_object()
            .unwrap()
            .is_empty()
    );
    assert_eq!(metrics["events_by_name"]["plan.approval_requested"], 1);
    let (_, text, _) = env.run(&["metrics", "--task", &task]);
    assert!(
        text.contains("plan_approvals_by_decision.approved"),
        "{text}"
    );
    let names = env.event_names(&task);
    for name in ["plan.approval_requested", "plan.approval_decided"] {
        assert_eq!(names.iter().filter(|n| *n == name).count(), 1, "{names:?}");
    }
    assert!(env.scope.join("src/lib.rs").exists());
    // An ordinary task: its plan is shown, and nothing else of the gate.
    let ordinary = Env::new("plan_then_edit.json");
    let scope = ordinary.scope.to_string_lossy().into_owned();
    let (code, stdout, stderr) = ordinary.run(&["run", "add a file", "--scope", &scope]);
    assert_eq!(code, 0, "{stdout}\n{stderr}");
    let (_, list) = ordinary.json(&["task", "list"]);
    let id = list[0]["task_id"].as_str().unwrap();
    let (_, text, _) = ordinary.run(&["task", "show", id]);
    assert!(text.contains("plan      version 2, made "), "{text}");
    assert!(!text.contains("gate      "), "{text}");
    let shown = ordinary.show(id);
    assert!(shown["task"].get("plan_first").is_none(), "{shown}");
    assert!(shown["task"].get("plan_approved").is_none());
    assert_eq!(shown["plan"]["version"], 2);
    // And no key of the metrics that a store without a plan decision never had.
    let (_, metrics) = ordinary.json(&["metrics"]);
    assert!(
        metrics.get("plan_approvals_by_decision").is_none(),
        "{metrics}"
    );
}

/// REQ-TASK-015: a task without `--plan-first` has no `plan_first` key in its record and no key in
/// its creation event; with the flag, both.
#[test]
fn req_task_015_the_flag_is_in_the_record_and_the_event_only_when_given() {
    let env = Env::new("plan_then_edit.json");
    let scope = env.scope.to_string_lossy().into_owned();
    let (_, plain) = env.json(&["task", "new", "plain", "--scope", &scope, "--hold"]);
    assert!(plain["task"].get("plan_first").is_none(), "{plain}");
    let id = plain["task"]["task_id"].as_str().unwrap().to_owned();
    let (_, cancelled) = env.json(&["task", "cancel", &id]);
    assert_eq!(cancelled["task"]["state"], "cancelled");
    let (_, gated) = env.json(&[
        "task",
        "new",
        "gated",
        "--scope",
        &scope,
        "--hold",
        "--plan-first",
    ]);
    assert_eq!(gated["task"]["plan_first"], true, "{gated}");
    let gated_id = gated["task"]["task_id"].as_str().unwrap();
    let (_, events) = env.json(&["events", &id]);
    let created = |value: &Value| -> Value {
        value
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["record"]["event"] == "task.created")
            .unwrap()["record"]
            .clone()
    };
    assert!(created(&events).get("plan_first").is_none());
    let (_, events) = env.json(&["events", gated_id]);
    assert_eq!(created(&events)["plan_first"], true);
}

/// REQ-TASK-015: the gate is in the task's record: created in one process, resumed in another,
/// a first `act` is `plan_required` three times and the task halts `budget_rejections`; the events
/// carry the reason, the metrics count them under `state`, and nothing acted.
#[test]
fn req_task_015_an_act_first_is_rejected_across_processes_and_three_halt() {
    let env = Env::new("act_first.json");
    let scope = env.scope.to_string_lossy().into_owned();
    let (_, made) = env.json(&[
        "task",
        "new",
        "gated",
        "--scope",
        &scope,
        "--hold",
        "--plan-first",
    ]);
    let id = made["task"]["task_id"].as_str().unwrap().to_owned();
    let (code, stdout, stderr) = env.run(&["task", "resume", &id]);
    assert_eq!(code, 0, "{stdout}\n{stderr}");
    assert!(
        stdout.contains("reason=plan_required"),
        "the event says why: {stdout}"
    );
    let shown = env.show(&id);
    assert_eq!(shown["task"]["state"], "halted");
    assert_eq!(shown["task"]["halt_reason"], "budget_rejections");
    let (_, metrics) = env.json(&["metrics", "--task", &id]);
    assert_eq!(metrics["rejections_by_code"]["state"], 3, "{metrics}");
    let names = env.event_names(&id);
    assert!(!names.iter().any(|n| n.starts_with("action.")));
    assert!(!env.scope.join("src/new.rs").exists());
}

/// REQ-TASK-015: `reject --note` keeps the gate across processes: the plan is asked for again and
/// waits again (the next plan is the second version); `reject --halt` halts `approval_rejected`;
/// `task cancel` cancels from the wait.
#[test]
fn req_task_015_reject_keeps_the_gate_halt_and_cancel_end_the_wait() {
    let env = Env::new("plan_then_edit.json");
    let task = env.plan_wait();
    let (code, stdout, stderr) = env.run(&["reject", &task, "--note", "smaller"]);
    assert_eq!(code, 0, "{stdout}\n{stderr}");
    let shown = env.show(&task);
    assert_eq!(
        state_of(&shown),
        "waiting_user:plan",
        "the plan waits again"
    );
    assert_eq!(shown["task"]["pending"]["version"], 2);
    assert!(shown["task"].get("plan_approved").is_none());
    let names = env.event_names(&task);
    assert_eq!(
        names
            .iter()
            .filter(|n| *n == "plan.approval_requested")
            .count(),
        2
    );
    let (code, _, stderr) = env.run(&["reject", &task, "--halt"]);
    assert_eq!(code, 0, "{stderr}");
    let shown = env.show(&task);
    assert_eq!(shown["task"]["state"], "halted");
    assert_eq!(shown["task"]["halt_reason"], "approval_rejected");
    // A halted task is still gated.
    let (_, text, _) = env.run(&["task", "show", &task]);
    assert!(text.contains("gate      plan first; no plan is approved yet"));
    // Cancel from the wait.
    let env = Env::new("plan_then_edit.json");
    let task = env.plan_wait();
    let (code, _, stderr) = env.run(&["task", "cancel", &task]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(env.show(&task)["task"]["state"], "cancelled");
}

/// REQ-TASK-015: a note above the bound is refused with the reason and the task still waits; a
/// plan wait refuses `answer` with the state error.
#[test]
fn req_task_015_a_long_note_and_an_answer_are_refused_and_the_task_still_waits() {
    let env = Env::new("plan_then_edit.json");
    let task = env.plan_wait();
    let long = "x".repeat(mimlys_core::limits::LIM_D2_REASON_CHARS + 1);
    let (code, stdout, stderr) = env.run(&["--json", "approve", &task, "--note", &long]);
    assert_eq!(code, 1);
    let text = if stdout.trim().is_empty() {
        stderr
    } else {
        stdout
    };
    let error: Value = serde_json::from_str(text.trim()).unwrap();
    assert_eq!(error["error"]["kind"], "invalid_request", "{error}");
    let (code, _, stderr) = env.run(&["answer", &task, "yes"]);
    assert_eq!(code, 1);
    assert!(stderr.contains("error (state)"), "{stderr}");
    assert_eq!(state_of(&env.show(&task)), "waiting_user:plan");
}

/// The commands that meet a plan wait, derived from the command line's own help, each with what it
/// does (REQ-TASK-015; the list is in the request of handoff 015b): a command that is added is not
/// in the table, and fails here until someone says what it does to a task that waits for its plan.
#[test]
fn req_task_015_every_command_has_a_stated_meeting_with_a_plan_wait() {
    /// What a command does to a task that waits for its plan.
    enum Meets {
        /// It takes no task.
        NoTask,
        /// It decides the plan; the task goes on.
        Decides(&'static str),
        /// It acts on the task as it does on any waiting one; the wait stays, unless it ends it.
        Acts(&'static str, i32),
        /// It reads and changes nothing.
        Reads,
        /// It is refused, with this error kind; the wait stays.
        Refused(&'static str),
    }
    use Meets::*;
    let table: Vec<(&str, Meets)> = vec![
        ("run", NoTask),
        ("task new", NoTask),
        ("task show", Reads),
        ("task list", NoTask),
        ("task resume", Refused("state")),
        ("task pause", Refused("state")),
        ("task cancel", Acts("cancelled", 0)),
        ("task delete", Refused("state")),
        ("task prune", NoTask),
        ("task output", Refused("state")),
        ("task help", NoTask),
        ("approve", Decides("completed")),
        ("reject", Decides("waiting_user:plan")),
        ("answer", Refused("state")),
        ("grant", Acts("waiting_user:plan", 0)),
        ("revoke", Refused("not_found")),
        ("rollback", Acts("waiting_user:plan", 0)),
        ("promote", Reads),
        ("events", Reads),
        ("metrics", Reads),
        ("status", NoTask),
        ("check", NoTask),
        ("runtime", NoTask),
        ("model", NoTask),
        ("interpreters", NoTask),
        ("toolchain", NoTask),
        ("help", NoTask),
    ];
    // The commands of the help, and the subcommands of `task`.
    let env = Env::new("plan_then_edit.json");
    let names_of = |args: &[&str]| -> Vec<String> {
        let (_, help, _) = env.run(args);
        help.lines()
            .skip_while(|l| !l.starts_with("Commands:"))
            .skip(1)
            .take_while(|l| l.starts_with("  ") && !l.trim().is_empty())
            .filter_map(|l| l.split_whitespace().next().map(str::to_owned))
            .collect()
    };
    let mut found: Vec<String> = Vec::new();
    for name in names_of(&["--help"]) {
        if name == "task" {
            for sub in names_of(&["task", "--help"]) {
                found.push(format!("task {sub}"));
            }
        } else {
            found.push(name);
        }
    }
    assert!(found.len() > 15, "the help was read: {found:?}");
    let known: Vec<&str> = table.iter().map(|(n, _)| *n).collect();
    for name in &found {
        assert!(
            known.contains(&name.as_str()),
            "`{name}` is a command of the help and is not in the table: say what it does to a task that waits for its plan"
        );
    }
    for name in &known {
        assert!(
            found.iter().any(|f| f == name),
            "`{name}` is in the table and not in the help"
        );
    }
    // Each command that takes the task, on a task that waits for its plan, in a store of its own.
    for (name, meets) in &table {
        let invocation: Vec<String> = match *name {
            "task show" => vec!["task", "show", "{t}"],
            "task resume" => vec!["task", "resume", "{t}"],
            "task pause" => vec!["task", "pause", "{t}"],
            "task cancel" => vec!["task", "cancel", "{t}"],
            "task delete" => vec!["task", "delete", "{t}", "--yes"],
            "task output" => vec!["task", "output", "{t}"],
            "approve" => vec!["approve", "{t}"],
            "reject" => vec!["reject", "{t}"],
            "answer" => vec!["answer", "{t}", "yes"],
            "grant" => vec!["grant", "{t}", "tool", "read_file"],
            "revoke" => vec!["revoke", "{t}", "deadbeef"],
            "rollback" => vec!["rollback", "{t}"],
            "promote" => vec!["promote", "{t}", "--patch"],
            "events" => vec!["events", "{t}"],
            "metrics" => vec!["metrics", "--task", "{t}"],
            _ => continue,
        }
        .into_iter()
        .map(str::to_owned)
        .collect();
        let env = Env::new("plan_then_edit.json");
        let task = env.plan_wait();
        let before = state_of(&env.show(&task));
        assert_eq!(before, "waiting_user:plan");
        let args: Vec<String> = invocation.iter().map(|a| a.replace("{t}", &task)).collect();
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let (code, stdout, stderr) = env.run(&args);
        let after = state_of(&env.show(&task));
        match meets {
            NoTask => unreachable!(),
            Decides(state) => {
                assert_eq!(code, 0, "{name}: {stdout}\n{stderr}");
                assert_eq!(&after, state, "{name}");
            }
            Acts(state, exit) => {
                assert_eq!(code, *exit, "{name}: {stdout}\n{stderr}");
                assert_eq!(&after, state, "{name}");
            }
            Reads => {
                assert_eq!(code, 0, "{name}: {stdout}\n{stderr}");
                assert_eq!(after, before, "{name}");
            }
            Refused(kind) => {
                assert_eq!(code, 1, "{name}: {stdout}\n{stderr}");
                assert!(
                    stderr.contains(&format!("error ({kind})")),
                    "{name}: {stderr}"
                );
                assert_eq!(after, before, "{name} changed the wait");
            }
        }
    }
}

// The `run` prompt, over the test harness (`mimlys-testloop`): the fake user decides every plan
// with `--decide`.
#[cfg(feature = "test-bins")]
mod prompt {
    use super::*;

    fn testloop(script_name: &str, decide: &str) -> Value {
        let dir = tempfile::tempdir().unwrap();
        let scope = dir.path().join("project");
        std::fs::create_dir_all(scope.join("src")).unwrap();
        std::fs::write(scope.join("src/lib.rs"), "fn a() {}\n").unwrap();
        let mut cmd = common::mimlys_testloop();
        cmd.args(["--script"])
            .arg(script(script_name))
            .arg("--data-dir")
            .arg(dir.path().join("data"))
            .arg("--scope")
            .arg(&scope)
            .args(["--plan-first", "--decide", decide]);
        let out = run_bounded(&mut cmd, dir.path());
        assert!(out.status.success(), "{}", out.stderr);
        serde_json::from_str(&out.stdout).unwrap()
    }

    fn names(report: &Value) -> Vec<String> {
        report["events"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["name"].as_str().unwrap().to_owned())
            .collect()
    }

    /// REQ-TASK-015: `run --plan-first` at a prompt, the user approves: the task goes on and
    /// completes; one plan wait, one decision.
    #[test]
    fn req_task_015_the_run_prompt_approved() {
        let report = testloop("plan_then_edit.json", "approve");
        assert_eq!(report["state"]["state"], "completed", "{report}");
        assert_eq!(report["task"]["plan_approved"], 1);
        let names = names(&report);
        assert_eq!(
            names
                .iter()
                .filter(|n| *n == "plan.approval_requested")
                .count(),
            1
        );
        assert_eq!(
            names
                .iter()
                .filter(|n| *n == "plan.approval_decided")
                .count(),
            1
        );
    }

    /// REQ-TASK-015: at the prompt the user rejects every plan with a note: each is asked again
    /// and waits again, nothing acts, and the third rejection halts `budget_rejections`; the
    /// model's next request holds the note as `rejection_note`.
    #[test]
    fn req_task_015_the_run_prompt_rejected() {
        let report = testloop("plans.json", "reject");
        assert_eq!(report["state"]["state"], "halted", "{report}");
        assert_eq!(report["state"]["halt_reason"], "budget_rejections");
        let names = names(&report);
        assert_eq!(
            names
                .iter()
                .filter(|n| *n == "plan.approval_requested")
                .count(),
            3
        );
        assert!(!names.iter().any(|n| n.starts_with("action.")));
        assert!(report["task"].get("plan_approved").is_none());
        let feedback = report["feedback"].as_array().unwrap();
        assert!(
            feedback
                .iter()
                .flat_map(|f| f.as_array().unwrap())
                .any(|m| m.as_str().unwrap().contains("approval_rejected")),
            "the model was told: {feedback:?}"
        );
    }

    /// REQ-TASK-015: at the prompt the user rejects and halts: the task halts `approval_rejected`.
    #[test]
    fn req_task_015_the_run_prompt_halted() {
        let report = testloop("plans.json", "halt");
        assert_eq!(
            report["state"]["halt_reason"], "approval_rejected",
            "{report}"
        );
    }
}
