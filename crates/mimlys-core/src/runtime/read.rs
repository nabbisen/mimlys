//! The reading half of the control API (REQ-PERS-013, RFC 013): the views a
//! command that only reads needs (`get_task`, `list_tasks`, `get_events`,
//! `get_metrics`, the footprint, the runtime's activity, a promotion's
//! plan). They are written once, over [`PersistenceRead`] and
//! [`WorkspaceRead`], and both [`Runtime`](super::Runtime) and [`Reader`]
//! call them. A [`Reader`] holds nothing that writes: its persistence and its
//! workspaces are the reading traits, so a command built on it cannot call a
//! writing method; no runtime check stands in for that.

use super::RuntimeError;
use super::control::TaskView;
use crate::clock::{Clock, Instant};
use crate::ids::TaskId;
use crate::snapshot::{Footprint, Promotion, PromotionPlan, SnapshotRecord};
use crate::spi::{Metrics, PersistenceRead, PositionedEvent, TaskSummary, WorkspaceRead};
use crate::task::{PendingItem, Task, TaskState, WaitKind};
use std::sync::Arc;

pub(super) fn load_task(
    persistence: &dyn PersistenceRead,
    task_id: TaskId,
) -> Result<Task, RuntimeError> {
    persistence
        .load_task(task_id)?
        .ok_or(RuntimeError::NotFound {
            what: "task".into(),
        })
}

/// What the task holds on disk (RFC 006 §6): the latest snapshot's bytes,
/// the objects exclusive to the task against those shared, the blobs.
pub(super) fn footprint(
    persistence: &dyn PersistenceRead,
    workspaces: &dyn WorkspaceRead,
    task_id: TaskId,
    snapshots: &[SnapshotRecord],
    promotions: &[Promotion],
) -> Result<Footprint, RuntimeError> {
    let mut per_task: std::collections::BTreeMap<crate::hash::Hash, (u64, u64)> =
        std::collections::BTreeMap::new();
    for s in snapshots {
        if let Ok(manifest) = workspaces.manifest(s) {
            for (hash, size) in manifest.objects() {
                let e = per_task.entry(hash).or_insert((size, 0));
                e.1 += 1;
            }
        }
    }
    let hashes: Vec<crate::hash::Hash> = per_task.keys().cloned().collect();
    let mut exclusive = 0;
    let mut shared = 0;
    for (hash, size, refs) in persistence.object_references(&hashes)? {
        let mine = per_task.get(&hash).map(|(_, n)| *n).unwrap_or(0);
        if refs <= mine {
            exclusive += size;
        } else {
            shared += size;
        }
    }
    Ok(Footprint {
        workspace_bytes: workspaces.tree_bytes(task_id).unwrap_or(0),
        snapshots: snapshots.len() as u64,
        object_bytes_exclusive: exclusive,
        object_bytes_shared: shared,
        blob_bytes: persistence.blob_bytes(task_id)?,
        promotions: promotions
            .iter()
            .filter(|p| p.reversed_at.is_none())
            .count() as u64,
    })
}

/// The task with its pending items, plan and grants.
pub(super) fn get_task(
    persistence: &dyn PersistenceRead,
    workspaces: &dyn WorkspaceRead,
    now: Instant,
    task_id: TaskId,
) -> Result<TaskView, RuntimeError> {
    let task = load_task(persistence, task_id)?;
    let pending_approval = match task.pending {
        Some(PendingItem::Approval { .. }) => persistence.load_pending_approval(task_id)?,
        _ => None,
    };
    let pending_question = match task.pending {
        Some(PendingItem::Question { .. }) => persistence.load_pending_question(task_id)?,
        _ => None,
    };
    let plan = persistence.load_plan(task_id)?;
    let grants = persistence
        .load_grants(task_id)?
        .into_iter()
        .filter(|g| g.is_active(now))
        .collect();
    let snapshots = persistence.load_snapshots(task_id)?;
    let promotions = persistence.load_promotions(task_id)?;
    let footprint = footprint(persistence, workspaces, task_id, &snapshots, &promotions)?;
    Ok(TaskView {
        task,
        pending_approval,
        pending_question,
        plan,
        grants,
        snapshots,
        promotions,
        footprint,
    })
}

/// Tasks `running` and tasks waiting for the user, with what they wait for.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Activity {
    /// Tasks in `running`.
    pub running: Vec<TaskId>,
    /// Tasks waiting for the user, with what they wait for.
    pub pending: Vec<(TaskId, WaitKind)>,
}

