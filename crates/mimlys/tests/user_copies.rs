//! The user's copy of a stream (handoff 004d; REQ-PERS-005), end to end through the
//! `mimlys-testloop` harness and a real worker: a WebAssembly module prints more than an
//! excerpt holds, with a configured secret in the middle, a carriage return and an escape
//! character; the blob that holds the whole text exists on disk, is named by the execution,
//! holds the cleaned and redacted text and no secret; a module whose output fits writes none;
//! a process that ends between the blob and the execution's record leaves a file that the next
//! start removes, and a record that is consistent.

use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;

mod common;
use common::run_bounded;

const SECRET: &str = "sk-live-0123456789abcdef";

/// What the module prints: far more than an excerpt (LIM-C2) holds, a secret in the middle, a
/// rewritten line (a carriage return that ends no line) and an escape sequence.
fn printed() -> String {
    let mut text = String::new();
    for n in 0..800 {
        text.push_str(&format!("line {n:04} of the tool's output\n"));
        if n == 400 {
            text.push_str(&format!("the token is {SECRET} here\n"));
            text.push_str("progress 50%\rprogress 100%\n");
            text.push_str("\u{1b}[31mred\u{1b}[0m\n");
        }
    }
    text
}

/// What the printed text is after the cleaning and the redaction.
fn cleaned() -> String {
    printed()
        .replace('\u{1b}', "")
        .replace(SECRET, "[redacted:TOKEN]")
}

/// A WebAssembly module (text) that writes `stdout` to fd 1 and `stderr` to fd 2.
fn printer(stdout: &str, stderr: &str) -> Vec<u8> {
    let escape = |text: &str| -> String { text.bytes().map(|b| format!("\\{b:02x}")).collect() };
    let at = stdout.len() as u32 + 16;
    let wat = format!(
        r#"(module
        (import "wasi_snapshot_preview1" "fd_write" (func $fd_write (param i32 i32 i32 i32) (result i32)))
        (memory 1) (export "memory" (memory 0))
        (data (i32.const 0) "{out}")
        (data (i32.const {at}) "{err}")
        (func (export "_start")
            (i32.store (i32.const 60000) (i32.const 0)) (i32.store (i32.const 60004) (i32.const {out_len}))
            (drop (call $fd_write (i32.const 1) (i32.const 60000) (i32.const 1) (i32.const 60100)))
            (i32.store (i32.const 60000) (i32.const {at})) (i32.store (i32.const 60004) (i32.const {err_len}))
            (drop (call $fd_write (i32.const 2) (i32.const 60000) (i32.const 1) (i32.const 60100)))))"#,
        out = escape(stdout),
        err = escape(stderr),
        out_len = stdout.len(),
        err_len = stderr.len(),
    );
    wat::parse_str(wat).unwrap()
}

fn scope(stdout: &str, stderr: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("tools")).unwrap();
    std::fs::write(dir.path().join("tools/print.wasm"), printer(stdout, stderr)).unwrap();
    dir
}

fn script(dir: &Path) -> PathBuf {
    let path = dir.join("script.json");
    let steps = json!([
        {"decision": "act", "tool": "run_wasm",
         "arguments": {"module": "tools/print.wasm", "mode": "read_only"}},
        {"decision": "complete", "summary": "printed"}
    ]);
    std::fs::write(&path, steps.to_string()).unwrap();
    path
}

struct Run {
    status: std::process::ExitStatus,
    report: Option<Value>,
    stderr: String,
}

