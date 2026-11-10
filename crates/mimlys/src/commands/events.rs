//! `mimlys events [<task>] [--since N] [--follow]` (REQ-OBS-002).

use crate::app::{AppError, ReadApp};
use crate::follow::event_text;
use crate::messages;
use crate::output::Output;
use mimlys_core::task::TaskState;

/// How often `events --follow` reads for new events, in milliseconds. A
/// reader's poll; it takes no lock and writes nothing. (Not in the limits
/// register: the value was a literal before this name; see the review request.)
pub const EVENTS_POLL_MS: u64 = 500;

/// Prints events after `since`; with `follow`, keeps polling until the
/// task ends (or forever without a task).
pub async fn run(
    app: &ReadApp,
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
        let batch = app.reader.get_events(task_id, position, 1000)?;
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
        if let Some(id) = task_id {
            let state = app.reader.get_task(id)?.task.state;
            if !matches!(
                state,
                TaskState::Running
                    | TaskState::WaitingUser { .. }
                    | TaskState::Created
                    | TaskState::Paused
            ) {
                break;
            }
            // The runtime that ran the task is gone and nobody has recovered
            // the store: the task will not move on its own, so following it
            // would wait for ever. Say so and stop (REQ-PERS-013).
            if matches!(state, TaskState::Running)
                && app.store.probe_lock() == mimlys_persist::LockState::Free
            {
                out.line(messages::FOLLOW_RUNTIME_GONE);
                break;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(EVENTS_POLL_MS)).await;
    }
    if all.is_empty() {
        out.line(messages::EVENTS_EMPTY);
    }
    out.value(&all);
    Ok(())
}
