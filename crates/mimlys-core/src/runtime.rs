//! The runtime: the step loop and the control API over the adapter SPI
//! (external design §4, §5, §6.1, §16.2; REQ-ARCH-004, REQ-DEC-001,
//! REQ-DEC-009, REQ-TASK-001 … 012, REQ-SCHED-002). Pure orchestration: no
//! I/O of its own, every transition written through `Persistence` in the
//! order of external design §16.2, the executor called only with an
//! `AuthorizedAction`, and the backend called only for the step decision.

mod contain;
mod context;
mod control;
mod handler;
mod loop_;
mod read;
mod recorded;
mod requests;
mod steps;
mod storage;

pub use control::{
    CreateTask, EffortPlan, ModelChange, RuntimeStatus, StartRefusal, TaskView, UnknownAction,
    Waiting,
};
pub use loop_::{PROMPT_LOG_TARGET, StepResultView};
pub use read::{Activity, Reader, TaskShown, WaitWord, readable_footprint_records};
pub use storage::{Holdings, TaskHolding};

use crate::clock::Clock;
use crate::event::{Actor, Event, EventRecord};
use crate::hash::Hash;
use crate::ids::{ActionId, EventId, IdSource, StepId, TaskId};
use crate::path::PlatformRules;
use crate::policy::{ExclusionMatcher, Policy};
use crate::spi::{
    BackendError, Cancellation, ModelBackend, Persistence, PersistenceError, UnknownBackend,
    WorkspaceError, WorkspaceProvider,
};
use crate::task::{HaltDetail, StateError, Task};
use std::sync::Arc;

/// The session the control API acts under (v1: the CLI process).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Session {
    /// A session identifier, `pid:<n>` in v1.
    pub id: String,
    /// The operating-system user.
    pub os_user: String,
}

impl Session {
    /// The actor for audit records.
    pub fn actor(&self) -> Actor {
        Actor::User {
            session: self.id.clone(),
            os_user: self.os_user.clone(),
        }
    }
}

/// Everything the runtime is built from.
pub struct RuntimeDeps {
    /// The store.
    pub persistence: Arc<dyn Persistence>,
    /// The model backend.
    pub backend: Arc<dyn ModelBackend>,
    /// The name the configuration gives that backend (`default`: a task runs on no other until
    /// RFC 015's effort levels), so that a halt names it (`halt_detail`, REQ-SEC-010).
    pub backend_name: String,
    /// Workspaces and the tools over them.
    pub workspaces: Arc<dyn WorkspaceProvider>,
    /// The policy in force.
    pub policy: Policy,
    /// Identifier source.
    pub ids: Arc<dyn IdSource>,
    /// Clock.
    pub clock: Arc<dyn Clock>,
    /// The runtime's version string.
    pub runtime_version: String,
    /// The platform's path rules.
    pub rules: PlatformRules,
    /// What the confinement probe found at startup (REQ-CONF-003).
    pub probe: crate::confine::Probe,
    /// Configured secrets as `(name, value)`, redacted from observations
    /// before persistence (REQ-CTX-007).
    pub secrets: Vec<(String, String)>,
    /// The context budget in characters (LIM-C1 or the backend's override).
    pub context_chars: usize,
    /// LIM-S4 as configured (`sandbox.output_bytes`): what a script's stream may hold. The bound
    /// of a stream's whole copy follows it ([`crate::observation::WholeStreams::bound_bytes`]),
    /// as the bound of the worker's capture does.
    pub output_bytes: usize,
}

