//! `mimlys approve <task>` (REQ-APR-007): the action runs in this process.

use crate::app::{App, AppError};
use crate::commands::finish;
use crate::output::Output;
use crate::prompt::Interaction;

/// Approves the pending action and follows the task.
pub async fn run(
    app: &App,
    task: &str,
    interaction: &mut dyn Interaction,
    out: &Output,
) -> Result<(), AppError> {
    let id = app.resolve_task(task)?;
    app.runtime.approve_action(&app.session, id)?;
    finish(app, id, interaction, out).await
}
