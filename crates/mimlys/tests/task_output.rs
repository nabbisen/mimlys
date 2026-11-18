//! `mimlys task output` (handoff 004d; REQ-PERS-005; external design §19.2), end to end over a
//! store that a real run made: a WebAssembly module prints more than an excerpt holds, and the
//! command shows the whole copies: the index (text and JSON) without a flag, each flag on a
//! stream that was cut and on one that was not, the result, a carriage return made visible on
//! a terminal and exact through a pipe, the reader's independence of another runtime that holds
//! the store, and its refusals (an action that is not the task's, one that never ran, a copy that
//! was not kept or was removed). The external tool's result is tested in `m5_mcp.rs`.

use serde_json::{Value, json};
#[cfg(unix)]
use std::process::Stdio;

mod common;
use common::printer::*;

const COMPLAINT: &str = "a short complaint\n";

/// A task whose module printed `printed()` and a short complaint: the data directory (inside the
/// returned directory), the task's id and its action's id.
struct Run {
    dir: tempfile::TempDir,
    task: String,
    action: String,
}

impl Run {
    fn new(stdout: &str, stderr: &str) -> Self {
        let scope = scope(stdout, stderr);
        let dir = tempfile::tempdir().unwrap();
        let script = script(dir.path());
        let run = run_printer(&dir.path().join("data"), scope.path(), &script, &[], &[]);
        assert!(run.status.success(), "{}", run.stderr);
        let report = run.report.unwrap();
        Self {
            task: report["task_id"].as_str().unwrap().to_owned(),
            action: report["actions"][0]["action_id"]
                .as_str()
                .unwrap()
                .to_owned(),
            dir,
        }
    }

    fn data(&self) -> std::path::PathBuf {
        self.dir.path().join("data")
    }

    /// Exit code, standard output and standard error of `mimlys task output <task> …`.
    fn output(&self, rest: &[&str]) -> (i32, String, String) {
        let mut args = vec!["task", "output", self.task.as_str()];
        args.extend_from_slice(rest);
        mimlys(&self.data(), &args)
    }

    fn json(&self, rest: &[&str]) -> (i32, Value) {
        let mut args = vec!["--json", "task", "output", self.task.as_str()];
        args.extend_from_slice(rest);
        let (code, stdout, stderr) = mimlys(&self.data(), &args);
        let value = serde_json::from_str(stdout.trim())
            .unwrap_or_else(|e| panic!("not JSON ({e}): {stdout}\n{stderr}"));
        (code, value)
    }

    fn sql(&self, statement: &str) {
        let conn = rusqlite::Connection::open(self.data().join("mimlys.db")).unwrap();
        conn.execute_batch(statement).unwrap();
    }
}

