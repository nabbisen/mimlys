//! The user's copy of a stream (handoff 004d; REQ-PERS-005): a stream whose excerpt is a cut
//! of it is stored whole as a blob that its execution names, holding the cleaned and redacted
//! text the excerpt was cut from, within LIM-S4 (a script's stream) or LIM-H4 (a host tool's);
//! a stream that fits writes nothing; the blob is written before the execution's record.

use super::*;
use crate::limits::LIM_H4_HOST_OUTPUT_BYTES;
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

/// REQ-PERS-005 (handoff 004d-4): the bound of a stream's whole copy is the bound in force
/// times three: LIM-S4 as configured for a script (not the constant), LIM-H4 for a host tool. A
/// byte that is no UTF-8 becomes three bytes of U+FFFD, so the text of a capture that holds its
/// bound fits, whole; one character more is cut at the bound, on a character boundary, never
/// refused (the step goes on).
#[test]
fn req_pers_005_the_bound_is_three_times_the_capture_bound_in_force() {
    const MIB: usize = 1024 * 1024;
    let h3 = LIM_H4_HOST_OUTPUT_BYTES * 3;
    assert_eq!(WholeStreams::bound_bytes("run_script", MIB), 3 * MIB);
    assert_eq!(WholeStreams::bound_bytes("run_wasm", 4 * MIB), 12 * MIB);
    assert_eq!(
        WholeStreams::bound_bytes(crate::host_tool::RUN_HOST_TOOL, MIB),
        h3
    );
    assert_eq!(
        WholeStreams::bound_bytes(crate::host_tool::RUN_HOST_TOOL, 64 * MIB),
        h3,
        "a host tool's bound is LIM-H4, whatever LIM-S4 is"
    );
    for (tool, configured, capture) in [
        ("run_script", MIB, MIB),
        ("run_script", 4 * MIB, 4 * MIB),
        (
            crate::host_tool::RUN_HOST_TOOL,
            MIB,
            LIM_H4_HOST_OUTPUT_BYTES,
        ),
    ] {
        let mut h = harness();
        h.runtime.deps.output_bytes = configured;
        let id = h.create();
        // The text of a capture of `capture` bytes, every one of them replaced: three bytes each.
        let whole = |chars: usize| WholeStreams {
            stdout: Some("\u{FFFD}".repeat(chars)),
            stderr: None,
        };
        let (blob, _) = h.runtime.keep_whole_streams(id, tool, whole(capture));
        let stored = h
            .persistence
            .get_blob(blob.expect("a stream within its bound"))
            .unwrap();
        assert_eq!(stored.len(), 3 * capture, "{tool} at {configured}: whole");
        // One character more is cut at the bound, and the cut is on a character boundary.
        let (blob, _) = h.runtime.keep_whole_streams(id, tool, whole(capture + 1));
        let stored = h.persistence.get_blob(blob.unwrap()).unwrap();
        assert_eq!(stored.len(), 3 * capture, "{tool} at {configured}: cut");
        assert!(String::from_utf8(stored).is_ok());
    }
}

/// REQ-PERS-005 (handoff 004d-4): a capture of invalid bytes that holds its bound (here a script's
/// 1 MiB, every byte U+FFFD as text, three times the bytes) is stored whole, through the whole
/// path of an execution, and the notice that says the capture was cut is the observation's,
/// naming the bytes kept.
#[test]
fn req_pers_005_invalid_bytes_up_to_the_bound_are_stored_whole() {
    const MIB: usize = 1024 * 1024;
    let mut outcome = ExecutionOutcome::succeeded(json!({}), 3);
    outcome.stdout = vec![0xffu8; MIB];
    outcome.notices = vec![Notice::OutputTruncated {
        stream: crate::observation::Stream::Stdout,
        total_bytes: MIB as u64,
        captured_bytes: Some(MIB as u64),
    }];
    let (h, id) = run(outcome);
    let execution = execution_of(&h, id);
    let blob = execution.stdout_blob.expect("a blob");
    let stored = h.persistence.get_blob(blob).unwrap();
    assert_eq!(stored.len(), 3 * MIB, "the replaced text, whole");
    assert!(
        String::from_utf8(stored)
            .unwrap()
            .chars()
            .all(|c| c == '\u{FFFD}')
    );
    let observation = h.persistence.load_observations(id).unwrap().remove(0);
    assert!(observation.notices.contains(&Notice::OutputTruncated {
        stream: crate::observation::Stream::Stdout,
        total_bytes: MIB as u64,
        captured_bytes: Some(MIB as u64),
    }));
}

/// REQ-PERS-005 (handoff 004d-4), what the factor of three does not cover: a redaction can make
/// the text longer than the bytes it was made of (a secret of eight characters becomes a marker
/// of the secret's name, which can be much longer), so the text of a capture that holds its bound
/// can exceed three times it. The copy is then cut at its bound, on a character boundary, never
/// refused and never longer than the bound; the log says so (the user's index does not: a design
/// question in the request of 004d-4). No secret is ever on the disk either way.
#[test]
fn req_pers_005_a_redaction_that_lengthens_the_text_past_its_bound_is_cut_at_it() {
    const MIB: usize = 1024 * 1024;
    let secret = "abcd1234"; // MIN_SECRET_CHARS, the shortest value that is redacted
    let name = "A_NAME_OF_FORTY_CHARACTERS_FOR_THE_TEST"; // the marker is `[redacted:<name>]`
    let h = Harness::with_backend_and_tools(
        Arc::new(ScriptedBackend::with_decisions(vec![act(
            "run_script",
            "x",
        )])),
        Policy::shipped(),
        vec![(name.into(), secret.into())],
        crate::limits::LIM_C1_CONTEXT_CHARS,
        vec![tool("run_script", EffectKind::WorkspaceRead)],
    );
    h.tools.script("run_script", {
        let mut outcome = ExecutionOutcome::succeeded(json!({}), 3);
        // A capture at its bound (1 MiB) that is the secret over and over.
        outcome.stdout = secret.repeat(MIB / secret.len()).into_bytes();
        outcome
    });
    let id = h.create();
    h.step(id);
    let blob = execution_of(&h, id)
        .stdout_blob
        .expect("a blob is written, cut at its bound");
    let stored = h.persistence.get_blob(blob).unwrap();
    assert!(
        stored.len() <= 3 * MIB && stored.len() > 3 * MIB - 80,
        "{}",
        stored.len()
    );
    let stored = String::from_utf8(stored).expect("cut on a character boundary");
    assert!(!stored.contains(secret), "no secret on the disk");
    assert!(stored.starts_with(&format!("[redacted:{name}]")));
}
