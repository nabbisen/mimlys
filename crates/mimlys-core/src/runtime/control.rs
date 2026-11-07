//! The control API (external design §4; REQ-TASK-001, 005, 006, 012;
//! REQ-APR-007, 009, 010; REQ-CAP-004, 005; REQ-OBS-002, 003, 006). Every
//! verb acts under a `Session`, writes one transition with its event, and
//! never submits a decision, validates an action or executes anything.

use super::loop_::{PROVIDER, state_changed};
use super::{Runtime, RuntimeError, Session};
use crate::action::{Action, ActionInput, ActionStatus};
use crate::approval::{ApprovalRequest, ApprovalStatus, InvalidationReason, UserDecision};
use crate::clock::Instant;
use crate::event::{Actor, ApprovalDecision, Event};
use crate::grant::{Grant, GrantKind, GrantSet};
use crate::ids::{GrantId, SnapshotId, TaskId};
use crate::limits::{
    LIM_D4_TEXT_CHARS, LIM_T5_DEADLINE_MS, LIM_T5_DEADLINE_MS_RANGE, LIM_X1_RUNNING_TASKS,
};
use crate::question::Question;
use crate::rejection::RejectionCode;
use crate::snapshot::{Footprint, Promotion, PromotionPlan, SnapshotRecord, WorkspaceState};
use crate::spi::{Metrics, PositionedEvent, TaskSummary};
use crate::step::{Budgets, StepOutcome, StepResult};
use crate::task::{
    CompletionCheck, Extension, ExtensionCeiling, PendingItem, PlanVersion, ScopeSpec, Task,
    TaskInput, TaskSpec, TaskState, TaskVersions, WaitKind,
};

/// A request to create a task (REQ-TASK-001).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CreateTask {
    /// The goal.
    pub goal: String,
    /// The project scope.
    pub scope: ScopeSpec,
    /// Profile; the policy's default when `None`.
    pub profile: Option<String>,
    /// Extra grants within the ceiling.
    pub grants: Vec<GrantKind>,
    /// Budgets within the ceiling; the profile's when `None`.
    pub budgets: Option<Budgets>,
    /// Deadline from creation in milliseconds; LIM-T5 when `None`.
    pub deadline_ms: Option<i64>,
    /// A completion check.
    pub completion_check: Option<CompletionCheck>,
    /// Leave the task in `created` instead of starting it.
    pub hold: bool,
    /// Fork: the task whose snapshot the workspace is materialized from (REQ-TASK-004).
    pub from_task: Option<TaskId>,
    /// Fork: the snapshot; the source's latest when `None`.
    pub from_snapshot: Option<SnapshotId>,
}

/// A task as the control API shows it.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct TaskView {
    /// The task.
    pub task: Task,
    /// The pending approval, when waiting for one.
    pub pending_approval: Option<ApprovalRequest>,
    /// The pending question, when waiting for one.
    pub pending_question: Option<Question>,
    /// The current plan.
    pub plan: Option<PlanVersion>,
    /// Active grants.
    pub grants: Vec<Grant>,
    /// Snapshots, oldest first.
    pub snapshots: Vec<SnapshotRecord>,
    /// Promotions, oldest first.
    pub promotions: Vec<Promotion>,
    /// What the task holds on disk.
    pub footprint: Footprint,
}

/// A status whose backend descriptor was moved into a richer view (the
/// CLI's `backend` object) serializes without it.
fn backend_is_taken(backend: &crate::spi::BackendDescriptor) -> bool {
    backend.kind.is_empty()
}

/// The runtime's status (REQ-OBS-006).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct RuntimeStatus {
    /// Runtime version.
    pub version: String,
    /// The backend in force: kind, model, locality.
    #[serde(skip_serializing_if = "backend_is_taken")]
    pub backend: crate::spi::BackendDescriptor,
    /// The tier: `core` or `full` (REQ-CONF-003).
    pub tier: String,
    /// What the probe found per clause.
    pub clauses: std::collections::BTreeMap<crate::confine::Clause, crate::confine::ClauseState>,
    /// Clauses not enforced here, with reasons.
    pub unavailable_clauses: Vec<String>,
    /// Tasks in `running`.
    pub running: Vec<TaskId>,
    /// Tasks waiting for the user, with what they wait for.
    pub pending: Vec<(TaskId, WaitKind)>,
}

impl Runtime {
    fn ceiling_for(&self, task: &Task) -> ExtensionCeiling {
        ExtensionCeiling {
            max_steps: self.deps.policy.ceiling.max_steps,
            max_deadline: task
                .created_at
                .plus_millis(LIM_T5_DEADLINE_MS_RANGE.1)
                .unwrap_or(task.deadline),
        }
    }

