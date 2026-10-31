//! The recovery plan (external design §16.3; REQ-PERS-007, REQ-PERS-008):
//! pure logic over the persisted task and its in-flight action. The store
//! applies the plan as ordinary transitions.

use crate::action::{Action, ActionStatus, AfterUnknown, after_unknown};
use crate::event::RecoveryDisposition;
use crate::ids::{ActionId, SnapshotId};
use crate::policy::Recovery;
use crate::task::{HaltReason, Task, TaskInput, TaskState};
use serde::{Deserialize, Serialize};

/// What to do with the in-flight action.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionRecovery {
    /// The action.
    pub action_id: ActionId,
    /// The rule its effect gives.
    pub after_unknown: AfterUnknown,
    /// The snapshot to restore when the rule says so (M3; `None` until then).
    pub restore_snapshot: Option<SnapshotId>,
}

/// The plan for one task found `running` at startup.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecoveryPlan {
    /// The in-flight action to mark `unknown`, if any.
    pub action: Option<ActionRecovery>,
    /// The disposition recorded in `task.recovered`.
    pub disposition: RecoveryDisposition,
    /// The input that ends `recovering`.
    pub outcome: TaskInput,
}

/// Plans recovery for a task; `None` when the task was not running.
/// Rules (REQ-PERS-007): no action in flight → `recovered`, resumable by
/// policy; a reversible effect → restore its snapshot, then `recovered`,
/// resumable by policy only when a snapshot was restored (M3) or nothing
/// needed restoring; a non-reversible effect → `unknown_outcome`, never
/// resumed automatically (REQ-PERS-008).
pub fn plan_recovery(
    task: &Task,
    running_action: Option<&Action>,
    recovery: &Recovery,
    snapshot_before: Option<SnapshotId>,
) -> Option<RecoveryPlan> {
    if task.state != TaskState::Running {
        return None;
    }
    let Some(action) = running_action else {
        return Some(RecoveryPlan {
            action: None,
            disposition: RecoveryDisposition::Clean,
            outcome: TaskInput::Recovered {
                resume: recovery.auto_resume,
                reason: HaltReason::Recovered,
            },
        });
    };
    debug_assert_eq!(action.status, ActionStatus::Running);
    let rule = after_unknown(action.content().effect());
    let (disposition, resume, reason, restore) = match rule {
        AfterUnknown::RedoPermitted => (
            RecoveryDisposition::Clean,
            recovery.auto_resume,
            HaltReason::Recovered,
            None,
        ),
        AfterUnknown::Rollback => (
            RecoveryDisposition::RolledBack,
            recovery.auto_resume && snapshot_before.is_some(),
            HaltReason::Recovered,
            snapshot_before,
        ),
        AfterUnknown::UndoThenHalt | AfterUnknown::Halt => (
            RecoveryDisposition::UnknownOutcome,
            false,
            HaltReason::UnknownOutcome,
            None,
        ),
    };
    Some(RecoveryPlan {
        action: Some(ActionRecovery {
            action_id: action.action_id,
            after_unknown: rule,
            restore_snapshot: restore,
        }),
        disposition,
        outcome: TaskInput::Recovered { resume, reason },
    })
}

#[cfg(test)]
mod tests;
