//! The reading half of the control API (REQ-PERS-013, RFC 013): the views a
//! command that only reads needs (`get_task`, `list_tasks`, `get_events`,
//! `get_metrics`, the footprint, the runtime's activity, a promotion's
//! plan). They are written once, over [`PersistenceRead`] and
//! [`WorkspaceRead`], and both [`Runtime`](super::Runtime) and [`Reader`]
//! call them. A [`Reader`] holds nothing that writes: its persistence and its
//! workspaces are the reading traits, so a command built on it cannot call a
//! writing method; no runtime check stands in for that.

use super::RuntimeError;
use super::control::{TaskView, UnknownAction};
use crate::action::Action;
use crate::action::ActionStatus;
use crate::approval::{ApprovalRequest, ApprovalStatus};
use crate::clock::{Clock, Instant};
use crate::event::Event;
use crate::grant::Grant;
use crate::ids::TaskId;
use crate::newer::{Newer, NewerRecord, Read, RecordKind, UnknownWord};
use crate::question::{Question, QuestionStatus};
use crate::snapshot::{Footprint, Promotion, PromotionPlan, SnapshotRecord};
use crate::spi::{
    Metrics, PersistenceRead, PositionedEvent, PositionedRead, TaskSummary, WorkspaceRead,
};
use crate::task::{HaltReason, PendingItem, PlanVersion, Task, TaskState, WaitKind};
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
        promotions: promotions.iter().filter(|p| p.can_be_reversed()).count() as u64,
    })
}

/// What a task's view is made of besides the task: its pending items, plan, grants, snapshots,
/// promotions and actions.
struct Parts {
    pending_approval: Option<ApprovalRequest>,
    pending_question: Option<Question>,
    plan: Option<PlanVersion>,
    grants: Vec<Grant>,
    snapshots: Vec<SnapshotRecord>,
    promotions: Vec<Promotion>,
    actions: Vec<Action>,
}

/// The task with its pending items, plan and grants. A record that holds a word this release does
/// not know is the refusal that names it (REQ-MAIN-006): this is the view a command that changes
/// or runs the task works from. [`show_task`] is the one that shows what it can.
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
    let parts = Parts {
        pending_approval,
        pending_question,
        plan: persistence.load_plan(task_id)?,
        grants: persistence.load_grants(task_id)?,
        snapshots: persistence.load_snapshots(task_id)?,
        promotions: persistence.load_promotions(task_id)?,
        actions: persistence.load_actions(task_id)?,
    };
    view_of(persistence, workspaces, now, task, parts)
}

fn view_of(
    persistence: &dyn PersistenceRead,
    workspaces: &dyn WorkspaceRead,
    now: Instant,
    task: Task,
    parts: Parts,
) -> Result<TaskView, RuntimeError> {
    let task_id = task.task_id;
    let Parts {
        mut pending_approval,
        pending_question,
        plan,
        grants,
        snapshots,
        promotions,
        actions,
    } = parts;
    // A request recorded before the arguments were (a task that waited across
    // an upgrade) has none: the action holds them, without the tool's paths
    // resolved (REQ-APR-002: the user can always see every argument).
    if let Some(approval) = pending_approval.as_mut().filter(|a| a.arguments.is_none())
        && let Some(action) = actions.iter().find(|a| a.action_id == approval.action_id)
        && let Some(arguments) = action.content().arguments().as_object()
    {
        approval.arguments = Some(crate::approval::argument_views(arguments, &[], &[], None));
    }
    let grants = grants.into_iter().filter(|g| g.is_active(now)).collect();
    let footprint = footprint(persistence, workspaces, task_id, &snapshots, &promotions)?;
    let unknown_actions = actions
        .iter()
        .filter(|a| a.status == ActionStatus::Unknown)
        .map(|a| UnknownAction {
            action_id: a.action_id,
            tool: a.content().tool().to_owned(),
            effect: a.content().effect().kind,
        })
        .collect();
    let unreadable = match task.state {
        TaskState::Halted {
            halt_reason: HaltReason::WorkspaceUnreadable,
            ..
        } => unreadable_path(persistence, task_id)?,
        _ => None,
    };
    let rollback_failed = match task.state {
        TaskState::Halted { .. } => last_halt_followed_a_failed_rollback(persistence, task_id)?,
        _ => false,
    };
    let (unreadable, unreadable_kind) = match unreadable {
        Some((path, kind)) => (Some(path), kind),
        None => (None, None),
    };
    Ok(TaskView {
        task,
        pending_approval,
        pending_question,
        plan,
        grants,
        snapshots,
        promotions,
        footprint,
        unknown_actions,
        unreadable,
        unreadable_kind,
        rollback_failed,
        waiting: None,
    })
}