    /// Creates a task (REQ-TASK-001, REQ-TASK-012, REQ-CAP-003).
    pub fn create_task(
        &self,
        session: &Session,
        request: CreateTask,
    ) -> Result<Task, RuntimeError> {
        let goal_len = request.goal.chars().count();
        if goal_len == 0 || goal_len > LIM_D4_TEXT_CHARS {
            return Err(RuntimeError::InvalidRequest {
                detail: format!("goal must be 1 to {LIM_D4_TEXT_CHARS} characters"),
            });
        }
        if request.scope.root.is_empty() {
            return Err(RuntimeError::InvalidRequest {
                detail: "scope root is required".into(),
            });
        }
        let profile_name = request
            .profile
            .clone()
            .unwrap_or_else(|| self.deps.policy.default_profile.clone());
        let profile = self
            .deps
            .policy
            .profile(&profile_name)
            .cloned()
            .ok_or_else(|| RuntimeError::InvalidRequest {
                detail: format!(
                    "unknown profile {profile_name}; profiles: {:?}",
                    self.deps.policy.profiles.keys().collect::<Vec<_>>()
                ),
            })?;
        let locality = self.guard("model_backend", || self.deps.backend.locality())?;
        if locality == crate::spi::Locality::Remote && !profile.external {
            return Err(RuntimeError::Denied {
                detail: format!(
                    "profile {profile_name} forbids external communication (external = false); the backend is remote"
                ),
            });
        }
        let deadline_ms = request.deadline_ms.unwrap_or(LIM_T5_DEADLINE_MS);
        if deadline_ms < LIM_T5_DEADLINE_MS_RANGE.0 || deadline_ms > LIM_T5_DEADLINE_MS_RANGE.1 {
            return Err(RuntimeError::InvalidRequest {
                detail: format!("deadline_ms must be within {:?}", LIM_T5_DEADLINE_MS_RANGE),
            });
        }
        let budgets = request.budgets.unwrap_or(profile.budgets);
        if budgets.steps > self.deps.policy.ceiling.max_steps {
            return Err(RuntimeError::Denied {
                detail: format!(
                    "steps {} exceed the ceiling {}",
                    budgets.steps, self.deps.policy.ceiling.max_steps
                ),
            });
        }
        for kind in &request.grants {
            self.deps
                .policy
                .ceiling
                .permits(kind)
                .map_err(|e| RuntimeError::Denied {
                    detail: e.to_string(),
                })?;
        }
        // One active task per project scope (REQ-TASK-012).
        for summary in self.deps.persistence.list_tasks()? {
            if let Some(other) = self.deps.persistence.load_task(summary.task_id)?
                && !other.state.is_terminal()
                && other.scope.overlaps(&request.scope)
            {
                return Err(RuntimeError::Conflict {
                    detail: format!("task {} is active on an overlapping scope", other.task_id),
                });
            }
        }
        let now = self.deps.clock.now();
        let task_id = TaskId::new(self.deps.ids.as_ref());
        // Fork (REQ-TASK-004): the source must be terminal or halted; its snapshot becomes the new base.
        let fork = match request.from_task {
            Some(source_id) => {
                let source = self.load(source_id)?;
                if !(source.state.is_terminal() || matches!(source.state, TaskState::Halted { .. }))
                {
                    return Err(RuntimeError::State(crate::task::StateError::Illegal {
                        from: source.state.name(),
                        input: "fork",
                    }));
                }
                let snapshots = self.deps.persistence.load_snapshots(source_id)?;
                let from = match request.from_snapshot {
                    Some(id) => snapshots
                        .iter()
                        .find(|s| s.snapshot_id == id)
                        .cloned()
                        .ok_or(RuntimeError::NotFound {
                            what: "snapshot".into(),
                        })?,
                    None => snapshots
                        .iter()
                        .rfind(|s| s.reason != crate::snapshot::SnapshotReason::Backup)
                        .cloned()
                        .ok_or(RuntimeError::NotFound {
                            what: "snapshot".into(),
                        })?,
                };
                Some((source, from))
            }
            None => None,
        };
        self.deps
            .persistence
            .record_workspace_state(task_id, WorkspaceState::Allocated, None)
            .ok();
        let (info, prepared_base) = match &fork {
            Some((_, from)) => {
                let (info, prepared) = self.deps.workspaces.materialize_from(
                    task_id,
                    SnapshotId::new(self.deps.ids.as_ref()),
                    from,
                    &request.scope,
                    now,
                )?;
                (info, Some(prepared))
            }
            None => (
                self.deps.workspaces.materialize(task_id, &request.scope)?,
                None,
            ),
        };
        let tools = self.deps.workspaces.tools(task_id)?;
        let mut hashes = std::collections::BTreeMap::new();
        for d in self.guard(PROVIDER, || tools.definitions())? {
            hashes.insert(
                d.name.clone(),
                crate::hash::hash_canonical(&serde_json::to_value(&d).expect("serializes")),
            );
        }
        let versions = TaskVersions {
            policy_hash: self.policy_hash.clone(),
            decision_schema_version: crate::decision::DECISION_SCHEMA_VERSION,
            tool_definition_hashes: hashes,
            runtime_version: self.deps.runtime_version.clone(),
            backend: Some(self.guard("model_backend", || self.deps.backend.describe())?),
            tool_imports: self.guard(PROVIDER, || tools.imports())?,
        };
        // Drained before anything is persisted (REQ-SEC-011): a provider that
        // panics here is an error to the caller while no task exists, not a
        // half-created task. They are appended where they always were.
        let provider_events = self.guard(PROVIDER, || tools.drain_events())?;
        let mut task = Task::create(
            TaskSpec {
                task_id,
                goal: request.goal,
                deadline_ms,
                budgets,
                profile: profile_name,
                versions,
                scope: request.scope,
            },
            now,
        )
        .map_err(|e| RuntimeError::InvalidRequest {
            detail: e.to_string(),
        })?;
        if let Some(check) = request.completion_check {
            task = task.with_completion_check(check);
        }
        task.forked_from = fork.as_ref().map(|(source, _)| source.task_id);
        // A fork copies the source's active grants, never widened (REQ-TASK-004).
        let inherited: Vec<GrantKind> = match &fork {
            Some((source, _)) => self
                .deps
                .persistence
                .load_grants(source.task_id)?
                .into_iter()
                .filter(|g| g.is_active(now))
                .map(|g| g.kind().clone())
                .collect(),
            None => self.deps.policy.initial_grants(&profile),
        };
        let mut grants = Vec::new();
        for kind in inherited.into_iter().chain(request.grants) {
            grants.push(Grant::issue(
                GrantId::new(self.deps.ids.as_ref()),
                task_id,
                kind,
                session.actor(),
                now,
                None,
            ));
        }
        self.deps.persistence.create_task(
            &task,
            &grants,
            &self.event(
                session.actor(),
                Some(task_id),
                None,
                None,
                Event::TaskCreated,
            ),
        )?;
        self.deps.persistence.append_event(&self.event(
            crate::event::Actor::Runtime,
            Some(task_id),
            None,
            None,
            Event::WorkspaceMaterialized,
        ))?;
        // RFC 009 §2: every imported external definition is pinned on the task.
        for (name, record) in &task.versions.tool_imports {
            if record.unavailable.is_none() {
                self.deps.persistence.append_event(&self.event(
                    Actor::Runtime,
                    Some(task_id),
                    None,
                    None,
                    Event::ToolPinned {
                        tool: name.clone(),
                        hash: record.hash.clone(),
                    },
                ))?;
            }
        }
        self.append_provider_events(provider_events)?;
        self.deps
            .persistence
            .record_workspace_state(task_id, WorkspaceState::Active, None)?;
        let _ = info;
        // The base snapshot (REQ-WS-004): of the copy, or the fork's prepared one.
        match prepared_base {
            Some(prepared) => {
                self.deps.persistence.record_snapshot(
                    &prepared.record,
                    &prepared.objects,
                    &self.event(
                        Actor::Runtime,
                        Some(task_id),
                        None,
                        None,
                        Event::WorkspaceSnapshot {
                            snapshot_id: prepared.record.snapshot_id,
                            reason: prepared.record.reason.name().to_owned(),
                        },
                    ),
                )?;
                self.deps.workspaces.seal(prepared.record.snapshot_id)?;
            }
            None => {
                self.take_snapshot(&task, crate::snapshot::SnapshotReason::Base, None, now)?;
            }
        }
        if request.hold {
            return Ok(task);
        }
        self.resume_task(session, task_id, None)
    }

