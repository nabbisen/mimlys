//! `mimlys events [<task>] [--since N] [--follow]` (REQ-OBS-002).

use crate::app::{App, AppError};
use crate::follow::event_text;
use crate::messages;
use crate::output::Output;
use mimlys_core::task::TaskState;

/// Prints events after `since`; with `follow`, keeps polling until the
/// task ends (or forever without a task).
pub async fn run(
    app: &App,
    task: Option<&str>,
    since: u64,
    follow: bool,
    out: &Output,
) -> Result<(), AppError> {
    let task_id = match task {
        Some(t) => Some(app.resolve_task(t)?),
        None => None,
    };
    let mut position = since;
    let mut all = Vec::new();
    loop {
        let batch = app.runtime.get_events(task_id, position, 1000)?;
        for e in &batch {
            out.line(event_text(e));
            position = e.position;
        }
        all.extend(batch.iter().cloned());
        if !batch.is_empty() {
            continue;
        }
        if !follow {
            break;
        }
        if let Some(id) = task_id
            && !matches!(
                app.runtime.get_task(id)?.task.state,
                TaskState::Running
                    | TaskState::WaitingUser { .. }
                    | TaskState::Created
                    | TaskState::Paused
            )
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    if all.is_empty() {
        out.line(messages::EVENTS_EMPTY);
    }
    out.value(&all);
    Ok(())
}