/// A task as `task show` meets it (REQ-MAIN-006): whole, with the records of it that this
/// release cannot read named; or its own record holds a word this release does not know, and what
/// is shown is the record as written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TaskShown {
    /// The task reads. `newer` lists the records of it that hold a word this release does not
    /// know (an action whose status is newer, a grant of a newer kind, an event): they are left
    /// out of the view, which shows every other record as it always did.
    Whole {
        /// The view.
        view: Box<TaskView>,
        /// The records left out.
        newer: Vec<NewerRecord>,
    },
    /// The task's own record holds a word this release does not know: its state, its wait kind,
    /// its halt reason. Nothing is made of it but what it says.
    Newer {
        /// The task.
        task_id: TaskId,
        /// Its record as written, with the words.
        record: Newer,
        /// The other records of the task that hold such a word.
        others: Vec<NewerRecord>,
    },
}

/// The records of one kind that read, and those that hold a word this release does not know.
fn split<T: crate::newer::Stored>(
    persistence: &dyn PersistenceRead,
    kind: RecordKind,
    task_id: TaskId,
    newer: &mut Vec<NewerRecord>,
) -> Result<Vec<T>, RuntimeError> {
    let mut known = Vec::new();
    for (key, record) in crate::newer::read_records::<T>(persistence, kind, task_id)? {
        match record {
            Read::Known(value) => known.push(value),
            Read::Newer(n) => newer.push(NewerRecord {
                kind,
                key,
                newer: n,
            }),
        }
    }
    Ok(known)
}

/// The task as `task show` shows it: every record that reads is read as it always was, and a
/// record that holds a word this release does not know is named and left out, never a failure.
pub(super) fn show_task(
    persistence: &dyn PersistenceRead,
    workspaces: &dyn WorkspaceRead,
    now: Instant,
    task_id: TaskId,
) -> Result<TaskShown, RuntimeError> {
    let raw = persistence
        .raw_records(RecordKind::Task, task_id)?
        .into_iter()
        .next()
        .ok_or(RuntimeError::NotFound {
            what: "task".into(),
        })?;
    let task = match Read::<Task>::parse(&raw.text)? {
        Read::Known(task) => task,
        Read::Newer(record) => {
            let others = newer_records_but_task(persistence, task_id)?;
            return Ok(TaskShown::Newer {
                task_id,
                record,
                others,
            });
        }
    };
    let mut newer = Vec::new();
    let approvals =
        split::<ApprovalRequest>(persistence, RecordKind::Approval, task_id, &mut newer)?;
    let questions = split::<Question>(persistence, RecordKind::Question, task_id, &mut newer)?;
    let plans = split::<PlanVersion>(persistence, RecordKind::Plan, task_id, &mut newer)?;
    let parts = Parts {
        pending_approval: match task.pending {
            Some(PendingItem::Approval { .. }) => approvals
                .into_iter()
                .rfind(|a| matches!(a.status, ApprovalStatus::Pending | ApprovalStatus::Approved)),
            _ => None,
        },
        pending_question: match task.pending {
            Some(PendingItem::Question { .. }) => questions
                .into_iter()
                .rfind(|q| q.status == QuestionStatus::Pending),
            _ => None,
        },
        plan: plans.into_iter().max_by_key(|p| p.version),
        grants: split::<Grant>(persistence, RecordKind::Grant, task_id, &mut newer)?,
        snapshots: split::<SnapshotRecord>(persistence, RecordKind::Snapshot, task_id, &mut newer)?,
        promotions: split::<Promotion>(persistence, RecordKind::Promotion, task_id, &mut newer)?,
        actions: split::<Action>(persistence, RecordKind::Action, task_id, &mut newer)?,
    };
    // The records `show` does not use are listed as well: the task holds them.
    for kind in [
        RecordKind::Step,
        RecordKind::Execution,
        RecordKind::Observation,
        RecordKind::Event,
    ] {
        for record in persistence.raw_records(kind, task_id)? {
            if let Ok(Some(n)) = kind.check(&record.text) {
                newer.push(NewerRecord {
                    kind,
                    key: record.key,
                    newer: n,
                });
            }
        }
    }
    let view = view_of(persistence, workspaces, now, task, parts)?;
    Ok(TaskShown::Whole {
        view: Box::new(view),
        newer,
    })
}

