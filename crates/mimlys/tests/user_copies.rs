//! The user's copy of a stream (handoff 004d; REQ-PERS-005), end to end through the
//! `mimlys-testloop` harness and a real worker: a WebAssembly module prints more than an
//! excerpt holds, with a configured secret in the middle, a carriage return and an escape
//! character; the blob that holds the whole text exists on disk, is named by the execution,
//! holds the cleaned and redacted text and no secret; a module whose output fits writes none;
//! a process that ends between the blob and the execution's record leaves a file that the next
//! start removes, and a record that is consistent.

use serde_json::Value;

mod common;
use common::printer::*;

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

const KIB: usize = 1024;

/// A run of a module that prints `out_kib` KiB of `byte` (`x`, or `\ff` for bytes that are no
/// text), with LIM-S4 left at its default or set to `bound` (`sandbox.output_bytes`), through the
/// real worker: the data directory's parent and the report of the run.
fn run_printing(out_kib: u32, byte: &str, bound: Option<usize>) -> (tempfile::TempDir, Value) {
    let scope = scope_printing(out_kib, 0, byte);
    let dir = tempfile::tempdir().unwrap();
    let script = script(dir.path());
    let bound = bound.map(|b| b.to_string());
    let mut extra = vec![];
    if let Some(bound) = &bound {
        extra.extend(["--sandbox-output-bytes", bound.as_str()]);
    }
    let run = run_printer(&dir.path().join("data"), scope.path(), &script, &extra, &[]);
    assert!(run.status.success(), "{}", run.stderr);
    (dir, run.report.unwrap())
}

/// The blob the first execution names, as bytes.
fn first_blob(dir: &tempfile::TempDir, report: &Value) -> Vec<u8> {
    let blob = report["executions"][0]["stdout_blob"]
        .as_str()
        .expect("the execution names a blob");
    std::fs::read(dir.path().join("data/blobs").join(blob)).unwrap()
}

/// `mimlys --json task output <task> [rest]` over the run's store.
fn output_json(dir: &tempfile::TempDir, report: &Value, rest: &[&str]) -> Value {
    let task = report["task_id"].as_str().unwrap();
    let mut args = vec!["--json", "task", "output", task];
    args.extend_from_slice(rest);
    let (code, stdout, stderr) = mimlys(&dir.path().join("data"), &args);
    assert_eq!(code, 0, "{stdout}\n{stderr}");
    serde_json::from_str(stdout.trim()).unwrap()
}

/// REQ-PERS-005, LIM-S4 (handoff 004d-4): through the real worker, a script that prints exactly
/// its bound, at the default LIM-S4 and at a configured 4 MiB, is stored whole: the worker
/// carries the record, its blob holds every byte (no cut, none refused), the execution ends
/// `exited`, the task completes, and the index says the capture was not cut. A worker that
/// could not send more than about 600 KB of text ended instead (0.11 and earlier).
#[test]
fn req_pers_005_a_script_that_prints_exactly_its_bound_is_stored_whole() {
    for bound in [None, Some(4 * 1024 * KIB)] {
        let want = bound.unwrap_or(1024 * KIB);
        let (dir, report) = run_printing((want / KIB) as u32, "x", bound);
        assert_eq!(report["state"]["state"], "completed", "{bound:?}");
        assert_eq!(
            report["executions"][0]["termination"], "exited",
            "{bound:?}: the worker lived and sent its record"
        );
        let stored = first_blob(&dir, &report);
        assert_eq!(stored.len(), want, "{bound:?}: every byte, no more");
        assert!(stored.iter().all(|b| *b == b'x'));
        let index = output_json(&dir, &report, &[]);
        let stdout = &index["copies"][1];
        assert_eq!(stdout["kind"], "stdout");
        assert_eq!(stdout["bytes"], want, "{index}");
        assert_eq!(stdout["cut"], true, "the model read a cut of it");
        assert_eq!(stdout["capture_cut"], false, "{index}");
        assert!(stdout.get("captured_bytes").is_none(), "{index}");
    }
}

/// REQ-PERS-005, LIM-S4 (handoff 004d-4): a script that prints past its bound is ended
/// (`output_limit`), the copy holds the bound, the first bytes it printed, and the index and the
/// JSON of `task output` say the capture was cut and how many bytes were kept: the user is not
/// told that a cut copy is whole.
#[test]
fn req_pers_005_a_script_that_prints_past_its_bound_has_a_copy_that_says_the_capture_was_cut() {
    let bound = 1024 * KIB;
    let (dir, report) = run_printing(1025, "x", None);
    assert_eq!(report["executions"][0]["termination"], "output_limit");
    let stored = first_blob(&dir, &report);
    assert_eq!(stored.len(), bound, "the copy holds the bound");
    let index = output_json(&dir, &report, &[]);
    let stdout = &index["copies"][1];
    assert_eq!(stdout["capture_cut"], true, "{index}");
    assert_eq!(stdout["captured_bytes"], bound, "{index}");
    assert_eq!(stdout["bytes"], bound, "{index}");
    let stderr = &index["copies"][2];
    assert_eq!(stderr["capture_cut"], false, "{index}");
    let shown = output_json(&dir, &report, &["--stdout"]);
    assert_eq!(shown["capture_cut"], true, "{shown}");
    assert_eq!(shown["captured_bytes"], bound, "{shown}");
    assert_eq!(shown["text"].as_str().unwrap().len(), bound);
    // The text index says it too.
    let task = report["task_id"].as_str().unwrap();
    let (code, text, stderr) = mimlys(&dir.path().join("data"), &["task", "output", task]);
    assert_eq!(code, 0, "{stderr}");
    let line = text
        .lines()
        .find(|l| l.trim_start().starts_with("standard output"))
        .unwrap();
    assert!(
        line.contains("the tool printed more than mimlys keeps")
            && line.contains(&format!("the first {bound} bytes")),
        "{text}"
    );
}

/// REQ-PERS-005 (handoff 004d-4): a capture of bytes that are no text, up to its bound, is stored
/// whole: each byte is U+FFFD as text, three bytes, so the copy is three times the bound, never
/// cut again, at the default and at a configured 4 MiB. It is not a capture cut.
#[test]
fn req_pers_005_invalid_bytes_up_to_the_bound_are_stored_whole() {
    for bound in [None, Some(4 * 1024 * KIB)] {
        let want = bound.unwrap_or(1024 * KIB);
        let (dir, report) = run_printing((want / KIB) as u32, "\\ff", bound);
        assert_eq!(report["state"]["state"], "completed", "{bound:?}");
        let stored = first_blob(&dir, &report);
        assert_eq!(stored.len(), 3 * want, "{bound:?}: three bytes for each");
        assert!(stored.chunks(3).all(|c| c == [0xef, 0xbf, 0xbd]));
        let index = output_json(&dir, &report, &[]);
        let stdout = &index["copies"][1];
        assert_eq!(stdout["bytes"], 3 * want, "{index}");
        assert_eq!(stdout["capture_cut"], false, "{index}");
    }
}
