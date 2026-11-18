//! The user's copy of a stream (handoff 004d; REQ-PERS-005): a stream whose excerpt is a cut
//! of it is stored whole as a blob that its execution names, holding the cleaned and redacted
//! text the excerpt was cut from, within LIM-S4 (a script's stream) or LIM-H4 (a host tool's);
//! a stream that fits writes nothing; the blob is written before the execution's record.

use super::*;
use crate::limits::{LIM_H4_HOST_OUTPUT_BYTES, LIM_S4_OUTPUT_BYTES};
use crate::observation::{Notice, WholeStreams, sanitize};

const SECRET: &str = "sk-live-0123456789abcdef";

fn harness() -> Harness {
    Harness::with_backend_and_tools(
        Arc::new(ScriptedBackend::with_decisions(vec![act(
            "run_script",
            "x",
        )])),
        Policy::shipped(),
        vec![("TOKEN".into(), SECRET.into())],
        crate::limits::LIM_C1_CONTEXT_CHARS,
        vec![tool("run_script", EffectKind::WorkspaceRead)],
    )
}

/// What a script printed: lines, a secret in the middle, an escape sequence and a carriage return
/// that rewrites a line; far more than an excerpt (LIM-C2) holds.
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

/// The text the excerpt is cut from: cleaned, then redacted.
fn cleaned(text: &str) -> String {
    let (clean, _) = sanitize(text);
    crate::context::Redactor::new(&[("TOKEN".into(), SECRET.into())])
        .redact(&clean)
        .0
}

fn run(outcome: ExecutionOutcome) -> (Harness, TaskId) {
    let h = harness();
    h.tools.script("run_script", outcome);
    let id = h.create();
    h.step(id);
    (h, id)
}

fn execution_of(h: &Harness, id: TaskId) -> crate::action::Execution {
    let action = h.persistence.load_actions(id).unwrap().remove(0);
    h.persistence
        .load_executions(action.action_id)
        .unwrap()
        .remove(0)
}

/// REQ-PERS-005: a stream whose excerpt is a cut of it is stored whole, as the text the excerpt
/// is cut from (decoded, cleaned, redacted): the blob holds what the excerpt does not, a carriage
/// return as the tool printed it, no escape character and no secret; the execution names it.
/// The stream that fits (standard error) writes none.
#[test]
fn req_pers_005_a_cut_stream_is_stored_whole_cleaned_and_redacted() {
    let text = printed();
    let mut outcome = ExecutionOutcome::succeeded(json!({}), 3);
    outcome.stdout = text.clone().into_bytes();
    outcome.stderr = b"a short complaint\n".to_vec();
    let (h, id) = run(outcome);

    let execution = execution_of(&h, id);
    let blob = execution.stdout_blob.expect("the cut stream has a blob");
    assert_eq!(
        execution.stderr_blob, None,
        "a stream that fits writes none"
    );
    let stored = String::from_utf8(h.persistence.get_blob(blob).unwrap()).unwrap();
    assert_eq!(stored, cleaned(&text));
    assert!(!stored.contains(SECRET), "a secret never reaches the disk");
    assert!(stored.contains("[redacted:TOKEN]"));
    assert!(
        !stored.contains('\u{1b}'),
        "no control character but tab, newline and carriage return"
    );
    assert!(stored.contains("progress 50%\rprogress 100%"));
    // It holds what the excerpt left out, and the excerpt is a cut of it.
    let observation = h.persistence.load_observations(id).unwrap().remove(0);
    assert!(stored.contains("line 0200 of the tool's output"));
    assert!(
        !observation
            .stdout
            .contains("line 0200 of the tool's output")
    );
    assert!(stored.starts_with(observation.stdout.split("\n[…").next().unwrap()));
    assert!(
        observation
            .notices
            .iter()
            .any(|n| matches!(n, Notice::OutputTruncated { .. }))
    );
    assert_eq!(observation.stderr, "a short complaint\n");
    // Exactly one blob was written: the other stream's excerpt is its whole.
    assert_eq!(h.persistence.blobs().len(), 1);
}