fn testloop(args: &[&str], envs: &[(&str, &str)]) -> Run {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_mimlys-testloop"));
    cmd.args(args);
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

fn run_printer(
    data: &Path,
    scope: &Path,
    script: &Path,
    extra: &[&str],
    envs: &[(&str, &str)],
) -> Run {
    let mut args = vec![
        "--script",
        script.to_str().unwrap(),
        "--data-dir",
        data.to_str().unwrap(),
        "--scope",
        scope.to_str().unwrap(),
        "--secret",
        "TOKEN=sk-live-0123456789abcdef",
    ];
    args.extend_from_slice(extra);
    testloop(&args, envs)
}

fn blob_files(data: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(data.join("blobs"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

fn blob_rows(data: &Path) -> Vec<String> {
    let conn = rusqlite::Connection::open(data.join("mimlys.db")).unwrap();
    let mut stmt = conn
        .prepare("select blob_id from blobs order by blob_id")
        .unwrap();
    stmt.query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

/// REQ-PERS-005: the whole text of a stream whose excerpt is a cut of it is a blob: its file
/// exists, the execution names it, and it holds the cleaned and redacted text (a carriage return
/// as printed, no escape character, no secret), where the observation holds the excerpt. The
/// stream that fits (standard error) has none.
#[test]
fn req_pers_005_a_module_whose_output_exceeds_its_excerpt_leaves_a_blob_with_the_whole_text() {
    let scope = scope(&printed(), "a short complaint\n");
    let data = tempfile::tempdir().unwrap();
    let script = script(data.path());
    let run = run_printer(&data.path().join("data"), scope.path(), &script, &[], &[]);
    assert!(run.status.success(), "{}", run.stderr);
    let report = run.report.unwrap();
    assert_eq!(report["state"]["state"], "completed", "{}", report["state"]);

    let execution = &report["executions"][0];
    let blob = execution["stdout_blob"].as_str().expect("a blob is named");
    assert!(
        execution["stderr_blob"].is_null(),
        "a short stream writes none"
    );
    let file = data.path().join("data/blobs").join(blob);
    let stored = std::fs::read_to_string(&file).unwrap();
    assert_eq!(stored, cleaned());
    assert!(!stored.contains(SECRET), "a secret never reaches the disk");
    assert!(stored.contains("progress 50%\rprogress 100%"));
    assert!(!stored.contains('\u{1b}'));
    // The model's copy is the excerpt, a cut of it, and the notice says so.
    let observation = &report["observations"][0];
    let excerpt = observation["stdout"].as_str().unwrap();
    assert!(excerpt.chars().count() < stored.chars().count());
    assert!(!excerpt.contains("line 0200"));
    assert!(
        observation["notices"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n["code"] == "output_truncated" && n["stream"] == "stdout")
    );
    assert_eq!(observation["stderr"], "a short complaint\n");
    // One file, one row, and the row is the file's.
    assert_eq!(blob_files(&data.path().join("data")), [blob.to_owned()]);
    assert_eq!(blob_rows(&data.path().join("data")), [blob.to_owned()]);
}

/// REQ-PERS-005: a module whose output fits its excerpt writes no blob.
#[test]
fn req_pers_005_a_module_whose_output_fits_writes_no_blob() {
    let scope = scope("hello\n", "");
    let data = tempfile::tempdir().unwrap();
    let script = script(data.path());
    let run = run_printer(&data.path().join("data"), scope.path(), &script, &[], &[]);
    assert!(run.status.success(), "{}", run.stderr);
    let report = run.report.unwrap();
    let execution = &report["executions"][0];
    assert!(execution["stdout_blob"].is_null() && execution["stderr_blob"].is_null());
    assert!(blob_files(&data.path().join("data")).is_empty());
    assert!(blob_rows(&data.path().join("data")).is_empty());
    assert_eq!(report["observations"][0]["stdout"], "hello\n");
}

/// REQ-PERS-005, REQ-TEST-007: the process ends at each point between the blob's file and the
/// record of the execution that names it. The next start removes the file that has no row (and
/// only that), the task resumes to its end, and every file of `blobs` has a row and every
/// execution's blob exists. A row whose execution never came stays, the task's, until the
/// task is deleted.
#[cfg(feature = "crash-injection")]
#[test]
fn req_test_007_a_crash_between_the_blob_and_the_executions_record_leaves_a_consistent_store() {
    // Per point: whether the file is there without a row after the crash.
    for (target, file_without_row) in [
        ("blob.file", true),
        ("blob.before", true),
        ("blob.after", false),
        ("execution.before", false),
    ] {
        let scope = scope(&printed(), "");
        let dir = tempfile::tempdir().unwrap();
        let script = script(dir.path());
        let data = dir.path().join("data");
        let crashed = run_printer(
            &data,
            scope.path(),
            &script,
            &[],
            &[("MIMLYS_CRASH_AT", target)],
        );
        assert!(!crashed.status.success(), "{target} must abort the process");
        let left = blob_files(&data);
        let rows = blob_rows(&data);
        assert_eq!(left.len(), 1, "{target}: the blob's file was written");
        assert_eq!(
            rows.is_empty(),
            file_without_row,
            "{target}: a file without a row, or a row"
        );
        // The next start (a writer) sweeps, and the task resumes.
        let resumed = run_printer(&data, scope.path(), &script, &["--resume", "latest"], &[]);
        assert!(resumed.status.success(), "{target}: {}", resumed.stderr);
        let report = resumed.report.unwrap();
        assert_eq!(report["state"]["state"], "completed", "{target}");
        let rows = blob_rows(&data);
        let files = blob_files(&data);
        assert_eq!(
            files, rows,
            "{target}: every file of blobs has a row, and every row a file"
        );
        if file_without_row {
            assert!(
                !files.contains(&left[0]),
                "{target}: the file without a row was removed"
            );
        } else {
            assert!(
                files.contains(&left[0]),
                "{target}: a blob with a row is the task's and stays"
            );
        }
        for execution in report["executions"].as_array().unwrap() {
            if let Some(blob) = execution["stdout_blob"].as_str() {
                assert!(files.contains(&blob.to_owned()), "{target}: {blob} exists");
            }
        }
        assert!(
            report["executions"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["stdout_blob"].is_string()),
            "{target}: the completed execution names its blob"
        );
    }
}

/// The command line over the data directory a run of the harness made: a configuration that
/// names it, and no backend (the commands here run no step).
fn mimlys(data: &Path, args: &[&str]) -> (i32, String, String) {
    let config = data.parent().unwrap().join("config.toml");
    std::fs::write(
        &config,
        format!(
            "config_version = 1\n[runtime]\ndata_dir = {:?}\n[backend]\nkind = \"fake\"\n",
            data.to_string_lossy()
        ),
    )
    .unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_mimlys"));
    cmd.arg("--config")
        .arg(&config)
        .args(args)
        .env("MIMLYS_CONFIG", "")
        .env("MIMLYS_WORKER_BIN", env!("CARGO_BIN_EXE_mimlys-worker"));
    let out = run_bounded(&mut cmd, data.parent().unwrap());
    (out.code(), out.stdout, out.stderr)
}

/// The blob bytes the storage view reports, by kind and for the first task.
fn blob_bytes_of(view: &Value) -> (u64, u64) {
    (
        view["bytes"]["blobs"].as_u64().unwrap(),
        view["tasks"][0]["blobs"].as_u64().unwrap(),
    )
}

/// REQ-PERS-005, REQ-WS-012, REQ-WS-013: what a run wrote as a blob is counted by the storage
/// view (by kind and by task), is part of what the task holds (what deleting it frees), and is
/// freed with the task, file and row, by `task delete` and by `task prune`.
#[test]
fn req_pers_005_the_storage_view_counts_a_blob_and_task_delete_and_prune_free_it() {
    for by_prune in [false, true] {
        let scope = scope(&printed(), "");
        let dir = tempfile::tempdir().unwrap();
        let script = script(dir.path());
        let data = dir.path().join("data");
        let run = run_printer(&data, scope.path(), &script, &[], &[]);
        assert!(run.status.success(), "{}", run.stderr);
        let report = run.report.unwrap();
        let task = report["task_id"].as_str().unwrap().to_owned();
        let blob = report["executions"][0]["stdout_blob"].as_str().unwrap();
        let size = std::fs::metadata(data.join("blobs").join(blob))
            .unwrap()
            .len();
        assert_eq!(size as usize, cleaned().len());

        let (code, stdout, stderr) = mimlys(&data, &["--json", "status", "--storage"]);
        assert_eq!(code, 0, "{stdout}\n{stderr}");
        let view: Value = serde_json::from_str(stdout.trim()).unwrap();
        assert_eq!(blob_bytes_of(&view), (size, size), "{view}");

        let (code, stdout, stderr) = mimlys(&data, &["--json", "task", "show", &task]);
        assert_eq!(code, 0, "{stdout}\n{stderr}");
        let shown: Value = serde_json::from_str(stdout.trim()).unwrap();
        assert_eq!(shown["footprint"]["blob_bytes"], size, "{shown}");

        let (code, stdout, stderr) = if by_prune {
            mimlys(&data, &["--json", "task", "prune", "--yes"])
        } else {
            mimlys(&data, &["--json", "task", "delete", &task, "--yes"])
        };
        assert_eq!(code, 0, "{stdout}\n{stderr}");
        assert!(blob_files(&data).is_empty(), "the blob's file is removed");
        assert!(blob_rows(&data).is_empty(), "the blob's row is removed");
        let (_, stdout, _) = mimlys(&data, &["--json", "status", "--storage"]);
        let view: Value = serde_json::from_str(stdout.trim()).unwrap();
        assert_eq!(view["bytes"]["blobs"], 0, "{view}");
    }
}
