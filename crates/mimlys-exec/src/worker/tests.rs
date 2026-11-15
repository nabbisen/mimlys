use super::*;

/// REQ-TOOL-006, REQ-CONF-009 (handoff 011n item 10): the runtime waits for a
/// worker's answer for the execution's deadline and two LIM-O2: one for what
/// the worker does before a host tool's own clock starts, one for the end of
/// the tool's tree after it.
#[test]
fn req_tool_006_the_answer_budget_covers_the_deadline_and_two_lim_o2() {
    let lim_o2 = Duration::from_millis(LIM_O2_CANCEL_MS);
    assert_eq!(
        answer_budget(10_000),
        Duration::from_millis(10_000) + lim_o2 * 2
    );
    // A tool whose tree takes the whole of LIM-O2 to end, started after a
    // set-up that took LIM-O2, is answered for in time; the old budget (one
    // LIM-O2) was exceeded by either alone.
    let slow = Duration::from_millis(10_000) + lim_o2 + lim_o2;
    assert!(slow <= answer_budget(10_000));
    assert!(slow > Duration::from_millis(10_000) + lim_o2);
    // A negative deadline is no deadline at all, not a wrap-around.
    assert_eq!(answer_budget(-5), lim_o2 * 2);
}

/// REQ-TOOL-006 (handoff 011n item 10): a worker the runtime has to end is
/// recorded `deadline` when it is alive, nobody asked to cancel, and the
/// execution's deadline has passed; in every other case it is lost
/// (`worker_died`): it exited by itself, it did not take a cancellation, or the
/// deadline had not passed (a worker silent before it).
#[test]
fn req_tool_006_an_unanswered_execution_past_its_deadline_is_a_deadline() {
    let ms = Duration::from_millis;
    let deadline = ms(10_000);
    assert_eq!(
        unanswered(false, false, ms(14_100), deadline),
        Unanswered::Deadline
    );
    assert_eq!(
        unanswered(false, false, deadline, deadline),
        Unanswered::Deadline,
        "the deadline has been reached"
    );
    assert_eq!(
        unanswered(false, false, ms(9_999), deadline),
        Unanswered::WorkerDied,
        "the deadline had not passed"
    );
    assert_eq!(
        unanswered(true, false, ms(14_100), deadline),
        Unanswered::WorkerDied,
        "it exited before answering"
    );
    assert_eq!(
        unanswered(false, true, ms(14_100), deadline),
        Unanswered::WorkerDied,
        "it did not take the cancellation"
    );
}

/// REQ-TOOL-006 (handoff 011n item 10): what is recorded for the late worker
/// is `deadline`, failure `timeout`, with nothing of the worker's output.
#[test]
fn req_tool_006_the_late_workers_record_is_a_deadline() {
    let outcome = deadline_fault(Instant::now(), "the worker did not answer");
    assert_eq!(outcome.termination, Termination::Deadline);
    assert_eq!(outcome.failure, Some(FailureCause::Timeout));
    assert!(outcome.stdout.is_empty() && outcome.stderr.is_empty());
}