/// Every record of the task but its own that holds a word this release does not know.
fn newer_records_but_task(
    persistence: &dyn PersistenceRead,
    task_id: TaskId,
) -> Result<Vec<NewerRecord>, RuntimeError> {
    Ok(crate::newer::scan(persistence, task_id)?
        .into_iter()
        .filter(|r| r.kind != RecordKind::Task)
        .collect())
}

/// The snapshots and the promotions of a task that this release reads, and how many records of
/// them it does not (`task list`'s footprint, `status --storage`): a task that holds a newer
/// snapshot is still listed, with what it can count.
pub fn readable_footprint_records(
    persistence: &dyn PersistenceRead,
    task_id: TaskId,
) -> Result<(Vec<SnapshotRecord>, Vec<Promotion>, usize), RuntimeError> {
    let mut newer = Vec::new();
    let snapshots =
        split::<SnapshotRecord>(persistence, RecordKind::Snapshot, task_id, &mut newer)?;
    let promotions = split::<Promotion>(persistence, RecordKind::Promotion, task_id, &mut newer)?;
    Ok((snapshots, promotions, newer.len()))
}

/// The event, when it reads whole.
fn known_event(read: &PositionedRead) -> Option<PositionedEvent> {
    match &read.record {
        Read::Known(record) => Some(PositionedEvent {
            position: read.position,
            record: record.clone(),
        }),
        Read::Newer(_) => None,
    }
}

/// Whether the task's last halt followed a rollback of the workspace that failed: the last
/// `task.state_changed` to `halted` says so (`rollback_failed`).
fn last_halt_followed_a_failed_rollback(
    persistence: &dyn PersistenceRead,
    task_id: TaskId,
) -> Result<bool, RuntimeError> {
    let mut found = false;
    let mut position = 0;
    loop {
        let page = persistence.read_events_since(position, Some(task_id), 1_000)?;
        let Some(last) = page.last() else {
            return Ok(found);
        };
        position = last.position;
        // An event a newer release wrote is none of these.
        for event in page.iter().filter_map(known_event) {
            if let Event::TaskStateChanged {
                reason: Some(_),
                rollback_failed,
                ..
            } = &event.record.event
            {
                found = *rollback_failed;
            }
        }
    }
}

/// The path of the task's last `workspace.unreadable` event, with the kind of the failure when
/// the event has one.
fn unreadable_path(
    persistence: &dyn PersistenceRead,
    task_id: TaskId,
) -> Result<Option<(String, Option<String>)>, RuntimeError> {
    let mut found = None;
    let mut position = 0;
    loop {
        let page = persistence.read_events_since(position, Some(task_id), 1_000)?;
        let Some(last) = page.last() else {
            return Ok(found);
        };
        position = last.position;
        for event in page.iter().filter_map(known_event) {
            if let Event::WorkspaceUnreadable { path, kind } = &event.record.event {
                found = Some((path.clone(), kind.clone()));
            }
        }
    }
}

