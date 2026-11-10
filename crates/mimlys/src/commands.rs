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
pub mod promote;
pub mod prune;
pub mod reject;
pub mod revoke;
pub mod rollback;
pub mod run;
pub mod status;
pub mod storage;
pub mod task;
pub mod toolchain;

use crate::app::{App, AppError};
use crate::follow;
use crate::messages;
use crate::output::Output;
use crate::prompt::Interaction;
use mimlys_core::ids::TaskId;
use mimlys_core::runtime::TaskView;
use mimlys_core::task::TaskState;

/// Follows the task when it is running, then prints the view.
pub async fn finish(
    app: &App,
    task_id: TaskId,
    interaction: &mut dyn Interaction,
    out: &Output,
) -> Result<(), AppError> {
    let view = match app.runtime.get_task(task_id)?.task.state {
        TaskState::Running | TaskState::WaitingUser { .. } => {
            follow::drive(app, task_id, interaction, out).await?
        }
        _ => app.runtime.get_task(task_id)?,
    };
    print_view(&view, out);
    Ok(())
}

/// The task view: lines for humans, the object for JSON.
pub fn print_view(view: &TaskView, out: &Output) {
    out.line(messages::task_state(
        &view.task.task_id.to_string(),
        &view.task.state,
    ));
    out.value(view);
}

/// Parses `30s`, `15m`, `2h` or `1d` into milliseconds.
pub fn parse_duration_ms(text: &str) -> Result<i64, AppError> {
    let text = text.trim();
    let (number, unit) = text.split_at(
        text.trim_end_matches(|c: char| c.is_ascii_alphabetic())
            .len(),
    );
    let value: i64 = number.parse().map_err(|_| AppError::Io {
        detail: format!("`{text}` is not a duration; use a number with s, m, h or d"),
    })?;
    let factor = match unit {
        "s" => 1000,
        "m" | "" => 60_000,
        "h" => 3_600_000,
        "d" => 86_400_000,
        _ => {
            return Err(AppError::Io {
                detail: format!("`{text}` is not a duration; use a number with s, m, h or d"),
            });
        }
    };
    value.checked_mul(factor).ok_or(AppError::Io {
        detail: "duration too large".into(),
    })
}

#[cfg(test)]
mod tests;
