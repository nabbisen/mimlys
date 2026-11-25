//! The task state machine (external design §5; REQ-TASK-002 … 012). A task
//! is in exactly one of nine states; `transition` implements the table of
//! external design §5.1 and refuses everything else. Terminal states are
//! immutable (REQ-TASK-003).

use crate::clock::Instant;
use crate::ids::{ActionId, QuestionId, TaskId};
use crate::step::{Budgets, Counters};
use serde::{Deserialize, Serialize};

/// What a task in `waiting_user` waits for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WaitKind {
    /// An approval request is pending.
    Approval,
    /// A question to the user is pending.
    Question,
    /// The plan of a plan-first task is pending (from 0.13; REQ-TASK-015).
    Plan,
}

/// Why the runtime stopped a task (external design §5.2, REQ-TASK-007).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HaltReason {
    /// LIM-T1 reached.
    BudgetSteps,
    /// LIM-T2 reached.
    BudgetRejections,
    /// LIM-T6 reached.
    BudgetFailures,
    /// The deadline passed.
    BudgetDeadline,
    /// LIM-T4 reached.
    Convergence,
    /// The user rejected an action with the halt flag, or the profile halts on rejection.
    ApprovalRejected,
    /// An approval request expired.
    ApprovalExpired,
    /// A question expired.
    QuestionExpired,
    /// Recovered after a restart; the user decides whether to continue.
    Recovered,
    /// An in-flight action with a non-reversible effect has an unknown outcome.
    UnknownOutcome,
    /// LIM-W1 or LIM-W2 breached.
    WorkspaceQuota,
    /// Part of the task's tree could not be read, so a snapshot, a rollback or the quota
    /// check could not be made; the user makes the path named in `workspace.unreadable`
    /// readable again (usually by restoring its permissions), then resumes.
    WorkspaceUnreadable,
    /// The model backend could not be reached after retries.
    BackendUnavailable,
    /// A tool could not be reached after retries.
    ToolUnavailable,
    /// Policy stopped the task.
    Policy,
    /// The definitions of the tools the task may use do not fit LIM-I1 (handoff
    /// 009i, audit A-32). A grant that would make them exceed it is refused, so
    /// this is the last guard: it is reached only by a task made before the
    /// refusal existed, or whose tools changed under it.
    DefinitionsTooLarge,
}

impl HaltReason {
    /// Every reason, in the order of external design §5.2.
    pub const ALL: [HaltReason; 16] = [
        HaltReason::BudgetSteps,
        HaltReason::BudgetRejections,
        HaltReason::BudgetFailures,
        HaltReason::BudgetDeadline,
        HaltReason::Convergence,
        HaltReason::ApprovalRejected,
        HaltReason::ApprovalExpired,
        HaltReason::QuestionExpired,
        HaltReason::Recovered,
        HaltReason::UnknownOutcome,
        HaltReason::WorkspaceQuota,
        HaltReason::WorkspaceUnreadable,
        HaltReason::BackendUnavailable,
        HaltReason::ToolUnavailable,
        HaltReason::Policy,
        HaltReason::DefinitionsTooLarge,
    ];
}

/// The rule that stopped a task, for a halt `policy`, which has more than one cause (external
/// design §5.2; from 0.13). A script tells the causes apart by `rule`, as the text line does.
///
/// Additive: it is a field of the `halted` state that is absent from every record written before
/// 0.13 and from every halt that has no detail, and an older reader that does not know it ignores
/// it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "rule", rename_all = "snake_case")]
pub enum HaltDetail {
    /// The task's backend is `remote` and its profile does not allow external communication,
    /// found at a start, before anything was sent (REQ-SEC-010).
    RemoteBackendForbidden {
        /// The backend's name.
        backend: String,
        /// The profile's name.
        profile: String,
    },
    /// The configured completion check was refused by the validator, or would need the user's
    /// approval, which a check never gets (REQ-TASK-007).
    CompletionCheckRefused,
    /// A task found `running` whose backend is no longer configured (recovery resumed it before
    /// the file changed): halted `backend_unavailable` before anything was sent, naming the
    /// backend and the ones the configuration has (external design §5.2, §19.4).
    BackendNotConfigured {
        /// The backend's name, as the task's envelope has it.
        backend: String,
        /// The names the configuration has now.
        configured: Vec<String>,
    },
}

