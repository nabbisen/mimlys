//! One probe of one backend, from the scope it makes to the scope it removes (REQ-MDL-011).

use super::classify::{Verdict, classify};
use super::drive::{Driven, Measures, Stop, drive, file_is_as_asked, observe};
use super::{FILE, FILE_START, GOAL, PROFILE, SCOPE_PREFIX, WALL_MS};
use crate::app::{App, AppError};
use crate::backends::Backend;
use crate::probe_results::LastProbe;
use crate::workspace::LocalWorkspaces;
use mimlys_core::clock::Clock;
use mimlys_core::ids::{IdSource, SystemIds, TaskId};
use mimlys_core::path::PlatformRules;
use mimlys_core::runtime::{CreateTask, Runtime, RuntimeDeps};
use mimlys_core::spi::WorkspaceProvider;
use mimlys_core::task::ScopeSpec;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// How one probe ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ended {
    /// A class was found: the record to write, and what could not be removed afterwards (the next
    /// start that writes removes it).
    Measured {
        /// What the probe found.
        last: LastProbe,
        /// Paths the removal could not remove.
        left: Vec<String>,
    },
    /// Nothing was measured of the model, and nothing is recorded: why.
    NotMeasured {
        /// What happened.
        reason: String,
        /// Paths the removal could not remove.
        left: Vec<String>,
    },
    /// A stop was requested: what was made is deleted, nothing is recorded.
    Stopped {
        /// Paths the removal could not remove.
        left: Vec<String>,
    },
}

/// The probe's scope: the directory `<data_dir>/tmp/probe-<id>/` the runtime makes, with the one
/// file. Removed when the probe ends and, if the process leaves some other way, when this value
/// is dropped (a crash that no drop sees is the next start's: [`super::leftovers`]).
struct Scope {
    dir: PathBuf,
    data_dir: PathBuf,
    removed: bool,
}

impl Scope {
    /// Makes the directory (owner-only) and the file.
    fn make(data_dir: &Path, ids: &dyn IdSource) -> Result<Self, AppError> {
        let name = format!("{SCOPE_PREFIX}{}", ids.next().hyphenated());
        let dir = data_dir.join("tmp").join(name);
        let io = |what: &str, e: std::io::Error| AppError::Io {
            detail: format!("cannot make the probe's scope, {what}: {}", e.kind()),
        };
        mimlys_exec::confine::create_private_subdir(&dir).map_err(|e| io("the directory", e))?;
        let scope = Self {
            dir,
            data_dir: data_dir.to_owned(),
            removed: false,
        };
        std::fs::write(scope.dir.join(FILE), FILE_START).map_err(|e| io("its file", e))?;
        Ok(scope)
    }