    /// The user's rollback (REQ-WS-006): valid when the task is not `running`
    /// or `recovering`; the latest snapshot when none is named. The task's
    /// state is unchanged.
    pub fn rollback_workspace(
        &self,
        session: &Session,
        task_id: TaskId,
        snapshot_id: Option<SnapshotId>,
    ) -> Result<SnapshotRecord, RuntimeError> {
        let task = self.load(task_id)?;
        if matches!(task.state, TaskState::Running | TaskState::Recovering) {
            return Err(RuntimeError::State(crate::task::StateError::Illegal {
                from: task.state.name(),
                input: "rollback_workspace",
            }));
        }
        let snapshots = self.deps.persistence.load_snapshots(task_id)?;
        let record = match snapshot_id {
            Some(id) => snapshots.iter().find(|s| s.snapshot_id == id).cloned(),
            None => snapshots
                .iter()
                .rfind(|s| s.reason != crate::snapshot::SnapshotReason::Backup)
                .cloned(),
        }
        .ok_or(RuntimeError::NotFound {
            what: "snapshot".into(),
        })?;
        self.deps.workspaces.restore(task_id, &record)?;
        self.deps.persistence.append_event(&self.event(
            session.actor(),
            Some(task_id),
            None,
            record.action_id,
            Event::WorkspaceRollback {
                snapshot_id: record.snapshot_id,
            },
        ))?;
        Ok(record)
    }

    /// Phase 1 of a promotion (REQ-WS-007): the plan, or the conflicts.
    pub fn promotion_plan(&self, task_id: TaskId) -> Result<PromotionPlan, RuntimeError> {
        let task = self.load(task_id)?;
        if matches!(task.state, TaskState::Running | TaskState::Recovering) {
            return Err(RuntimeError::State(crate::task::StateError::Illegal {
                from: task.state.name(),
                input: "promote_workspace",
            }));
        }
        let base = self.base_snapshot(task_id)?;
        Ok(self
            .deps
            .workspaces
            .promotion_plan(task_id, &base, &task.scope.root)?)
    }