/// How a task completed (external design §5.3, REQ-TASK-010).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Completion {
    /// The configured completion check passed.
    Verified,
    /// The model declared completion and no check was configured.
    Asserted,
}

/// Why a task failed (external design §5.3, REQ-TASK-011).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "failure_reason", rename_all = "snake_case")]
pub enum FailureReason {
    /// The model gave up; `reason` is its bounded text.
    ModelGaveUp {
        /// The model's stated reason (bounded by LIM-D2 at validation).
        reason: String,
    },
    /// The runtime met an unrecoverable condition.
    Unrecoverable {
        /// A fixed-vocabulary detail from the runtime.
        detail: String,
    },
    /// The workspace no longer exists.
    WorkspaceLost,
}

/// The state of a task. Serialized flat: a `state` field plus the payload's fields.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum TaskState {
    /// Persisted, not started.
    Created,
    /// The loop is active.
    Running,
    /// An approval, a question or a plan is pending; no resources are held.
    WaitingUser {
        /// What the task waits for.
        wait_kind: WaitKind,
    },
    /// The user paused it.
    Paused,
    /// The runtime stopped it; the user resumes or cancels.
    Halted {
        /// Why.
        halt_reason: HaltReason,
        /// The rule, for a halt `policy` (from 0.13); absent otherwise and in older records.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        halt_detail: Option<HaltDetail>,
    },
    /// Found running at startup; resolves to `halted` or `running`.
    Recovering,
    /// Terminal: the goal is reached.
    Completed {
        /// Verified or asserted.
        completion: Completion,
    },
    /// Terminal: the task could not be completed.
    Failed {
        /// Why.
        reason: FailureReason,
    },
    /// Terminal: cancelled by the user.
    Cancelled,
}

impl TaskState {
    /// Whether the state is terminal (immutable thereafter, REQ-TASK-003).
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            TaskState::Completed { .. } | TaskState::Failed { .. } | TaskState::Cancelled
        )
    }

    /// The snake_case name of the state without its payload.
    pub fn name(&self) -> &'static str {
        match self {
            TaskState::Created => "created",
            TaskState::Running => "running",
            TaskState::WaitingUser { .. } => "waiting_user",
            TaskState::Paused => "paused",
            TaskState::Halted { .. } => "halted",
            TaskState::Recovering => "recovering",
            TaskState::Completed { .. } => "completed",
            TaskState::Failed { .. } => "failed",
            TaskState::Cancelled => "cancelled",
        }
    }
}

/// The pending user item of a task in `waiting_user`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "pending", rename_all = "snake_case")]
pub enum PendingItem {
    /// An approval request for the action.
    Approval {
        /// The action awaiting approval.
        action_id: ActionId,
    },
    /// A question to the user.
    Question {
        /// The question.
        question_id: QuestionId,
    },
    /// The first plan of a plan-first task, which the user approves before anything acts (from
    /// 0.13; REQ-TASK-015). The step that holds the wait is the open step whose
    /// [`PlanWait`](crate::plan_wait::PlanWait) has this version.
    Plan {
        /// The plan version that waits.
        version: u32,
    },
}

impl PendingItem {
    /// The wait kind of this item.
    pub fn wait_kind(&self) -> WaitKind {
        match self {
            PendingItem::Approval { .. } => WaitKind::Approval,
            PendingItem::Question { .. } => WaitKind::Question,
            PendingItem::Plan { .. } => WaitKind::Plan,
        }
    }
}

/// A runtime-verifiable completion check: a tool action that must succeed
/// after the last mutation (external design §5.3; REQ-TASK-010).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompletionCheck {
    /// The tool to run.
    pub tool: String,
    /// Its arguments.
    pub arguments: serde_json::Map<String, serde_json::Value>,
}

/// What the loop does with a `complete` decision (REQ-TASK-010).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompletionStep {
    /// End the task `completed` with this completion kind.
    Finish(Completion),
    /// Run the configured check first.
    RunCheck,
    /// The check failed: the result is an observation, the task continues,
    /// and the failure counts toward LIM-T6.
    CheckFailed,
}

/// The project scope a task is materialized from (external design §14.2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScopeSpec {
    /// Absolute path of the project root on the host.
    pub root: String,
    /// Include patterns relative to the root; empty means everything.
    #[serde(default)]
    pub include: Vec<String>,
}

