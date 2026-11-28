//! `mimlys grant <task> <kind> <scope>` (REQ-CAP-004).

use crate::app::{App, AppError};
use crate::client::Applied;
use crate::commands::finish;
use crate::messages;
use crate::output::Output;
use crate::prompt::{Interaction, parse_grant};

/// Issues a grant within the ceiling; a pending approval is asked again
/// under the new grant set.
pub async fn run(
    app: &App,
    task: &str,
    words: &[String],
    interaction: &mut dyn Interaction,
    out: &Output,
) -> Result<(), AppError> {
    let id = app.resolve_task(task)?;
    let kind = parse_grant(&words.join(" ")).map_err(|e| AppError::Io {
        detail: e.to_owned(),
    })?;
    app.check_host_tool_grant(&kind)?;
    // Sent once, but a repeat after a broken connection may find it recorded and its task deleted
    // since: it took effect, and the task is gone.
    let Applied::Done(grant) = app.client.grant_capability(id, kind)? else {
        crate::commands::applied_earlier_gone(out, "grant_capability", id);
        return Ok(());
    };
    out.line(messages::grant_issued(
        &grant.grant_id.to_string(),
        &serde_json::to_string(grant.kind()).unwrap_or_default(),
    ));
    out.value(&grant);
    if out.json() {
        return Ok(());
    }
    finish(app, id, interaction, out).await
}
