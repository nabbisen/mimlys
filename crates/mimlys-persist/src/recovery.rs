//! Applying recovery at startup (external design §16.3; REQ-PERS-007):
//! every task found `running` goes through `recovering`, its in-flight
//! action becomes `unknown`, and the plan of `mimlys_core::recovery`
//! decides whether the task halts or, by policy, runs again. A task found
//! `recovering` is recovered again, every step repeatable. Expired
//! approvals and questions halt their tasks.

use crate::snapshots::{JournalReport, SnapshotFiles};
use crate::sqlite::Store;
use mimlys_core::action::{ActionInput, ActionStatus};
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

/// A task that recovery left as it is, because a record of it holds a word that a newer release
/// of mimlys wrote and this release does not know (REQ-MAIN-006; handoff 004e, item 2): it is
/// not halted, rewritten or recovered, and the start does not fail. The user goes back to the
/// release that wrote it to act on it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LeftAsIs {
    /// The task.
    pub task_id: TaskId,
    /// The kind of record that holds the word (`task`, `action`, `event`, …).
    pub record: String,
    /// The word and the field that holds it.
    pub word: mimlys_core::newer::UnknownWord,
}

/// What a recovery pass did: the tasks it recovered, and the tasks it left as they are.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RecoveryPass {
    /// The tasks recovered.
    pub recovered: Vec<Recovered>,
    /// The tasks left as they are, each once.
    pub left: Vec<LeftAsIs>,
}

/// The first word a task's records hold that this release does not know, with the kind of record
/// that holds it. A damaged record is not one: it fails where it is read, as it always did.
fn first_newer(store: &Store, task_id: TaskId) -> Result<Option<LeftAsIs>, PersistenceError> {
    Ok(mimlys_core::newer::scan(store, task_id)?
        .into_iter()
        .next()
        .map(|record| LeftAsIs {
            task_id,
            record: record.kind.name().to_owned(),
            word: record.newer.first().clone(),
        }))
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
            TaskState::Halted { halt_reason, .. } => Some(*halt_reason),
            _ => None,
        },
        rollback_failed: false,
        halt_detail: match &to.state {
            TaskState::Halted { halt_detail, .. } => halt_detail.clone(),
            _ => None,
        },
    }
}

/// Recovers every task found `running` or `recovering`, then processes expired user items.
///
/// A task found `recovering` is a recovery the process did not finish (a crash, an interrupt,
/// §16.3): it is recovered again from the start. Each step can be repeated: the restore of the
/// snapshot, the mark of the action (the action is marked only when it is still `running`), and
/// `task.recovered`, which is recorded once. The restore comes before the mark, so that an
/// action found `unknown` in a `recovering` task always had its restore done or tried again.
pub fn recover_all(
    store: &Store,
    ctx: &RecoveryContext<'_>,
    workspaces: &dyn mimlys_core::spi::WorkspaceProvider,
) -> Result<Vec<Recovered>, PersistenceError> {
    Ok(recover_all_reporting(store, ctx, workspaces)?.recovered)
}