impl ScopeSpec {
    /// Whether two scopes overlap: one root is the other or lies below it (REQ-TASK-012).
    pub fn overlaps(&self, other: &ScopeSpec) -> bool {
        let a = self.root.trim_end_matches(['/', '\\']).replace('\\', "/");
        let b = other.root.trim_end_matches(['/', '\\']).replace('\\', "/");
        a == b || a.starts_with(&format!("{b}/")) || b.starts_with(&format!("{a}/"))
    }
}

/// The versions in force when a task was created (REQ-PERS-012, REQ-MAIN-002).
/// A record written before the event and action schema versions were
/// recorded (0.9.0 and earlier) reads with the values of its era, taken from
/// its `runtime_version`: see [`event_schema_version_of`] and
/// [`action_schema_version_of`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "TaskVersionsRecord")]
pub struct TaskVersions {
    /// Hash of the policy.
    pub policy_hash: crate::hash::Hash,
    /// The decision schema version.
    pub decision_schema_version: u32,
    /// The event schema version ([`crate::event::EVENT_SCHEMA_VERSION`]).
    pub event_schema_version: u32,
    /// The action, execution and observation schema version
    /// ([`crate::action::ACTION_SCHEMA_VERSION`]).
    pub action_schema_version: u32,
    /// Hash of each tool definition visible at creation, by tool name.
    pub tool_definition_hashes: std::collections::BTreeMap<String, crate::hash::Hash>,
    /// The runtime's version.
    pub runtime_version: String,
    /// The backend at creation (REQ-SEC-010); absent on records written before it was recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend: Option<crate::spi::BackendDescriptor>,
    /// The external tools imported at creation, by `server.tool` (RFC 009 §2).
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub tool_imports: std::collections::BTreeMap<String, crate::tool::ImportRecord>,
}

/// `TaskVersions` as stored: the two schema versions are absent from records
/// written by 0.9.0 and earlier.
#[derive(Deserialize)]
struct TaskVersionsRecord {
    policy_hash: crate::hash::Hash,
    decision_schema_version: u32,
    #[serde(default)]
    event_schema_version: Option<u32>,
    #[serde(default)]
    action_schema_version: Option<u32>,
    tool_definition_hashes: std::collections::BTreeMap<String, crate::hash::Hash>,
    runtime_version: String,
    #[serde(default)]
    backend: Option<crate::spi::BackendDescriptor>,
    #[serde(default)]
    tool_imports: std::collections::BTreeMap<String, crate::tool::ImportRecord>,
}

impl From<TaskVersionsRecord> for TaskVersions {
    fn from(r: TaskVersionsRecord) -> Self {
        TaskVersions {
            event_schema_version: r
                .event_schema_version
                .unwrap_or_else(|| event_schema_version_of(&r.runtime_version)),
            action_schema_version: r
                .action_schema_version
                .unwrap_or_else(|| action_schema_version_of(&r.runtime_version)),
            policy_hash: r.policy_hash,
            decision_schema_version: r.decision_schema_version,
            tool_definition_hashes: r.tool_definition_hashes,
            runtime_version: r.runtime_version,
            backend: r.backend,
            tool_imports: r.tool_imports,
        }
    }
}

/// The `major.minor.patch` of a released runtime version. A pre-release (a
/// development or test build: `0.10.0-dev`, `0.0.0-test`) is no release and
/// has none: it is built from the code that writes the current schemas.
fn release_of(runtime_version: &str) -> Option<(u32, u32, u32)> {
    // Strict: a pre-release suffix makes the patch part not a number.
    let core = runtime_version.split('+').next()?;
    let mut parts = core.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    parts.next().is_none().then_some((major, minor, patch))
}

/// The event schema version of the release that wrote a task: 1 before 0.9.0,
/// 2 from it. A version that is not a release (`major.minor.patch`) reads as
/// the current one.
pub fn event_schema_version_of(runtime_version: &str) -> u32 {
    match release_of(runtime_version) {
        Some(release) if release < (0, 9, 0) => 1,
        _ => crate::event::EVENT_SCHEMA_VERSION,
    }
}

/// The action schema version of the release that wrote a task: 1 before
/// 0.8.0, 2 from it. A version that is not a release (`major.minor.patch`)
/// reads as the current one.
pub fn action_schema_version_of(runtime_version: &str) -> u32 {
    match release_of(runtime_version) {
        Some(release) if release < (0, 8, 0) => 1,
        _ => crate::action::ACTION_SCHEMA_VERSION,
    }
}

