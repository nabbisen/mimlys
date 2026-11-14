//! Applying recovery at startup (external design §16.3; REQ-PERS-007):
//! every task found `running` goes through `recovering`, its in-flight
//! action becomes `unknown`, and the plan of `mimlys_core::recovery`
//! decides whether the task halts or, by policy, runs again. Expired
//! approvals and questions halt their tasks.

use crate::snapshots::{JournalReport, SnapshotFiles};
use crate::sqlite::Store;
use mimlys_core::action::ActionInput;
use mimlys_core::approval::ApprovalRequest;
use mimlys_core::clock::Instant;
use mimlys_core::event::{Actor, ApprovalDecision, Event, EventRecord, RecoveryDisposition};
use mimlys_core::hash::Hash;
use mimlys_core::ids::{EventId, IdSource, TaskId};
use mimlys_core::policy::Recovery;
use mimlys_core::recovery::plan_recovery;
use mimlys_core::spi::{Persistence, PersistenceError, PersistenceRead};
use mimlys_core::task::{HaltReason, Task, TaskInput, TaskState, WaitKind};

/// What recovery did to one task.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Recovered {
    /// The task.
    pub task_id: TaskId,
    /// The disposition.
    pub disposition: RecoveryDisposition,
    /// The state name after recovery.
    pub state: String,
}

/// Everything recovery needs to write events.
pub struct RecoveryContext<'a> {
    /// Identifier source.
    pub ids: &'a dyn IdSource,
    /// The current instant.
    pub now: Instant,
    /// The policy hash in force.
    pub policy_hash: Hash,
    /// Recovery options.
    pub recovery: Recovery,
}

impl RecoveryContext<'_> {
    fn event(&self, task: &Task, event: Event) -> EventRecord {
        EventRecord {
            event_id: EventId::new(self.ids),
            at: self.now,
            actor: Actor::Runtime,
            task_id: Some(task.task_id),
            step_id: None,
            action_id: None,
            policy_hash: self.policy_hash.clone(),
            event,
        }
    }
}

fn state_changed(from: &Task, to: &Task) -> Event {
    Event::TaskStateChanged {
        from: from.state.name().to_owned(),
        to: to.state.name().to_owned(),
        reason: match &to.state {
            TaskState::Halted { halt_reason } => Some(*halt_reason),
            _ => None,
        },
    }
}

/// Recovers every task found `running`, then processes expired user items.
pub fn recover_all(
    store: &Store,
    ctx: &RecoveryContext<'_>,
    workspaces: &dyn mimlys_core::spi::WorkspaceProvider,
) -> Result<Vec<Recovered>, PersistenceError> {
    let mut out = Vec::new();
    for task_id in store.tasks_in_state("running")? {
        let Some(task) = store.load_task(task_id)? else {
            continue;
        };
        let running = store.load_running_action(task_id)?;
        let snapshot_before = running.as_ref().and_then(|a| a.snapshot_before);
        let Some(mut plan) = plan_recovery(&task, running.as_ref(), &ctx.recovery, snapshot_before)
        else {
            continue;
        };
        let recovering = task
            .transition(TaskInput::StartRecovery, ctx.now)
            .map_err(state_error)?;
        store.record_task_transition(
            &recovering,
            &ctx.event(&recovering, state_changed(&task, &recovering)),
        )?;
        if let (Some(action), Some(_)) = (running, &plan.action) {
            let unknown = action
                .transition(ActionInput::MarkUnknown, ctx.now)
                .map_err(|e| PersistenceError::Ordering {
                    detail: e.to_string(),
                })?;
            let mut event = ctx.event(
                &recovering,
                Event::ActionFinished {
                    status: "unknown".into(),
                    cause: None,
                },
            );
            event.action_id = Some(unknown.action_id);
            store.record_action_terminal(&unknown, &event)?;
        }
        // M3 (REQ-PERS-007): a reversible write found running is restored to its
        // pre-execution snapshot; a restore that fails leaves the outcome unknown.
        if let Some(recovery) = &plan.action
            && let Some(snapshot) = recovery.restore_snapshot
        {
            let restored = match store.load_snapshot(snapshot)? {
                Some(record) => workspaces.restore(task_id, &record).map(|_| ()),
                None => Err(mimlys_core::spi::WorkspaceError::NotFound {
                    what: "snapshot".into(),
                }),
            };
            match restored {
                Ok(_) => {
                    let mut event = ctx.event(
                        &recovering,
                        Event::WorkspaceRollback {
                            snapshot_id: snapshot,
                        },
                    );
                    event.action_id = Some(recovery.action_id);
                    store.append_event(&event)?;
                }
                Err(_) => {
                    plan.disposition = RecoveryDisposition::UnknownOutcome;
                    plan.outcome = TaskInput::Recovered {
                        resume: false,
                        reason: HaltReason::UnknownOutcome,
                    };
                }
            }
        }
        store.append_event(&ctx.event(
            &recovering,
            Event::TaskRecovered {
                disposition: plan.disposition,
            },
        ))?;
        let after = recovering
            .transition(plan.outcome.clone(), ctx.now)
            .map_err(state_error)?;
        store.record_task_transition(
            &after,
            &ctx.event(&after, state_changed(&recovering, &after)),
        )?;
        out.push(Recovered {
            task_id,
            disposition: plan.disposition,
            state: after.state.name().to_owned(),
        });
    }
    expire_waiting(store, ctx)?;
    Ok(out)
}

