//! The approval request as the user reads it from the command line
//! (REQ-APR-002): every argument is named on the card, and `task show
//! --arguments` and the JSON of `task show` give every argument in full,
//! sanitized, before the decision.

use serde_json::{Value, json};
use std::path::PathBuf;
use std::process::Command;

mod common;
use common::run_bounded;

const CONTENT_CHARS: usize = 5_800;

struct Env {
    data: tempfile::TempDir,
    config: PathBuf,
}

impl Env {
    /// A fake backend that proposes `write_file` with a long `content`, a
    /// profile that asks for approval of `write_file`.
    fn new() -> Self {
        let data = tempfile::tempdir().unwrap();
        let script = [
            json!({
                "decision": "act",
                "tool": "write_file",
                "arguments": {"path": "notes/out.txt", "content": format!("{}\u{202e}end", "c".repeat(CONTENT_CHARS))},
                "reason": "save the notes",
            }),
            json!({"decision": "complete", "summary": "done"}),
        ];
        let script_path = data.path().join("script.json");
        std::fs::write(&script_path, serde_json::to_string(&script).unwrap()).unwrap();
        let config = data.path().join("config.toml");
        std::fs::write(
            &config,
            format!(
                "config_version = 1\n[runtime]\ndata_dir = {:?}\n[backend]\nkind = \"fake\"\nscript = {:?}\n[policy.profiles.workspace]\napproval_tools = [\"write_file\"]\n",
                data.path().join("data").to_string_lossy(),
                script_path.to_string_lossy(),
            ),
        )
        .unwrap();
        Self { data, config }
    }

    fn run(&self, args: &[&str]) -> (i32, String, String) {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_mimlys"));
        cmd.arg("--config")
            .arg(&self.config)
            .args(args)
            .env("MIMLYS_CONFIG", "");
        let out = run_bounded(&mut cmd, self.data.path());
        (out.code(), out.stdout, out.stderr)
    }

    fn json(&self, args: &[&str]) -> Value {
        let mut all = vec!["--json"];
        all.extend_from_slice(args);
        let (code, stdout, stderr) = self.run(&all);
        assert_eq!(code, 0, "{stdout}\n{stderr}");
        serde_json::from_str(stdout.trim())
            .unwrap_or_else(|e| panic!("not JSON ({e}): {stdout}\n{stderr}"))
    }

    /// `run` without a terminal: it stops at the approval. Returns the task's id.
    fn waiting_task(&self) -> String {
        let scope = self.data.path().join("scope");
        std::fs::create_dir_all(&scope).unwrap();
        let (code, stdout, stderr) =
            self.run(&["run", "write it", "--scope", scope.to_str().unwrap()]);
        assert_eq!(code, 0, "{stdout}\n{stderr}");
        let tasks = self.json(&["task", "list"]);
        assert_eq!(tasks[0]["state"], "waiting_user");
        tasks[0]["task_id"].as_str().unwrap().to_owned()
    }
}

/// REQ-APR-002: the card names both arguments, the long one says how much of it
/// is cut, and `task show --arguments` prints both in full.
#[test]
fn req_apr_002_task_show_arguments_prints_every_argument_in_full() {
    let env = Env::new();
    let id = env.waiting_task();

    let (code, plain, stderr) = env.run(&["task", "show", &id]);
    assert_eq!(code, 0, "{stderr}");
    assert!(
        plain.contains("path = \"notes/out.txt\" (resolved: "),
        "{plain}"
    );
    assert!(plain.contains("content = \""), "{plain}");
    assert!(plain.contains(" more characters)"), "{plain}");
    assert!(
        !plain.contains(messages_heading()),
        "the arguments in full only when asked: {plain}"
    );
    assert!(!plain.contains(&"c".repeat(CONTENT_CHARS)), "{plain}");

    let (code, full, stderr) = env.run(&["task", "show", &id, "--arguments"]);
    assert_eq!(code, 0, "{stderr}");
    assert!(full.contains(messages_heading()), "{full}");
    assert!(
        full.contains(&format!(
            "  content = \"{}end\"\n",
            "c".repeat(CONTENT_CHARS)
        )),
        "the whole content, sanitized: {full}"
    );
    assert!(
        full.contains("  path = \"notes/out.txt\" (resolved: "),
        "{full}"
    );
    assert!(!full.contains('\u{202e}'), "sanitized");
}

/// REQ-APR-002: the JSON of `task show` carries the arguments in full, in the
/// order of the card, as an additive field.
#[test]
fn req_apr_002_the_json_of_task_show_carries_the_arguments() {
    let env = Env::new();
    let id = env.waiting_task();
    let view = env.json(&["task", "show", &id]);
    let arguments = view["pending_approval"]["arguments"].as_array().unwrap();
    let names: Vec<&str> = arguments
        .iter()
        .map(|a| a["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["path", "content"]);
    assert_eq!(
        arguments[1]["value"],
        format!("\"{}end\"", "c".repeat(CONTENT_CHARS))
    );
    assert!(
        arguments[0]["resolved"]
            .as_str()
            .unwrap()
            .ends_with("/notes/out.txt\"")
    );
    // The fields that were there are still there.
    assert!(view["pending_approval"]["rendered"]["lines"].is_array());
    assert_eq!(view["pending_approval"]["status"], "pending");
}

/// REQ-APR-002: with no action waiting, `--arguments` says so and exits 0.
#[test]
fn req_apr_002_task_show_arguments_without_a_pending_request_says_so() {
    let env = Env::new();
    let id = env.waiting_task();
    let rejected = env.json(&["reject", &id, "--halt", "--note", "no"]);
    assert_eq!(rejected["task"]["state"], "halted");
    let (code, shown, stderr) = env.run(&["task", "show", &id, "--arguments"]);
    assert_eq!(code, 0, "{stderr}");
    assert!(
        shown.contains("no action is waiting for your approval"),
        "{shown}"
    );
    let view = env.json(&["task", "show", &id]);
    assert!(view["pending_approval"].is_null());
}

fn messages_heading() -> &'static str {
    mimlys::messages::ARGUMENTS_IN_FULL
}