/// One immutable version of a task's plan (external design §6.2, REQ-DEC-011).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanVersion {
    /// The task.
    pub task_id: TaskId,
    /// One-based version.
    pub version: u32,
    /// Intended steps.
    pub steps: Vec<String>,
    /// When created.
    pub created_at: Instant,
}

/// A persistent unit of work (external design §5; REQ-TASK-001).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    /// Identifier.
    pub task_id: TaskId,
    /// The user's goal (bounded by LIM-D4 at creation).
    pub goal: String,
    /// Current state.
    #[serde(flatten)]
    pub state: TaskState,
    /// Creation instant.
    pub created_at: Instant,
    /// Instant of the last transition.
    pub updated_at: Instant,
    /// Absolute deadline (REQ-TASK-009).
    pub deadline: Instant,
    /// Budgets in force.
    pub budgets: Budgets,
    /// Durable counters (REQ-TASK-008).
    pub counters: Counters,
    /// A pause was requested and takes effect at the next step boundary.
    pub pause_requested: bool,
    /// The pending user item while `waiting_user`.
    pub pending: Option<PendingItem>,
    /// The completion check, when configured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion_check: Option<CompletionCheck>,
    /// The versions in force at creation.
    pub versions: TaskVersions,
    /// The profile name.
    pub profile: String,
    /// The project scope.
    pub scope: ScopeSpec,
    /// The task this one was forked from (REQ-TASK-004).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forked_from: Option<TaskId>,
    /// The task was created with `--plan-first` (from 0.13; REQ-TASK-015): until a plan is
    /// approved, an `act` or a `complete` is rejected `plan_required`. Absent when false, so that
    /// a task without the flag has the record it always had.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub plan_first: bool,
    /// The version of the plan the user approved, once they did (from 0.13). It is what ends the
    /// gate, and is read from the task's own record, never from the configuration, so that a
    /// restart, a recovery and a resume keep it. Absent before, and for a task that is not
    /// plan-first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_approved: Option<u32>,
    /// The effort level, who chose it and the envelope the task runs with, resolved at creation
    /// and never changed during the task (from 0.13; REQ-TASK-013). Absent in a record written
    /// before 0.13, which reads as `standard` ([`Task::effort_in_force`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<crate::effort::Effort>,
}

/// A budget extension requested at resumption from a budget halt (REQ-TASK-005).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Extension {
    /// New step limit, if raised.
    pub steps: Option<u32>,
    /// New deadline, if moved later.
    pub deadline: Option<Instant>,
}

/// The ceiling an extension must respect (from the administrator's policy, handoff 003).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExtensionCeiling {
    /// Highest step limit the profile allows.
    pub max_steps: u32,
    /// Latest deadline the profile allows, measured from creation.
    pub max_deadline: Instant,
}

/// An input to the task state machine (one row of external design §5.1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TaskInput {
    /// `resume_task`, valid from `created`, `paused`, `halted`.
    Resume {
        /// Budget extension, valid only from a budget halt.
        extend: Option<Extension>,
        /// The ceiling for the extension.
        ceiling: ExtensionCeiling,
    },
    /// `pause_task`: records the request; the loop pauses at the boundary.
    PauseRequest,
    /// The loop reached a step boundary with a pause requested.
    Pause,
    /// `cancel_task`, valid from every non-terminal state.
    Cancel,
    /// An approval or question is pending.
    NeedsUser(PendingItem),
    /// The user approved, rejected without halt, or answered.
    UserDecided,
    /// The user approved the plan the task waits for: the gate ends, and the version is kept
    /// (from 0.13).
    PlanApproved,
    /// The user rejected with the halt flag.
    UserHalted,
    /// The pending item expired.
    Expired,
    /// The runtime halts the task. A halt `policy` is [`TaskInput::HaltPolicy`], which says why.
    Halt(HaltReason),
    /// The runtime halts the task `policy`, naming the rule (from 0.13).
    HaltPolicy(HaltDetail),
    /// The runtime halts the task `backend_unavailable`, naming the rule: its backend is no
    /// longer configured (from 0.13).
    HaltBackendNotConfigured(HaltDetail),
    /// The task completed.
    Complete(Completion),
    /// The task failed.
    Fail(FailureReason),
    /// Runtime start found the task running.
    StartRecovery,
    /// Recovery finished: `resume` continues the loop, otherwise the task halts with `reason`.
    Recovered {
        /// Continue the loop (policy `recovery.auto_resume` with a clean disposition).
        resume: bool,
        /// Halt reason when not resuming (`Recovered` or `UnknownOutcome`).
        reason: HaltReason,
    },
}