    /// Phase 2: after the user's confirmation, the backup, the writes and the record.
    pub fn promote_workspace(
        &self,
        session: &Session,
        task_id: TaskId,
        plan: &PromotionPlan,
    ) -> Result<Promotion, RuntimeError> {
        let task = self.load(task_id)?;
        if matches!(task.state, TaskState::Running | TaskState::Recovering) {
            return Err(RuntimeError::State(crate::task::StateError::Illegal {
                from: task.state.name(),
                input: "promote_workspace",
            }));
        }
        let now = self.deps.clock.now();
        let (backup, promotion) = self.deps.workspaces.promote(
            task_id,
            plan,
            &task.scope.root,
            SnapshotId::new(self.deps.ids.as_ref()),
            crate::ids::PromotionId::new(self.deps.ids.as_ref()),
            now,
        )?;
        self.deps.persistence.record_snapshot(
            &backup.record,
            &backup.objects,
            &self.event(
                session.actor(),
                Some(task_id),
                None,
                None,
                Event::WorkspaceSnapshot {
                    snapshot_id: backup.record.snapshot_id,
                    reason: backup.record.reason.name().to_owned(),
                },
            ),
        )?;
        self.deps.workspaces.seal(backup.record.snapshot_id)?;
        self.deps.persistence.record_promotion(
            &promotion,
            &self.event(
                session.actor(),
                Some(task_id),
                None,
                None,
                Event::WorkspacePromoted {
                    files: promotion.files.iter().map(|c| c.path.clone()).collect(),
                },
            ),
        )?;
        Ok(promotion)
    }

    /// Reverses a promotion from its backup (REQ-WS-007).
    pub fn revert_promotion(
        &self,
        session: &Session,
        task_id: TaskId,
        promotion_id: crate::ids::PromotionId,
    ) -> Result<Promotion, RuntimeError> {
        let mut promotion = self
            .deps
            .persistence
            .load_promotions(task_id)?
            .into_iter()
            .find(|p| p.promotion_id == promotion_id)
            .ok_or(RuntimeError::NotFound {
                what: "promotion".into(),
            })?;
        if promotion.reversed_at.is_some() {
            return Err(RuntimeError::State(crate::task::StateError::Illegal {
                from: "reversed",
                input: "revert_promotion",
            }));
        }
        let backup = self
            .deps
            .persistence
            .load_snapshot(promotion.backup_snapshot)?
            .ok_or(RuntimeError::NotFound {
                what: "snapshot".into(),
            })?;
        self.deps.workspaces.revert_promotion(&promotion, &backup)?;
        let now = self.deps.clock.now();
        promotion.reversed_at = Some(now);
        self.deps.persistence.record_promotion(
            &promotion,
            &self.event(
                session.actor(),
                Some(task_id),
                None,
                None,
                Event::WorkspaceRollback {
                    snapshot_id: backup.snapshot_id,
                },
            ),
        )?;
        Ok(promotion)
    }

    /// Deletes a terminal task's workspace, snapshots, blobs and backups
    /// (RFC 006 §6): rows first, then files; the audit stays unless `audit`.
    pub fn delete_task(
        &self,
        session: &Session,
        task_id: TaskId,
        audit: bool,
    ) -> Result<Footprint, RuntimeError> {
        let task = self.load(task_id)?;
        if !task.state.is_terminal() {
            return Err(RuntimeError::State(crate::task::StateError::Illegal {
                from: task.state.name(),
                input: "delete_task",
            }));
        }
        let snapshots = self.deps.persistence.load_snapshots(task_id)?;
        let promotions = self.deps.persistence.load_promotions(task_id)?;
        let footprint = self.footprint(task_id, &snapshots, &promotions)?;
        let rows = self.deps.persistence.delete_task_data(
            task_id,
            audit,
            &self.event(
                session.actor(),
                Some(task_id),
                None,
                None,
                Event::TaskDeleted { audit },
            ),
        )?;
        self.deps.workspaces.delete_files(task_id, &rows)?;
        self.clear_cancellation(task_id);
        Ok(footprint)
    }