/// REQ-PERS-005: an output that fits its excerpt writes no blob, and its execution names none.
#[test]
fn req_pers_005_a_stream_that_fits_its_excerpt_writes_no_blob() {
    let mut outcome = ExecutionOutcome::succeeded(json!({}), 3);
    outcome.stdout = vec![b'x'; 4000]; // exactly the excerpt: LIM-C2 is 4000 characters
    outcome.stderr = b"warning\n".to_vec();
    let (h, id) = run(outcome);
    let execution = execution_of(&h, id);
    assert_eq!((execution.stdout_blob, execution.stderr_blob), (None, None));
    assert!(h.persistence.blobs().is_empty());
}

/// REQ-PERS-005 (the order of the record): the blob is written before the record of the
/// execution that names it, so that a record never names a blob that is not there.
#[test]
fn req_pers_005_the_blob_is_written_before_the_execution_that_names_it() {
    let mut outcome = ExecutionOutcome::succeeded(json!({}), 3);
    outcome.stdout = printed().into_bytes();
    let (h, _) = run(outcome);
    let writes = h.persistence.writes();
    let blob = writes.iter().position(|w| *w == "blob").expect("a blob");
    let execution = writes
        .iter()
        .position(|w| *w == "execution")
        .expect("an execution");
    assert!(blob < execution, "{writes:?}");
}

/// REQ-PERS-005: a blob that cannot be written does not lose the execution: the action ran,
/// and its record (what recovery and the audit stand on) is written without the blob; the
/// excerpt, whose cut `output_truncated` says, is then the only copy.
#[test]
fn req_pers_005_a_blob_that_cannot_be_written_does_not_lose_the_execution() {
    let h = harness();
    let mut outcome = ExecutionOutcome::succeeded(json!({}), 3);
    outcome.stdout = printed().into_bytes();
    h.tools.script("run_script", outcome);
    h.persistence.fail_write("blob");
    let id = h.create();
    h.step(id);
    let execution = execution_of(&h, id);
    assert_eq!(execution.stdout_blob, None);
    let action = h.persistence.load_actions(id).unwrap().remove(0);
    assert_eq!(action.status, ActionStatus::Succeeded);
}

/// REQ-PERS-005: the bound is LIM-S4 for a script's stream and LIM-H4 for a host tool's: a text
/// between the two is stored whole for the host tool, and cut at LIM-S4, on a character boundary,
/// for a script (never refused: the step goes on).
#[test]
fn req_pers_005_the_bound_is_lim_s4_for_a_script_and_lim_h4_for_a_host_tool() {
    let h = harness();
    let id = h.create();
    // Three bytes a character: LIM-S4 is not a multiple of three, so the cut moves back.
    let text = "€".repeat(LIM_S4_OUTPUT_BYTES as usize / 3 + 1000);
    assert!(text.len() > LIM_S4_OUTPUT_BYTES as usize && text.len() < LIM_H4_HOST_OUTPUT_BYTES);
    let whole = || WholeStreams {
        stdout: Some(text.clone()),
        stderr: None,
    };

    let (blob, _) = h
        .runtime
        .keep_whole_streams(id, crate::host_tool::RUN_HOST_TOOL, whole());
    let stored = h
        .persistence
        .get_blob(blob.expect("a host tool's stream"))
        .unwrap();
    assert_eq!(
        stored.len(),
        text.len(),
        "a host tool's stream is stored whole"
    );

    let (blob, _) = h.runtime.keep_whole_streams(id, "run_script", whole());
    let stored = h
        .persistence
        .get_blob(blob.expect("a script's stream"))
        .unwrap();
    assert!(stored.len() <= LIM_S4_OUTPUT_BYTES as usize);
    assert!(stored.len() > LIM_S4_OUTPUT_BYTES as usize - 3);
    let stored = String::from_utf8(stored).expect("cut on a character boundary");
    assert!(text.starts_with(&stored));
}