/// What start-up did with a promotion it found `applying`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecoveredPromotion {
    /// The task.
    pub task_id: TaskId,
    /// The promotion, now recorded `failed`.
    pub promotion_id: mimlys_core::ids::PromotionId,
    /// Paths left as they are: edited since, or a directory that is not empty.
    pub kept: Vec<String>,
    /// Why the restore failed, when it did; the backup snapshot is kept.
    pub restore_error: Option<String>,
}

/// A promotion found `applying` did not finish (external design §14.5, §16.3): what it had
/// written is restored from its backup, which was committed before the project was touched, and
/// it is recorded `failed` with the reason. A path edited since is left as it is and listed.
pub fn recover_promotions(
    store: &Store,
    ctx: &RecoveryContext<'_>,
    workspaces: &dyn mimlys_core::spi::WorkspaceProvider,
) -> Result<Vec<RecoveredPromotion>, PersistenceError> {
    let mut out = Vec::new();
    for mut promotion in store.promotions_applying()? {
        let restored = match store.load_snapshot(promotion.backup_snapshot)? {
            Some(backup) => workspaces.revert_promotion(&promotion, &backup),
            None => Err(mimlys_core::spi::WorkspaceError::NotFound {
                what: "snapshot".into(),
            }),
        };
        let (kept, restore_error) = match restored {
            Ok(report) => (report.left, None),
            Err(e) => (Vec::new(), Some(e.to_string())),
        };
        let reason = match (&restore_error, kept.is_empty()) {
            (Some(e), _) => format!(
                "found applying at start; restoring the project from its backup failed ({e}); the backup snapshot is kept"
            ),
            (None, true) => {
                "found applying at start; the project was restored from its backup".to_owned()
            }
            (None, false) => format!(
                "found applying at start; the project was restored from its backup except {} path(s) edited since, which are left as they are: {}",
                kept.len(),
                kept.join(", ")
            ),
        };
        promotion.state = mimlys_core::snapshot::PromotionState::Failed;
        promotion.reason = Some(reason.clone());
        store.record_promotion(
            &promotion,
            &EventRecord {
                event_id: EventId::new(ctx.ids),
                at: ctx.now,
                actor: Actor::Runtime,
                task_id: Some(promotion.task_id),
                step_id: None,
                action_id: None,
                policy_hash: ctx.policy_hash.clone(),
                event: Event::WorkspacePromotionFailed { reason },
            },
        )?;
        out.push(RecoveredPromotion {
            task_id: promotion.task_id,
            promotion_id: promotion.promotion_id,
            kept,
            restore_error,
        });
    }
    Ok(out)
}

/// The last recovery step (external design §16.3; REQ-WS-012): the snapshot
/// journals a crash left are finished (their row exists) or undone (it does
/// not), keeping every object a snapshot row references. The event
/// `snapshot.journals_recovered` is recorded only when something was done.
pub fn recover_snapshot_journals(
    store: &Store,
    files: &SnapshotFiles,
    ctx: &RecoveryContext<'_>,
) -> Result<JournalReport, PersistenceError> {
    let report = files.recover_journals(store)?;
    if report != JournalReport::default() {
        store.append_event(&EventRecord {
            event_id: EventId::new(ctx.ids),
            at: ctx.now,
            actor: Actor::Runtime,
            task_id: None,
            step_id: None,
            action_id: None,
            policy_hash: ctx.policy_hash.clone(),
            event: Event::SnapshotJournalsRecovered {
                finished: u32::try_from(report.finished).unwrap_or(u32::MAX),
                undone: u32::try_from(report.undone).unwrap_or(u32::MAX),
                objects_removed: u32::try_from(report.objects_removed).unwrap_or(u32::MAX),
            },
        })?;
    }
    Ok(report)
}

fn expire_waiting(store: &Store, ctx: &RecoveryContext<'_>) -> Result<(), PersistenceError> {
    for task_id in store.tasks_in_state("waiting_user")? {
        let Some(task) = store.load_task(task_id)? else {
            continue;
        };
        let TaskState::WaitingUser { wait_kind } = task.state else {
            continue;
        };
        match wait_kind {
            WaitKind::Approval => {
                let Some(mut approval) = store.load_pending_approval(task_id)? else {
                    continue;
                };
                if approval.expire(ctx.now) {
                    let halted = task
                        .transition(TaskInput::Expired, ctx.now)
                        .map_err(state_error)?;
                    let mut event = ctx.event(
                        &halted,
                        Event::ApprovalDecided {
                            approval_id: approval.approval_id,
                            decision: ApprovalDecision::Expired,
                        },
                    );
                    event.action_id = Some(approval.action_id);
                    store.record_approval_decision(&halted, &approval, &event)?;
                    store.append_event(&ctx.event(&halted, state_changed(&task, &halted)))?;
                }
            }
            WaitKind::Question => {
                let Some(mut question) = store.load_pending_question(task_id)? else {
                    continue;
                };
                if question.expire(ctx.now) {
                    let halted = task
                        .transition(TaskInput::Expired, ctx.now)
                        .map_err(state_error)?;
                    store.record_question_update(
                        &halted,
                        &question,
                        &ctx.event(&halted, state_changed(&task, &halted)),
                    )?;
                }
            }
        }
    }
    Ok(())
}

fn state_error(e: mimlys_core::task::StateError) -> PersistenceError {
    PersistenceError::Ordering {
        detail: e.to_string(),
    }
}

/// Exposed for tests: an approval request type path.
pub type Approval = ApprovalRequest;

#[cfg(test)]
mod tests;
