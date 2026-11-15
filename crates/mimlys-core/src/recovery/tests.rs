use super::*;
use crate::action::{EffectKind, EffectView, Proposal};
use crate::clock::{Clock, FakeClock};
use crate::hash::hash_bytes;
use crate::ids::{ScriptedIds, StepId, TaskId};
use crate::limits::LIM_T5_DEADLINE_MS;
use crate::step::Budgets;
use crate::task::test_versions;

fn running_task() -> Task {
    let clock = FakeClock::default();
    let task = Task::create(
        crate::task::TaskSpec {
            task_id: TaskId::new(&ScriptedIds::default()),
            goal: "g".into(),
            deadline_ms: LIM_T5_DEADLINE_MS,
            budgets: Budgets::default(),
            profile: "workspace".into(),
            versions: test_versions(),
            scope: crate::task::ScopeSpec {
                root: "/project".into(),
                include: vec![],
            },
        },
        clock.now(),
    )
    .unwrap();
    let mut task = task;
    task.state = TaskState::Running;
    task
}

fn running_action(task: &Task, kind: EffectKind, reversible: bool, idempotent: bool) -> Action {
    let ids = ScriptedIds::starting_at(10);
    let mut a = Action::propose(
        Proposal {
            action_id: ActionId::new(&ids),
            task_id: task.task_id,
            step_id: StepId::new(&ids),
            tool: "t".into(),
            arguments: serde_json::json!({}),
            grant_set_hash: hash_bytes(b"g"),
            effect: EffectView {
                kind,
                reversible,
                idempotent,
            },
            approval_required: false,
            deadline_ms: 1000,
        },
        FakeClock::default().now(),
    );
    a.status = ActionStatus::Running;
    a.executions = 1;
    a
}

/// REQ-PERS-007: the recovery plan table, per effect, with and without auto-resume.
/// INV-12 (external design §21.2): no row of the table turns an unfinished
/// action into a success. A read in flight is redone, a workspace write is
/// rolled back, a non-reversible or external effect is an unknown outcome
/// that halts the task and is never resumed.
#[test]
fn req_pers_007_recovery_plan_table() {
    let task = running_task();
    let off = Recovery { auto_resume: false };
    let on = Recovery { auto_resume: true };
    // Not running: nothing to do.
    let mut paused = task.clone();
    paused.state = TaskState::Paused;
    assert_eq!(plan_recovery(&paused, None, &on, None), None);
    // No action in flight.
    let p = plan_recovery(&task, None, &off, None).unwrap();
    assert_eq!(p.disposition, RecoveryDisposition::Clean);
    assert_eq!(
        p.outcome,
        TaskInput::Recovered {
            resume: false,
            reason: HaltReason::Recovered
        }
    );
    assert!(matches!(
        plan_recovery(&task, None, &on, None).unwrap().outcome,
        TaskInput::Recovered { resume: true, .. }
    ));
    // A read in flight: clean, resumable by policy.
    let read = running_action(&task, EffectKind::WorkspaceRead, true, true);
    let p = plan_recovery(&task, Some(&read), &on, None).unwrap();
    assert_eq!(p.disposition, RecoveryDisposition::Clean);
    assert_eq!(
        p.action.as_ref().unwrap().after_unknown,
        AfterUnknown::RedoPermitted
    );
    assert!(matches!(
        p.outcome,
        TaskInput::Recovered {
            resume: true,
            reason: HaltReason::Recovered
        }
    ));
    // A workspace write: rolled back; resumable only when a snapshot was restored.
    let write = running_action(&task, EffectKind::WorkspaceWrite, true, false);
    let p = plan_recovery(&task, Some(&write), &on, None).unwrap();
    assert_eq!(p.disposition, RecoveryDisposition::RolledBack);
    assert!(
        matches!(
            p.outcome,
            TaskInput::Recovered {
                resume: false,
                reason: HaltReason::Recovered
            }
        ),
        "no snapshot in M1: halt"
    );
    let snap = SnapshotId::new(&ScriptedIds::starting_at(77));
    let p = plan_recovery(&task, Some(&write), &on, Some(snap)).unwrap();
    assert_eq!(p.action.as_ref().unwrap().restore_snapshot, Some(snap));
    assert!(matches!(
        p.outcome,
        TaskInput::Recovered { resume: true, .. }
    ));
    // Non-reversible effects: unknown outcome, never resumed (REQ-PERS-008).
    for (kind, reversible, idempotent) in [
        (EffectKind::ExternalWrite, false, false),
        (EffectKind::HostWrite, false, false),
        (EffectKind::ExternalRead, true, false),
    ] {
        let a = running_action(&task, kind, reversible, idempotent);
        let p = plan_recovery(&task, Some(&a), &on, Some(snap)).unwrap();
        assert_eq!(
            p.disposition,
            RecoveryDisposition::UnknownOutcome,
            "{kind:?}"
        );
        assert_eq!(
            p.outcome,
            TaskInput::Recovered {
                resume: false,
                reason: HaltReason::UnknownOutcome
            },
            "{kind:?}"
        );
        assert_eq!(p.action.as_ref().unwrap().restore_snapshot, None);
    }
    // A host write the tool backed up is recorded as such (`UndoThenHalt`), but the
    // runtime undoes none: nothing is restored and the task halts `unknown_outcome`.
    let hw = running_action(&task, EffectKind::HostWrite, true, false);
    let p = plan_recovery(&task, Some(&hw), &on, Some(snap)).unwrap();
    let recovery = p.action.unwrap();
    assert_eq!(recovery.after_unknown, AfterUnknown::UndoThenHalt);
    assert_eq!(recovery.restore_snapshot, None);
    assert_eq!(
        p.outcome,
        TaskInput::Recovered {
            resume: false,
            reason: HaltReason::UnknownOutcome
        }
    );
}

