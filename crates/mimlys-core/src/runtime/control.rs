//! The control API (external design §4; REQ-TASK-001, 005, 006, 012;
//! REQ-APR-007, 009, 010; REQ-CAP-004, 005; REQ-OBS-002, 003, 006). Every
//! verb acts under a `Session`, writes one transition with its event, and
//! never submits a decision, validates an action or executes anything.

use super::loop_::{PROVIDER, state_changed};
use super::{MODEL_BACKEND, Runtime, RuntimeError, Session};
use crate::action::{Action, ActionInput, ActionStatus, EffectKind, can_be_undone};
use crate::approval::{ApprovalRequest, ApprovalStatus, InvalidationReason, UserDecision};
use crate::clock::Instant;
use crate::event::{Actor, ApprovalDecision, Event};
use crate::grant::{Grant, GrantKind, GrantSet};
use crate::ids::{ActionId, GrantId, SnapshotId, TaskId};
use crate::limits::{LIM_D4_TEXT_CHARS, LIM_T5_DEADLINE_MS_RANGE, LIM_X1_RUNNING_TASKS};
use crate::question::Question;
use crate::rejection::RejectionCode;
use crate::snapshot::{
    Footprint, Promotion, PromotionPlan, PromotionState, SnapshotRecord, WorkspaceState,
};
use crate::spi::{EditedPaths, Metrics, PositionedEvent, TaskSummary};
use crate::step::{Budgets, StepOutcome, StepResult};
use crate::task::{
    CompletionCheck, Extension, ExtensionCeiling, PendingItem, PlanVersion, ScopeSpec, Task,
    TaskInput, TaskSpec, TaskState, TaskVersions, WaitKind,
};

/// A request to create a task (REQ-TASK-001).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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
    /// `--plan-first` (REQ-TASK-015): the task's first plan is approved by the user before
    /// anything acts. A fork is plan-first only when it is created with this itself.
    pub plan_first: bool,
    /// `--effort` (REQ-TASK-013): the level the user chose. Without it the level is `standard`
    /// (or the lower `effort_max`), or the source's for a fork. `budgets` and `deadline_ms`
    /// stand for `--steps` and `--deadline`: a flag wins over the level.
    pub effort: Option<crate::effort::EffortLevel>,
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
    /// The actions whose outcome is unknown: found running at a restart, or
    /// interrupted although they cannot be undone (REQ-CLI-005, REQ-TASK-006,
    /// external design §19.2). The user, not the record, decides what happened
    /// to them.
    pub unknown_actions: Vec<UnknownAction>,
    /// The path whose permissions the user restores, while the task is halted
    /// `workspace_unreadable`: the last `workspace.unreadable` event of the task.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unreadable: Option<String>,
    /// The kind of the failure, when the path could not be read for a reason other than its
    /// permissions (an entry that vanished, a failing device): from the same event.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unreadable_kind: Option<String>,
    /// Whether the task halted after a rollback of its workspace that failed, so that the
    /// workspace was not rolled back: from the halt's event.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub rollback_failed: bool,
    /// What the task's execution or model request waits for among the user's runtimes (RFC 013;
    /// external design §19.3), while it waits. The runtime does not know it: the host that reads
    /// the registry fills it in. The task is `running` meanwhile.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub waiting: Option<Waiting>,
    /// The task's level, who chose it and its envelope (REQ-TASK-013): a record written before
    /// 0.13 reads as `standard`, chosen by default, with the envelope of its own budgets.
    pub effort: crate::effort::Effort,
    /// What the task cost so far (REQ-OBS-010): steps against the envelope, time against the
    /// deadline, the model's time, tokens where reported, and the backends and models used.
    pub cost: crate::effort::Cost,
}

/// What a `running` task waits for among the user's runtimes (external design §19.3).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Waiting {
    /// `admission` (a slot) or `engine` (the gate of the backend's endpoint).
    pub reason: String,
    /// The limit it waits against: `admission_slots` or `max_in_flight`.
    pub limit: u32,
    /// Slots or gates held now.
    pub taken: u32,
    /// The processes that hold them.
    pub holders: Vec<u32>,
    /// Since when the task waits.
    pub since: crate::clock::Instant,
    /// The endpoint, for `engine`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// An action whose outcome is unknown (REQ-CLI-005).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct UnknownAction {
    /// The action.
    pub action_id: ActionId,
    /// The tool it ran.
    pub tool: String,
    /// Its effect class.
    pub effect: EffectKind,
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
    /// The CPU part of clause C, beside the clauses; it is never part of the tier (REQ-CONF-013).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu: Option<crate::confine::ClauseState>,
    /// Tasks in `running`.
    pub running: Vec<TaskId>,
    /// Tasks waiting for the user, with what they wait for: a wait kind of this release, or the
    /// word a newer release wrote (REQ-MAIN-006).
    pub pending: Vec<(TaskId, super::read::WaitWord)>,
    /// Tasks whose state is a word this release does not know, with the word as written
    /// (REQ-MAIN-006). Absent when there is none.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unknown_state: Vec<(TaskId, String)>,
    /// The words above that this release does not know, with the field that holds each
    /// (`pending`, `unknown_state`). Absent when there is none.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unknown_to_this_release: Vec<crate::newer::UnknownWord>,
}

/// The scope is the request's input: a part of it that cannot be read (or whose name cannot be
/// kept) is an invalid request, not a state of a task's tree (external design §18, §20); every
/// other failure of the copy is what it was.
fn scope_error(error: crate::spi::WorkspaceError) -> RuntimeError {
    match error {
        crate::spi::WorkspaceError::Unreadable { .. }
        | crate::spi::WorkspaceError::ReadFailed { .. } => RuntimeError::InvalidRequest {
            detail: error.to_string(),
        },
        other => RuntimeError::Workspace(other),
    }
}

