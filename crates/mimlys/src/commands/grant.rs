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
    // Sent once, so never answered that it took effect before and its task is gone; if it were,
    // the client has said so.
    let Applied::Done(grant) = app.client.grant_capability(id, kind)? else {
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
