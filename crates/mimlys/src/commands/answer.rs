//! `mimlys answer <task> <text>` (REQ-APR-010).

use crate::app::{App, AppError};
use crate::commands::finish;
use crate::output::Output;
use crate::prompt::Interaction;

/// Answers the pending question and follows the task.
pub async fn run(
    app: &App,
    task: &str,
    text: &str,
    interaction: &mut dyn Interaction,
    out: &Output,
) -> Result<(), AppError> {
    let id = app.resolve_task(task)?;
    app.runtime.answer_question(&app.session, id, text)?;
    finish(app, id, interaction, out).await
}
