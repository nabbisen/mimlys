use super::*;

fn step(replied: bool, malformed: bool, acted: bool) -> StepFacts {
    StepFacts {
        replied,
        malformed,
        acted,
    }
}

fn plan() -> StepFacts {
    step(true, false, false)
}

fn act() -> StepFacts {
    step(true, false, true)
}

fn garbage() -> StepFacts {
    step(true, true, false)
}

fn failed_request() -> StepFacts {
    step(false, false, false)
}

fn observed(end: End, steps: Vec<StepFacts>, verified: bool) -> Observed {
    Observed {
        end,
        steps,
        verified,
    }
}

fn class(end: End, steps: Vec<StepFacts>, verified: bool) -> Verdict {
    classify(&observed(end, steps, verified))
}

/// REQ-MDL-011: a task that completed and whose file the runtime found as asked is `acts`.
#[test]
fn req_mdl_011_a_completed_task_with_the_file_as_asked_acts() {
    assert_eq!(
        class(End::Completed, vec![act(), act(), plan()], true),
        Verdict::Class(Class::Acts, None)
    );
}

/// REQ-MDL-011: the verdict is the runtime's, not the model's. A model that says `complete` having
/// acted, and whose file the runtime did not find as asked, did not act well: `incomplete`. One
/// that said `complete` and never acted is `plans_only`. Neither is `acts`, whatever the task's
/// state says.
#[test]
fn req_mdl_011_the_models_word_that_it_finished_is_never_the_verdict() {
    assert_eq!(
        class(End::Completed, vec![act(), plan()], false),
        Verdict::Class(Class::Incomplete, Some(Detail::NotFinished))
    );
    assert_eq!(
        class(End::Completed, vec![plan()], false),
        Verdict::Class(Class::PlansOnly, None)
    );
}

/// REQ-MDL-011: the three details of `incomplete`, each only for a model that acted.
#[test]
fn req_mdl_011_incomplete_says_how_it_ended() {
    assert_eq!(
        class(End::AskedUser, vec![act(), plan()], false),
        Verdict::Class(Class::Incomplete, Some(Detail::AskedUser))
    );
    assert_eq!(
        class(End::GaveUp, vec![act(), plan()], false),
        Verdict::Class(Class::Incomplete, Some(Detail::GaveUp))
    );
    for reason in [
        HaltReason::BudgetSteps,
        HaltReason::BudgetDeadline,
        HaltReason::BudgetRejections,
        HaltReason::BudgetFailures,
        HaltReason::Convergence,
        HaltReason::BackendUnavailable,
    ] {
        assert_eq!(
            class(End::Halted(reason), vec![plan(), act(), plan()], false),
            Verdict::Class(Class::Incomplete, Some(Detail::NotFinished)),
            "{reason:?}"
        );
    }
}

/// REQ-MDL-011: `plans_only` is no `act` within the budget, however the task ended: a model that
/// planned until the steps ran out, asked the user before it tried anything, gave up before it
/// tried anything, or had its decisions rejected for another reason than their form.
#[test]
fn req_mdl_011_a_model_that_never_acted_is_plans_only() {
    let rejected = step(true, false, false);
    for end in [
        End::Halted(HaltReason::BudgetSteps),
        End::Halted(HaltReason::BudgetRejections),
        End::Halted(HaltReason::Convergence),
        End::AskedUser,
        End::GaveUp,
    ] {
        assert_eq!(
            class(end.clone(), vec![plan(), rejected, plan()], false),
            Verdict::Class(Class::PlansOnly, None),
            "{end:?}"
        );
    }
}

/// REQ-MDL-011: `no_structured_call` is replies that were all `malformed`; one reply that was
/// readable, a plan for instance, makes it `plans_only`.
#[test]
fn req_mdl_011_only_malformed_replies_are_no_structured_call() {
    assert_eq!(
        class(
            End::Halted(HaltReason::BudgetRejections),
            vec![garbage(), garbage(), garbage()],
            false
        ),
        Verdict::Class(Class::NoStructuredCall, None)
    );
    assert_eq!(
        class(
            End::Halted(HaltReason::BudgetRejections),
            vec![garbage(), plan(), garbage()],
            false
        ),
        Verdict::Class(Class::PlansOnly, None)
    );
}

/// REQ-MDL-011: no reply at all is `unreachable` when the backend failed; the task's time running
/// out before any request was sent (the engine's gate held by another runtime) measured nothing;
/// any other end without a reply is a fault, never a class.
#[test]
fn req_mdl_011_no_reply_is_unreachable_only_when_the_backend_failed() {
    assert_eq!(
        class(
            End::Halted(HaltReason::BackendUnavailable),
            vec![failed_request()],
            false
        ),
        Verdict::Class(Class::Unreachable, None)
    );
    assert_eq!(
        class(
            End::Halted(HaltReason::BudgetDeadline),
            vec![failed_request()],
            false
        ),
        Verdict::NotMeasured
    );
    assert_eq!(
        class(End::Halted(HaltReason::Policy), vec![], false),
        Verdict::Fault
    );
    assert_eq!(
        class(End::Other("paused".into()), vec![], false),
        Verdict::Fault
    );
}

/// REQ-MDL-011: a backend that answered and then failed is judged by what it did before: a model
/// that acted is `incomplete`, one that did not is `plans_only`. It is not `unreachable`.
#[test]
fn req_mdl_011_a_backend_that_failed_after_replying_is_not_unreachable() {
    assert_eq!(
        class(
            End::Halted(HaltReason::BackendUnavailable),
            vec![act(), failed_request()],
            false
        ),
        Verdict::Class(Class::Incomplete, Some(Detail::NotFinished))
    );
    assert_eq!(
        class(
            End::Halted(HaltReason::BackendUnavailable),
            vec![plan(), failed_request()],
            false
        ),
        Verdict::Class(Class::PlansOnly, None)
    );
}

/// REQ-MDL-011: the words of the record are the five classes of the reader and the three details
/// the book names.
#[test]
fn req_mdl_011_the_classes_are_the_words_the_reader_knows() {
    let words: Vec<&str> = [
        Class::Acts,
        Class::Incomplete,
        Class::PlansOnly,
        Class::NoStructuredCall,
        Class::Unreachable,
    ]
    .iter()
    .map(|c| c.as_str())
    .collect();
    assert_eq!(words, crate::probe_results::CLASSES);
    let details: Vec<&str> = [Detail::NotFinished, Detail::AskedUser, Detail::GaveUp]
        .iter()
        .map(|d| d.as_str())
        .collect();
    assert_eq!(details, ["not_finished", "asked_user", "gave_up"]);
}