/// The workspace made for a task that is not recorded yet (handoff 013c; REQ-TASK-001,
/// REQ-WS-014): from the start of its materialization until the task is recorded, a
/// creation that ends in any error, whatever refused it (the scope, the provider, the store,
/// the clock, the limit on the definitions), leaves no workspace. The guard holds the task's
/// tool provider, which holds a handle on the tree (a directory with an open handle cannot be
/// removed on Windows), and lets it go before the workspace is removed. A guard dropped
/// without [`Creation::recorded`] also covers a panic that unwinds out of the creation.
///
/// A workspace the removal could not remove is logged and left: no task refers to it, and
/// `mimlys task prune` lists it as `no task: left by a refused creation`.
struct Creation<'a> {
    workspaces: &'a dyn crate::spi::WorkspaceProvider,
    task_id: TaskId,
    tools: Option<std::sync::Arc<dyn crate::spi::ToolProvider>>,
    armed: bool,
}

impl<'a> Creation<'a> {
    /// Begins the guard, before the workspace is materialized.
    fn begin(workspaces: &'a dyn crate::spi::WorkspaceProvider, task_id: TaskId) -> Self {
        Self {
            workspaces,
            task_id,
            tools: None,
            armed: true,
        }
    }

    /// The task's tool provider: opened once, and held by the guard, which lets it go
    /// before it removes the workspace.
    fn tools(&mut self) -> Result<&std::sync::Arc<dyn crate::spi::ToolProvider>, RuntimeError> {
        if self.tools.is_none() {
            self.tools = Some(self.workspaces.tools(self.task_id)?);
        }
        Ok(self.tools.as_ref().expect("opened above"))
    }

    /// The task is recorded: the workspace is its own, and the guard ends.
    fn recorded(&mut self) {
        self.armed = false;
        self.tools = None;
    }
}

impl Drop for Creation<'_> {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        // The provider is let go first.
        self.tools = None;
        if let Err(error) = self
            .workspaces
            .delete_files(self.task_id, &crate::spi::DeletedRows::default())
        {
            tracing::warn!(
                target: "mimlys_core",
                task_id = %self.task_id,
                %error,
                "the workspace of a refused creation could not be removed; `mimlys task prune` lists it"
            );
        }
    }
}

/// What a snapshot is asked for, to word the refusal of a promotion's backup.
#[derive(Clone, Copy)]
enum BackupUse {
    /// `rollback --snapshot`: the task's own tree is restored to it.
    Rollback,
    /// `run --from-snapshot`: a new task's workspace is made from it.
    Fork,
}