/// REQ-PERS-007 (external design §16.3): a task found `recovering` is planned like a
/// running one, for the action still `running` and for the one an earlier pass marked
/// `unknown`; any other state has no plan.
#[test]
fn req_pers_007_a_task_found_recovering_has_the_plan_of_a_running_one() {
    let running = running_task();
    let mut recovering = running.clone();
    recovering.state = TaskState::Recovering;
    let on = Recovery { auto_resume: true };
    let snap = SnapshotId::new(&ScriptedIds::starting_at(77));
    let write = running_action(&running, EffectKind::WorkspaceWrite, true, false);
    let mut marked = write.clone();
    marked.status = ActionStatus::Unknown;
    for action in [None, Some(&write), Some(&marked)] {
        assert_eq!(
            plan_recovery(&recovering, action, &on, Some(snap)),
            plan_recovery(&running, action, &on, Some(snap)),
            "{action:?}"
        );
        assert!(plan_recovery(&recovering, action, &on, Some(snap)).is_some());
    }
    let external = running_action(&running, EffectKind::ExternalWrite, false, false);
    let mut marked = external.clone();
    marked.status = ActionStatus::Unknown;
    let plan = plan_recovery(&recovering, Some(&marked), &on, Some(snap)).unwrap();
    assert_eq!(plan.disposition, RecoveryDisposition::UnknownOutcome);
    for state in [
        TaskState::Paused,
        TaskState::Halted {
            halt_reason: HaltReason::Recovered,
        },
    ] {
        let mut other = running.clone();
        other.state = state;
        assert_eq!(plan_recovery(&other, None, &on, None), None);
    }
}

/// REQ-CLI-005, REQ-PERS-007 (handoff 010e-3): the stop signal and recovery agree
/// on what is done, not only on which actions. For every effect kind and every
/// flag, `can_be_undone` is true exactly when the plan for a running action of
/// that effect does not give `unknown_outcome`: where the runtime itself
/// restores what an interruption left, a first signal may interrupt; where it
/// halts, it may not. A host write is on the second side, with a backup or
/// without: the runtime undoes none.
#[test]
fn req_cli_005_can_be_undone_is_true_exactly_when_recovery_does_not_give_unknown_outcome() {
    use crate::action::can_be_undone;
    let task = running_task();
    let on = Recovery { auto_resume: true };
    let snapshot = SnapshotId::new(&ScriptedIds::starting_at(50));
    for kind in [
        EffectKind::None,
        EffectKind::WorkspaceRead,
        EffectKind::WorkspaceWrite,
        EffectKind::HostRead,
        EffectKind::HostWrite,
        EffectKind::ExternalRead,
        EffectKind::ExternalWrite,
    ] {
        for reversible in [false, true] {
            for idempotent in [false, true] {
                let action = running_action(&task, kind, reversible, idempotent);
                let plan = plan_recovery(&task, Some(&action), &on, Some(snapshot)).unwrap();
                let halts = plan.disposition == RecoveryDisposition::UnknownOutcome;
                assert_eq!(
                    can_be_undone(action.content().effect()),
                    !halts,
                    "{kind:?} reversible={reversible} idempotent={idempotent}"
                );
            }
        }
    }
}
