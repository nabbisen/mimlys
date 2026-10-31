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
    // A reversible host write undoes through the tool and halts.
    let hw = running_action(&task, EffectKind::HostWrite, true, false);
    let p = plan_recovery(&task, Some(&hw), &on, None).unwrap();
    assert_eq!(p.action.unwrap().after_unknown, AfterUnknown::UndoThenHalt);
    assert_eq!(
        p.outcome,
        TaskInput::Recovered {
            resume: false,
            reason: HaltReason::UnknownOutcome
        }
    );
}