/// REQ-PERS-005: without a flag, the index says what the action has: for the result and each
/// stream, its size, whether the model read a cut of it, and the flag that shows it. The stream
/// that was cut is the blob's size (the whole), not the excerpt's; the one that was not is the
/// excerpt's.
#[test]
fn req_pers_005_the_index_says_what_the_action_has_and_what_was_cut() {
    let run = Run::new(&printed(), COMPLAINT);
    let (code, stdout, stderr) = run.output(&[]);
    assert_eq!(code, 0, "{stdout}\n{stderr}");
    let line = |label: &str| -> String {
        stdout
            .lines()
            .find(|l| l.trim_start().starts_with(label))
            .unwrap_or_else(|| panic!("no `{label}` line in:\n{stdout}"))
            .to_owned()
    };
    assert!(
        stdout.lines().next().unwrap().contains("run_wasm"),
        "{stdout}"
    );
    assert!(stdout.contains(&run.action), "{stdout}");
    let whole = cleaned().len();
    assert!(
        line("standard output").contains(&format!("{whole} bytes"))
            && line("standard output").contains("the model read a cut of it")
            && line("standard output").contains("--stdout shows the whole"),
        "{stdout}"
    );
    assert!(
        line("standard error").contains(&format!("{} bytes", COMPLAINT.len()))
            && line("standard error").contains("the model read it whole"),
        "{stdout}"
    );
    assert!(
        line("result").contains("the model read it whole") && line("result").contains("--result"),
        "{stdout}"
    );

    let (code, index) = run.json(&[]);
    assert_eq!(code, 0, "{index}");
    assert_eq!(index["task_id"], run.task);
    assert_eq!(index["action_id"], run.action);
    assert_eq!(index["tool"], "run_wasm");
    assert_eq!(index["attempt"], 1);
    assert!(index["execution_id"].is_string());
    let copies = index["copies"].as_array().unwrap();
    let kinds: Vec<&str> = copies.iter().map(|c| c["kind"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["result", "stdout", "stderr"]);
    assert_eq!(
        copies[1],
        json!({"kind": "stdout", "bytes": whole, "cut": true, "source": "blob", "flag": "--stdout", "capture_cut": false})
    );
    assert_eq!(
        copies[2],
        json!({"kind": "stderr", "bytes": COMPLAINT.len(), "cut": false, "source": "excerpt", "flag": "--stderr", "capture_cut": false})
    );
    assert_eq!(copies[0]["cut"], false);
    assert_eq!(copies[0]["source"], "payload");
    assert_eq!(copies[0]["flag"], "--result");
}

/// REQ-PERS-005: each flag prints its copy alone, exactly as stored: `--stdout` the blob (the
/// whole cleaned and redacted text, a carriage return as printed) where the excerpt was a cut,
/// `--stderr` the excerpt where nothing was cut, `--result` the result as JSON. Piped, nothing is
/// added or changed.
#[test]
fn req_pers_005_each_flag_prints_its_copy_exactly_through_a_pipe() {
    let run = Run::new(&printed(), COMPLAINT);
    let (code, stdout, stderr) = run.output(&["--stdout"]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(stdout, cleaned(), "the whole text, byte for byte");
    assert!(
        stdout.contains("progress 50%\rprogress 100%"),
        "a carriage return is exact"
    );
    assert!(stderr.is_empty(), "{stderr}");
    // Not the excerpt the model read: the middle of the output is there.
    assert!(stdout.contains("line 0200 of the tool's output"));

    let (code, stdout, stderr) = run.output(&["--stderr"]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(stdout, COMPLAINT, "the excerpt, which is the whole");

    let (code, stdout, stderr) = run.output(&["--result"]);
    assert_eq!(code, 0, "{stderr}");
    let result: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(result["termination"], "exited", "{result}");
    assert!(stdout.ends_with("}\n"));

    // A stream that fits, in a task of its own: its excerpt exactly.
    let small = Run::new("hello\n", "");
    let (code, stdout, _) = small.output(&["--stdout"]);
    assert_eq!(code, 0);
    assert_eq!(stdout, "hello\n");
    let (code, stdout, _) = small.output(&["--stderr"]);
    assert_eq!(code, 0);
    assert_eq!(stdout, "", "an empty stream prints nothing");
    let (_, index) = small.json(&[]);
    assert_eq!(index["copies"][1]["cut"], false);
    assert_eq!(index["copies"][1]["source"], "excerpt");

    // --json with a flag: the copy with its kind, size and source.
    let (code, one) = run.json(&["--stdout"]);
    assert_eq!(code, 0, "{one}");
    assert_eq!(one["kind"], "stdout");
    assert_eq!(one["source"], "blob");
    assert_eq!(one["cut"], true);
    assert_eq!(one["bytes"], cleaned().len());
    assert_eq!(one["text"], cleaned());
    let (_, one) = run.json(&["--result"]);
    assert_eq!(one["kind"], "result");
    assert_eq!(one["result"]["termination"], "exited");
}

/// REQ-PERS-005: a user reads it while another runtime holds the store (the reader opens it
/// without its lock) and the command says nothing of a lock.
#[test]
fn req_pers_013_task_output_works_while_another_runtime_holds_the_store() {
    let run = Run::new(&printed(), COMPLAINT);
    let holder = mimlys_persist::open(&run.data()).expect("this test holds the store");
    let (code, stdout, stderr) = run.output(&["--stdout"]);
    assert_eq!(code, 0, "{stdout}\n{stderr}");
    assert_eq!(stdout, cleaned());
    assert!(!stderr.to_lowercase().contains("locked"), "{stderr}");
    let (code, stdout, stderr) = run.output(&[]);
    assert_eq!(code, 0, "{stdout}\n{stderr}");
    assert!(!stdout.to_lowercase().contains("locked"), "{stdout}");
    // The writer's command, by contrast, waits for the lock and refuses.
    let (code, _, stderr) = mimlys(&run.data(), &["task", "delete", &run.task, "--yes"]);
    assert_eq!(code, 1);
    assert!(stderr.contains("locked"), "{stderr}");
    drop(holder);
}

/// REQ-PERS-005: an action that is not the task's is `not_found`; a prefix too short or one
/// that fits several is a usage error of the request; the action can be named by id or prefix.
#[test]
fn req_pers_005_an_unknown_action_is_not_found_and_a_prefix_names_one() {
    let run = Run::new(&printed(), COMPLAINT);
    let unknown = "01a1eecb-0000-7000-8000-000000000000";
    let (code, error) = run.json(&[unknown, "--stdout"]);
    assert_eq!(code, 1, "{error}");
    assert_eq!(error["error"]["kind"], "not_found", "{error}");
    let (code, stdout, stderr) = run.output(&["zzzz"]);
    assert_eq!(code, 1, "{stdout}");
    assert!(
        stderr.contains("not_found") || stderr.contains("no action"),
        "{stderr}"
    );
    let (code, error) = run.json(&["abc"]);
    assert_eq!(code, 1);
    assert_eq!(error["error"]["kind"], "invalid_request", "{error}");
    // By id and by prefix, the same action.
    let (code, by_id) = run.json(&[&run.action, "--stdout"]);
    assert_eq!(code, 0, "{by_id}");
    let (code, by_prefix) = run.json(&[&run.action[..8], "--stdout"]);
    assert_eq!(code, 0, "{by_prefix}");
    assert_eq!(by_id, by_prefix);
    assert_eq!(by_id["action_id"], run.action);
    // An unknown task.
    let (code, stdout, stderr) = mimlys(
        &run.data(),
        &[
            "--json",
            "task",
            "output",
            "01a1eecb-0000-7000-8000-000000000001",
        ],
    );
    assert_eq!(code, 1, "{stdout}\n{stderr}");
    let error: Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(error["error"]["kind"], "not_found", "{error}");
    // Two flags are a usage error.
    let (code, _, _) = run.output(&["--stdout", "--stderr"]);
    assert_eq!(code, 2);
}

/// REQ-PERS-005: an action that never ran says so (kind `state`), by name and as the task's
/// latest: a task whose only action was rejected has no action that ran.
#[test]
fn req_pers_005_an_action_that_never_ran_says_so() {
    let scope = scope("hello\n", "");
    let dir = tempfile::tempdir().unwrap();
    let script = script(dir.path());
    // The profile asks for the user's approval of `run_wasm`, and the user rejects it.
    let run = run_printer(
        &dir.path().join("data"),
        scope.path(),
        &script,
        &["--approval-tool", "run_wasm", "--decide", "reject"],
        &[],
    );
    assert!(run.status.success(), "{}", run.stderr);
    let report = run.report.unwrap();
    let task = report["task_id"].as_str().unwrap().to_owned();
    let action = report["actions"][0]["action_id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(report["executions"].as_array().unwrap().is_empty());
    let data = dir.path().join("data");
    let (code, stdout, stderr) = mimlys(&data, &["--json", "task", "output", &task, &action]);
    assert_eq!(code, 1, "{stdout}\n{stderr}");
    let error: Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(error["error"]["kind"], "state", "{error}");
    let message = error["error"]["message"].as_str().unwrap();
    assert!(
        message.contains("never ran") && message.contains(&action),
        "{message}"
    );
    let (code, stdout, _) = mimlys(&data, &["--json", "task", "output", &task]);
    assert_eq!(code, 1, "{stdout}");
    let error: Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(error["error"]["kind"], "state", "{error}");
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap()
            .contains("no action that ran"),
        "{error}"
    );
}

/// REQ-PERS-005: a stream the model read a cut of, whose whole copy is not there, is said so and
/// not shown as if it were whole: a record from before the copies were kept (no blob named), and
/// one whose blob `task delete` removed. The index says it; the flag refuses (`not_found`); the
/// stream that was not cut still shows its excerpt.
#[test]
fn req_pers_005_a_cut_stream_without_a_whole_copy_is_never_shown_as_whole() {
    let run = Run::new(&printed(), COMPLAINT);
    // As 0.11 made it: the execution names no blob.
    run.sql("update executions set record = json_set(record, '$.stdout_blob', json('null'))");
    let (code, index) = run.json(&[]);
    assert_eq!(code, 0, "{index}");
    assert_eq!(index["copies"][1]["unavailable"], "not_kept", "{index}");
    assert_eq!(index["copies"][1]["cut"], true);
    assert!(index["copies"][1]["bytes"].is_null() && index["copies"][1]["source"].is_null());
    let (code, stdout, stderr) = run.output(&[]);
    assert_eq!(code, 0, "{stderr}");
    assert!(stdout.contains("no whole copy was kept"), "{stdout}");
    let (code, error) = run.json(&["--stdout"]);
    assert_eq!(code, 1, "{error}");
    assert_eq!(error["error"]["kind"], "not_found", "{error}");
    let (code, stdout, _) = run.output(&["--stderr"]);
    assert_eq!(code, 0);
    assert_eq!(stdout, COMPLAINT);

    // Its blob removed by `task delete`, which leaves the record: another task.
    let run = Run::new(&printed(), COMPLAINT);
    let (code, _, stderr) = mimlys(&run.data(), &["task", "delete", &run.task, "--yes"]);
    assert_eq!(code, 0, "{stderr}");
    assert!(blob_files(&run.data()).is_empty());
    let (_, index) = run.json(&[]);
    assert_eq!(index["copies"][1]["unavailable"], "removed", "{index}");
    let (code, error) = run.json(&["--stdout"]);
    assert_eq!(code, 1, "{error}");
    assert_eq!(error["error"]["kind"], "not_found", "{error}");
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap()
            .contains("task delete"),
        "{error}"
    );
}

/// REQ-PERS-005: the copy is the latest execution's. An action with a second attempt shows the
/// second's blob; the index says the attempt.
#[test]
fn req_pers_005_the_copy_is_the_latest_executions() {
    let run = Run::new(&printed(), COMPLAINT);
    let data = run.data();
    // A second execution of the action, as a retry records it: its own blob and observation.
    let conn = rusqlite::Connection::open(data.join("mimlys.db")).unwrap();
    let second_blob = "01a1eecb-1111-7000-8000-000000000002";
    let second_text = "the second attempt printed this\n";
    std::fs::write(data.join("blobs").join(second_blob), second_text).unwrap();
    conn.execute(
        "insert into blobs (blob_id, task_id, size, created_at) values (?1, ?2, ?3, '2026-11-18T00:00:00Z')",
        rusqlite::params![second_blob, run.task, second_text.len() as i64],
    )
    .unwrap();
    let record: String = conn
        .query_row("select record from executions", [], |r| r.get(0))
        .unwrap();
    let mut execution: Value = serde_json::from_str(&record).unwrap();
    execution["execution_id"] = json!("01a1eecb-2222-7000-8000-000000000003");
    execution["attempt"] = json!(2);
    execution["stdout_blob"] = json!(second_blob);
    conn.execute(
        "insert into executions (execution_id, action_id, attempt, termination, record, record_version) values (?1, ?2, 2, 'exited', ?3, 1)",
        rusqlite::params![
            execution["execution_id"].as_str().unwrap(),
            run.action,
            execution.to_string()
        ],
    )
    .unwrap();
    let record: String = conn
        .query_row("select record from observations", [], |r| r.get(0))
        .unwrap();
    let mut observation: Value = serde_json::from_str(&record).unwrap();
    observation["observation_id"] = json!("01a1eecb-3333-7000-8000-000000000004");
    conn.execute(
        "insert into observations (observation_id, action_id, created_at, record, record_version) values (?1, ?2, '2026-11-18T00:00:01Z', ?3, 1)",
        rusqlite::params![
            observation["observation_id"].as_str().unwrap(),
            run.action,
            observation.to_string()
        ],
    )
    .unwrap();
    drop(conn);
    let (code, stdout, stderr) = run.output(&["--stdout"]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(stdout, second_text);
    let (_, index) = run.json(&[]);
    assert_eq!(index["attempt"], 2);
    assert_eq!(
        index["execution_id"],
        "01a1eecb-2222-7000-8000-000000000003"
    );
}

/// REQ-PERS-005: on a terminal a carriage return that ends no line is shown as `\r`, so that no
/// text can hide what precedes it on its line, and through a pipe the same text is exact. The
/// terminal is a pseudo-terminal, which turns each line feed into a carriage return and a line
/// feed, so only the carriage returns the text had are told from its own by what follows them.
#[cfg(unix)]
#[test]
fn req_pers_005_a_carriage_return_is_shown_on_a_terminal_and_exact_in_a_pipe() {
    let hiding = "the password is hunter2\rnothing to see here    \nafter\n";
    let run = Run::new(hiding, "");
    // A pipe: exact.
    let (code, piped, stderr) = run.output(&["--stdout"]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(piped, hiding);
    // A terminal.
    let terminal = common::pty::Terminal::open();
    let mut cmd = mimlys_command(&run.data(), &["task", "output", &run.task, "--stdout"]);
    cmd.stdout(Stdio::from(terminal.slave.try_clone().unwrap()));
    cmd.stdin(Stdio::null());
    let status = cmd.status().unwrap();
    assert!(status.success());
    // The reader thread has what the child wrote once it has the last line.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while !terminal.text().contains("after") && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let shown = terminal.text();
    assert!(
        shown.contains("the password is hunter2\\rnothing to see here"),
        "the carriage return is shown as `\\r`: {shown:?}"
    );
    assert!(
        !shown.contains("hunter2\rnothing"),
        "no raw carriage return hides the line: {shown:?}"
    );
    assert!(
        shown.ends_with("after\r\n"),
        "the last line is ended: {shown:?}"
    );
}

/// A pipe closed by its reader ends the output quietly (it has what it wanted): the exit code is
/// 0 and nothing is said, as for any command that prints text.
#[cfg(unix)]
#[test]
fn req_pers_005_a_closed_pipe_is_not_an_error() {
    let run = Run::new(&printed(), COMPLAINT);
    let mut cmd = mimlys_command(&run.data(), &["task", "output", &run.task, "--stdout"]);
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().unwrap();
    // Read nothing and close: the child writes into a pipe whose reader is gone.
    drop(child.stdout.take());
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "{:?}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        out.stderr.is_empty(),
        "{:?}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// REQ-CLI-007 (handoff 004d): the command's help, the command line reference and the book say
/// the same of it: each carries the phrases that state what the command does, so that none of
/// them can drift from the others without this failing.
#[test]
fn req_cli_007_the_help_the_reference_and_the_book_say_the_same_of_task_output() {
    let docs = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/src");
    if !docs.join("SUMMARY.md").is_file() {
        common::skip(
            "req_cli_007_the_help_the_reference_and_the_book_say_the_same_of_task_output",
            "the book is not in this source tree",
        );
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let (code, help, stderr) = mimlys(&data, &["task", "output", "--help"]);
    assert_eq!(code, 0, "{stderr}");
    let help = help.split_whitespace().collect::<Vec<_>>().join(" ");
    let page = |name: &str| -> String {
        std::fs::read_to_string(docs.join(name))
            .unwrap()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    };
    let reference = page("reference/cli.md");
    let book = page("how-a-task-runs.md");
    for phrase in [
        "latest action that ran",
        "that of its latest execution",
        "carriage return that ends no line is shown as `\\r`",
        "no text can hide what precedes it on its line",
        "works while another mimlys process runs a task",
        "printed more than mimlys keeps",
        "--stdout",
        "--stderr",
        "--result",
    ] {
        for (name, text) in [("help", &help), ("reference", &reference), ("book", &book)] {
            assert!(text.contains(phrase), "the {name} lacks `{phrase}`");
        }
    }
}