pub(super) fn activity(persistence: &dyn PersistenceRead) -> Result<Activity, RuntimeError> {
    let mut pending = Vec::new();
    for id in persistence.tasks_in_state("waiting_user")? {
        if let Some(task) = persistence.load_task(id)?
            && let TaskState::WaitingUser { wait_kind } = task.state
        {
            pending.push((id, wait_kind));
        }
    }
    Ok(Activity {
        running: persistence.tasks_in_state("running")?,
        pending,
    })
}

/// Phase 1 of a promotion (REQ-WS-007): the plan, or the conflicts. It
/// writes nothing: the plan reads the base manifest, the working tree and
/// the project.
pub(super) fn promotion_plan(
    persistence: &dyn PersistenceRead,
    workspaces: &dyn WorkspaceRead,
    task_id: TaskId,
) -> Result<PromotionPlan, RuntimeError> {
    let task = load_task(persistence, task_id)?;
    if matches!(task.state, TaskState::Running | TaskState::Recovering) {
        return Err(RuntimeError::State(crate::task::StateError::Illegal {
            from: task.state.name(),
            input: "promote_workspace",
        }));
    }
    let base = base_snapshot(persistence, task_id)?;
    Ok(workspaces.promotion_plan(task_id, &base, &task.scope.root)?)
}

/// The task's first snapshot: `base` for a copy, `fork` for a fork.
pub(super) fn base_snapshot(
    persistence: &dyn PersistenceRead,
    task_id: TaskId,
) -> Result<SnapshotRecord, RuntimeError> {
    persistence
        .load_snapshots(task_id)?
        .into_iter()
        .find(|s| {
            matches!(
                s.reason,
                crate::snapshot::SnapshotReason::Base | crate::snapshot::SnapshotReason::Fork
            )
        })
        .ok_or(RuntimeError::NotFound {
            what: "base snapshot".into(),
        })
}

/// The views of a command that only reads. Built over the reading traits
/// alone (REQ-PERS-013).
pub struct Reader {
    persistence: Arc<dyn PersistenceRead>,
    workspaces: Arc<dyn WorkspaceRead>,
    clock: Arc<dyn Clock>,
}

impl Reader {
    /// A reader over a read-only store and the reading half of the workspaces.
    pub fn new(
        persistence: Arc<dyn PersistenceRead>,
        workspaces: Arc<dyn WorkspaceRead>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            persistence,
            workspaces,
            clock,
        }
    }

    /// The store, reading methods only.
    pub fn persistence(&self) -> &dyn PersistenceRead {
        self.persistence.as_ref()
    }

    /// The task with its pending items, plan, grants, snapshots and footprint.
    pub fn get_task(&self, task_id: TaskId) -> Result<TaskView, RuntimeError> {
        get_task(
            self.persistence.as_ref(),
            self.workspaces.as_ref(),
            self.clock.now(),
            task_id,
        )
    }

    /// Tasks, newest first.
    pub fn list_tasks(&self) -> Result<Vec<TaskSummary>, RuntimeError> {
        Ok(self.persistence.list_tasks()?)
    }

    /// Events after a position (REQ-OBS-002).
    pub fn get_events(
        &self,
        task_id: Option<TaskId>,
        since: u64,
        limit: usize,
    ) -> Result<Vec<PositionedEvent>, RuntimeError> {
        Ok(self.persistence.events_since(since, task_id, limit)?)
    }

    /// Metrics (REQ-OBS-003).
    pub fn get_metrics(&self, task_id: Option<TaskId>) -> Result<Metrics, RuntimeError> {
        Ok(self.persistence.metrics(task_id)?)
    }

    /// What the task holds on disk.
    pub fn footprint(
        &self,
        task_id: TaskId,
        snapshots: &[SnapshotRecord],
        promotions: &[Promotion],
    ) -> Result<Footprint, RuntimeError> {
        footprint(
            self.persistence.as_ref(),
            self.workspaces.as_ref(),
            task_id,
            snapshots,
            promotions,
        )
    }

    /// Tasks `running` and tasks waiting for the user (REQ-OBS-006).
    pub fn activity(&self) -> Result<Activity, RuntimeError> {
        activity(self.persistence.as_ref())
    }

    /// What the store holds, by kind and by task, from its records (REQ-WS-013).
    pub fn holdings(&self) -> Result<super::storage::Holdings, RuntimeError> {
        super::storage::holdings(self.persistence.as_ref(), self.workspaces.as_ref())
    }

    /// Phase 1 of a promotion: the plan or the conflicts; writes nothing.
    pub fn promotion_plan(&self, task_id: TaskId) -> Result<PromotionPlan, RuntimeError> {
        promotion_plan(self.persistence.as_ref(), self.workspaces.as_ref(), task_id)
    }
}