/// What a task waits for, as a reader finds it: a kind this release has, or the word a newer
/// release wrote (REQ-MAIN-006). It serializes as the word.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(untagged)]
pub enum WaitWord {
    /// A wait kind of this release.
    Known(WaitKind),
    /// A word of a newer release, as written.
    Unknown(String),
}

/// Tasks `running` and tasks waiting for the user, with what they wait for.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Activity {
    /// Tasks in `running`.
    pub running: Vec<TaskId>,
    /// Tasks waiting for the user, with what they wait for.
    pub pending: Vec<(TaskId, WaitWord)>,
    /// Tasks whose state is a word this release does not know, with the word.
    pub unknown_state: Vec<(TaskId, String)>,
}

impl Activity {
    /// The unknown words of the activity, each with the field of the status that holds it
    /// (`pending`, `unknown_state`).
    pub fn unknown_words(&self) -> Vec<UnknownWord> {
        let waits = self.pending.iter().filter_map(|(_, w)| match w {
            WaitWord::Unknown(word) => Some(UnknownWord::new("pending", word.clone())),
            WaitWord::Known(_) => None,
        });
        let states = self
            .unknown_state
            .iter()
            .map(|(_, word)| UnknownWord::new("unknown_state", word.clone()));
        waits.chain(states).collect()
    }
}

pub(super) fn activity(persistence: &dyn PersistenceRead) -> Result<Activity, RuntimeError> {
    let mut pending = Vec::new();
    for id in persistence.tasks_in_state("waiting_user")? {
        let Some(raw) = persistence
            .raw_records(RecordKind::Task, id)?
            .into_iter()
            .next()
        else {
            continue;
        };
        match Read::<Task>::parse(&raw.text)? {
            Read::Known(task) => {
                if let TaskState::WaitingUser { wait_kind } = task.state {
                    pending.push((id, WaitWord::Known(wait_kind)));
                }
            }
            // A task waiting for something this release does not know is still waiting: it is
            // listed, with the word as written.
            Read::Newer(newer) => {
                if let Some(word) = newer.text("wait_kind") {
                    let kind = serde_json::from_value::<WaitKind>(serde_json::json!(word)).ok();
                    pending.push((
                        id,
                        match kind {
                            Some(kind) => WaitWord::Known(kind),
                            None => WaitWord::Unknown(word.to_owned()),
                        },
                    ));
                }
            }
        }
    }
    let unknown_state = persistence
        .list_tasks()?
        .into_iter()
        .filter_map(|t| {
            t.unknown
                .iter()
                .find(|w| w.field == "state")
                .map(|w| (t.task_id, w.word.clone()))
        })
        .collect();
    Ok(Activity {
        running: persistence.tasks_in_state("running")?,
        pending,
        unknown_state,
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

    /// The task as `task show` shows it: what it can read, and the records that hold a word
    /// this release does not know, named (REQ-MAIN-006). [`Reader::get_task`] refuses them.
    pub fn show_task(&self, task_id: TaskId) -> Result<TaskShown, RuntimeError> {
        show_task(
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

    /// Events after a position (REQ-OBS-002), as a reader meets them: an event that holds a word
    /// a newer release wrote is kept as written, marked, and never fails the read
    /// (REQ-MAIN-006). The control API's [`Runtime::get_events`](super::Runtime::get_events)
    /// refuses it instead, naming the word.
    pub fn get_events(
        &self,
        task_id: Option<TaskId>,
        since: u64,
        limit: usize,
    ) -> Result<Vec<PositionedRead>, RuntimeError> {
        Ok(self.persistence.read_events_since(since, task_id, limit)?)
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