/// [`recover_all`], with the tasks it left as they are.
///
/// A task that holds a word of a newer release is left exactly as it is: not recovered, not
/// halted, not expired, and the pass goes on with the others and does not fail (REQ-MAIN-006).
/// What it would have done to it, only the release that wrote the word can decide.
pub fn recover_all_reporting(
    store: &Store,
    ctx: &RecoveryContext<'_>,
    workspaces: &dyn mimlys_core::spi::WorkspaceProvider,
) -> Result<RecoveryPass, PersistenceError> {
    let mut out = Vec::new();
    let mut left = Vec::new();
    // A task whose own state this release does not know cannot be classified: it is none of the
    // tasks below, and is named so that the user knows it is not tended to.
    for summary in store.list_tasks()? {
        if let Some(word) = summary.unknown.iter().find(|w| w.field == "state") {
            left.push(LeftAsIs {
                task_id: summary.task_id,
                record: "task".into(),
                word: word.clone(),
            });
        }
    }
    let mut found = store.tasks_in_state("recovering")?;
    found.extend(store.tasks_in_state("running")?);
    for task_id in found {
        if let Some(newer) = first_newer(store, task_id)? {
            left.push(newer);
            continue;
        }
        let Some(task) = store.load_task(task_id)? else {
            continue;
        };
        let mut in_flight = store.load_running_action(task_id)?;
        if in_flight.is_none() && task.state == TaskState::Recovering {
            in_flight = store.load_marked_by_recovery(task_id)?;
        }
        let snapshot_before = in_flight.as_ref().and_then(|a| a.snapshot_before);
        let Some(mut plan) =
            plan_recovery(&task, in_flight.as_ref(), &ctx.recovery, snapshot_before)
        else {
            continue;
        };
        let recovering = if task.state == TaskState::Recovering {
            task
        } else {
            let recovering = task
                .transition(TaskInput::StartRecovery, ctx.now)
                .map_err(state_error)?;
            store.record_task_transition(
                &recovering,
                &ctx.event(&recovering, state_changed(&task, &recovering)),
            )?;
            recovering
        };
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
        // Only an action still `running` is marked: one an earlier pass marked stays as it is.
        if let (Some(action), Some(_)) = (in_flight, &plan.action)
            && action.status == ActionStatus::Running
        {
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
        if !store.recovered_recorded(task_id)? {
            store.append_event(&ctx.event(
                &recovering,
                Event::TaskRecovered {
                    disposition: plan.disposition,
                },
            ))?;
        }
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
    expire_waiting(store, ctx, &mut left)?;
    Ok(RecoveryPass {
        recovered: out,
        left,
    })
}

/// What the start did with the journals of task deletions (RFC 006 §6; handoff 006b).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeletionReport {
    /// Deletions that had committed: their files were removed now.
    pub finished: usize,
    /// Journals of deletions that never committed: nothing was removed, the journal is gone.
    pub discarded: usize,
    /// Paths a finished deletion still could not remove; its journal stays for the next start.
    pub failed: Vec<String>,
}

/// The files of a task deletion that committed are removed, every one attempted; a journal whose
/// deletion did not commit (its task still holds its rows) is dropped. The journal of a deletion
/// is written, fsynced, before its rows go, so a crash or a failed removal never leaves files
/// that no row names.
pub fn recover_deletions(
    store: &Store,
    files: &SnapshotFiles,
    workspaces: &dyn mimlys_core::spi::WorkspaceProvider,
) -> Result<DeletionReport, PersistenceError> {
    let mut report = DeletionReport::default();
    for (task_id, rows) in files.pending_deletions()? {
        // A task that holds a word of a newer release still holds its rows: its deletion did
        // not commit (REQ-MAIN-006).
        let gone = match store.load_task(task_id) {
            Ok(task) => task.is_none(),
            Err(PersistenceError::NewerRecord { .. }) => false,
            Err(e) => return Err(e),
        };
        let marked_deleted = match store.load_workspace_state(task_id) {
            Ok(state) => state == Some(mimlys_core::snapshot::WorkspaceState::Deleted),
            Err(PersistenceError::NewerRecord { .. }) => false,
            Err(e) => return Err(e),
        };
        let committed = gone || marked_deleted;
        if !committed {
            files.finish_deletion(task_id)?;
            report.discarded += 1;
            continue;
        }
        match workspaces.delete_files(task_id, &rows) {
            Ok(()) => report.finished += 1,
            Err(mimlys_core::spi::WorkspaceError::NotRemoved { paths, .. }) => {
                report.failed.extend(paths);
            }
            Err(e) => report.failed.push(e.to_string()),
        }
    }
    Ok(report)
}

/// What the start did with the files of `<data>/blobs` that no row names.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BlobSweep {
    /// The files removed (their names, which are blob ids).
    pub removed: Vec<String>,
    /// The files that could not be removed, with the reason's kind; tried again at the next start.
    pub failed: Vec<String>,
}

/// Removes each file of `<data>/blobs` that has no row (REQ-PERS-005): a blob's file is
/// written and synced before its row (and its row before the record of the execution that
/// names it), so a crash between the two leaves a file that nothing names. It is the store's
/// writer that sweeps, at its start and before anything is written: the lock keeps every other
/// writer out, so no file is waiting for its row.
///
/// Only what this program makes is removed: a regular file named as a blob id. Anything else in
/// the directory (a directory, a link, a file of another name) is left alone, and `status
/// --storage` does not count it. A row no execution names (a crash after the row) is the task's
/// and goes with it.
pub fn recover_blob_files(store: &Store) -> Result<BlobSweep, PersistenceError> {
    let dir = store.data_dir().join("blobs");
    let mut report = BlobSweep::default();
    for name in store.orphan_blobs()? {
        if uuid::Uuid::parse_str(&name).is_err() {
            continue;
        }
        let path = dir.join(&name);
        // A link is not followed, and not removed: it is not a file this program wrote.
        match std::fs::symlink_metadata(&path) {
            Ok(m) if m.is_file() => {}
            _ => continue,
        }
        match std::fs::remove_file(&path) {
            Ok(()) => report.removed.push(name),
            Err(e) => report.failed.push(format!("{name} ({})", e.kind())),
        }
    }
    Ok(report)
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
    /// The command that restores the project from the kept backup, when the restore failed.
    pub restore_hint: Option<String>,
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
        let backup = match store.load_snapshot(promotion.backup_snapshot) {
            Ok(backup) => backup,
            // The backup is a record a newer release wrote: only that release can say how to
            // restore from it, and the promotion is left as it is (REQ-MAIN-006).
            Err(PersistenceError::NewerRecord { .. }) => continue,
            Err(e) => return Err(e),
        };
        let restored = match backup {
            Some(backup) => workspaces.revert_promotion(
                &promotion,
                &backup,
                mimlys_core::spi::EditedPaths::Keep,
            ),
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
                "found applying at start; restoring the project from its backup failed ({e}); {}",
                promotion.restore_hint()
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
        // A restore that failed is not tried again by a later start; the user's command is
        // the way back (`rollback --promotion`).
        promotion.restored = restore_error.is_none();
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
            restore_hint: restore_error.as_ref().map(|_| promotion.restore_hint()),
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

fn expire_waiting(
    store: &Store,
    ctx: &RecoveryContext<'_>,
    left: &mut Vec<LeftAsIs>,
) -> Result<(), PersistenceError> {
    for task_id in store.tasks_in_state("waiting_user")? {
        if let Some(newer) = first_newer(store, task_id)? {
            left.push(newer);
            continue;
        }
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
mod found_recovering_tests;
#[cfg(test)]
mod tests;
