//! `mimlys approve <task> [--note T]` (REQ-APR-007, REQ-TASK-015): the action runs in this
//! process; a plan-first task's plan, approved, ends the gate and the task goes on.

use crate::app::{App, AppError};
use crate::commands::finish;
use crate::output::Output;
use crate::prompt::Interaction;

/// Approves the pending action, or the plan the task waits for, with the user's optional note for
/// the model, and follows the task.
pub async fn run(
    app: &App,
    task: &str,
    note: Option<String>,
    interaction: &mut dyn Interaction,
    out: &Output,
) -> Result<(), AppError> {
    let id = app.resolve_task(task)?;
    app.client.approve_action(id, note)?;
    finish(app, id, interaction, out).await
}
