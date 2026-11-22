use super::*;

fn step(replied: bool, malformed: bool, acted: bool) -> StepFacts {
    StepFacts {
        replied,
        malformed,
        acted,
    }
}

/// A reply that was not an `act`: a plan, a question, `complete`, `give_up`.
fn plan() -> StepFacts {
    step(true, false, false)
}

/// A reply whose decision was an `act`, accepted or rejected after parsing: both are acting.
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

fn incomplete(detail: Detail) -> Verdict {
    Verdict::Class(Class::Incomplete, Some(detail))
}

fn plans_only(detail: Detail) -> Verdict {
    Verdict::Class(Class::PlansOnly, Some(detail))
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
/// acted, and whose file the runtime did not find as asked, is `incomplete` and `not_verified`;
/// one that said `complete` and never acted is `plans_only` and `not_verified`. Neither is
/// `acts`, whatever the task's state says.
#[test]
fn req_mdl_011_the_models_word_that_it_finished_is_never_the_verdict() {
    assert_eq!(
        class(End::Completed, vec![act(), plan()], false),
        incomplete(Detail::NotVerified)
    );
    assert_eq!(
        class(End::Completed, vec![plan()], false),
        plans_only(Detail::NotVerified)
    );
}

/// REQ-MDL-011: what counts as acting is the decision, not the action record: a step whose decision
/// was an `act` counts when it was rejected after parsing (`schema`, `denied`, …) as well as when
/// it was accepted. A model whose every call to a tool is invalid acted, badly: it is
/// `incomplete`, not `plans_only`, with how it ended (the guards halt it).
#[test]
fn req_mdl_011_a_rejected_act_is_acting() {
    // Every `act` rejected; the task halted for its rejections.
    assert_eq!(
        class(
            End::Halted(HaltReason::BudgetRejections),
            vec![act(), act(), act()],
            false
        ),
        incomplete(Detail::NotFinished)
    );
    // The same for a model that was rejected once and then asked the user.
    assert_eq!(
        class(End::AskedUser, vec![act(), plan()], false),
        incomplete(Detail::AskedUser)
    );
    // And what is not an `act` is not acting: plans and replies that did not parse as a decision
    // (rejected, but not known to be an `act`) are not.
    assert_eq!(
        class(
            End::Halted(HaltReason::BudgetRejections),
            vec![plan(), step(true, false, false), plan()],
            false
        ),
        plans_only(Detail::NotFinished)
    );
}

/// REQ-MDL-011: both classes that did not act to the end carry how the probe ended, the four
/// words: for each of them, and each end that maps to one.
#[test]
fn req_mdl_011_incomplete_and_plans_only_say_how_they_ended() {
    for (steps, make) in [
        (vec![act(), plan()], incomplete as fn(Detail) -> Verdict),
        (vec![plan(), plan()], plans_only as fn(Detail) -> Verdict),
    ] {
        assert_eq!(
            class(End::AskedUser, steps.clone(), false),
            make(Detail::AskedUser)
        );
        assert_eq!(
            class(End::GaveUp, steps.clone(), false),
            make(Detail::GaveUp)
        );
        assert_eq!(
            class(End::Completed, steps.clone(), false),
            make(Detail::NotVerified)
        );
        for reason in [
            HaltReason::BudgetSteps,
            HaltReason::BudgetDeadline,
            HaltReason::BudgetRejections,
            HaltReason::BudgetFailures,
            HaltReason::Convergence,
            HaltReason::BackendUnavailable,
            HaltReason::Policy,
        ] {
            assert_eq!(
                class(End::Halted(reason), steps.clone(), false),
                make(Detail::NotFinished),
                "{reason:?}"
            );
        }
        assert_eq!(
            class(End::Other("paused".into()), steps.clone(), false),
            make(Detail::NotFinished)
        );
    }
}

/// REQ-MDL-011: `no_structured_call` is replies that were all `malformed`; one reply that was
/// readable, a plan for instance, makes it `plans_only`. It carries no ending.
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
        plans_only(Detail::NotFinished)
    );
}

/// REQ-MDL-011: no reply at all is `unreachable` when the backend failed, which is no class (it is
/// said and never recorded); the task's time running out before any request was sent (the
/// engine's gate held by another runtime) measured nothing; any other end without a reply is a
/// fault, never a class.
#[test]
fn req_mdl_011_no_reply_is_unreachable_only_when_the_backend_failed() {
    assert_eq!(
        class(
            End::Halted(HaltReason::BackendUnavailable),
            vec![failed_request()],
            false
        ),
        Verdict::Unreachable
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
/// that acted is `incomplete`, one that did not is `plans_only`, both `not_finished`. It is not
/// `unreachable`.
#[test]
fn req_mdl_011_a_backend_that_failed_after_replying_is_not_unreachable() {
    assert_eq!(
        class(
            End::Halted(HaltReason::BackendUnavailable),
            vec![act(), failed_request()],
            false
        ),
        incomplete(Detail::NotFinished)
    );
    assert_eq!(
        class(
            End::Halted(HaltReason::BackendUnavailable),
            vec![plan(), failed_request()],
            false
        ),
        plans_only(Detail::NotFinished)
    );
}

/// REQ-MDL-011: the words of the record are the four classes of the reader, the four endings the
/// book names, and `unreachable` is not among them.
#[test]
fn req_mdl_011_the_classes_are_the_words_the_reader_knows() {
    let words: Vec<&str> = [
        Class::Acts,
        Class::Incomplete,
        Class::PlansOnly,
        Class::NoStructuredCall,
    ]
    .iter()
    .map(|c| c.as_str())
    .collect();
    assert_eq!(words, crate::probe_results::CLASSES);
    assert!(!crate::probe_results::CLASSES.contains(&"unreachable"));
    let details: Vec<&str> = [
        Detail::NotFinished,
        Detail::NotVerified,
        Detail::AskedUser,
        Detail::GaveUp,
    ]
    .iter()
    .map(|d| d.as_str())
    .collect();
    assert_eq!(
        details,
        ["not_finished", "not_verified", "asked_user", "gave_up"]
    );
}