    /// The directory as the task's scope names it: absolute, canonical, no verbatim prefix.
    fn root(&self) -> Result<String, AppError> {
        let root = std::fs::canonicalize(&self.dir).map_err(|e| AppError::Io {
            detail: format!("cannot resolve the probe's scope: {}", e.kind()),
        })?;
        let text = root.to_string_lossy().into_owned();
        Ok(text
            .strip_prefix(r"\\?\")
            .map(str::to_owned)
            .unwrap_or(text))
    }

    /// Removes the directory; the paths that stayed.
    fn remove(&mut self) -> Vec<String> {
        self.removed = true;
        crate::workspace::remove_made_dir(&self.dir, &self.data_dir)
    }
}

impl Drop for Scope {
    fn drop(&mut self) {
        if !self.removed {
            let _ = self.remove();
        }
    }
}

/// Probes `backend` once and deletes everything the probe made (REQ-MDL-011). The class is the
/// runtime's: [`classify`] over what the store and the workspace hold.
///
/// An error is a probe that could not be run (the scope could not be made, the store refused):
/// nothing is recorded, and what was made is removed.
///
/// `remote_named` is true for the one backend the user named, that is remote, and that the
/// administrator allowed ([`super::Plan::remote_named`]), and for no other: it is the only thing
/// that sets `external` in the probe's profile, so a remote backend that reached here without it
/// (which the plan never lets) is refused by REQ-SEC-010 when the task is created, and nothing is
/// sent.
pub async fn probe_backend(
    app: &App,
    backend: &Backend,
    remote_named: bool,
    stop: &Arc<Stop>,
    clock: Arc<dyn Clock>,
) -> Result<Ended, AppError> {
    let config = app.config();
    let ids = SystemIds;
    // The one place the probe's profile departs from granting nothing: a remote backend, named
    // for the probe and allowed (`super::plan` decided), is permitted by `external`, so that
    // REQ-SEC-010's rule, asked again at every start of the task, lets it send its fixed request.
    let external = remote_named;
    let mut scope = Scope::make(&app.data_dir, &ids)?;
    let policy = super::policy(external);
    let patterns: Vec<String> = policy
        .exclusions
        .materialize
        .iter()
        .chain(policy.exclusions.secrets.iter())
        .cloned()
        .collect();
    // A provider of its own, over the same data directory: the built-in tools only (no worker, no
    // server), and the one scope inside the data directory that this probe made.
    let workspaces = Arc::new(
        LocalWorkspaces::new(
            app.data_dir.clone(),
            patterns,
            config.workspace.max_bytes,
            config.workspace.max_files,
        )?
        .admitting_probe_scope(scope.dir.clone()),
    );
    let gate = backend.gate(app.registry_dir.clone(), app.owner.clone());
    // No admission: the tools of the probe take no slot (REQ-SCHED-006 is for executions of the
    // worker's and of host tools); its model requests take the engine's gate.
    let runtime = Runtime::new(RuntimeDeps {
        persistence: Arc::clone(&app.store),
        backend: Arc::clone(&backend.adapter),
        workspaces: Arc::clone(&workspaces) as Arc<dyn WorkspaceProvider>,
        policy,
        ids: Arc::new(SystemIds),
        clock: Arc::clone(&clock),
        runtime_version: env!("CARGO_PKG_VERSION").to_owned(),
        rules: PlatformRules::current(),
        probe: app.confinement.clone(),
        secrets: app.loaded.secrets.clone(),
        context_chars: backend.section.context_chars,
        output_bytes: config.sandbox.output_bytes as usize,
    })?;
    let runtime = Arc::new(match gate {
        Some(gate) => runtime.with_engine_gate(gate),
        None => runtime,
    });
    let mut left = Vec::new();
    let mut task_id: Option<TaskId> = None;
    let measured = measure(
        app,
        backend,
        &runtime,
        &workspaces,
        &scope,
        stop,
        &clock,
        &mut task_id,
    )
    .await;
    // Whatever the class, and whatever went wrong: the task with every record of it, its workspace,
    // and then the scope (in that order: a scope that is there says a task may be, which is how
    // the next start finds what a crash left).
    if let Some(task_id) = task_id {
        left.extend(super::leftovers::discard_task(
            &*app.store,
            &*workspaces,
            task_id,
            clock.as_ref(),
        ));
    }
    left.extend(scope.remove());
    match measured? {
        Measuring::Stopped => Ok(Ended::Stopped { left }),
        Measuring::NotMeasured(reason) => Ok(Ended::NotMeasured { reason, left }),
        Measuring::Found(last) => Ok(Ended::Measured { last, left }),
    }
}

/// What `measure` found.
enum Measuring {
    Found(LastProbe),
    NotMeasured(String),
    Stopped,
}

#[allow(clippy::too_many_arguments)]
async fn measure(
    app: &App,
    backend: &Backend,
    runtime: &Arc<Runtime>,
    workspaces: &Arc<LocalWorkspaces>,
    scope: &Scope,
    stop: &Arc<Stop>,
    clock: &Arc<dyn Clock>,
    task_id: &mut Option<TaskId>,
) -> Result<Measuring, AppError> {
    if stop.requested() {
        return Ok(Measuring::Stopped);
    }
    let task = runtime.create_task(
        &app.session,
        CreateTask {
            goal: GOAL.to_owned(),
            scope: ScopeSpec {
                root: scope.root()?,
                include: Vec::new(),
            },
            profile: Some(PROFILE.to_owned()),
            grants: Vec::new(),
            budgets: None,
            deadline_ms: Some(WALL_MS),
            completion_check: None,
            hold: false,
            from_task: None,
            from_snapshot: None,
        },
    )?;
    *task_id = Some(task.task_id);
    if drive(runtime, task.task_id, stop).await? == Driven::Stopped {
        return Ok(Measuring::Stopped);
    }
    let tree = PathBuf::from(workspaces.open(task.task_id)?.root);
    let (
        observed,
        Measures {
            steps,
            model_ms_per_step,
            transport,
        },
    ) = observe(runtime, task.task_id, || file_is_as_asked(&tree))?;
    let (class, detail) = match classify(&observed) {
        Verdict::Class(class, detail) => (class, detail),
        Verdict::NotMeasured => {
            return Ok(Measuring::NotMeasured(
                crate::messages::probe_not_measured_gate(&backend.name),
            ));
        }
        Verdict::Fault => {
            return Err(AppError::Io {
                detail: crate::messages::probe_fault(&backend.name, &format!("{:?}", observed.end)),
            });
        }
    };
    Ok(Measuring::Found(LastProbe {
        class: class.as_str().to_owned(),
        detail: detail.map(|d| d.as_str().to_owned()),
        steps,
        model_ms_per_step,
        transport: transport.map(|t| t.as_str().to_owned()),
        model: backend.adapter.describe().model,
        at: clock.now().to_rfc3339(),
    }))
}
