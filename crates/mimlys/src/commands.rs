//! The commands of external design §19.2 (REQ-CLI-001). Each command is one
//! function over the `App`; state-changing commands end in follow mode when
//! the task is `running`, because in deployment v1 steps run only inside
//! the process that drives them.

pub mod answer;
pub mod approve;
pub mod check;
pub mod events;
pub mod grant;
pub mod interpreters;
pub mod metrics;
pub mod model;
pub mod promote;
pub mod prune;
pub mod reject;
pub mod revoke;
pub mod rollback;
pub mod run;
pub mod runtime;
pub mod status;
pub mod storage;
pub mod task;
pub mod task_output;
pub mod toolchain;

use crate::app::{App, AppError};
use crate::follow;
use crate::messages;
use crate::output::Output;
use crate::prompt::Interaction;
use mimlys_core::ids::TaskId;
use mimlys_core::runtime::TaskView;
use mimlys_core::task::{HaltDetail, TaskState};

/// Follows the task when it is running, then prints the view.
pub async fn finish(
    app: &App,
    task_id: TaskId,
    interaction: &mut dyn Interaction,
    out: &Output,
) -> Result<(), AppError> {
    let state = app.reader.get_task(task_id)?.task.state;
    // A task that waits in the daemon's line is followed too (external design §19.5).
    let in_line = app.client.queued(task_id).is_some();
    let follows = matches!(state, TaskState::Running | TaskState::WaitingUser { .. }) || in_line;
    let view = if follows && !app.detach {
        follow::drive(app, task_id, interaction, out).await?
    } else {
        app.reader.get_task(task_id)?
    };
    print_view(&view, out);
    // `--detach`: the daemon goes on with the task, and the line says how to follow or stop it.
    if follows && app.detach {
        out.line(messages::detached_from(&task_id.to_string()));
    }
    // A task halted `policy` is told why, by the rule that stopped it (`halt_detail`, from the
    // task's own record: not asked again of what is in force now), REQ-SEC-010.
    if let TaskState::Halted {
        halt_detail: Some(detail),
        ..
    } = &view.task.state
    {
        out.line(match detail {
            HaltDetail::RemoteBackendForbidden { backend, profile } => {
                messages::start_refused(backend, profile)
            }
            HaltDetail::CompletionCheckRefused => messages::completion_check_refused(),
            HaltDetail::BackendNotConfigured {
                backend,
                configured,
            } => messages::backend_not_configured_halt(
                &view.task.task_id.to_string(),
                backend,
                configured,
            ),
            HaltDetail::BackendUnusable { backend, reason } => {
                messages::backend_unusable_halt(&view.task.task_id.to_string(), backend, reason)
            }
        });
    }
    Ok(())
}

/// A request that took effect before (a repeat after a broken connection found it recorded) and
/// whose task has been deleted since, so that what it made (a grant, a snapshot, a promotion) went
/// with the task's data (external design §4, "The recorded outcome"; handoff 014c, item 7). It is
/// a success, not an error: the client has said on standard error that it was done earlier and
/// that the task is gone, and the command says the same on standard output, in one line, and as
/// one JSON object, so that a script does not read an empty answer.
pub fn applied_earlier_gone(out: &Output, verb: &str, task_id: TaskId) {
    out.line(messages::applied_earlier_gone(verb, &task_id.to_string()));
    out.value(&serde_json::json!({
        "applied": true,
        "gone": true,
        "request": verb,
        "task_id": task_id,
    }));
}

/// The task view: lines for humans, the object for JSON.
pub fn print_view(view: &TaskView, out: &Output) {
    out.line(messages::task_state_after(
        &view.task.task_id.to_string(),
        &view.task.state,
        view.rollback_failed,
    ));
    if let Some(path) = &view.unreadable {
        out.line(messages::unreadable_line(
            path,
            view.unreadable_kind.as_deref(),
        ));
    }
    for a in &view.unknown_actions {
        out.line(messages::unknown_action_line(
            &a.action_id.to_string(),
            &a.tool,
            a.effect,
        ));
    }
    out.value(view);
}

/// Parses `30s`, `15m`, `2h` or `1d` into milliseconds.
pub fn parse_duration_ms(text: &str) -> Result<i64, AppError> {
    crate::config::parse_duration_ms(text).map_err(|detail| AppError::Io { detail })
}

#[cfg(test)]
mod tests;
