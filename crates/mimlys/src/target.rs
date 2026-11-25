//! The backend the task of a command runs on (REQ-TASK-013, REQ-CFG-007; RFC 015, Amendment 1).
//!
//! A command that creates a task or sets one running must know which backend that is **before**
//! it contacts anything: to say that the profile forbids it (REQ-SEC-010), to probe it and to
//! warn about its context, to refuse a level whose backend does not answer, and to say that its
//! model has changed. The backend is the one the task's envelope names, found by that name; for
//! a task not yet made it is the level's, as the creation will resolve it (`Runtime::plan_effort`).
//! No other backend is asked: a command opens a connection to the backend of its task and to no
//! other.

use crate::app::App;
use mimlys_core::effort::EffortLevel;
use mimlys_core::task::Task;

/// What a command that creates or starts a task is about to run on.
#[derive(Clone, Debug)]
pub struct Target {
    /// The backend's name.
    pub backend: String,
    /// The task a command starts, as it is now; `None` for a command that creates one.
    pub task: Option<Task>,
    /// What the creation chose, for a command that creates a task.
    pub creation: Option<Creation>,
}

/// The level a creation resolved to, and whether the user chose it.
#[derive(Clone, Copy, Debug)]
pub struct Creation {
    /// The level.
    pub level: EffortLevel,
    /// `--effort` was given.
    pub flag_given: bool,
}

impl Target {
    /// The target of `run` and `task new`, from what they were given. `None` when the creation
    /// is refused already (a level above `effort_max`, one with no key, an unknown profile): the
    /// command says why itself, when it asks the runtime to create the task.
    pub fn for_new(
        app: &App,
        profile: Option<&str>,
        effort: Option<EffortLevel>,
        from_task: Option<&str>,
    ) -> Option<Self> {
        let policy = app.runtime.policy();
        let profile = policy.profile(profile.unwrap_or(&policy.default_profile))?;
        let from = from_task.and_then(|t| app.resolve_task(t).ok());
        let plan = app.runtime.plan_effort(profile, effort, from).ok()?;
        Some(Self {
            backend: plan.backend,
            task: None,
            creation: Some(Creation {
                level: plan.level,
                flag_given: effort.is_some(),
            }),
        })
    }

    /// The target of a command that sets the task named `task` running. `None` when the task is
    /// not there: the command says so itself.
    pub fn for_task(app: &App, task: &str) -> Option<Self> {
        let id = app.resolve_task(task).ok()?;
        let task = app.runtime.get_task(id).ok()?.task;
        Some(Self {
            backend: task.effort_in_force().envelope.backend,
            task: Some(task),
            creation: None,
        })
    }

    /// Whether the creation must be refused when the backend does not answer: a level the user
    /// chose with `--effort`, or one that runs on a backend of its own (REQ-TASK-013). A task
    /// that runs on `default` without a flag is created as it always was, and halts
    /// `backend_unavailable` at its first request.
    pub fn refuses_unreachable(&self) -> bool {
        self.creation
            .is_some_and(|c| c.flag_given || self.backend != crate::backends::DEFAULT)
    }
}
