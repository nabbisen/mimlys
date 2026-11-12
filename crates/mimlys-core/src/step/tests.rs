use super::*;
use crate::clock::{Clock, FakeClock};
use crate::hash::hash_bytes;
use crate::ids::ScriptedIds;

fn result(
    outcome: StepOutcome,
    action: Option<ActionOutcome>,
    decision: &str,
    snapshot: Option<SnapshotId>,
) -> StepResult {
    StepResult {
        outcome,
        action,
        decision_hash: Some(hash_bytes(decision.as_bytes())),
        snapshot,
    }
}

fn continued(decision: &str) -> StepResult {
    result(
        StepOutcome::Continued,
        Some(ActionOutcome::Succeeded),
        decision,
        None,
    )
}

/// REQ-TASK-008 / LIM-T1 … LIM-T6: defaults come from the limits register.
#[test]
fn req_task_008_budgets_default_to_the_limits_register() {
    let b = Budgets::default();
    assert_eq!(b.steps, LIM_T1_STEPS);
    assert_eq!(b.consecutive_rejections, LIM_T2_CONSECUTIVE_REJECTIONS);
    assert_eq!(b.executions_per_action, LIM_T3_EXECUTIONS_PER_ACTION);
    assert_eq!(b.identical_decisions, LIM_T4_IDENTICAL_DECISIONS);
    assert_eq!(b.consecutive_failures, LIM_T6_CONSECUTIVE_FAILURES);
}

/// REQ-DEC-010 / LIM-T1: the step budget halts the task on the step that reaches it.
/// REQ-TEST-011 (`budget_steps`): the halt comes on the step that reaches LIM-T1 and on no earlier one.
#[test]
fn req_dec_010_steps_budget_halts() {
    let budgets = Budgets::default();
    let mut c = Counters::default();
    for i in 1..LIM_T1_STEPS {
        assert_eq!(
            c.apply(&continued(&format!("d{i}")), &budgets),
            None,
            "step {i}"
        );
    }
    assert_eq!(
        c.apply(&continued("last"), &budgets),
        Some(HaltReason::BudgetSteps)
    );
    assert_eq!(c.steps, LIM_T1_STEPS);
}

/// REQ-DEC-010 / LIM-T2: consecutive rejections halt; an accepted step resets the count.
/// REQ-TEST-011 (`budget_rejections`): the halt comes at LIM-T2 and an accepted step resets the count.
#[test]
fn req_dec_010_consecutive_rejections_halt_and_reset() {
    let budgets = Budgets::default();
    let mut c = Counters::default();
    let rejected = |d: &str| result(StepOutcome::Rejected, None, d, None);
    for i in 1..LIM_T2_CONSECUTIVE_REJECTIONS {
        assert_eq!(c.apply(&rejected(&format!("r{i}")), &budgets), None);
    }
    assert_eq!(c.apply(&continued("ok"), &budgets), None);
    assert_eq!(c.consecutive_rejections, 0);
    for i in 1..LIM_T2_CONSECUTIVE_REJECTIONS {
        assert_eq!(c.apply(&rejected(&format!("s{i}")), &budgets), None);
    }
    assert_eq!(
        c.apply(&rejected("final"), &budgets),
        Some(HaltReason::BudgetRejections)
    );
}

/// REQ-DEC-010 / LIM-T6: consecutive failed actions halt; a succeeded action resets; a step without an action leaves the count.
/// REQ-TEST-011 (`budget_failures`): the halt comes at LIM-T6 and a succeeded action resets the count.
#[test]
fn req_dec_010_consecutive_failures_halt_and_reset() {
    let budgets = Budgets::default();
    let mut c = Counters::default();
    let failed = |d: &str| result(StepOutcome::Continued, Some(ActionOutcome::Failed), d, None);
    for i in 1..LIM_T6_CONSECUTIVE_FAILURES {
        assert_eq!(c.apply(&failed(&format!("f{i}")), &budgets), None);
    }
    assert_eq!(
        c.apply(
            &result(StepOutcome::Continued, None, "plan", None),
            &budgets
        ),
        None
    );
    assert_eq!(c.consecutive_failures, LIM_T6_CONSECUTIVE_FAILURES - 1);
    assert_eq!(c.apply(&continued("fixed"), &budgets), None);
    assert_eq!(c.consecutive_failures, 0);
    for i in 1..LIM_T6_CONSECUTIVE_FAILURES {
        assert_eq!(c.apply(&failed(&format!("g{i}")), &budgets), None);
    }
    assert_eq!(
        c.apply(&failed("final"), &budgets),
        Some(HaltReason::BudgetFailures)
    );
}