impl Runtime {
    /// Refuses a promotion's backup as the snapshot to roll back to or to fork from: it holds only
    /// the project's changed paths (REQ-WS-006). The same `InvalidRequest` for both; the advice
    /// is the way that does what was probably meant.
    fn refuse_backup(
        &self,
        task_id: TaskId,
        record: &SnapshotRecord,
        what: BackupUse,
    ) -> Result<(), RuntimeError> {
        if record.reason != crate::snapshot::SnapshotReason::Backup {
            return Ok(());
        }
        let (target, advice) = match what {
            BackupUse::Rollback => {
                let promotion = self
                    .deps
                    .persistence
                    .load_promotions(task_id)?
                    .into_iter()
                    .find(|p| p.backup_snapshot == record.snapshot_id);
                let advice = match promotion {
                    Some(p) => format!(
                        "reverse the promotion with `mimlys rollback {task_id} --promotion {}`",
                        p.promotion_id
                    ),
                    None => "reverse the promotion with `mimlys rollback <task> --promotion <id>`"
                        .to_owned(),
                };
                ("a target of `rollback --snapshot`", advice)
            }
            BackupUse::Fork => (
                "a source of a fork (`run --from-snapshot`)",
                "choose another snapshot of the task (`mimlys task show` lists them)".to_owned(),
            ),
        };
        Err(RuntimeError::InvalidRequest {
            detail: format!(
                "snapshot {} is the backup of a promotion: it holds only the project's changed paths and is not {target}; {advice}",
                record.snapshot_id
            ),
        })
    }

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
    pub(crate) fn create_task(
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
        // The level, and the backend its envelope names (REQ-TASK-013): refused here, before
        // anything is made, when it is above `effort_max` or has no key, shipped or configured.
        let plan = self.plan_effort(&profile, request.effort, request.from_task)?;
        let task_backend = self.backend_named(&plan.backend)?;
        if self.remote_forbidden_on(&task_backend, &profile)? {
            return Err(RuntimeError::Denied {
                detail: format!(
                    "profile {profile_name} forbids external communication (external = false); the backend is remote"
                ),
            });
        }
        // The envelope: a flag wins over the level, the level over the profile, and the profile
        // over the register's default (external design §19.4); the context budget is recorded
        // only when the level sets it.
        let envelope = crate::effort::resolve(&crate::effort::Inputs {
            keys: self.levels.keys(plan.level),
            flag_steps: request.budgets.map(|b| b.steps),
            flag_deadline_ms: request.deadline_ms,
            profile_steps: profile.budgets.steps,
            default_backend: &self.deps.backend_name,
        });
        let deadline_ms = envelope.deadline_ms;
        if deadline_ms < LIM_T5_DEADLINE_MS_RANGE.0 || deadline_ms > LIM_T5_DEADLINE_MS_RANGE.1 {
            return Err(RuntimeError::InvalidRequest {
                detail: format!("deadline_ms must be within {:?}", LIM_T5_DEADLINE_MS_RANGE),
            });
        }
        let budgets = Budgets {
            steps: envelope.steps,
            ..request.budgets.unwrap_or(profile.budgets)
        };
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
            // A task that holds a word of a newer release is read from its record as written:
            // what it holds of the scope is known even when its state is not, and a state this
            // release cannot classify holds the scope (REQ-MAIN-006).
            let Some(other) = self.scope_holder(summary.task_id)? else {
                continue;
            };
            if other.active && other.scope.overlaps(&request.scope) {
                // A halted source still holds its scope (REQ-TASK-012): its fork on that
                // scope is refused, and the refusal names both ways on (REQ-TASK-004).
                let detail = if request.from_task == Some(other.task_id) && other.halted {
                    format!(
                        "task {source} is halted and still holds this scope, so a fork of it cannot use the same scope; \
                         fork with another --scope, or run `mimlys task cancel {source}` first",
                        source = other.task_id
                    )
                } else {
                    format!("task {} is active on an overlapping scope", other.task_id)
                };
                return Err(RuntimeError::Conflict { detail });
            }
        }
        let now = self.deps.clock.now();
        let task_id = TaskId::new(self.deps.ids.as_ref());
        // Fork (REQ-TASK-004): the source must be terminal or halted; its snapshot becomes the new base.
        let fork = match request.from_task {
            Some(source_id) => {
                let source = self.writable(source_id)?;
                if !(source.state.is_terminal() || matches!(source.state, TaskState::Halted { .. }))
                {
                    return Err(RuntimeError::State(crate::task::StateError::Illegal {
                        from: source.state.name().into(),
                        input: "fork".into(),
                    }));
                }
                let snapshots = self.deps.persistence.load_snapshots(source_id)?;
                let from = match request.from_snapshot {
                    Some(id) => {
                        let found = snapshots
                            .iter()
                            .find(|s| s.snapshot_id == id)
                            .cloned()
                            .ok_or(RuntimeError::NotFound {
                                what: "snapshot".into(),
                            })?;
                        // A promotion's backup is not a source either: a workspace of only the
                        // paths the promotion changed (REQ-WS-006).
                        self.refuse_backup(source_id, &found, BackupUse::Fork)?;
                        found
                    }
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
        // No workspace state is written before the task is: the workspace's row
        // refers to the task's, so the store refuses it (`active` follows the record).
        // From here until the task is recorded, an error leaves no workspace (handoff 013c).
        let mut creation = Creation::begin(&*self.deps.workspaces, task_id);
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
                self.deps
                    .workspaces
                    .materialize(task_id, &request.scope)
                    .map_err(scope_error)?,
                None,
            ),
        };
        let tools = creation.tools()?;
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
            event_schema_version: crate::event::EVENT_SCHEMA_VERSION,
            action_schema_version: crate::action::ACTION_SCHEMA_VERSION,
            tool_definition_hashes: hashes,
            runtime_version: self.deps.runtime_version.clone(),
            backend: Some(self.guard(MODEL_BACKEND, || task_backend.backend.describe())?),
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
                plan_first: request.plan_first,
            },
            now,
        )
        .map_err(|e| RuntimeError::InvalidRequest {
            detail: e.to_string(),
        })?;
        if let Some(check) = request.completion_check {
            task = task.with_completion_check(check);
        }
        // Recorded with every key resolved, and never changed during the task (REQ-TASK-013).
        task = task.with_effort(crate::effort::Effort {
            level: plan.level,
            chosen_by: plan.chosen_by,
            envelope,
        });
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
        let kinds: Vec<GrantKind> = inherited.into_iter().chain(request.grants).collect();
        // The tools these grants make visible must fit LIM-I1 together: the
        // profile, a fork's inherited grants and the request's are judged as one
        // set, before anything is recorded (handoff 009i, audit A-32).
        if let Err(refused) = self.check_definitions_fit(task_id, &kinds) {
            // The workspace is removed by the guard when this returns, as it is
            // for any other refusal. No workspace state is recorded: the task has
            // no row, and the workspace's row refers to it.
            // The servers started for the import are on the record all the same.
            let _ = self.append_provider_events(provider_events);
            return Err(refused);
        }
        let mut grants = Vec::new();
        for kind in kinds {
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
                Event::TaskCreated {
                    plan_first: task.plan_first,
                    effort: task.effort.clone(),
                },
            ),
        )?;
        // The task is recorded: the workspace is its own.
        creation.recorded();
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
    pub(crate) fn rollback_workspace(
        &self,
        session: &Session,
        task_id: TaskId,
        snapshot_id: Option<SnapshotId>,
    ) -> Result<SnapshotRecord, RuntimeError> {
        let task = self.writable(task_id)?;
        if matches!(task.state, TaskState::Running | TaskState::Recovering) {
            return Err(RuntimeError::State(crate::task::StateError::Illegal {
                from: task.state.name().into(),
                input: "rollback_workspace".into(),
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
        // A promotion's backup holds only the project's changed paths: restoring the working
        // tree to it would delete the rest (REQ-WS-006). The promotion's reversal is the way.
        self.refuse_backup(task_id, &record, BackupUse::Rollback)?;
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
        super::read::promotion_plan(
            self.deps.persistence.as_ref(),
            self.deps.workspaces.as_ref(),
            task_id,
        )
    }

    /// Phase 2: after the user's confirmation (external design §14.5). The order is the rule:
    /// the backup is written, its row committed and sealed, the promotion recorded `applying`;
    /// only then is the project written; then the promotion is recorded `applied`. An error
    /// while applying restores what was applied from the backup and records `failed`.
    pub(crate) fn promote_workspace(
        &self,
        session: &Session,
        task_id: TaskId,
        plan: &PromotionPlan,
    ) -> Result<Promotion, RuntimeError> {
        let task = self.writable(task_id)?;
        if matches!(task.state, TaskState::Running | TaskState::Recovering) {
            return Err(RuntimeError::State(crate::task::StateError::Illegal {
                from: task.state.name().into(),
                input: "promote_workspace".into(),
            }));
        }
        let base = super::read::base_snapshot(self.deps.persistence.as_ref(), task_id)?;
        if base.snapshot_id != plan.base_snapshot {
            return Err(RuntimeError::Workspace(
                crate::spi::WorkspaceError::Refused {
                    reason: "the plan is not against the task's base snapshot; plan again".into(),
                },
            ));
        }
        let now = self.deps.clock.now();
        // 1. The backup is written; the project is not touched.
        let (backup, mut promotion) = self.deps.workspaces.promote_backup(
            task_id,
            &crate::spi::PromotionRequest {
                plan,
                base: &base,
                project_root: &task.scope.root,
                backup_id: SnapshotId::new(self.deps.ids.as_ref()),
                promotion_id: crate::ids::PromotionId::new(self.deps.ids.as_ref()),
                now,
            },
        )?;
        // 2. Its row is committed and sealed.
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
        self.deps
            .workspaces
            .seal(backup.record.snapshot_id)
            .map_err(RuntimeError::from)
            .map_err(|e| self.stopped_before_apply(session, task_id, e))?;
        // 3. The promotion is recorded `applying`.
        let files: Vec<String> = promotion.files.iter().map(|c| c.path.clone()).collect();
        promotion.state = PromotionState::Applying;
        self.deps
            .persistence
            .record_promotion(
                &promotion,
                &self.event(
                    session.actor(),
                    Some(task_id),
                    None,
                    None,
                    Event::WorkspacePromotionStarted {
                        files: files.clone(),
                    },
                ),
            )
            .map_err(RuntimeError::from)
            .map_err(|e| self.stopped_before_apply(session, task_id, e))?;
        // 4. The project is written.
        match self
            .deps
            .workspaces
            .promote_apply(task_id, &promotion, &base)
        {
            Ok(()) => {
                // 5. Applied.
                promotion.state = PromotionState::Applied;
                promotion.applied_at = self.deps.clock.now();
                self.deps.persistence.record_promotion(
                    &promotion,
                    &self.event(
                        session.actor(),
                        Some(task_id),
                        None,
                        None,
                        Event::WorkspacePromoted { files },
                    ),
                )?;
                Ok(promotion)
            }
            Err(cause) => {
                let restored = self.deps.workspaces.revert_promotion(
                    &promotion,
                    &backup.record,
                    EditedPaths::Keep,
                );
                let (kept, restore_error) = match restored {
                    Ok(report) => (report.left, None),
                    Err(e) => (Vec::new(), Some(e.to_string())),
                };
                // A restore that failed leaves the backup as the way back (`restore_hint`).
                promotion.restored = restore_error.is_none();
                let error = RuntimeError::PromotionFailed {
                    cause,
                    kept,
                    restore_hint: promotion.restore_hint(),
                    restore_error,
                };
                promotion.state = PromotionState::Failed;
                promotion.reason = Some(error.to_string());
                self.deps.persistence.record_promotion(
                    &promotion,
                    &self.event(
                        session.actor(),
                        Some(task_id),
                        None,
                        None,
                        Event::WorkspacePromotionFailed {
                            reason: error.to_string(),
                            kind: Some(error.kind().to_owned()),
                        },
                    ),
                )?;
                Err(error)
            }
        }
    }

    /// A promotion that stopped after its backup was recorded and before its apply began (the
    /// backup could not be sealed, or the promotion could not be recorded `applying`): the project
    /// was not touched. The failure is recorded, with its kind, so that the same request sent
    /// again is answered with the kind it had (external design §4, "The recorded outcome"),
    /// and the error is returned as it was. The record is the best that can be done: when the
    /// store cannot take it either, the repeat finds the backup alone, and answers `internal`.
    fn stopped_before_apply(
        &self,
        session: &Session,
        task_id: TaskId,
        error: RuntimeError,
    ) -> RuntimeError {
        let recorded = self.deps.persistence.append_event(&self.event(
            session.actor(),
            Some(task_id),
            None,
            None,
            Event::WorkspacePromotionFailed {
                reason: error.to_string(),
                kind: Some(error.kind().to_owned()),
            },
        ));
        if let Err(cause) = recorded {
            tracing::warn!(
                target: "mimlys_core",
                task = %task_id,
                %cause,
                "the stop of a promotion before its apply could not be recorded"
            );
        }
        error
    }

    /// The promotion to reverse and its backup: an applied promotion that was not reversed, or a
    /// failed one whose own restore failed too.
    fn promotion_to_revert(
        &self,
        task_id: TaskId,
        promotion_id: crate::ids::PromotionId,
    ) -> Result<(Promotion, SnapshotRecord), RuntimeError> {
        let promotion = self
            .deps
            .persistence
            .load_promotions(task_id)?
            .into_iter()
            .find(|p| p.promotion_id == promotion_id)
            .ok_or(RuntimeError::NotFound {
                what: "promotion".into(),
            })?;
        // An applied promotion, or one that failed and whose own restore failed too: the
        // project is not as the backup holds it, and the backup is kept to restore it from.
        if promotion.reversed_at.is_some() {
            return Err(RuntimeError::State(crate::task::StateError::Illegal {
                from: "reversed".into(),
                input: "revert_promotion".into(),
            }));
        }
        if !promotion.can_be_reversed() {
            return Err(RuntimeError::State(crate::task::StateError::Illegal {
                from: promotion.state.name().into(),
                input: "revert_promotion".into(),
            }));
        }
        let backup = self
            .deps
            .persistence
            .load_snapshot(promotion.backup_snapshot)?
            .ok_or(RuntimeError::NotFound {
                what: "snapshot".into(),
            })?;
        Ok((promotion, backup))
    }

    /// What reversing a promotion would do, and which paths were edited since (REQ-WS-007).
    /// Nothing is changed.
    pub fn revert_plan(
        &self,
        task_id: TaskId,
        promotion_id: crate::ids::PromotionId,
    ) -> Result<crate::snapshot::RevertPlan, RuntimeError> {
        let (promotion, backup) = self.promotion_to_revert(task_id, promotion_id)?;
        Ok(self
            .deps
            .workspaces
            .promotion_revert_plan(&promotion, &backup)?)
    }

    /// Reverses a promotion from its backup (REQ-WS-007). Paths edited since the promotion are
    /// not overwritten: the reversal is refused, naming them, unless `overwrite_edited` says the
    /// user confirmed.
    pub(crate) fn revert_promotion(
        &self,
        session: &Session,
        task_id: TaskId,
        promotion_id: crate::ids::PromotionId,
        overwrite_edited: bool,
    ) -> Result<Promotion, RuntimeError> {
        self.writable(task_id)?;
        let (mut promotion, backup) = self.promotion_to_revert(task_id, promotion_id)?;
        if !overwrite_edited {
            let plan = self
                .deps
                .workspaces
                .promotion_revert_plan(&promotion, &backup)?;
            if !plan.conflicts.is_empty() {
                return Err(RuntimeError::Workspace(
                    crate::spi::WorkspaceError::EditedSince {
                        paths: plan.conflicts,
                    },
                ));
            }
        }
        self.deps.workspaces.revert_promotion(
            &promotion,
            &backup,
            if overwrite_edited {
                EditedPaths::Overwrite
            } else {
                EditedPaths::Keep
            },
        )?;
        let now = self.deps.clock.now();
        promotion.reversed_at = Some(now);
        promotion.restored = true;
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
    /// (RFC 006 §6): the files are journaled, the rows go, then every file is removed; the audit stays unless `audit`.
    pub(crate) fn delete_task(
        &self,
        session: &Session,
        task_id: TaskId,
        audit: bool,
    ) -> Result<Footprint, RuntimeError> {
        let task = self.writable(task_id)?;
        if !task.state.is_terminal() {
            return Err(RuntimeError::State(crate::task::StateError::Illegal {
                from: task.state.name().into(),
                input: "delete_task".into(),
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
        // Every removal is attempted; the failures are reported together, after the task is forgotten.
        let removed = self.deps.workspaces.delete_files(task_id, &rows);
        self.forget_task(task_id);
        removed?;
        Ok(footprint)
    }

    /// What the task holds on disk (RFC 006 §6): the latest snapshot's bytes,
    /// the objects exclusive to the task against those shared, the blobs.
    pub fn footprint(
        &self,
        task_id: TaskId,
        snapshots: &[SnapshotRecord],
        promotions: &[Promotion],
    ) -> Result<Footprint, RuntimeError> {
        super::read::footprint(
            self.deps.persistence.as_ref(),
            self.deps.workspaces.as_ref(),
            task_id,
            snapshots,
            promotions,
        )
    }

    /// Whether `backend` is `remote` and `profile` forbids external communication (REQ-SEC-010;
    /// `external = false`). The one place the rule is evaluated: a task's creation and every
    /// start of it ask it, of the backend the task runs on.
    pub(crate) fn remote_forbidden_on(
        &self,
        backend: &super::TaskBackend,
        profile: &crate::policy::Profile,
    ) -> Result<bool, RuntimeError> {
        let locality = self.guard(MODEL_BACKEND, || backend.backend.locality())?;
        Ok(locality == crate::spi::Locality::Remote && !profile.external)
    }

    /// The level a task would be given and the backend its envelope names, from what the user
    /// asked (`requested`, `--effort`) and the fork's source (REQ-TASK-013; external design
    /// §19.4): no I/O but the source's record, and nothing created. Refused when the level is
    /// above `effort_max` (naming what sets it) or has no key, shipped or configured (naming the
    /// section that configures it). `create_task` asks this itself, so what a command is told
    /// before it contacts a backend is what the creation will say.
    pub(crate) fn plan_effort(
        &self,
        profile: &crate::policy::Profile,
        requested: Option<crate::effort::EffortLevel>,
        from_task: Option<TaskId>,
    ) -> Result<EffortPlan, RuntimeError> {
        let source = match from_task {
            Some(id) => Some(self.writable(id)?.effort_in_force().level),
            None => None,
        };
        let bound = self.deps.policy.effort_bound(profile);
        let (level, chosen_by) = crate::effort::choose(requested, source, &bound, &self.levels)
            .map_err(|refusal| match refusal {
                crate::effort::Refusal::AboveMax { .. } => RuntimeError::Denied {
                    detail: refusal.to_string(),
                },
                crate::effort::Refusal::NoKeys { .. } => RuntimeError::InvalidRequest {
                    detail: refusal.to_string(),
                },
            })?;
        let chose = self.levels.keys(level).backend.clone();
        let backend_chosen_by_level = chose.is_some();
        let backend = chose.unwrap_or_else(|| self.deps.backend_name.clone());
        Ok(EffortPlan {
            level,
            chosen_by,
            backend,
            backend_chosen_by_level,
        })
    }

    /// Whether a task created under `profile` (the policy's default when `None`) on `backend`
    /// (the name its level's envelope gives, [`Runtime::plan_effort`]) would be refused it
    /// (REQ-SEC-010): `false` for a profile the policy does not have, which the creation reports
    /// by itself. Lets a command that creates a task know, before it contacts the backend for
    /// anything, that the creation will be refused.
    pub(crate) fn creation_forbidden(
        &self,
        profile: Option<&str>,
        backend: &str,
    ) -> Result<bool, RuntimeError> {
        let name = profile.unwrap_or(&self.deps.policy.default_profile);
        match self.deps.policy.profile(name) {
            Some(profile) => self.remote_forbidden_on(&self.backend_named(backend)?, profile),
            None => Ok(false),
        }
    }

    /// Whether `task` may not start: its backend is `remote` under a profile without `external`
    /// (REQ-SEC-010). It is the answer the next step gives: the task halts `policy` before any
    /// request is sent (external design §20). The rule is asked of the backend and the profile *in
    /// force*, not of the ones the task was created under: the configuration may have been edited
    /// since, and a task never sends where its profile forbids. The backend is the task's own,
    /// found by its name: a name that is no longer configured is an error naming the configured
    /// ones, and nothing moves the task to another backend (external design §20).
    pub(crate) fn start_refusal(&self, task: &Task) -> Result<Option<StartRefusal>, RuntimeError> {
        let profile = self.profile_of(task)?;
        let backend = self.backend_of(task)?;
        Ok(self
            .remote_forbidden_on(&backend, &profile)?
            .then(|| StartRefusal {
                backend: backend.name.clone(),
                profile: task.profile.clone(),
            }))
    }

    /// The model the task's backend names now when it is not the one the task last ran with: the
    /// last step that recorded a model, else the task's creation (external design §20). The task
    /// goes on, and the command that starts it says both models (a notice at the start); each
    /// step records its model, so the notice is made once per change. `None` when the model is
    /// the same, or the backend names none.
    pub(crate) fn model_change(&self, task: &Task) -> Result<Option<ModelChange>, RuntimeError> {
        let backend = self.backend_of(task)?;
        let now = self
            .guard(MODEL_BACKEND, || backend.backend.describe())?
            .model;
        let last_step_model = self
            .deps
            .persistence
            .load_steps(task.task_id)?
            .into_iter()
            .rev()
            .find_map(|s| s.model);
        let was = last_step_model.or_else(|| task.versions.backend.as_ref()?.model.clone());
        Ok(match (was, now) {
            (Some(was), Some(now)) if was != now => Some(ModelChange {
                backend: backend.name,
                was,
                now,
            }),
            _ => None,
        })
    }

    /// Refuses the start of `task` when its backend is no longer configured (RFC 015, Amendment 1,
    /// item 4): before any state changes, so the task stays as it was. Nothing is contacted.
    fn ensure_backend_configured(&self, task: &Task) -> Result<(), RuntimeError> {
        self.backend_of(task).map(|_| ())
    }

    /// Resumes from `created`, `paused` or `halted` (REQ-TASK-005).
    pub(crate) fn resume_task(
        &self,
        session: &Session,
        task_id: TaskId,
        extend: Option<Extension>,
    ) -> Result<Task, RuntimeError> {
        let task = self.writable(task_id)?;
        // A task whose backend is no longer configured is not started, and stays as it was.
        self.ensure_backend_configured(&task)?;
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
        self.transitioned(&task, &next);
        Ok(next)
    }

    /// Requests a pause; it takes effect at the next step boundary (REQ-TASK-005).
    pub(crate) fn pause_task(
        &self,
        session: &Session,
        task_id: TaskId,
    ) -> Result<Task, RuntimeError> {
        let task = self.writable(task_id)?;
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
    ///
    /// **A step in flight is asked to stop first** (REQ-TASK-006, "abort a model request in
    /// flight, terminate an execution in flight"): the request is the strongest one
    /// ([`Runtime::interrupt_step_anyway`]), so that a model request is aborted, an execution is
    /// terminated within LIM-O2, and an action that cannot be undone is recorded with its
    /// outcome unknown and listed by the task's view. The cancel then waits for the step to end,
    /// at most LIM-O19 (LIM-O2 plus 3000 ms), and only then reads the task and
    /// writes. A task with no step in flight is cancelled as before.
    ///
    /// **Made safe against the step finishing while the cancel runs** (the slot of
    /// `runtime::steps`). The step writes from the task it loaded when it began, and so does
    /// the cancel: neither may write over the other. The cancel claims the task before it reads
    /// it, so that no step begins meanwhile (it is refused as `conflict`), and it reads the task
    /// only after the step in flight has ended and written all it will write. Whichever comes
    /// second finds the other's work done: a step that came first has left the task `running`,
    /// paused, waiting, halted or ended, and the cancel acts on what it left (a task the step
    /// completed is refused as the state error it always was); a cancel that came first has left
    /// the task `cancelled`, and a step that begins after it is refused by the state.
    ///
    /// It blocks the calling thread while it waits: it must not be called from inside the step
    /// it waits for. A step that has not ended LIM-O19 after the request is not waited for any
    /// longer: the cancel is refused as `conflict`, nothing of the task is changed, the request
    /// stays, and the step ends `cancelled` when it does.
    pub(crate) fn cancel_task(
        &self,
        session: &Session,
        task_id: TaskId,
    ) -> Result<Task, RuntimeError> {
        let (_cancelling, step_in_flight) = self.steps.begin_cancel(task_id)?;
        if !step_in_flight {
            return self.cancel_idle_task(session, task_id);
        }
        match self.interrupt_step_anyway(task_id) {
            Ok(()) => {}
            // The step ended the task or it was deleted meanwhile: reading it again, once the
            // step has ended, says which, as for a cancel that found no step.
            Err(RuntimeError::State(_) | RuntimeError::NotFound { .. }) => {}
            Err(other) => return Err(other),
        }
        let wait_ms = crate::limits::cancel_wait_ms(self.cancel_grace_ms);
        let within = std::time::Duration::from_millis(wait_ms);
        if !self.steps.wait_for_step_end(task_id, within) {
            return Err(RuntimeError::Conflict {
                detail: format!(
                    "the step of task {task_id} did not end within {wait_ms} ms of the request to stop it: the task was not cancelled; the request stays, and the step ends when it can: cancel again",
                ),
            });
        }
        let cancelled = self.cancel_idle_task(session, task_id);
        if cancelled.is_err() {
            // The request made for this cancel is not left to meet the next step.
            self.clear_interruption(task_id);
        }
        cancelled
    }

    /// The cancel itself, with no step of the task in flight and none to begin (the caller
    /// holds the claim).
    pub(super) fn cancel_idle_task(
        &self,
        session: &Session,
        task_id: TaskId,
    ) -> Result<Task, RuntimeError> {
        let task = self.writable(task_id)?;
        let now = self.deps.clock.now();
        let next = task.transition(TaskInput::Cancel, now)?;
        // A task that waits for its plan (REQ-TASK-015) has no action and no approval: the step
        // that holds the wait ends `cancelled` with the task, in one write.
        if matches!(
            task.state,
            TaskState::WaitingUser {
                wait_kind: WaitKind::Plan
            }
        ) {
            let step = self.plan_wait_step(&task)?;
            let after = self.finish_plan_step(
                session.actor(),
                (&task, next),
                step,
                (StepOutcome::Cancelled, vec![]),
                now,
            )?;
            self.forget_task(task_id);
            return Ok(after);
        }
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
                // An action running that cannot be undone is not cancelled as
                // if nothing had happened: its outcome is unknown, as after a
                // crash, and the task is cancelled all the same (REQ-TASK-006).
                let input = if action.status == ActionStatus::Running
                    && !can_be_undone(action.content().effect())
                {
                    ActionInput::MarkUnknown
                } else {
                    ActionInput::Cancel
                };
                self.close_action(
                    session.actor(),
                    &task,
                    &action,
                    input,
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
        self.forget_task(task_id);
        Ok(next)
    }

    /// Approves the pending action (REQ-APR-007, REQ-APR-009), or the plan a plan-first task waits
    /// for (REQ-TASK-015). The action runs at the task's next step, under the grants in force
    /// then; an approved plan ends the gate, and the task runs. The optional `note` is the
    /// user's own words for the model (`approval_note`), bounded as a rejection's note is.
    pub(crate) fn approve_action(
        &self,
        session: &Session,
        task_id: TaskId,
        note: Option<String>,
    ) -> Result<Task, RuntimeError> {
        let task = self.writable(task_id)?;
        // Approving sets the task running again: a task whose backend is no longer configured
        // is not started, and stays as it was (the approval stays pending).
        self.ensure_backend_configured(&task)?;
        if matches!(
            task.state,
            TaskState::WaitingUser {
                wait_kind: WaitKind::Plan
            }
        ) {
            return self.decide_plan(session, task, PlanVerdict::Approve { note });
        }
        let (task, mut approval) = self.pending_approval_of(task)?;
        let now = self.deps.clock.now();
        approval
            .decide(UserDecision::Approve { note }, session.actor(), now)
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
        self.transitioned(&task, &next);
        Ok(next)
    }

    /// Rejects the pending action, or the plan of a plan-first task, with an optional note
    /// (REQ-APR-007, REQ-TASK-015). The profile's `on_reject` applies unless `halt` says
    /// otherwise; the rejection ends the step and counts toward LIM-T2. A rejected plan leaves
    /// the task gated: the next `plan` waits again.
    pub(crate) fn reject_action(
        &self,
        session: &Session,
        task_id: TaskId,
        note: Option<String>,
        halt: Option<bool>,
    ) -> Result<Task, RuntimeError> {
        let task = self.writable(task_id)?;
        // A rejection that does not halt sets the task running again: a task whose backend is no
        // longer configured is not started, and stays as it was.
        let halting =
            halt.unwrap_or(self.profile_of(&task)?.on_reject == crate::policy::OnReject::Halt);
        if !halting {
            self.ensure_backend_configured(&task)?;
        }
        if matches!(
            task.state,
            TaskState::WaitingUser {
                wait_kind: WaitKind::Plan
            }
        ) {
            return self.decide_plan(session, task, PlanVerdict::Reject { note, halt });
        }
        let (task, mut approval) = self.pending_approval_of(task)?;
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
        self.transitioned(&task, &after);
        Ok(after)
    }

    /// Answers the pending question (REQ-APR-010).
    pub(crate) fn answer_question(
        &self,
        session: &Session,
        task_id: TaskId,
        answer: &str,
    ) -> Result<Task, RuntimeError> {
        let task = self.writable(task_id)?;
        // Answering sets the task running again: a task whose backend is no longer configured is
        // not started, and stays as it was (the question stays pending).
        self.ensure_backend_configured(&task)?;
        if !matches!(
            task.state,
            TaskState::WaitingUser {
                wait_kind: WaitKind::Question
            }
        ) {
            return Err(RuntimeError::State(crate::task::StateError::Illegal {
                from: task.state.name().into(),
                input: "answer_question".into(),
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
        self.transitioned(&task, &next);
        Ok(next)
    }

    /// Issues a grant within the ceiling (REQ-CAP-003, REQ-CAP-004).
    pub(crate) fn grant_capability(
        &self,
        session: &Session,
        task_id: TaskId,
        kind: GrantKind,
    ) -> Result<Grant, RuntimeError> {
        let task = self.writable(task_id)?;
        if task.state.is_terminal() {
            return Err(RuntimeError::State(crate::task::StateError::Illegal {
                from: task.state.name().into(),
                input: "grant_capability".into(),
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
        // A grant that would make the visible definitions exceed LIM-I1 is
        // refused, naming the tool and the sizes, so that no step ever fails for
        // their size (handoff 009i, audit A-32).
        let mut kinds: Vec<GrantKind> = self
            .deps
            .persistence
            .load_grants(task_id)?
            .into_iter()
            .filter(|g| g.is_active(now))
            .map(|g| g.kind().clone())
            .collect();
        kinds.push(kind.clone());
        self.check_definitions_fit(task_id, &kinds)?;
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

    /// Refuses the grants `kinds` (in order) when the definitions of the tools
    /// they make visible would exceed LIM-I1: `denied`, naming the tool whose
    /// grant crosses the limit and the sizes. A tool the provider does not know
    /// has no definition to show and counts for nothing.
    fn check_definitions_fit(
        &self,
        task_id: TaskId,
        kinds: &[GrantKind],
    ) -> Result<(), RuntimeError> {
        let mut names: Vec<&str> = Vec::new();
        for kind in kinds {
            let name = match kind {
                GrantKind::Tool { name } => name.as_str(),
                // A host tool is offered through the gateway (external design 9.1).
                GrantKind::HostTool { .. } => crate::host_tool::RUN_HOST_TOOL,
                _ => continue,
            };
            if !names.contains(&name) {
                names.push(name);
            }
        }
        if names.is_empty() {
            return Ok(());
        }
        let tools = self.deps.workspaces.tools(task_id)?;
        let defined = self.guard(PROVIDER, || tools.definitions())?;
        let granted: Vec<crate::tool::ToolDefinition> = names
            .iter()
            .filter_map(|n| defined.iter().find(|d| d.name == *n).cloned())
            .collect();
        match crate::tool::first_exceeding_definitions(&granted) {
            Some(exceeds) => Err(RuntimeError::Denied {
                detail: exceeds.to_string(),
            }),
            None => Ok(()),
        }
    }

    /// Revokes a grant at once (REQ-CAP-005). An approval requested under
    /// the old grant set is invalidated and its action rejected, whether the
    /// user has decided yet or not (REQ-APR-009).
    pub(crate) fn revoke_capability(
        &self,
        session: &Session,
        task_id: TaskId,
        grant_id: GrantId,
    ) -> Result<Grant, RuntimeError> {
        let task = self.writable(task_id)?;
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
                    from: "revoked".into(),
                    input: "revoke_capability".into(),
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
        let mut again = ApprovalRequest::new(
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
        again.arguments = approval.arguments.clone();
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
                snapshot: self.tree_identity(task.task_id, step.action_id)?,
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
        super::read::get_task(
            self.deps.persistence.as_ref(),
            self.deps.workspaces.as_ref(),
            self.deps.clock.now(),
            task_id,
        )
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
        let activity = super::read::activity(self.deps.persistence.as_ref())?;
        Ok(RuntimeStatus {
            version: self.deps.runtime_version.clone(),
            backend: self.guard(MODEL_BACKEND, || self.deps.backend.describe())?,
            tier: self
                .deps
                .probe
                .tier_with(&self.deps.policy.accept_best_effort)
                .describe(),
            clauses: self.deps.probe.clauses.clone(),
            unavailable_clauses: self.deps.probe.unavailable_lines(),
            cpu: self.deps.probe.cpu.clone(),
            unknown_to_this_release: activity.unknown_words(),
            running: activity.running,
            pending: activity.pending,
            unknown_state: activity.unknown_state,
        })
    }

    /// The task waiting for an approval, with that approval.
    fn pending_approval_of(&self, task: Task) -> Result<(Task, ApprovalRequest), RuntimeError> {
        let task_id = task.task_id;
        if !matches!(
            task.state,
            TaskState::WaitingUser {
                wait_kind: WaitKind::Approval
            }
        ) {
            return Err(RuntimeError::State(crate::task::StateError::Illegal {
                from: task.state.name().into(),
                input: "approve_action".into(),
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
                from: "approved".into(),
                input: "approve_action".into(),
            }));
        }
        Ok((task, approval))
    }

    /// The open step that holds the plan wait the task is in (REQ-TASK-015).
    fn plan_wait_step(&self, task: &Task) -> Result<crate::step::Step, RuntimeError> {
        let Some(PendingItem::Plan { version }) = task.pending else {
            return Err(RuntimeError::NotFound {
                what: "plan wait".into(),
            });
        };
        self.deps
            .persistence
            .load_steps(task.task_id)?
            .into_iter()
            .find(|s| {
                s.plan_wait
                    .as_ref()
                    .is_some_and(|w| w.version == version && w.is_pending())
            })
            .ok_or(RuntimeError::NotFound {
                what: "plan wait".into(),
            })
    }

    /// The user decides the plan the task waits for (REQ-TASK-015): `approve` ends the gate and
    /// the task runs; `reject` ends the plan's step `approval_rejected`, with the note, and the
    /// task runs gated or, with `halt`, halts `approval_rejected`.
    fn decide_plan(
        &self,
        session: &Session,
        task: Task,
        verdict: PlanVerdict,
    ) -> Result<Task, RuntimeError> {
        let now = self.deps.clock.now();
        let mut step = self.plan_wait_step(&task)?;
        let mut wait = step.plan_wait.take().expect("the step holds the wait");
        let version = wait.version;
        let (decision, note, input) = match verdict {
            PlanVerdict::Approve { note } => {
                (ApprovalDecision::Approved, note, TaskInput::PlanApproved)
            }
            PlanVerdict::Reject { note, halt } => {
                let halt = halt
                    .unwrap_or(self.profile_of(&task)?.on_reject == crate::policy::OnReject::Halt);
                (
                    ApprovalDecision::Rejected,
                    note,
                    if halt {
                        TaskInput::UserHalted
                    } else {
                        TaskInput::UserDecided
                    },
                )
            }
        };
        wait.decide(decision, session.actor(), note, now)
            .map_err(|e| RuntimeError::InvalidRequest {
                detail: e.to_string(),
            })?;
        step.plan_wait = Some(wait);
        let next = task.transition(input, now)?;
        let outcome = if decision == ApprovalDecision::Rejected {
            step.rejection = Some(RejectionCode::ApprovalRejected);
            StepOutcome::Rejected
        } else {
            StepOutcome::Continued
        };
        let decided = self.event(
            session.actor(),
            Some(task.task_id),
            Some(step.step_id),
            None,
            Event::PlanApprovalDecided { version, decision },
        );
        self.finish_plan_step(
            session.actor(),
            (&task, next),
            step,
            (outcome, vec![decided]),
            now,
        )
    }

    /// Ends the step that held a plan wait, applying the counters as the end of any step does
    /// (a budget reached halts a running task), and writes the step, the task and the events in
    /// one write. `task` is the task as it was and as the user's input left it; `end` is how the
    /// step ended and the events that came before its own end.
    fn finish_plan_step(
        &self,
        actor: Actor,
        (before, next): (&Task, Task),
        mut step: crate::step::Step,
        (outcome, mut events): (StepOutcome, Vec<crate::event::EventRecord>),
        now: Instant,
    ) -> Result<Task, RuntimeError> {
        let mut after = next;
        let budget = after.counters.apply(
            &StepResult {
                outcome,
                action: None,
                decision_hash: step.decision_hash.clone(),
                snapshot: self.tree_identity(before.task_id, None)?,
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
        events.push(self.event(
            actor.clone(),
            Some(before.task_id),
            Some(step.step_id),
            None,
            Event::StepFinished { outcome },
        ));
        events.push(self.event(
            actor,
            Some(before.task_id),
            None,
            None,
            state_changed(before, &after),
        ));
        self.deps
            .persistence
            .record_plan_step(&step, &after, None, &events)?;
        self.transitioned(before, &after);
        Ok(after)
    }

    /// The current instant, for callers that build records.
    pub fn now(&self) -> Instant {
        self.deps.clock.now()
    }
}

/// What the user decides about a plan (REQ-TASK-015).
enum PlanVerdict {
    Approve {
        note: Option<String>,
    },
    Reject {
        note: Option<String>,
        halt: Option<bool>,
    },
}

/// A start the profile forbids (REQ-SEC-010): the backend is `remote` and the task's profile has
/// `external = false`.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StartRefusal {
    /// The name of the backend in force.
    pub backend: String,
    /// The task's profile.
    pub profile: String,
}

/// The level a task is to be given and the backend its envelope names, before anything is created
/// ([`Runtime::plan_effort`]).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EffortPlan {
    /// The level.
    pub level: crate::effort::EffortLevel,
    /// Who chose it.
    pub chosen_by: crate::effort::ChosenBy,
    /// The backend the envelope names: the level's `backend`, else the runtime's own.
    pub backend: String,
    /// Whether the level's own `backend` key chose it (and not the runtime's `default`): what
    /// tells a message whether `[effort.<level>]` is a section that names the backend.
    pub backend_chosen_by_level: bool,
}

/// A backend that serves another model under the same name than the task last ran with
/// ([`Runtime::model_change`]).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ModelChange {
    /// The backend's name.
    pub backend: String,
    /// The model the task last ran with.
    pub was: String,
    /// The model the backend names now.
    pub now: String,
}
