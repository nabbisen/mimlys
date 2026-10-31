//! `mimlys run` and `mimlys task new` (REQ-TASK-001, REQ-CLI-003).

use crate::app::{App, AppError};
use crate::commands::{finish, parse_duration_ms, print_view};
use crate::messages;
use crate::output::Output;
use crate::prompt::{Interaction, parse_grant};
use mimlys_core::runtime::CreateTask;
use mimlys_core::step::Budgets;
use mimlys_core::task::{CompletionCheck, ScopeSpec};
use std::path::Path;

/// What `run` and `task new` take.
#[derive(Clone, Debug, Default)]
pub struct NewTask {
    /// The goal.
    pub goal: String,
    /// The project directory.
    pub scope: std::path::PathBuf,
    /// Profile name.
    pub profile: Option<String>,
    /// Extra grants, in the `kind:scope` syntax.
    pub grants: Vec<String>,
    /// Step budget.
    pub steps: Option<u32>,
    /// Deadline as a duration.
    pub deadline: Option<String>,
    /// Completion check as JSON `{"tool": ..., "arguments": {...}}`.
    pub completion_check: Option<String>,
    /// Leave the task in `created`.
    pub hold: bool,
    /// Fork from this task (id or prefix).
    pub from_task: Option<String>,
    /// Fork from this snapshot (id or prefix).
    pub from_snapshot: Option<String>,
}

/// Builds the request (REQ-TASK-001) and creates the task.
pub fn create(app: &App, args: &NewTask) -> Result<mimlys_core::task::Task, AppError> {
    let from_task = match &args.from_task {
        Some(t) => Some(app.resolve_task(t)?),
        None => None,
    };
    let from_snapshot = match (&args.from_snapshot, from_task) {
        (Some(s), Some(source)) => {
            let view = app.runtime.get_task(source)?;
            let id = crate::commands::rollback::resolve_id(
                s,
                view.snapshots.iter().map(|s| s.snapshot_id.to_string()),
            )?;
            Some(mimlys_core::ids::SnapshotId::from_uuid(
                uuid::Uuid::parse_str(&id).expect("listed id"),
            ))
        }
        (Some(_), None) => {
            return Err(AppError::Io {
                detail: "--from-snapshot needs --from-task".into(),
            });
        }
        _ => None,
    };
    // A fork defaults to the source's scope; otherwise the current directory.
    let scope = match (from_task, args.scope.as_os_str().is_empty()) {
        (Some(source), true) => {
            std::path::PathBuf::from(app.runtime.get_task(source)?.task.scope.root)
        }
        (None, true) => std::path::PathBuf::from("."),
        _ => args.scope.clone(),
    };
    let root = std::fs::canonicalize(&scope).map_err(|_| AppError::Io {
        detail: format!("scope {} is not an existing directory", scope.display()),
    })?;
    let mut grants = Vec::new();
    for g in &args.grants {
        let kind = parse_grant(g).map_err(|e| AppError::Io {
            detail: e.to_owned(),
        })?;
        app.check_host_tool_grant(&kind)?;
        grants.push(kind);
    }
    let profile_name = args
        .profile
        .clone()
        .unwrap_or_else(|| app.runtime.policy().default_profile.clone());
    let budgets = match args.steps {
        Some(steps) => {
            let base = app
                .runtime
                .policy()
                .profile(&profile_name)
                .map(|p| p.budgets)
                .unwrap_or_default();
            Some(Budgets { steps, ..base })
        }
        None => None,
    };
    let deadline_ms = match &args.deadline {
        Some(d) => Some(parse_duration_ms(d)?),
        None => None,
    };
    let completion_check = match &args.completion_check {
        Some(text) => Some(serde_json::from_str::<CompletionCheck>(text).map_err(|e| AppError::Io {
            detail: format!("--completion-check must be JSON {{\"tool\": NAME, \"arguments\": {{...}}}}: {e}"),
        })?),
        None => None,
    };
    let task = app.runtime.create_task(
        &app.session,
        CreateTask {
            goal: args.goal.clone(),
            scope: ScopeSpec {
                root: path_text(&root),
                include: vec![],
            },
            profile: Some(profile_name),
            grants,
            budgets,
            deadline_ms,
            completion_check,
            hold: args.hold,
            from_task,
            from_snapshot,
        },
    )?;
    Ok(task)
}

/// `run`: create, start and follow.
pub async fn run(
    app: &App,
    args: &NewTask,
    interaction: &mut dyn Interaction,
    out: &Output,
) -> Result<(), AppError> {
    let task = create(app, args)?;
    out.line(messages::task_created(
        &task.task_id.to_string(),
        &task.state,
    ));
    if args.hold {
        print_view(&app.runtime.get_task(task.task_id)?, out);
        return Ok(());
    }
    finish(app, task.task_id, interaction, out).await
}

fn path_text(path: &Path) -> String {
    let text = path.to_string_lossy().into_owned();
    // Windows canonical paths carry a verbatim prefix that tools do not expect.
    text.strip_prefix(r"\\?\")
        .map(str::to_owned)
        .unwrap_or(text)
}