/// A transition was refused (API error kind `state`).
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum StateError {
    /// The input is not valid in the current state.
    #[error("input {input} is not valid in state {from}")]
    Illegal {
        /// The state the task was in.
        from: &'static str,
        /// The input's name.
        input: &'static str,
    },
    /// An extension was requested where none is allowed, or beyond the ceiling.
    #[error("budget extension refused: {reason}")]
    Extension {
        /// What was wrong.
        reason: &'static str,
    },
    /// A resume would halt again at once because the deadline has passed.
    #[error("the deadline has passed; resume with a later deadline")]
    DeadlinePassed,
    /// A resume would halt again at once because the step budget is used up.
    #[error("the step budget is used up; resume with a higher step limit")]
    StepBudgetExhausted,
}

impl TaskInput {
    fn name(&self) -> &'static str {
        match self {
            TaskInput::Resume { .. } => "resume",
            TaskInput::PauseRequest => "pause_request",
            TaskInput::Pause => "pause",
            TaskInput::Cancel => "cancel",
            TaskInput::NeedsUser(_) => "needs_user",
            TaskInput::UserDecided => "user_decided",
            TaskInput::PlanApproved => "plan_approved",
            TaskInput::UserHalted => "user_halted",
            TaskInput::Expired => "expired",
            TaskInput::Halt(_)
            | TaskInput::HaltPolicy(_)
            | TaskInput::HaltBackendNotConfigured(_) => "halt",
            TaskInput::Complete(_) => "complete",
            TaskInput::Fail(_) => "fail",
            TaskInput::StartRecovery => "start_recovery",
            TaskInput::Recovered { .. } => "recovered",
        }
    }
}

/// What a new task is made of (REQ-TASK-001).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskSpec {
    /// The task's id.
    pub task_id: TaskId,
    /// The goal.
    pub goal: String,
    /// Milliseconds from creation to the deadline (LIM-T5).
    pub deadline_ms: i64,
    /// Budgets.
    pub budgets: Budgets,
    /// Profile name.
    pub profile: String,
    /// Pinned versions.
    pub versions: TaskVersions,
    /// Project scope.
    pub scope: ScopeSpec,
    /// Created with `--plan-first` (REQ-TASK-015).
    pub plan_first: bool,
}

impl Task {
    /// Creates a task in `created` (REQ-TASK-001) with the deadline at
    /// `now + deadline_ms` (REQ-TASK-009).
    pub fn create(spec: TaskSpec, now: Instant) -> Result<Self, crate::clock::InstantError> {
        Ok(Self {
            task_id: spec.task_id,
            goal: spec.goal,
            state: TaskState::Created,
            created_at: now,
            updated_at: now,
            deadline: now.plus_millis(spec.deadline_ms)?,
            budgets: spec.budgets,
            counters: Counters::default(),
            pause_requested: false,
            pending: None,
            completion_check: None,
            versions: spec.versions,
            profile: spec.profile,
            scope: spec.scope,
            forked_from: None,
            plan_first: spec.plan_first,
            plan_approved: None,
            effort: None,
        })
    }

    /// Records the level the task was created at and the envelope it runs with.
    pub fn with_effort(mut self, effort: crate::effort::Effort) -> Self {
        self.effort = Some(effort);
        self
    }

    /// The task's level, who chose it and its envelope. A record written before 0.13 has none:
    /// it reads as `standard`, chosen by default, with the envelope of its own budgets (the
    /// backend `default`, its steps, its deadline from creation, no context budget of its own
    /// and LIM-C3's recent steps).
    pub fn effort_in_force(&self) -> crate::effort::Effort {
        self.effort
            .clone()
            .unwrap_or_else(|| crate::effort::Effort {
                level: crate::effort::EffortLevel::Standard,
                chosen_by: crate::effort::ChosenBy::Default,
                envelope: crate::effort::Envelope {
                    backend: crate::effort::DEFAULT_BACKEND.to_owned(),
                    steps: self.budgets.steps,
                    deadline_ms: self.created_at.millis_until(self.deadline),
                    context_chars: None,
                    recent_steps: crate::limits::LIM_C3_RECENT_STEPS as u32,
                },
            })
    }