/// REQ-DEC-010 / LIM-T4: identical decisions with an unchanged snapshot halt; a changed snapshot or decision resets the run.
/// REQ-TEST-011 (the identical-decision rule): the halt `convergence` comes at LIM-T4 only for the same decision hash with an unchanged snapshot; a changed snapshot or decision resets the run.
#[test]
fn req_dec_010_identical_decisions_require_an_unchanged_snapshot() {
    let budgets = Budgets::default();
    let ids = ScriptedIds::default();
    let s1 = Some(SnapshotId::new(&ids));
    let s2 = Some(SnapshotId::new(&ids));
    let mut c = Counters::default();
    let same = |snapshot: Option<SnapshotId>| {
        result(
            StepOutcome::Continued,
            Some(ActionOutcome::Succeeded),
            "same",
            snapshot,
        )
    };
    for _ in 1..LIM_T4_IDENTICAL_DECISIONS {
        assert_eq!(c.apply(&same(s1), &budgets), None);
    }
    // The same decision after the workspace changed is a fresh run.
    assert_eq!(c.apply(&same(s2), &budgets), None);
    assert_eq!(c.identical_decisions, 1);
    for _ in 1..LIM_T4_IDENTICAL_DECISIONS - 1 {
        assert_eq!(c.apply(&same(s2), &budgets), None);
    }
    assert_eq!(c.apply(&same(s2), &budgets), Some(HaltReason::Convergence));
    // A different decision resets.
    let mut c = Counters::default();
    assert_eq!(c.apply(&same(s1), &budgets), None);
    assert_eq!(c.apply(&same(s1), &budgets), None);
    assert_eq!(c.apply(&continued("other"), &budgets), None);
    assert_eq!(c.identical_decisions, 1);
}

/// REQ-DEC-010: the first exceeded budget is reported in the order steps, rejections, failures, convergence.
/// REQ-TEST-011 (order of the halts): when several budgets are reached at once the reported reason is the first of steps, rejections, failures.
#[test]
fn req_dec_010_first_budget_wins_in_fixed_order() {
    let budgets = Budgets {
        steps: 1,
        consecutive_rejections: 1,
        consecutive_failures: 1,
        identical_decisions: 2,
        executions_per_action: 1,
    };
    let mut c = Counters::default();
    let r = result(
        StepOutcome::Rejected,
        Some(ActionOutcome::Failed),
        "x",
        None,
    );
    assert_eq!(c.apply(&r, &budgets), Some(HaltReason::BudgetSteps));
    let budgets = Budgets {
        steps: 100,
        ..budgets
    };
    let mut c = Counters::default();
    assert_eq!(c.apply(&r, &budgets), Some(HaltReason::BudgetRejections));
    let budgets = Budgets {
        consecutive_rejections: 100,
        ..budgets
    };
    let mut c = Counters::default();
    assert_eq!(c.apply(&r, &budgets), Some(HaltReason::BudgetFailures));
}

/// REQ-DEC-001: a step records its phases, decision, action and outcome, and serializes round trip.
#[test]
fn req_dec_001_step_record_round_trip() {
    let clock = FakeClock::default();
    let ids = ScriptedIds::default();
    let mut step = Step::start(StepId::new(&ids), TaskId::new(&ids), 0, clock.now());
    assert_eq!(step.phase, StepPhase::Assemble);
    step.phase = StepPhase::Decide;
    step.decision_hash = Some(hash_bytes(b"d"));
    step.action_id = Some(ActionId::new(&ids));
    clock.advance_millis(10);
    step.finish(StepOutcome::Continued, clock.now());
    assert_eq!(step.phase, StepPhase::Evaluate);
    assert_eq!(step.started_at.millis_until(step.finished_at.unwrap()), 10);
    let json = serde_json::to_value(&step).unwrap();
    assert_eq!(json["phase"], "evaluate");
    assert_eq!(json["outcome"], "continued");
    let back: Step = serde_json::from_value(json).unwrap();
    assert_eq!(back, step);
}

/// REQ-TASK-008: counters are plain data that round-trip through serde unchanged (durable).
#[test]
fn req_task_008_counters_round_trip() {
    let mut c = Counters::default();
    c.apply(&continued("a"), &Budgets::default());
    let json = serde_json::to_string(&c).unwrap();
    let back: Counters = serde_json::from_str(&json).unwrap();
    assert_eq!(back, c);
}