/// A runtime failure reported to the control API (external design §18).
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error, serde::Serialize, serde::Deserialize)]
#[serde(tag = "error", content = "detail", rename_all = "snake_case")]
pub enum RuntimeError {
    /// A parameter failed validation.
    #[error("invalid request: {detail}")]
    InvalidRequest {
        /// What.
        detail: String,
    },
    /// Unknown task, action or grant.
    #[error("not found: {what}")]
    NotFound {
        /// What.
        what: String,
    },
    /// The verb is not valid in the current state.
    #[error("{0}")]
    State(#[from] StateError),
    /// Beyond the ceiling or not allowed.
    #[error("denied: {detail}")]
    Denied {
        /// Why.
        detail: String,
    },
    /// Overlapping scope or similar conflict.
    #[error("conflict: {detail}")]
    Conflict {
        /// With what.
        detail: String,
    },
    /// Backend, worker or workspace not available.
    #[error("unavailable: {component}")]
    Unavailable {
        /// Which component.
        component: String,
    },
    /// The store failed.
    #[error("{0}")]
    Persistence(#[from] PersistenceError),
    /// The workspace failed.
    #[error("{0}")]
    Workspace(#[from] WorkspaceError),
    /// A promotion did not complete (REQ-WS-007): what it had written was restored from its
    /// backup, except the paths listed, which were edited since and are left as they are.
    #[error("{}", promotion_failed_message(cause, kept, restore_error.as_deref(), restore_hint))]
    PromotionFailed {
        /// Why the apply stopped.
        cause: WorkspaceError,
        /// Paths left as they are (edited since, or a directory that is not empty).
        kept: Vec<String>,
        /// Whether the restore itself failed.
        restore_error: Option<String>,
        /// The command that restores the project from the kept backup (see
        /// `Promotion::restore_hint`).
        restore_hint: String,
    },
    /// A change that a request wrote and then failed, answered when the request is sent again
    /// (external design §4, "The recorded outcome"): the reason as the change's event recorded
    /// it. The first answer was the verb's own error, whose kind and details the event does not
    /// keep, so this one is `internal`.
    #[error("{reason}")]
    Recorded {
        /// The event that recorded the failure.
        event: String,
        /// Why, as it was recorded.
        reason: String,
    },
    /// The backend failed.
    #[error("{0}")]
    Backend(#[from] BackendError),
}

/// The text of a promotion that did not complete: why, and what became of the project.
fn promotion_failed_message(
    cause: &WorkspaceError,
    kept: &[String],
    restore_error: Option<&str>,
    restore_hint: &str,
) -> String {
    if let Some(e) = restore_error {
        return format!(
            "the promotion failed ({cause}), and restoring the project from its backup failed too ({e}); {restore_hint}"
        );
    }
    if kept.is_empty() {
        return format!("the promotion failed ({cause}); the project was restored from its backup");
    }
    format!(
        "the promotion failed ({cause}); the project was restored from its backup except {} path(s) edited since, which are left as they are: {}",
        kept.len(),
        kept.join(", ")
    )
}

impl RuntimeError {
    /// The error kind of external design §18.
    pub fn kind(&self) -> &'static str {
        match self {
            RuntimeError::InvalidRequest { .. } => "invalid_request",
            RuntimeError::NotFound { .. } => "not_found",
            RuntimeError::State(_) => "state",
            RuntimeError::Denied { .. } => "denied",
            RuntimeError::Conflict { .. } => "conflict",
            RuntimeError::Unavailable { .. }
            | RuntimeError::Backend(_)
            | RuntimeError::Workspace(WorkspaceError::PermissionsNotApplied { .. }) => {
                "unavailable"
            }
            RuntimeError::Workspace(
                WorkspaceError::Conflict { .. }
                | WorkspaceError::Unlisted { .. }
                | WorkspaceError::EditedSince { .. },
            ) => "conflict",
            RuntimeError::PromotionFailed {
                cause: WorkspaceError::Conflict { .. } | WorkspaceError::Unlisted { .. },
                ..
            } => "conflict",
            RuntimeError::PromotionFailed { .. } | RuntimeError::Recorded { .. } => "internal",
            RuntimeError::Workspace(WorkspaceError::NotFound { .. }) => "not_found",
            RuntimeError::Workspace(
                WorkspaceError::Refused { .. }
                | WorkspaceError::Unreadable { .. }
                | WorkspaceError::ReadFailed { .. },
            ) => "state",
            RuntimeError::Workspace(
                WorkspaceError::TooLarge { .. }
                | WorkspaceError::ScopeNotFound
                | WorkspaceError::InsideDataDir
                | WorkspaceError::ContainsDataDir,
            ) => "invalid_request",
            // A record that a newer release wrote: the task is not in a state this release can act
            // on (REQ-MAIN-006).
            RuntimeError::Persistence(PersistenceError::NewerRecord { .. }) => "state",
            RuntimeError::Persistence(_) | RuntimeError::Workspace(_) => "internal",
        }
    }
}

/// What has been asked of the step in flight (REQ-CLI-005, external design
/// §19.2). Three requests, each stronger than the one before and implying it:
/// the weakest starts no new action; the first interrupts what can be undone
/// (a model request; an action whose rule after `unknown` is re-execution or
/// rollback); the second interrupts anything, an action that cannot be undone
/// included, whose outcome is then unknown.
#[derive(Clone, Debug, Default)]
struct Interruption {
    /// Read before an action is recorded as running and again after, before its
    /// call: when set at either look, the action is closed `cancelled` without
    /// a call.
    no_new_action: Cancellation,
    /// Observed by a model request and by an action that can be undone.
    undoable: Cancellation,
    /// Observed by an action that cannot be undone.
    anyway: Cancellation,
}

/// An action in flight that cannot be undone (REQ-CLI-005).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IrreversibleAction {
    /// The action.
    pub action_id: ActionId,
    /// The tool it runs.
    pub tool: String,
    /// Its effect, which tells the user why it cannot be undone.
    pub effect: crate::action::EffectKind,
}

/// What the start of a task is refused with when its backend is no longer configured (external
/// design §20; RFC 015, Amendment 1, item 4): the backend's name and the configured ones. The task
/// stays as it was, and nothing moves it to another backend.
pub fn backend_not_configured(name: &str, configured: &[String]) -> String {
    let have = if configured.is_empty() {
        "none is configured".to_owned()
    } else {
        format!(
            "configured: {}",
            configured
                .iter()
                .map(|n| format!("`{n}`"))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    format!(
        "the task's backend `{name}` is not configured ({have}); nothing moves a task to another backend: write `[backends.{name}]` in the configuration again, then start the task"
    )
}

/// The component of `RuntimeError::Unavailable` and of the guarded calls into the model backend
/// (REQ-SEC-011): the word the command line gives as `detail.component` of an `unavailable` error
/// (handoff 004d).
pub const MODEL_BACKEND: &str = "model_backend";

/// The component of the guarded calls into the tool provider.
pub const TOOL_PROVIDER: &str = "tool_provider";

/// The runtime.
pub struct Runtime {
    deps: RuntimeDeps,
    policy_hash: Hash,
    exclusions: ExclusionMatcher,
    /// The interruption requests of each task with work in flight (REQ-SCHED-004).
    interruptions: std::sync::Mutex<std::collections::HashMap<TaskId, Interruption>>,
    /// The tasks with a step in flight or a cancel in progress (REQ-DEC-001, REQ-TASK-006).
    steps: steps::Steps,
    /// The ids of the requests being handled now (external design §4).
    requests: requests::InFlight,
    /// The redactor built from the configured secrets.
    redactor: crate::context::Redactor,
    /// Each task's previous request in this process, for the prefix measure (REQ-CTX-010).
    last_requests: std::sync::Mutex<std::collections::HashMap<TaskId, Vec<crate::spi::Message>>>,
    /// External tools unpinned per task in this process (RFC 009 §7).
    unpinned:
        std::sync::Mutex<std::collections::HashMap<TaskId, std::collections::BTreeSet<String>>>,
    /// The admission of executions across the user's runtimes (RFC 013 decision 5); none in a
    /// runtime built without one (the tests of the core, a host with no registry).
    admission: Option<Arc<dyn crate::spi::Admission>>,
    /// The gate of the backend's endpoint among the user's runtimes (RFC 013 decision 7); none in
    /// a runtime built without one (the fake backend has no endpoint).
    engine_gate: Option<Arc<dyn crate::spi::Admission>>,
    /// The tasks whose last execution went on unadmitted, so that the event and the warning are
    /// made once while it lasts, not at every execution.
    unadmitted: std::sync::Mutex<std::collections::HashSet<TaskId>>,
    /// The configured backends by name, where a task's backend other than `deps.backend` is found
    /// (REQ-CFG-007, REQ-MDL-010); none in a runtime built without one, which then has the one
    /// backend of `deps` and no other.
    backends: Option<Arc<dyn crate::spi::BackendDirectory>>,
    /// The keys of the three effort levels (REQ-TASK-013): what ships, and what the configuration
    /// replaces it with.
    levels: crate::effort::Levels,
    /// LIM-O2, in milliseconds (a test may lower or raise it): the time an execution's tree
    /// has to end after a cancel. A cancel's wait for the step in flight follows it (LIM-O19).
    cancel_grace_ms: u64,
}

/// Why a backend name was not found.
enum BackendLookup {
    /// Nothing is configured under the name; these are the configured names.
    NotConfigured(Vec<String>),
    /// A configured backend that could not be made, and why.
    Unusable(String),
}

/// The backend a task runs on, found by its name (see [`Runtime::backend_named`]).
#[derive(Clone)]
pub(crate) struct TaskBackend {
    /// The name it was found by.
    pub(crate) name: String,
    /// The adapter.
    pub(crate) backend: Arc<dyn ModelBackend>,
    /// The gate of its endpoint among the user's runtimes, when it has one.
    pub(crate) gate: Option<Arc<dyn crate::spi::Admission>>,
    /// Its context budget in characters.
    pub(crate) context_chars: usize,
}

impl Runtime {
    /// Adds the configured backends by name: a task whose envelope names another backend than
    /// `deps.backend` runs on the one found here, resolved again at every start (REQ-CFG-007;
    /// RFC 015, Amendment 1, item 4). Nothing is contacted by adding it.
    #[must_use]
    pub fn with_backends(mut self, backends: Arc<dyn crate::spi::BackendDirectory>) -> Self {
        self.backends = Some(backends);
        self
    }

    /// Sets the keys of the effort levels (REQ-TASK-013). A runtime built without them has what
    /// ships: `quick`'s two keys, none for `standard` and `deep`.
    #[must_use]
    pub fn with_effort_levels(mut self, levels: crate::effort::Levels) -> Self {
        self.levels = levels;
        self
    }

    /// **The one place a backend name is resolved** (RFC 015, Amendment 1, item 4): the backend
    /// of `deps` when the name is its own (`backend_name`), else the one the directory has under
    /// that name. A name nothing has is refused, naming the configured ones, and never moved to
    /// another backend. Nothing is contacted: the backend is made, not asked.
    pub(crate) fn backend_named(&self, name: &str) -> Result<TaskBackend, RuntimeError> {
        self.find_backend(name).map_err(|lookup| match lookup {
            BackendLookup::NotConfigured(names) => RuntimeError::InvalidRequest {
                detail: crate::runtime::backend_not_configured(name, &names),
            },
            BackendLookup::Unusable(reason) => RuntimeError::InvalidRequest {
                detail: format!("backend `{name}` cannot be used: {reason}"),
            },
        })
    }

    /// [`Runtime::backend_named`] with the cause kept apart: a name that is no longer configured
    /// is told from a configured backend that could not be made, because a task found `running`
    /// is halted with a different `halt_detail` for each (external design §19.4).
    fn find_backend(&self, name: &str) -> Result<TaskBackend, BackendLookup> {
        if name == self.deps.backend_name {
            return Ok(TaskBackend {
                name: name.to_owned(),
                backend: Arc::clone(&self.deps.backend),
                gate: self.engine_gate.clone(),
                context_chars: self.deps.context_chars,
            });
        }
        let Some(directory) = &self.backends else {
            return Err(BackendLookup::NotConfigured(vec![
                self.deps.backend_name.clone(),
            ]));
        };
        match directory.resolve(name) {
            Ok(found) => Ok(TaskBackend {
                name: name.to_owned(),
                backend: found.backend,
                gate: found.gate,
                context_chars: found.context_chars,
            }),
            Err(UnknownBackend {
                unusable: Some(reason),
                ..
            }) => Err(BackendLookup::Unusable(reason)),
            Err(unknown) => Err(BackendLookup::NotConfigured(unknown.configured)),
        }
    }

    /// The halt a task found `running` is given when its backend cannot be had, and `None` when
    /// it can: the backend is no longer configured (`backend_not_configured`, naming the
    /// configured ones), or it is configured and cannot be made (`backend_unusable`, with the
    /// reason) (external design §19.4). Nothing is contacted.
    pub(crate) fn backend_halt_for(&self, task: &Task) -> Option<HaltDetail> {
        let backend = match &task.effort {
            Some(effort) => effort.envelope.backend.clone(),
            None => self.deps.backend_name.clone(),
        };
        match self.find_backend(&backend) {
            Err(BackendLookup::NotConfigured(configured)) => {
                Some(HaltDetail::BackendNotConfigured {
                    backend,
                    configured,
                })
            }
            Err(BackendLookup::Unusable(reason)) => {
                Some(HaltDetail::BackendUnusable { backend, reason })
            }
            Ok(_) => None,
        }
    }

    /// The backend a task runs on: the one its envelope names, or, for a task written before
    /// 0.13, the runtime's own.
    pub(crate) fn backend_of(&self, task: &Task) -> Result<TaskBackend, RuntimeError> {
        match &task.effort {
            Some(effort) => self.backend_named(&effort.envelope.backend),
            None => self.backend_named(&self.deps.backend_name.clone()),
        }
    }

    /// Sets the time an execution's tree has to end after a cancel, LIM-O2, for a test. The
    /// wait of a cancel for the step in flight is this plus 3000 ms (LIM-O19), so that a cancel
    /// is not refused while the execution it waits for is still within its own bound. LIM-O2
    /// has no configuration key, so a runtime outside the tests always has it at its value.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn with_cancel_grace(mut self, grace: std::time::Duration) -> Self {
        self.cancel_grace_ms = u64::try_from(grace.as_millis()).unwrap_or(u64::MAX);
        self
    }

    /// Adds the gate of the backend's endpoint: a model request takes it before it is sent and
    /// returns it when its answer is read (RFC 013 decision 7; external design §19.3).
    #[must_use]
    pub fn with_engine_gate(mut self, gate: Arc<dyn crate::spi::Admission>) -> Self {
        self.engine_gate = Some(gate);
        self
    }

    /// Adds the admission of executions across the user's runtimes: an execution takes a slot
    /// before its action is recorded `running` and returns it when the execution ends
    /// (RFC 013 decision 5; external design §19.3).
    #[must_use]
    pub fn with_admission(mut self, admission: Arc<dyn crate::spi::Admission>) -> Self {
        self.admission = Some(admission);
        self
    }

    /// Builds a runtime; the policy must validate (REQ-CFG-004).
    pub fn new(deps: RuntimeDeps) -> Result<Self, RuntimeError> {
        deps.policy
            .validate()
            .map_err(|e| RuntimeError::InvalidRequest {
                detail: e.to_string(),
            })?;
        let patterns: Vec<String> = deps
            .policy
            .exclusions
            .materialize
            .iter()
            .chain(deps.policy.exclusions.secrets.iter())
            .cloned()
            .collect();
        let exclusions = ExclusionMatcher::new(&patterns, crate::path::CaseMode::Sensitive)
            .map_err(|p| RuntimeError::InvalidRequest {
                detail: format!("exclusion pattern {p}"),
            })?;
        let policy_hash = deps.policy.hash();
        let redactor = crate::context::Redactor::new(&deps.secrets);
        Ok(Self {
            deps,
            policy_hash,
            exclusions,
            redactor,
            last_requests: std::sync::Mutex::new(std::collections::HashMap::new()),
            unpinned: std::sync::Mutex::new(std::collections::HashMap::new()),
            interruptions: std::sync::Mutex::new(std::collections::HashMap::new()),
            steps: steps::Steps::default(),
            requests: requests::InFlight::default(),
            admission: None,
            engine_gate: None,
            unadmitted: std::sync::Mutex::new(std::collections::HashSet::new()),
            backends: None,
            levels: crate::effort::Levels::shipped(),
            cancel_grace_ms: crate::limits::LIM_O2_CANCEL_MS,
        })
    }

    /// The token the task's model request and its actions that can be undone
    /// observe (REQ-SCHED-004): set by [`Runtime::interrupt_step`] and by
    /// [`Runtime::interrupt_step_anyway`]. A reader: it creates no entry, so a
    /// task that has none (one that is unknown, or ended) gets a token that
    /// nothing will set.
    pub fn step_interruption(&self, task_id: TaskId) -> Cancellation {
        self.interruption(task_id).undoable
    }

    /// How many tasks have an entry in the registry of interruptions (tests).
    #[cfg(test)]
    pub(crate) fn interruption_entries(&self) -> usize {
        self.interruptions
            .lock()
            .expect("interruption registry")
            .len()
    }

    /// The task's requests, as they are; nothing is created.
    fn interruption(&self, task_id: TaskId) -> Interruption {
        self.interruptions
            .lock()
            .expect("interruption registry")
            .get(&task_id)
            .cloned()
            .unwrap_or_default()
    }

    /// The entry of a task whose step begins, so that a request made while the step runs
    /// reaches the tokens the step holds. The step has just found the task `running`, so the
    /// task is known and not terminal; a request made earlier is kept.
    pub(crate) fn begin_interruption(&self, task_id: TaskId) {
        self.interruptions
            .lock()
            .expect("interruption registry")
            .entry(task_id)
            .or_default();
    }

    /// The entry a request is made on: of a task that is known and not terminal, and no other
    /// (REQ-SCHED-004; external design §4). A request for another id would leave an entry that
    /// nothing removes: `forget_task` runs when a task ends, and the id of a task that never was
    /// or was deleted would stay for the life of a long-lived process.
    ///
    /// The registry stays locked while the task is read, and `forget_task` takes the same lock:
    /// a request that read the task before it ended finishes its entry before the end removes
    /// it, and one that comes after reads the end and creates nothing.
    fn request_on(
        &self,
        task_id: TaskId,
        request: &'static str,
        set: impl FnOnce(&Interruption),
    ) -> Result<(), RuntimeError> {
        let mut registry = self.interruptions.lock().expect("interruption registry");
        let task =
            self.deps
                .persistence
                .load_task(task_id)?
                .ok_or_else(|| RuntimeError::NotFound {
                    what: format!(
                        "task {task_id}: it is not known, so there is no step to interrupt"
                    ),
                })?;
        if task.state.is_terminal() {
            return Err(RuntimeError::State(StateError::Illegal {
                from: task.state.name().into(),
                input: request.into(),
            }));
        }
        set(registry.entry(task_id).or_default());
        Ok(())
    }

    /// Whether the step in flight is asked to start no new action
    /// ([`Runtime::start_no_new_action`], and the two stronger requests that
    /// imply it).
    pub fn no_new_action_requested(&self, task_id: TaskId) -> bool {
        self.interruption(task_id).no_new_action.is_cancelled()
    }

    /// The token a wait made before anything is sent observes: at a step
    /// boundary (a listing of a server's tools) and inside an action's
    /// execution before its call (the pin check; `ExecutionContext::start_gate`).
    /// The request to start no new action, which the two stronger requests
    /// imply.
    pub(crate) fn boundary_interruption(&self, task_id: TaskId) -> Cancellation {
        self.interruption(task_id).no_new_action
    }

    /// The token an action's execution observes: an action that cannot be
    /// undone is not interrupted by [`Runtime::interrupt_step`], only by
    /// [`Runtime::interrupt_step_anyway`] (REQ-CLI-005). The one rule is
    /// `action::can_be_undone`, the rule of recovery read from the other side.
    pub(crate) fn execution_interruption(
        &self,
        task_id: TaskId,
        effect: crate::action::EffectView,
    ) -> Cancellation {
        let interruption = self.interruption(task_id);
        if crate::action::can_be_undone(effect) {
            interruption.undoable
        } else {
            interruption.anyway
        }
    }

    /// Asks the task's current step to start no new action, the weakest
    /// request and the first a stop signal makes. An action the step has not
    /// started yet is closed `cancelled` without starting (nothing ran, so the
    /// record is exact), the step ends `cancelled` and the caller pauses the
    /// task. Nothing in flight is interrupted: a model request and an action
    /// already running go on. The two stronger requests,
    /// [`Runtime::interrupt_step`] and [`Runtime::interrupt_step_anyway`],
    /// imply this one. Spent and cleared as they are.
    ///
    /// The request never blocks: it sets a token. An action's start is made
    /// safe against it by the step, which looks at the request, records the
    /// action `running`, and looks again (see `Runtime::execute`): a request
    /// the signal's look at what is in flight did not see was made before the
    /// action was recorded, so the second look finds it (handoff 010f-4; audit
    /// A-33).
    ///
    /// A task that is not known, or has ended, is refused, and nothing is
    /// created: the error says which (external design §4).
    pub(crate) fn start_no_new_action(&self, task_id: TaskId) -> Result<(), RuntimeError> {
        self.request_on(task_id, "start_no_new_action", |i| {
            i.no_new_action.cancel();
        })
    }

    /// Asks the task's current step to stop what can be undone, and to start
    /// no new action ([`Runtime::start_no_new_action`]): the model request, or
    /// an action whose rule after `unknown` is re-execution or rollback, ends
    /// `cancelled` and the step ends with outcome `cancelled`. It interrupts a
    /// step; it does not cancel the task, and the caller then pauses or cancels
    /// the task. An action that cannot be undone (an external write, a host
    /// write, an external read that is not idempotent) is not interrupted: it
    /// runs to its end, which its own deadline bounds
    /// ([`Runtime::irreversible_in_flight`] tells the caller to wait for it).
    /// A task with no step in flight is unaffected until its next step, which
    /// ends the same way.
    ///
    /// The request is spent by the step that ends `cancelled`: a task still
    /// `running` after it can be paused, and a pause requested then is taken at
    /// the next boundary (handoff 010d). It is also cleared at every task
    /// transition (pause, resume, halt, cancel, and the waits for the user), so
    /// that a request made while no step runs never reaches a step that began
    /// after a pause and a resume (handoff 010e). A request made while the step
    /// ends otherwise, with no transition, stays for the next step.
    ///
    /// A task that is not known, or has ended, is refused, and nothing is
    /// created: the error says which (external design §4).
    pub(crate) fn interrupt_step(&self, task_id: TaskId) -> Result<(), RuntimeError> {
        self.request_on(task_id, "interrupt_step", |i| {
            i.no_new_action.cancel();
            i.undoable.cancel();
        })
    }

    /// As [`Runtime::interrupt_step`], and also for an action that cannot be
    /// undone: it is interrupted, its outcome is recorded unknown (action
    /// status `unknown`) and the task halts `unknown_outcome` with the action
    /// listed, the rule of a crash (external design §16.3). Spent and cleared
    /// as [`Runtime::interrupt_step`] is, and refused for a task that is not
    /// known or has ended as it is.
    pub(crate) fn interrupt_step_anyway(&self, task_id: TaskId) -> Result<(), RuntimeError> {
        self.request_on(task_id, "interrupt_step_anyway", |i| {
            i.no_new_action.cancel();
            i.undoable.cancel();
            i.anyway.cancel();
        })
    }

    /// The action of the task now running that cannot be undone, if any: the
    /// one a stop signal waits for instead of interrupting (REQ-CLI-005).
    pub fn irreversible_in_flight(
        &self,
        task_id: TaskId,
    ) -> Result<Option<IrreversibleAction>, RuntimeError> {
        Ok(self
            .deps
            .persistence
            .load_running_action(task_id)?
            .filter(|action| !crate::action::can_be_undone(action.content().effect()))
            .map(|action| IrreversibleAction {
                action_id: action.action_id,
                tool: action.content().tool().to_owned(),
                effect: action.content().effect().kind,
            }))
    }

    /// Clears the task's requests: the task's next step gets fresh tokens (the
    /// execution that observed the old ones holds its own clone). Nothing is
    /// created for a task without any. Called by the step that ended
    /// `cancelled`, which spends the request, and at every task transition.
    pub(crate) fn clear_interruption(&self, task_id: TaskId) {
        if let Some(interruption) = self
            .interruptions
            .lock()
            .expect("interruption registry")
            .get_mut(&task_id)
        {
            // The tokens are fresh.
            *interruption = Interruption::default();
        }
    }

    /// Clears the task's requests when its state changed between `before` and
    /// `after`: a request never outlives a transition (handoff 010e).
    pub(crate) fn transitioned(&self, before: &crate::task::Task, after: &crate::task::Task) {
        if before.state != after.state {
            self.clear_interruption(after.task_id);
        }
    }

    /// A terminal task leaves nothing behind in the runtime (REQ-SCHED-005):
    /// its interruption requests, its previous request, its unpinned tools and what
    /// the providers count for it (`WorkspaceProvider::task_ended`) are dropped. Called where a task becomes terminal and where it is deleted;
    /// a daemon keeps the runtime alive, so an entry left here would be a leak
    /// for every finished task.
    pub(crate) fn forget_task(&self, task_id: TaskId) {
        self.interruptions
            .lock()
            .expect("interruption registry")
            .remove(&task_id);
        self.last_requests
            .lock()
            .expect("last requests lock")
            .remove(&task_id);
        self.unpinned
            .lock()
            .expect("unpinned lock")
            .remove(&task_id);
        self.unadmitted
            .lock()
            .expect("unadmitted lock")
            .remove(&task_id);
        // What the providers count for the task (the restarts of LIM-X3) ends with it.
        self.deps.workspaces.task_ended(task_id);
    }

    /// The policy in force.
    pub fn policy(&self) -> &Policy {
        &self.deps.policy
    }

    /// The policy's hash.
    pub fn policy_hash(&self) -> &Hash {
        &self.policy_hash
    }

    /// The store.
    pub fn persistence(&self) -> &dyn Persistence {
        self.deps.persistence.as_ref()
    }

    pub(crate) fn event(
        &self,
        actor: Actor,
        task_id: Option<TaskId>,
        step_id: Option<StepId>,
        action_id: Option<ActionId>,
        event: Event,
    ) -> EventRecord {
        EventRecord {
            event_id: EventId::new(self.deps.ids.as_ref()),
            at: self.now(),
            actor,
            task_id,
            step_id,
            action_id,
            policy_hash: self.policy_hash.clone(),
            request_id: requests::current(),
            event,
        }
    }

    /// Records `runtime.started` with the tier (`core` in M1).
    pub fn record_started(&self) -> Result<(), RuntimeError> {
        let backend = self.guard(MODEL_BACKEND, || self.deps.backend.describe())?;
        self.deps.persistence.append_event(
            &self.event(
                Actor::Runtime,
                None,
                None,
                None,
                Event::RuntimeStarted {
                    tier: self
                        .deps
                        .probe
                        .tier_with(&self.deps.policy.accept_best_effort)
                        .tier,
                    backend,
                },
            ),
        )?;
        Ok(())
    }

    /// Appends server events a provider produced outside a step (RFC 009 §1).
    pub fn record_server_events(&self, events: &[Event]) -> Result<(), RuntimeError> {
        for event in events {
            self.deps.persistence.append_event(&self.event(
                Actor::Runtime,
                None,
                None,
                None,
                event.clone(),
            ))?;
        }
        Ok(())
    }

    /// Records `runtime.stopped`.
    pub fn record_stopped(&self) -> Result<(), RuntimeError> {
        self.deps.persistence.append_event(&self.event(
            Actor::Runtime,
            None,
            None,
            None,
            Event::RuntimeStopped,
        ))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