    /// The steps kept whole under compression (LIM-C3, `recent_steps`), read from the task.
    pub fn recent_steps(&self) -> usize {
        self.effort
            .as_ref()
            .map_or(crate::limits::LIM_C3_RECENT_STEPS, |e| {
                e.envelope.recent_steps as usize
            })
    }

    /// Whether the gate of a plan-first task is closed: an `act` or a `complete` is rejected
    /// `plan_required` until a plan is approved (REQ-TASK-015). Read from the record alone.
    pub fn plan_required(&self) -> bool {
        self.plan_first && self.plan_approved.is_none()
    }

    /// Sets the completion check.
    pub fn with_completion_check(mut self, check: CompletionCheck) -> Self {
        self.completion_check = Some(check);
        self
    }

    /// The completion rule of external design §5.3: without a check the
    /// task finishes `asserted`; with one, the check runs first, and the
    /// task finishes `verified` only when it passed.
    pub fn completion_step(&self, check_result: Option<bool>) -> CompletionStep {
        match (&self.completion_check, check_result) {
            (None, _) => CompletionStep::Finish(Completion::Asserted),
            (Some(_), None) => CompletionStep::RunCheck,
            (Some(_), Some(true)) => CompletionStep::Finish(Completion::Verified),
            (Some(_), Some(false)) => CompletionStep::CheckFailed,
        }
    }

    /// Milliseconds until the deadline; zero or negative when it has passed.
    pub fn remaining_ms(&self, now: Instant) -> i64 {
        now.millis_until(self.deadline)
    }

    /// Whether the deadline has passed.
    pub fn deadline_passed(&self, now: Instant) -> bool {
        self.remaining_ms(now) <= 0
    }

    /// Applies `input` at `now`, returning the new task or refusing
    /// (external design §5.1). The task itself is not modified.
    pub fn transition(&self, input: TaskInput, now: Instant) -> Result<Task, StateError> {
        let illegal = || StateError::Illegal {
            from: self.state.name(),
            input: input.name(),
        };
        let mut next = self.clone();
        next.updated_at = now;
        let state = match (&self.state, &input) {
            // created
            (TaskState::Created, TaskInput::Resume { extend, .. }) => {
                if extend.is_some() {
                    return Err(StateError::Extension {
                        reason: "an extension is allowed only from a budget halt",
                    });
                }
                TaskState::Running
            }
            (TaskState::Created, TaskInput::Cancel) => TaskState::Cancelled,

            // running
            (TaskState::Running, TaskInput::PauseRequest) => {
                next.pause_requested = true;
                TaskState::Running
            }
            (TaskState::Running, TaskInput::Pause) => {
                next.pause_requested = false;
                TaskState::Paused
            }
            (TaskState::Running, TaskInput::NeedsUser(item)) => {
                next.pending = Some(*item);
                TaskState::WaitingUser {
                    wait_kind: item.wait_kind(),
                }
            }
            (TaskState::Running, TaskInput::Halt(reason)) => TaskState::Halted {
                halt_reason: *reason,
                halt_detail: None,
            },
            (TaskState::Running, TaskInput::HaltPolicy(detail)) => TaskState::Halted {
                halt_reason: HaltReason::Policy,
                halt_detail: Some(detail.clone()),
            },
            (TaskState::Running, TaskInput::HaltBackendNotConfigured(detail)) => {
                TaskState::Halted {
                    halt_reason: HaltReason::BackendUnavailable,
                    halt_detail: Some(detail.clone()),
                }
            }
            (TaskState::Running, TaskInput::Complete(completion)) => TaskState::Completed {
                completion: *completion,
            },
            (TaskState::Running, TaskInput::Fail(reason)) => TaskState::Failed {
                reason: reason.clone(),
            },
            (TaskState::Running, TaskInput::Cancel) => TaskState::Cancelled,
            (TaskState::Running, TaskInput::StartRecovery) => TaskState::Recovering,

            // waiting_user
            (TaskState::WaitingUser { .. }, TaskInput::UserDecided) => {
                next.pending = None;
                TaskState::Running
            }
            (
                TaskState::WaitingUser {
                    wait_kind: WaitKind::Plan,
                },
                TaskInput::PlanApproved,
            ) => {
                let Some(PendingItem::Plan { version }) = next.pending else {
                    return Err(illegal());
                };
                next.plan_approved = Some(version);
                next.pending = None;
                TaskState::Running
            }
            (TaskState::WaitingUser { .. }, TaskInput::UserHalted) => {
                next.pending = None;
                TaskState::Halted {
                    halt_reason: HaltReason::ApprovalRejected,
                    halt_detail: None,
                }
            }
            (TaskState::WaitingUser { wait_kind }, TaskInput::Expired) => {
                next.pending = None;
                TaskState::Halted {
                    halt_reason: match wait_kind {
                        WaitKind::Approval => HaltReason::ApprovalExpired,
                        WaitKind::Question => HaltReason::QuestionExpired,
                        // A plan is a request for approval: it expires as one does (LIM-A1).
                        WaitKind::Plan => HaltReason::ApprovalExpired,
                    },
                    halt_detail: None,
                }
            }
            (TaskState::WaitingUser { .. }, TaskInput::Cancel) => {
                next.pending = None;
                TaskState::Cancelled
            }

            // paused
            (TaskState::Paused, TaskInput::Resume { extend, .. }) => {
                if extend.is_some() {
                    return Err(StateError::Extension {
                        reason: "an extension is allowed only from a budget halt",
                    });
                }
                TaskState::Running
            }
            (TaskState::Paused, TaskInput::Cancel) => TaskState::Cancelled,

            // halted
            (TaskState::Halted { halt_reason, .. }, TaskInput::Resume { extend, ceiling }) => {
                if let Some(extension) = extend {
                    apply_extension(&mut next, *halt_reason, *extension, *ceiling)?;
                }
                // A resume that would halt again at the first boundary is refused (REQ-TASK-005).
                if next.deadline_passed(now) {
                    return Err(StateError::DeadlinePassed);
                }
                if next.counters.steps >= next.budgets.steps {
                    return Err(StateError::StepBudgetExhausted);
                }
                next.pause_requested = false;
                TaskState::Running
            }
            (TaskState::Halted { .. }, TaskInput::Cancel) => TaskState::Cancelled,

            // recovering
            (TaskState::Recovering, TaskInput::Recovered { resume: true, .. }) => {
                TaskState::Running
            }
            (
                TaskState::Recovering,
                TaskInput::Recovered {
                    resume: false,
                    reason,
                },
            ) => TaskState::Halted {
                halt_reason: *reason,
                halt_detail: None,
            },
            (TaskState::Recovering, TaskInput::Cancel) => TaskState::Cancelled,

            _ => return Err(illegal()),
        };
        next.state = state;
        if next.state.is_terminal() {
            next.pause_requested = false;
            next.pending = None;
        }
        Ok(next)
    }
}