    /// The task's first snapshot: `base` for a copy, `fork` for a fork.
    fn base_snapshot(&self, task_id: TaskId) -> Result<SnapshotRecord, RuntimeError> {
        self.deps
            .persistence
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

    /// What the task holds on disk (RFC 006 §6): the latest snapshot's bytes,
    /// the objects exclusive to the task against those shared, the blobs.
    pub fn footprint(
        &self,
        task_id: TaskId,
        snapshots: &[SnapshotRecord],
        promotions: &[Promotion],
    ) -> Result<Footprint, RuntimeError> {
        let mut per_task: std::collections::BTreeMap<crate::hash::Hash, (u64, u64)> =
            std::collections::BTreeMap::new();
        for s in snapshots {
            if let Ok(manifest) = self.deps.workspaces.manifest(s) {
                for (hash, size) in manifest.objects() {
                    let e = per_task.entry(hash).or_insert((size, 0));
                    e.1 += 1;
                }
            }
        }
        let hashes: Vec<crate::hash::Hash> = per_task.keys().cloned().collect();
        let mut exclusive = 0;
        let mut shared = 0;
        for (hash, size, refs) in self.deps.persistence.object_references(&hashes)? {
            let mine = per_task.get(&hash).map(|(_, n)| *n).unwrap_or(0);
            if refs <= mine {
                exclusive += size;
            } else {
                shared += size;
            }
        }
        Ok(Footprint {
            workspace_bytes: self.deps.workspaces.tree_bytes(task_id).unwrap_or(0),
            snapshots: snapshots.len() as u64,
            object_bytes_exclusive: exclusive,
            object_bytes_shared: shared,
            blob_bytes: self.deps.persistence.blob_bytes(task_id)?,
            promotions: promotions
                .iter()
                .filter(|p| p.reversed_at.is_none())
                .count() as u64,
        })
    }

    /// Resumes from `created`, `paused` or `halted` (REQ-TASK-005).
    pub fn resume_task(
        &self,
        session: &Session,
        task_id: TaskId,
        extend: Option<Extension>,
    ) -> Result<Task, RuntimeError> {
        let task = self.load(task_id)?;
        // At most LIM-X1 tasks run at once (REQ-SCHED-001).
        let running = self.deps.persistence.tasks_in_state("running")?;
        if running.len() >= LIM_X1_RUNNING_TASKS && !running.contains(&task_id) {
            return Err(RuntimeError::Conflict {
                detail: format!(
                    "{} task(s) already running; LIM-X1 allows {}",
                    running.len(),
                    LIM_X1_RUNNING_TASKS
                ),
            });
        }
        let now = self.deps.clock.now();
        let next = task.transition(
            TaskInput::Resume {
                extend,
                ceiling: self.ceiling_for(&task),
            },
            now,
        )?;
        self.deps.persistence.record_task_transition(
            &next,
            &self.event(
                session.actor(),
                Some(task_id),
                None,
                None,
                state_changed(&task, &next),
            ),
        )?;
        Ok(next)
    }

    /// Requests a pause; it takes effect at the next step boundary (REQ-TASK-005).
    pub fn pause_task(&self, session: &Session, task_id: TaskId) -> Result<Task, RuntimeError> {
        let task = self.load(task_id)?;
        let now = self.deps.clock.now();
        let next = task.transition(TaskInput::PauseRequest, now)?;
        self.deps.persistence.record_task_transition(
            &next,
            &self.event(
                session.actor(),
                Some(task_id),
                None,
                None,
                Event::TaskPauseRequested,
            ),
        )?;
        Ok(next)
    }

    /// Cancels from any non-terminal state (REQ-TASK-006): an action not
    /// yet terminal is cancelled and a pending approval invalidated.
    pub fn cancel_task(&self, session: &Session, task_id: TaskId) -> Result<Task, RuntimeError> {
        let task = self.load(task_id)?;
        let now = self.deps.clock.now();
        let next = task.transition(TaskInput::Cancel, now)?;
        if let Some(mut approval) = self.deps.persistence.load_pending_approval(task_id)? {
            approval.invalidate(InvalidationReason::GrantsChanged);
            self.deps.persistence.record_approval_decision(
                &task,
                &approval,
                &self.event(
                    session.actor(),
                    Some(task_id),
                    None,
                    Some(approval.action_id),
                    Event::ApprovalInvalidated {
                        approval_id: approval.approval_id,
                    },
                ),
            )?;
        }
        for action in self.deps.persistence.load_actions(task_id)? {
            if !action.status.is_terminal() {
                self.close_action(
                    session.actor(),
                    &task,
                    &action,
                    ActionInput::Cancel,
                    StepOutcome::Cancelled,
                    now,
                )?;
            }
        }
        self.deps.persistence.record_task_transition(
            &next,
            &self.event(
                session.actor(),
                Some(task_id),
                None,
                None,
                state_changed(&task, &next),
            ),
        )?;
        self.clear_cancellation(task_id);
        Ok(next)
    }

    /// Approves the pending action (REQ-APR-007, REQ-APR-009). The action
    /// runs at the task's next step, under the grants in force then.
    pub fn approve_action(&self, session: &Session, task_id: TaskId) -> Result<Task, RuntimeError> {
        let (task, mut approval) = self.pending_approval(task_id)?;
        let now = self.deps.clock.now();
        approval
            .decide(UserDecision::Approve, session.actor(), now)
            .map_err(|e| RuntimeError::InvalidRequest {
                detail: e.to_string(),
            })?;
        let next = task.transition(TaskInput::UserDecided, now)?;
        self.deps.persistence.record_approval_decision(
            &next,
            &approval,
            &self.event(
                session.actor(),
                Some(task_id),
                None,
                Some(approval.action_id),
                Event::ApprovalDecided {
                    approval_id: approval.approval_id,
                    decision: ApprovalDecision::Approved,
                },
            ),
        )?;
        self.deps.persistence.append_event(&self.event(
            session.actor(),
            Some(task_id),
            None,
            None,
            state_changed(&task, &next),
        ))?;
        Ok(next)
    }

    /// Rejects the pending action with an optional note (REQ-APR-007). The
    /// profile's `on_reject` applies unless `halt` says otherwise; the
    /// rejection ends the step and counts toward LIM-T2.
    pub fn reject_action(
        &self,
        session: &Session,
        task_id: TaskId,
        note: Option<String>,
        halt: Option<bool>,
    ) -> Result<Task, RuntimeError> {
        let (task, mut approval) = self.pending_approval(task_id)?;
        let now = self.deps.clock.now();
        let profile = self.profile_of(&task)?;
        let halt = halt.unwrap_or(profile.on_reject == crate::policy::OnReject::Halt);
        approval
            .decide(UserDecision::Reject { note, halt }, session.actor(), now)
            .map_err(|e| RuntimeError::InvalidRequest {
                detail: e.to_string(),
            })?;
        let next = task.transition(
            if halt {
                TaskInput::UserHalted
            } else {
                TaskInput::UserDecided
            },
            now,
        )?;
        self.deps.persistence.record_approval_decision(
            &next,
            &approval,
            &self.event(
                session.actor(),
                Some(task_id),
                None,
                Some(approval.action_id),
                Event::ApprovalDecided {
                    approval_id: approval.approval_id,
                    decision: ApprovalDecision::Rejected,
                },
            ),
        )?;
        let action = self.action_of(task_id, approval.action_id)?;
        let after = self.close_action(
            session.actor(),
            &next,
            &action,
            ActionInput::Reject(RejectionCode::ApprovalRejected),
            StepOutcome::Rejected,
            now,
        )?;
        self.deps.persistence.append_event(&self.event(
            session.actor(),
            Some(task_id),
            None,
            None,
            state_changed(&task, &after),
        ))?;
        Ok(after)
    }

    /// Answers the pending question (REQ-APR-010).
    pub fn answer_question(
        &self,
        session: &Session,
        task_id: TaskId,
        answer: &str,
    ) -> Result<Task, RuntimeError> {
        let task = self.load(task_id)?;
        if !matches!(
            task.state,
            TaskState::WaitingUser {
                wait_kind: WaitKind::Question
            }
        ) {
            return Err(RuntimeError::State(crate::task::StateError::Illegal {
                from: task.state.name(),
                input: "answer_question",
            }));
        }
        let mut question = self
            .deps
            .persistence
            .load_pending_question(task_id)?
            .ok_or(RuntimeError::NotFound {
                what: "question".into(),
            })?;
        let now = self.deps.clock.now();
        question.answer(answer, session.actor(), now).map_err(|e| {
            RuntimeError::InvalidRequest {
                detail: e.to_string(),
            }
        })?;
        let next = task.transition(TaskInput::UserDecided, now)?;
        self.deps.persistence.record_question_update(
            &next,
            &question,
            &self.event(
                session.actor(),
                Some(task_id),
                Some(question.step_id),
                None,
                Event::QuestionAnswered {
                    question_id: question.question_id,
                },
            ),
        )?;
        self.deps.persistence.append_event(&self.event(
            session.actor(),
            Some(task_id),
            None,
            None,
            state_changed(&task, &next),
        ))?;
        Ok(next)
    }

    /// Issues a grant within the ceiling (REQ-CAP-003, REQ-CAP-004).
    pub fn grant_capability(
        &self,
        session: &Session,
        task_id: TaskId,
        kind: GrantKind,
    ) -> Result<Grant, RuntimeError> {
        let task = self.load(task_id)?;
        if task.state.is_terminal() {
            return Err(RuntimeError::State(crate::task::StateError::Illegal {
                from: task.state.name(),
                input: "grant_capability",
            }));
        }
        if matches!(&kind, GrantKind::Tool { name } if name == crate::host_tool::RUN_HOST_TOOL) {
            return Err(RuntimeError::InvalidRequest {
                detail: crate::host_tool::RUN_HOST_TOOL_NOT_A_GRANT.into(),
            });
        }
        self.deps
            .policy
            .ceiling
            .permits(&kind)
            .map_err(|e| RuntimeError::Denied {
                detail: e.to_string(),
            })?;
        let now = self.deps.clock.now();
        let grant = Grant::issue(
            GrantId::new(self.deps.ids.as_ref()),
            task_id,
            kind,
            session.actor(),
            now,
            None,
        );
        self.deps.persistence.record_grant(
            &grant,
            &self.event(
                session.actor(),
                Some(task_id),
                None,
                None,
                Event::GrantIssued {
                    grant_id: grant.grant_id,
                },
            ),
        )?;
        let mut set = GrantSet::new();
        for g in self.deps.persistence.load_grants(task_id)? {
            set.add(g);
        }
        self.rebind_pending_approval(
            session.actor(),
            &task,
            set.hash(now),
            InvalidationReason::GrantsChanged,
            |_| false,
            now,
        )?;
        Ok(grant)
    }

    /// Revokes a grant at once (REQ-CAP-005). An approval requested under
    /// the old grant set is invalidated and its action rejected, whether the
    /// user has decided yet or not (REQ-APR-009).
    pub fn revoke_capability(
        &self,
        session: &Session,
        task_id: TaskId,
        grant_id: GrantId,
    ) -> Result<Grant, RuntimeError> {
        let task = self.load(task_id)?;
        let now = self.deps.clock.now();
        let mut set = GrantSet::new();
        for g in self.deps.persistence.load_grants(task_id)? {
            set.add(g);
        }
        let before = set.hash(now);
        set.revoke(grant_id).map_err(|e| match e {
            crate::grant::GrantError::NotFound => RuntimeError::NotFound {
                what: "grant".into(),
            },
            crate::grant::GrantError::NotActive => {
                RuntimeError::State(crate::task::StateError::Illegal {
                    from: "revoked",
                    input: "revoke_capability",
                })
            }
        })?;
        let grant = set
            .all()
            .iter()
            .find(|g| g.grant_id == grant_id)
            .cloned()
            .expect("just revoked");
        self.deps.persistence.record_grant_status(
            &grant,
            &self.event(
                session.actor(),
                Some(task_id),
                None,
                None,
                Event::GrantRevoked { grant_id },
            ),
        )?;
        if set.hash(now) == before {
            return Ok(grant);
        }
        let revoked_tool = match grant.kind() {
            GrantKind::Tool { name } => Some(name.clone()),
            _ => None,
        };
        self.rebind_pending_approval(
            session.actor(),
            &task,
            set.hash(now),
            InvalidationReason::GrantRevoked,
            |action| match &revoked_tool {
                Some(name) => name == action.content().tool(),
                // A non-tool grant may have authorized the action's paths or destinations: fail closed.
                None => true,
            },
            now,
        )?;
        Ok(grant)
    }

    /// A pending approval binds to the grant set it was requested under
    /// (INV-06). When the set changes, the request is withdrawn; unless a
    /// grant the action used was revoked (`used_by` says so, and the action
    /// is then rejected `denied`), the action is requested again under the
    /// new set and the task keeps waiting (REQ-APR-009).
    fn rebind_pending_approval(
        &self,
        actor: Actor,
        task: &Task,
        new_grant_hash: crate::hash::Hash,
        reason: InvalidationReason,
        used_by: impl Fn(&Action) -> bool,
        now: Instant,
    ) -> Result<Task, RuntimeError> {
        let persistence = &self.deps.persistence;
        let Some(mut approval) = persistence.load_pending_approval(task.task_id)? else {
            return Ok(task.clone());
        };
        let action = self.action_of(task.task_id, approval.action_id)?;
        approval.invalidate(reason);
        persistence.record_approval_decision(
            task,
            &approval,
            &self.event(
                actor.clone(),
                Some(task.task_id),
                Some(action.step_id),
                Some(action.action_id),
                Event::ApprovalInvalidated {
                    approval_id: approval.approval_id,
                },
            ),
        )?;
        if used_by(&action) {
            let next = match task.state {
                TaskState::WaitingUser {
                    wait_kind: WaitKind::Approval,
                } => task.transition(TaskInput::UserDecided, now)?,
                _ => task.clone(),
            };
            let after = self.close_action(
                actor.clone(),
                &next,
                &action,
                ActionInput::Reject(RejectionCode::Denied),
                StepOutcome::Rejected,
                now,
            )?;
            if task.state != after.state {
                persistence.append_event(&self.event(
                    actor,
                    Some(task.task_id),
                    None,
                    None,
                    state_changed(task, &after),
                ))?;
            }
            return Ok(after);
        }
        let again = ApprovalRequest::new(
            crate::ids::ApprovalId::new(self.deps.ids.as_ref()),
            task.task_id,
            action.action_id,
            action.content().action_hash().clone(),
            new_grant_hash,
            approval.rendered.clone(),
            approval.model_reason.as_deref(),
            now,
            crate::limits::LIM_A1_EXPIRY_MS.and_then(|ms| now.plus_millis(ms).ok()),
        );
        persistence.record_approval_request(
            task,
            &action,
            &again,
            &self.event(
                actor,
                Some(task.task_id),
                Some(action.step_id),
                Some(action.action_id),
                Event::ApprovalRequested {
                    approval_id: again.approval_id,
                },
            ),
        )?;
        Ok(task.clone())
    }

    /// Ends an action outside the loop (rejected or cancelled by a control
    /// verb) and finishes its step, applying the counters; a budget reached
    /// halts a running task. Returns the task as persisted.
    fn close_action(
        &self,
        actor: Actor,
        task: &Task,
        action: &Action,
        input: ActionInput,
        outcome: StepOutcome,
        now: Instant,
    ) -> Result<Task, RuntimeError> {
        let persistence = &self.deps.persistence;
        let closed = action.transition(input, now).map_err(|e| {
            RuntimeError::Persistence(crate::spi::PersistenceError::Ordering {
                detail: e.to_string(),
            })
        })?;
        persistence.record_action_terminal(
            &closed,
            &self.event(
                actor.clone(),
                Some(task.task_id),
                Some(action.step_id),
                Some(action.action_id),
                Event::ActionFinished {
                    status: closed.status.name().to_owned(),
                    cause: None,
                },
            ),
        )?;
        let Some(mut step) = persistence
            .load_steps(task.task_id)?
            .into_iter()
            .find(|s| s.step_id == action.step_id)
        else {
            return Ok(task.clone());
        };
        if let ActionStatus::Rejected { code } = closed.status {
            step.rejection = Some(code);
        }
        let mut after = task.clone();
        let budget = after.counters.apply(
            &StepResult {
                outcome,
                action: None,
                decision_hash: step.decision_hash.clone(),
                snapshot: None,
            },
            &after.budgets,
        );
        if let (Some(reason), TaskState::Running) = (budget, &after.state) {
            after = after.transition(TaskInput::Halt(reason), now)?;
        }
        let outcome = if matches!(after.state, TaskState::Halted { .. }) {
            StepOutcome::Halted
        } else {
            outcome
        };
        step.finish(outcome, now);
        persistence.record_step_outcome(
            &step,
            &after,
            &self.event(
                actor,
                Some(task.task_id),
                Some(step.step_id),
                Some(action.action_id),
                Event::StepFinished { outcome },
            ),
        )?;
        Ok(after)
    }

    fn action_of(
        &self,
        task_id: TaskId,
        action_id: crate::ids::ActionId,
    ) -> Result<Action, RuntimeError> {
        self.deps
            .persistence
            .load_actions(task_id)?
            .into_iter()
            .find(|a| a.action_id == action_id)
            .ok_or(RuntimeError::NotFound {
                what: "action".into(),
            })
    }

    /// The task with its pending items, plan and grants.
    pub fn get_task(&self, task_id: TaskId) -> Result<TaskView, RuntimeError> {
        let task = self.load(task_id)?;
        let now = self.deps.clock.now();
        let pending_approval = match task.pending {
            Some(PendingItem::Approval { .. }) => {
                self.deps.persistence.load_pending_approval(task_id)?
            }
            _ => None,
        };
        let pending_question = match task.pending {
            Some(PendingItem::Question { .. }) => {
                self.deps.persistence.load_pending_question(task_id)?
            }
            _ => None,
        };
        let plan = self.deps.persistence.load_plan(task_id)?;
        let grants = self
            .deps
            .persistence
            .load_grants(task_id)?
            .into_iter()
            .filter(|g| g.is_active(now))
            .collect();
        let snapshots = self.deps.persistence.load_snapshots(task_id)?;
        let promotions = self.deps.persistence.load_promotions(task_id)?;
        let footprint = self.footprint(task_id, &snapshots, &promotions)?;
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

    /// Tasks, newest first.
    pub fn list_tasks(&self) -> Result<Vec<TaskSummary>, RuntimeError> {
        Ok(self.deps.persistence.list_tasks()?)
    }

    /// Events after a position (REQ-OBS-002).
    pub fn get_events(
        &self,
        task_id: Option<TaskId>,
        since: u64,
        limit: usize,
    ) -> Result<Vec<PositionedEvent>, RuntimeError> {
        Ok(self.deps.persistence.events_since(since, task_id, limit)?)
    }

    /// Metrics (REQ-OBS-003).
    pub fn get_metrics(&self, task_id: Option<TaskId>) -> Result<Metrics, RuntimeError> {
        Ok(self.deps.persistence.metrics(task_id)?)
    }

    /// The runtime's status (REQ-OBS-006).
    pub fn get_runtime_status(&self) -> Result<RuntimeStatus, RuntimeError> {
        let mut pending = Vec::new();
        for id in self.deps.persistence.tasks_in_state("waiting_user")? {
            if let Some(task) = self.deps.persistence.load_task(id)?
                && let TaskState::WaitingUser { wait_kind } = task.state
            {
                pending.push((id, wait_kind));
            }
        }
        Ok(RuntimeStatus {
            version: self.deps.runtime_version.clone(),
            backend: self.guard("model_backend", || self.deps.backend.describe())?,
            tier: self
                .deps
                .probe
                .tier_with(&self.deps.policy.accept_best_effort)
                .describe(),
            clauses: self.deps.probe.clauses.clone(),
            unavailable_clauses: self.deps.probe.unavailable_lines(),
            running: self.deps.persistence.tasks_in_state("running")?,
            pending,
        })
    }

    fn pending_approval(&self, task_id: TaskId) -> Result<(Task, ApprovalRequest), RuntimeError> {
        let task = self.load(task_id)?;
        if !matches!(
            task.state,
            TaskState::WaitingUser {
                wait_kind: WaitKind::Approval
            }
        ) {
            return Err(RuntimeError::State(crate::task::StateError::Illegal {
                from: task.state.name(),
                input: "approve_action",
            }));
        }
        let approval = self
            .deps
            .persistence
            .load_pending_approval(task_id)?
            .ok_or(RuntimeError::NotFound {
                what: "approval".into(),
            })?;
        if approval.status != ApprovalStatus::Pending {
            return Err(RuntimeError::State(crate::task::StateError::Illegal {
                from: "approved",
                input: "approve_action",
            }));
        }
        Ok((task, approval))
    }

    /// The current instant, for callers that build records.
    pub fn now(&self) -> Instant {
        self.deps.clock.now()
    }
}
