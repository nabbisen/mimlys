//! `mimlys reject <task> [--note T] [--halt]` (REQ-APR-007).

use crate::app::{App, AppError};
use crate::commands::finish;
use crate::output::Output;
use crate::prompt::Interaction;

/// Rejects the pending action; the task continues (follow mode) or halts.
pub async fn run(
    app: &App,
    task: &str,
    note: Option<String>,
    halt: bool,
    interaction: &mut dyn Interaction,
    out: &Output,
) -> Result<(), AppError> {
    let id = app.resolve_task(task)?;
    app.runtime
        .reject_action(&app.session, id, note, if halt { Some(true) } else { None })?;
    finish(app, id, interaction, out).await
}
