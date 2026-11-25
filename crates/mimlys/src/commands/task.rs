//! `mimlys task show|list|resume|pause|cancel` (REQ-TASK-005, REQ-TASK-006).

use crate::app::{App, AppError, ReadApp};
use crate::commands::{finish, parse_duration_ms, print_view};
use crate::messages;
use crate::output::Output;
use crate::prompt::Interaction;
use mimlys_core::clock::Clock;
use mimlys_core::runtime::TaskShown;
use mimlys_core::task::Extension;

/// `task show`.
///
/// With `arguments`, the action waiting for approval is followed by every one
/// of its arguments in full, sanitized (REQ-APR-002); the JSON always carries
/// them, in `pending_approval.arguments`.
pub fn show(app: &ReadApp, task: &str, arguments: bool, out: &Output) -> Result<(), AppError> {
    let id = app.resolve_task(task)?;
    // A record that a newer release wrote is shown as written and marked, never a failure
    // (REQ-MAIN-006): the view of every record that reads is what it always was.
    let (mut view, newer) = match app.reader.show_task(id)? {
        TaskShown::Whole { view, newer } => (*view, newer),
        TaskShown::Newer {
            task_id,
            record,
            others,
        } => {
            for line in messages::show_newer_task_lines(&task_id.to_string(), &record, &others) {
                out.line(line);
            }
            let mut value = serde_json::json!({ "task": record.to_json() });
            if !others.is_empty() {
                value["unknown_records"] = unknown_records_json(&others);
            }
            out.value(&value);
            return Ok(());
        }
    };
    // A `running` task whose execution waits for a slot among the user's runtimes says why
    // (external design §19.3); the registry knows, the store does not.
    if view.task.state == mimlys_core::task::TaskState::Running {
        view.waiting = crate::machine::waiting_of(
            crate::registry::default_dir(),
            &app.data_dir.to_string_lossy(),
            &id.to_string(),
        );
    }
    // The backend's `context_chars` as the configuration has it now: the task's budget follows it
    // (external design §19.4).
    let backend_context = app
        .loaded
        .config
        .backends
        .get(&view.effort.envelope.backend)
        .map(|b| b.context_chars);
    for line in messages::show_lines(&view, backend_context) {
        out.line(line);
    }
    for record in &newer {
        out.line(messages::newer_record_line(
            record.kind.name(),
            &record.key,
            &record.newer.words,
        ));
    }
    if let Some(approval) = &view.pending_approval {
        out.line("");
        out.line(crate::prompt::approval_text(approval, None).trim_end_matches('\n'));
        if arguments {
            out.line("");
            out.line(crate::prompt::arguments_in_full(approval));
        }
    } else if arguments {
        out.line("");
        out.line(messages::NO_ARGUMENTS_TO_SHOW);
    }
    if let Some(question) = &view.pending_question {
        out.line("");
        out.line(crate::prompt::question_text(question).trim_end_matches('\n'));
    }
    if newer.is_empty() {
        out.value(&view);
    } else {
        let mut value = serde_json::to_value(&view).unwrap_or_default();
        value["unknown_records"] = unknown_records_json(&newer);
        out.value(&value);
    }
    Ok(())
}

/// The records of a task that hold a word this release does not know, for `--json`: each as it
/// was written, with the words listed in it (REQ-MAIN-006).
fn unknown_records_json(records: &[mimlys_core::newer::NewerRecord]) -> serde_json::Value {
    serde_json::Value::Array(
        records
            .iter()
            .map(|r| {
                serde_json::json!({
                    "kind": r.kind.name(),
                    "key": r.key,
                    "record": r.newer.to_json(),
                })
            })
            .collect(),
    )
}

/// `task list`.
pub fn list(app: &ReadApp, out: &Output) -> Result<(), AppError> {
    let tasks = app.reader.list_tasks()?;
    if !tasks.is_empty() {
        out.line(messages::LIST_HEADER);
    }
    let mut rows = Vec::new();
    for t in &tasks {
        // A snapshot or a promotion of a newer release is left out of the footprint, not a
        // failure of the listing (REQ-MAIN-006).
        let (snapshots, promotions, _) =
            mimlys_core::runtime::readable_footprint_records(app.reader.persistence(), t.task_id)?;
        let footprint = app.reader.footprint(t.task_id, &snapshots, &promotions)?;
        let mut row = serde_json::json!({
            "task_id": t.task_id, "goal": t.goal, "state": t.state, "created_at": t.created_at, "updated_at": t.updated_at,
            "footprint": footprint,
        });
        mimlys_core::newer::mark(&mut row, &t.unknown);
        rows.push(row);
        // The state, marked when it is the word that is not known; any other word of the record
        // that is not known follows the state.
        let state_unknown = t.unknown.iter().any(|w| w.field == "state");
        let others: Vec<String> = t
            .unknown
            .iter()
            .filter(|w| w.field != "state")
            .map(|w| format!("{} `{}` {}", w.field, w.word, messages::UNKNOWN_MARKER))
            .collect();
        let state = messages::marked(&t.state, state_unknown);
        out.line(format!(
            "{}  {:<20}  {}  {:>10} B  {}{}",
            t.task_id,
            state,
            t.updated_at.to_rfc3339(),
            footprint.workspace_bytes + footprint.object_bytes_exclusive + footprint.blob_bytes,
            serde_json::to_string(&t.goal).unwrap_or_default(),
            if others.is_empty() {
                String::new()
            } else {
                format!("  [{}]", others.join(", "))
            }
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
