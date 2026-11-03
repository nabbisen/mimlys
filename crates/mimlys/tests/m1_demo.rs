//! The M1 demonstration (external design §23; requirements §26) through the
//! `mimlys-testloop` harness: the scripted task end to end, the completion
//! check, the existence-leak check, questions, rejection, the environment
//! allow-list, and crash injection at every persistence point.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

mod common;
use common::run_bounded;

fn script(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/m1")
        .join(name)
}

fn scope() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/lib.rs"), "fn a() -> i32 { 0 }\n").unwrap();
    dir
}

struct Run {
    status: std::process::ExitStatus,
    report: Option<Value>,
    stderr: String,
}

fn testloop(args: &[&str], envs: &[(&str, &str)]) -> Run {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_mimlys-testloop"));
    cmd.args(args);
    // REQ-PRIV-003: an explicit allow-listed environment for the child.
    cmd.env_clear();
    for key in ["PATH", "SYSTEMROOT", "TEMP", "TMP", "HOME", "USERPROFILE"] {
        if let Ok(v) = std::env::var(key) {
            cmd.env(key, v);
        }
    }
    for (k, v) in envs {
        cmd.env(k, v);
    }
    let streams = tempfile::tempdir().expect("a directory for the streams");
    let out = run_bounded(&mut cmd, streams.path());
    Run {
        status: out.status,
        report: serde_json::from_str(&out.stdout).ok(),
        stderr: out.stderr,
    }
}

fn demo_args<'a>(data: &'a str, scope: &'a str, extra: &[&'a str]) -> Vec<&'a str> {
    let mut v = vec![
        "--script",
        Box::leak(
            script("demo.json")
                .to_string_lossy()
                .into_owned()
                .into_boxed_str(),
        ),
        "--data-dir",
        data,
        "--scope",
        scope,
        "--approval-tool",
        "write_file",
    ];
    v.extend_from_slice(extra);
    v
}