fn apply_extension(
    task: &mut Task,
    halt_reason: HaltReason,
    extension: Extension,
    ceiling: ExtensionCeiling,
) -> Result<(), StateError> {
    match halt_reason {
        HaltReason::BudgetSteps | HaltReason::BudgetDeadline => {}
        _ => {
            return Err(StateError::Extension {
                reason: "an extension is allowed only from a budget halt",
            });
        }
    }
    if let Some(steps) = extension.steps {
        if steps <= task.budgets.steps {
            return Err(StateError::Extension {
                reason: "the step limit can only be raised",
            });
        }
        if steps > ceiling.max_steps {
            return Err(StateError::Extension {
                reason: "the step limit exceeds the ceiling",
            });
        }
        task.budgets.steps = steps;
    }
    if let Some(deadline) = extension.deadline {
        if deadline <= task.deadline {
            return Err(StateError::Extension {
                reason: "the deadline can only be moved later",
            });
        }
        if deadline > ceiling.max_deadline {
            return Err(StateError::Extension {
                reason: "the deadline exceeds the ceiling",
            });
        }
        task.deadline = deadline;
    }
    Ok(())
}

/// Versions for tests and replay fixtures (REQ-PERS-012).
#[cfg(any(test, feature = "test-support"))]
pub fn test_versions() -> TaskVersions {
    TaskVersions {
        policy_hash: crate::hash::hash_bytes(b"policy"),
        decision_schema_version: 1,
        event_schema_version: crate::event::EVENT_SCHEMA_VERSION,
        action_schema_version: crate::action::ACTION_SCHEMA_VERSION,
        tool_definition_hashes: std::collections::BTreeMap::new(),
        runtime_version: "0.0.0-test".into(),
        backend: None,
        tool_imports: Default::default(),
    }
}

#[cfg(test)]
mod tests;
