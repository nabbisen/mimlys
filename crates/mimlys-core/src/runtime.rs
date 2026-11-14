//! The runtime: the step loop and the control API over the adapter SPI
//! (external design §4, §5, §6.1, §16.2; REQ-ARCH-004, REQ-DEC-001,
//! REQ-DEC-009, REQ-TASK-001 … 012, REQ-SCHED-002). Pure orchestration: no
//! I/O of its own, every transition written through `Persistence` in the
//! order of external design §16.2, the executor called only with an
//! `AuthorizedAction`, and the backend called only for the step decision.

mod contain;
mod context;
mod control;
mod loop_;
mod read;
mod storage;

pub use control::{CreateTask, RuntimeStatus, TaskView, UnknownAction};
pub use loop_::{PROMPT_LOG_TARGET, StepResultView};
pub use read::{Activity, Reader};
pub use storage::{Holdings, TaskHolding};

use crate::clock::Clock;
use crate::event::{Actor, Event, EventRecord};
use crate::hash::Hash;
use crate::ids::{ActionId, EventId, IdSource, StepId, TaskId};
use crate::path::PlatformRules;
use crate::policy::{ExclusionMatcher, Policy};
use crate::spi::{
    BackendError, Cancellation, ModelBackend, Persistence, PersistenceError, WorkspaceError,
    WorkspaceProvider,
};
use crate::task::StateError;
use std::sync::Arc;

/// The session the control API acts under (v1: the CLI process).
#[derive(Clone, Debug, PartialEq, Eq)]
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
}

/// A runtime failure reported to the control API (external design §18).
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
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
    /// The backend failed.
    #[error("{0}")]
    Backend(#[from] BackendError),
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
            RuntimeError::Unavailable { .. } | RuntimeError::Backend(_) => "unavailable",
            RuntimeError::Workspace(WorkspaceError::Conflict { .. }) => "conflict",
            RuntimeError::Workspace(WorkspaceError::NotFound { .. }) => "not_found",
            RuntimeError::Workspace(WorkspaceError::Refused { .. }) => "state",
            RuntimeError::Workspace(
                WorkspaceError::TooLarge { .. }
                | WorkspaceError::ScopeNotFound
                | WorkspaceError::InsideDataDir
                | WorkspaceError::ContainsDataDir,
            ) => "invalid_request",
            RuntimeError::Persistence(_) | RuntimeError::Workspace(_) => "internal",
        }
    }
}

/// What has been asked of the step in flight (REQ-CLI-005, external design
/// §19.2). Two requests, the second stronger: the first interrupts what can be
/// undone (a model request; an action whose rule after `unknown` is not to halt
/// `unknown_outcome`), the second interrupts anything, an action that cannot be
/// undone included, whose outcome is then unknown. The second implies the first.
#[derive(Clone, Debug, Default)]
struct Interruption {
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
}

/// The runtime.
pub struct Runtime {
    deps: RuntimeDeps,
    policy_hash: Hash,
    exclusions: ExclusionMatcher,
    /// The interruption requests of each task with work in flight (REQ-SCHED-004).
    cancellations: std::sync::Mutex<std::collections::HashMap<TaskId, Interruption>>,
    /// The redactor built from the configured secrets.
    redactor: crate::context::Redactor,
    /// Each task's previous request in this process, for the prefix measure (REQ-CTX-010).
    last_requests: std::sync::Mutex<std::collections::HashMap<TaskId, Vec<crate::spi::Message>>>,
    /// External tools unpinned per task in this process (RFC 009 §7).
    unpinned:
        std::sync::Mutex<std::collections::HashMap<TaskId, std::collections::BTreeSet<String>>>,
}

impl Runtime {
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
            cancellations: std::sync::Mutex::new(std::collections::HashMap::new()),
        })
    }

    /// The token the task's model request and its actions that can be undone
    /// observe (REQ-SCHED-004): set by [`Runtime::request_cancel`] and by
    /// [`Runtime::request_cancel_anyway`].
    pub fn cancellation(&self, task_id: TaskId) -> Cancellation {
        self.interruption(task_id).undoable
    }

    fn interruption(&self, task_id: TaskId) -> Interruption {
        self.cancellations
            .lock()
            .expect("cancellation registry")
            .entry(task_id)
            .or_default()
            .clone()
    }

    /// The token an action's execution observes: an action that cannot be
    /// undone is not interrupted by [`Runtime::request_cancel`], only by
    /// [`Runtime::request_cancel_anyway`] (REQ-CLI-005). The one rule is
    /// `action::after_unknown`, the rule of recovery.
    pub(crate) fn execution_cancellation(
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

    /// Asks the task's current step to stop what can be undone: the model
    /// request, or an action whose rule after `unknown` is not to halt
    /// `unknown_outcome`, ends `cancelled` and the step ends with outcome
    /// `cancelled`; the caller then cancels or pauses the task. An action that
    /// cannot be undone (an external write, a host write without a backup, an
    /// external read that is not idempotent) is not interrupted: it runs to its
    /// end, which its own deadline bounds ([`Runtime::irreversible_in_flight`]
    /// tells the caller to wait for it). A task with no step in flight is
    /// unaffected until its next step, which ends the same way. The request is
    /// consumed by the step that ends `cancelled` (`run_step`): a task still
    /// `running` after it can be paused, and a pause requested then is taken at
    /// the next boundary; a request made again is a new request. A request made
    /// while the step ends otherwise stays for the next step.
    pub fn request_cancel(&self, task_id: TaskId) {
        self.interruption(task_id).undoable.cancel();
    }

    /// As [`Runtime::request_cancel`], and also for an action that cannot be
    /// undone: it is interrupted, its outcome is recorded unknown (action
    /// status `unknown`) and the task halts `unknown_outcome` with the action
    /// listed, the rule of a crash (external design §16.3).
    pub fn request_cancel_anyway(&self, task_id: TaskId) {
        let interruption = self.interruption(task_id);
        interruption.undoable.cancel();
        interruption.anyway.cancel();
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
            }))
    }

    /// Takes the requests a `cancelled` step has acted on: the task's next
    /// step gets fresh tokens (the execution that observed the old ones holds
    /// its own clone). Nothing is created for a task without any.
    fn consume_cancellation(&self, task_id: TaskId) {
        if let Some(interruption) = self
            .cancellations
            .lock()
            .expect("cancellation registry")
            .get_mut(&task_id)
        {
            *interruption = Interruption::default();
        }
    }

    /// A terminal task leaves nothing behind in the runtime (REQ-SCHED-005):
    /// its cancellation token, its previous request and its unpinned tools are
    /// dropped. Called where a task becomes terminal and where it is deleted;
    /// a daemon keeps the runtime alive, so an entry left here would be a leak
    /// for every finished task.
    pub(crate) fn forget_task(&self, task_id: TaskId) {
        self.cancellations
            .lock()
            .expect("cancellation registry")
            .remove(&task_id);
        self.last_requests
            .lock()
            .expect("last requests lock")
            .remove(&task_id);
        self.unpinned
            .lock()
            .expect("unpinned lock")
            .remove(&task_id);
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
            event,
        }
    }

    /// Records `runtime.started` with the tier (`core` in M1).
    pub fn record_started(&self) -> Result<(), RuntimeError> {
        let backend = self.guard("model_backend", || self.deps.backend.describe())?;
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