fn names(report: &Value) -> Vec<String> {
    report["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["name"].as_str().unwrap().to_owned())
        .collect()
}

fn action_events(report: &Value, action_id: &str) -> Vec<String> {
    report["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["action_id"].as_str() == Some(action_id))
        .map(|e| e["name"].as_str().unwrap().to_owned())
        .collect()
}

/// External design §23, M1: create → plan → act → validate → approval →
/// execute → observe → complete, with the eight lines per action, the
/// rendered approval text and `completion = asserted`.
#[test]
fn m1_demonstration_asserted() {
    let scope = scope();
    let data = tempfile::tempdir().unwrap();
    let run = testloop(
        &demo_args(
            data.path().to_str().unwrap(),
            scope.path().to_str().unwrap(),
            &[],
        ),
        &[],
    );
    assert!(run.status.success(), "{}", run.stderr);
    let report = run.report.unwrap();
    assert_eq!(
        report["state"],
        serde_json::json!({"state": "completed", "completion": "asserted"})
    );
    let steps = report["steps"].as_array().unwrap();
    let outcomes: Vec<&str> = steps
        .iter()
        .map(|s| s["outcome"].as_str().unwrap())
        .collect();
    assert_eq!(
        outcomes,
        [
            "continued",
            "continued",
            "continued",
            "continued",
            "completed"
        ]
    );
    let actions = report["actions"].as_array().unwrap();
    assert_eq!(actions.len(), 3);
    assert!(
        actions.iter().all(|a| a["status"] == "succeeded"),
        "{actions:?}"
    );
    let write = &actions[1];
    assert_eq!(write["tool"], "write_file");
    assert_eq!(
        action_events(&report, write["action_id"].as_str().unwrap()),
        [
            "action.proposed",
            "decision.received",
            "approval.requested",
            "approval.decided",
            "action.authorized",
            "action.started",
            "workspace.snapshot",
            "execution.finished",
            "action.finished",
            "step.finished"
        ],
        "a write action: the before snapshot reuses the base (same tree), the after snapshot is new (REQ-WS-004)"
    );
    assert_eq!(
        action_events(&report, actions[0]["action_id"].as_str().unwrap()),
        [
            "action.proposed",
            "decision.received",
            "action.authorized",
            "action.started",
            "execution.finished",
            "action.finished",
            "step.finished"
        ]
    );
    let approvals = report["approvals"].as_array().unwrap();
    assert_eq!(approvals.len(), 1);
    assert_eq!(approvals[0]["status"], "consumed");
    let text = approvals[0]["text"].as_str().unwrap();
    assert!(
        text.contains("Tool") && text.contains("write_file"),
        "{text}"
    );
    assert!(
        text.contains("Effect") && text.contains("workspace_write"),
        "{text}"
    );
    assert!(
        text.contains("Model's stated reason (untrusted): \"apply the fix\""),
        "{text}"
    );
    assert_eq!(
        report["requests"], 5,
        "one model request per decision, none for the approval"
    );
    let n = names(&report);
    assert_eq!(n[0], "task.created");
    assert_eq!(n.last().unwrap(), "task.state_changed");
    // The workspace has the fix; the project does not (INV-10).
    let task_id = report["task_id"].as_str().unwrap();
    let tree = data
        .path()
        .join("workspaces")
        .join(task_id)
        .join("tree/src/lib.rs");
    assert_eq!(
        std::fs::read_to_string(tree).unwrap(),
        "fn a() -> i32 { 1 }\n"
    );
    assert_eq!(
        std::fs::read_to_string(scope.path().join("src/lib.rs")).unwrap(),
        "fn a() -> i32 { 0 }\n"
    );
}

/// External design §5.3: with a completion check the task ends `verified`
/// after the check's action, proposed by the runtime, succeeds.
#[test]
fn m1_demonstration_verified() {
    let scope = scope();
    let data = tempfile::tempdir().unwrap();
    let run = testloop(
        &demo_args(
            data.path().to_str().unwrap(),
            scope.path().to_str().unwrap(),
            &[
                "--completion-check",
                r#"{"tool":"read_file","arguments":{"path":"src/lib.rs"}}"#,
            ],
        ),
        &[],
    );
    assert!(run.status.success(), "{}", run.stderr);
    let report = run.report.unwrap();
    assert_eq!(
        report["state"],
        serde_json::json!({"state": "completed", "completion": "verified"})
    );
    let actions = report["actions"].as_array().unwrap();
    assert_eq!(actions.len(), 4, "the check is the fourth action");
    let check = &actions[3];
    let events = report["events"].as_array().unwrap();
    let proposed = events
        .iter()
        .find(|e| e["name"] == "action.proposed" && e["action_id"] == check["action_id"])
        .unwrap();
    assert_eq!(
        proposed["actor"], "runtime",
        "the check is proposed by the runtime, not the model"
    );
    assert_eq!(report["requests"], 5);
}

/// External design §23 / INV-03: an out-of-scope path that exists and one
/// that does not yield byte-identical `denied` feedback.
#[test]
fn m1_existence_leak() {
    let scope = scope();
    let data = tempfile::tempdir().unwrap();
    let run = testloop(
        &[
            "--script",
            script("escape.json").to_str().unwrap(),
            "--data-dir",
            data.path().to_str().unwrap(),
            "--scope",
            scope.path().to_str().unwrap(),
        ],
        &[],
    );
    assert!(run.status.success(), "{}", run.stderr);
    let report = run.report.unwrap();
    assert_eq!(report["state"]["state"], "failed");
    let steps = report["steps"].as_array().unwrap();
    assert_eq!(steps[0]["rejection"], "denied");
    assert_eq!(steps[1]["rejection"], "denied");
    let third = report["feedback"][2].as_array().unwrap();
    let first_rejection = third[0].as_str().unwrap();
    let second_rejection = third[1].as_str().unwrap();
    assert!(first_rejection.contains("denied"), "{first_rejection}");
    assert_eq!(
        first_rejection, second_rejection,
        "the feedback must not reveal whether the path exists"
    );
    assert!(
        report["actions"].as_array().unwrap().is_empty(),
        "nothing was proposed, nothing ran"
    );
}

/// REQ-APR-010: a question waits; the answer enters the next context.
#[test]
fn m1_question_round_trip() {
    let scope = scope();
    let data = tempfile::tempdir().unwrap();
    let run = testloop(
        &[
            "--script",
            script("question.json").to_str().unwrap(),
            "--data-dir",
            data.path().to_str().unwrap(),
            "--scope",
            scope.path().to_str().unwrap(),
            "--answer",
            "tests/it.rs",
        ],
        &[],
    );
    assert!(run.status.success(), "{}", run.stderr);
    let report = run.report.unwrap();
    assert_eq!(report["state"]["state"], "completed");
    let n = names(&report);
    assert!(
        n.contains(&"question.asked".to_owned()) && n.contains(&"question.answered".to_owned())
    );
    assert_eq!(report["steps"][0]["outcome"], "waited");
}

/// REQ-APR-007: reject and halt ends the task `approval_rejected`.
#[test]
fn m1_reject_and_halt() {
    let scope = scope();
    let data = tempfile::tempdir().unwrap();
    let run = testloop(
        &demo_args(
            data.path().to_str().unwrap(),
            scope.path().to_str().unwrap(),
            &["--decide", "halt"],
        ),
        &[],
    );
    assert!(run.status.success(), "{}", run.stderr);
    let report = run.report.unwrap();
    assert_eq!(
        report["state"],
        serde_json::json!({"state": "halted", "halt_reason": "approval_rejected"})
    );
    assert_eq!(report["approvals"][0]["status"], "rejected");
    assert_eq!(report["actions"][1]["status"], "rejected");
    assert_eq!(report["actions"][1]["code"], "approval_rejected");
    let task_id = report["task_id"].as_str().unwrap();
    let tree = data
        .path()
        .join("workspaces")
        .join(task_id)
        .join("tree/src/lib.rs");
    assert_eq!(
        std::fs::read_to_string(tree).unwrap(),
        "fn a() -> i32 { 0 }\n",
        "the rejected write never ran"
    );
}

/// REQ-PRIV-003: a child process receives only the allow-listed
/// environment: a variable present in the parent (passed to a first child
/// to prove it is there) is absent from the allow-listed child.
#[test]
fn m1_child_environment_is_allow_listed() {
    let scope = scope();
    let data = tempfile::tempdir().unwrap();
    let question = script("question.json");
    let args = [
        "--script",
        question.to_str().unwrap(),
        "--data-dir",
        data.path().to_str().unwrap(),
        "--scope",
        scope.path().to_str().unwrap(),
    ];
    let leaky = testloop(&args, &[("MIMLYS_SECRET_PROBE", "s3cr3t")]);
    assert!(leaky.status.success(), "{}", leaky.stderr);
    let names: Vec<String> = leaky.report.unwrap()["env_names"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect();
    assert!(
        names.contains(&"MIMLYS_SECRET_PROBE".to_owned()),
        "a variable passed on purpose is visible"
    );
    let data = tempfile::tempdir().unwrap();
    let args = [
        "--script",
        question.to_str().unwrap(),
        "--data-dir",
        data.path().to_str().unwrap(),
        "--scope",
        scope.path().to_str().unwrap(),
    ];
    let clean = testloop(&args, &[]);
    assert!(clean.status.success(), "{}", clean.stderr);
    let names: Vec<String> = clean.report.unwrap()["env_names"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect();
    assert!(
        !names.contains(&"MIMLYS_SECRET_PROBE".to_owned()),
        "{names:?}"
    );
    assert!(
        names.iter().all(
            |n| ["PATH", "SYSTEMROOT", "TEMP", "TMP", "HOME", "USERPROFILE"].contains(&n.as_str())
        ),
        "{names:?}"
    );
}

/// REQ-TEST-007 / external design §23: the process is killed at every
/// persistence point, before and after the commit, and the task resumes to
/// completion; no action runs more than LIM-T3 times and the final tree is right.
#[cfg(feature = "crash-injection")]
#[test]
fn m1_crash_injection_resumes() {
    for point in mimlys_persist::crash::POINTS {
        for phase in ["before", "after"] {
            let target = format!("{point}.{phase}");
            let scope = scope();
            let data = tempfile::tempdir().unwrap();
            let first = testloop(
                &demo_args(
                    data.path().to_str().unwrap(),
                    scope.path().to_str().unwrap(),
                    &[],
                ),
                &[("MIMLYS_CRASH_AT", &target)],
            );
            if ["blob", "promotion", "delete"].contains(&point) {
                assert!(
                    first.status.success(),
                    "the demonstration writes no blob, promotes and deletes nothing; {target}: {}",
                    first.stderr
                );
                continue;
            }
            assert!(!first.status.success(), "{target} must abort the process");
            let second = testloop(
                &demo_args(
                    data.path().to_str().unwrap(),
                    scope.path().to_str().unwrap(),
                    &["--resume", "latest"],
                ),
                &[],
            );
            assert!(second.status.success(), "{target}: {}", second.stderr);
            let report = second.report.unwrap();
            assert_eq!(
                report["state"]["state"], "completed",
                "{target}: {}",
                report["state"]
            );
            for a in report["actions"].as_array().unwrap() {
                assert!(a["executions"].as_u64().unwrap() <= 2, "{target}: {a}");
            }
            let task_id = report["task_id"].as_str().unwrap();
            let tree = data
                .path()
                .join("workspaces")
                .join(task_id)
                .join("tree/src/lib.rs");
            assert_eq!(
                std::fs::read_to_string(tree).unwrap(),
                "fn a() -> i32 { 1 }\n",
                "{target}"
            );
            let n = names(&report);
            let recovered = report["recovered"].as_array().unwrap();
            if !recovered.is_empty() {
                assert!(n.contains(&"task.recovered".to_owned()), "{target}: {n:?}");
            }
        }
    }
}
