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

pub use control::{CreateTask, RuntimeStatus, TaskView};
pub use loop_::StepResultView;

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

/// The runtime.
pub struct Runtime {
    deps: RuntimeDeps,
    policy_hash: Hash,
    exclusions: ExclusionMatcher,
    /// One cancellation token per task with work in flight (REQ-SCHED-004).
    cancellations: std::sync::Mutex<std::collections::HashMap<TaskId, Cancellation>>,
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

    /// The task's cancellation token: the model request and the execution
    /// of its current step observe it (REQ-SCHED-004).
    pub fn cancellation(&self, task_id: TaskId) -> Cancellation {
        self.cancellations
            .lock()
            .expect("cancellation registry")
            .entry(task_id)
            .or_default()
            .clone()
    }

    /// Asks the task's current step to stop: the model request or the
    /// execution in flight ends `cancelled` and the step ends with outcome
    /// `cancelled`; the caller then cancels or pauses the task. A task with
    /// no step in flight is unaffected until its next step, which ends the
    /// same way.
    pub fn request_cancel(&self, task_id: TaskId) {
        self.cancellation(task_id).cancel();
    }

    fn clear_cancellation(&self, task_id: TaskId) {
        self.cancellations
            .lock()
            .expect("cancellation registry")
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
