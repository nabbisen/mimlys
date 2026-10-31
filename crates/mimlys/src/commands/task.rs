//! `mimlys task show|list|resume|pause|cancel` (REQ-TASK-005, REQ-TASK-006).

use crate::app::{App, AppError};
use crate::commands::{finish, parse_duration_ms, print_view};
use crate::messages;
use crate::output::Output;
use crate::prompt::Interaction;
use mimlys_core::clock::Clock;
use mimlys_core::task::Extension;

/// `task show`.
pub fn show(app: &App, task: &str, out: &Output) -> Result<(), AppError> {
    let id = app.resolve_task(task)?;
    let view = app.runtime.get_task(id)?;
    for line in messages::show_lines(&view) {
        out.line(line);
    }
    if let Some(approval) = &view.pending_approval {
        out.line("");
        out.line(crate::prompt::approval_text(approval, None).trim_end_matches('\n'));
    }
    if let Some(question) = &view.pending_question {
        out.line("");
        out.line(crate::prompt::question_text(question).trim_end_matches('\n'));
    }
    out.value(&view);
    Ok(())
}

/// `task list`.
pub fn list(app: &App, out: &Output) -> Result<(), AppError> {
    let tasks = app.runtime.list_tasks()?;
    if !tasks.is_empty() {
        out.line(messages::LIST_HEADER);
    }
    let mut rows = Vec::new();
    for t in &tasks {
        let snapshots = app.runtime.persistence().load_snapshots(t.task_id)?;
        let promotions = app.runtime.persistence().load_promotions(t.task_id)?;
        let footprint = app.runtime.footprint(t.task_id, &snapshots, &promotions)?;
        rows.push(serde_json::json!({
            "task_id": t.task_id, "goal": t.goal, "state": t.state, "created_at": t.created_at, "updated_at": t.updated_at,
            "footprint": footprint,
        }));
        out.line(format!(
            "{}  {:<20}  {}  {:>10} B  {}",
            t.task_id,
            t.state,
            t.updated_at.to_rfc3339(),
            footprint.workspace_bytes + footprint.object_bytes_exclusive + footprint.blob_bytes,
            serde_json::to_string(&t.goal).unwrap_or_default()
        ));
    }
    if tasks.is_empty() {
        out.line(messages::LIST_EMPTY);
    }
    out.value(&rows);
    Ok(())
}

/// `task resume`, with optional extensions from `halted`.
pub async fn resume(
    app: &App,
    task: &str,
    extend_steps: Option<u32>,
    extend_deadline: Option<&str>,
    interaction: &mut dyn Interaction,
    out: &Output,
) -> Result<(), AppError> {
    let id = app.resolve_task(task)?;
    let extend = if extend_steps.is_some() || extend_deadline.is_some() {
        let deadline = match extend_deadline {
            Some(d) => Some(
                mimlys_core::clock::SystemClock
                    .now()
                    .plus_millis(parse_duration_ms(d)?)
                    .map_err(|e| AppError::Io {
                        detail: e.to_string(),
                    })?,
            ),
            None => None,
        };
        Some(Extension {
            steps: extend_steps,
            deadline,
        })
    } else {
        None
    };
    app.runtime.resume_task(&app.session, id, extend)?;
    finish(app, id, interaction, out).await
}

/// `task pause`: only meaningful for a task this process runs; from
/// another process the task is never `running` (the data directory is locked).
pub fn pause(app: &App, task: &str, out: &Output) -> Result<(), AppError> {
    let id = app.resolve_task(task)?;
    app.runtime.pause_task(&app.session, id)?;
    print_view(&app.runtime.get_task(id)?, out);
    Ok(())
}

/// `task delete` (RFC 006 §6): the plan is shown, the user confirms, rows first then files.
pub fn delete(
    app: &App,
    task: &str,
    audit: bool,
    yes: bool,
    interaction: &mut dyn Interaction,
    out: &Output,
) -> Result<(), AppError> {
    let id = app.resolve_task(task)?;
    let view = app.runtime.get_task(id)?;
    if !view.task.state.is_terminal() {
        return Err(AppError::Runtime(
            mimlys_core::runtime::RuntimeError::State(mimlys_core::task::StateError::Illegal {
                from: view.task.state.name(),
                input: "delete_task",
            }),
        ));
    }
    for line in messages::delete_plan(&id.to_string(), &view.footprint, audit) {
        out.line(line);
    }
    let confirmed = yes
        || (interaction.interactive()
            && !out.json()
            && interaction.confirm(messages::DELETE_CONFIRM));
    if !confirmed {
        out.line(messages::DELETE_NOT_CONFIRMED);
        out.value(&serde_json::json!({ "footprint": view.footprint, "deleted": false }));
        return Ok(());
    }
    let footprint = app.runtime.delete_task(&app.session, id, audit)?;
    out.line(messages::deleted(&id.to_string()));
    out.value(&serde_json::json!({ "footprint": footprint, "deleted": true, "audit": audit }));
    Ok(())
}

/// `task cancel`.
pub fn cancel(app: &App, task: &str, out: &Output) -> Result<(), AppError> {
    let id = app.resolve_task(task)?;
    app.runtime.cancel_task(&app.session, id)?;
    print_view(&app.runtime.get_task(id)?, out);
    Ok(())
}
