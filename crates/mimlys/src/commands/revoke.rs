//! `mimlys revoke <task> <grant>` (REQ-CAP-005).

use crate::app::{App, AppError};
use crate::commands::finish;
use crate::messages;
use crate::output::Output;
use crate::prompt::Interaction;
use mimlys_core::ids::GrantId;

/// Revokes a grant at once; an approval that used it is withdrawn.
pub async fn run(
    app: &App,
    task: &str,
    grant: &str,
    interaction: &mut dyn Interaction,
    out: &Output,
) -> Result<(), AppError> {
    let id = app.resolve_task(task)?;
    let grant_id = resolve_grant(app, id, grant)?;
    let grant = app.runtime.revoke_capability(&app.session, id, grant_id)?;
    out.line(messages::grant_revoked(&grant.grant_id.to_string()));
    out.value(&grant);
    if out.json() {
        return Ok(());
    }
    finish(app, id, interaction, out).await
}

fn resolve_grant(
    app: &App,
    task_id: mimlys_core::ids::TaskId,
    text: &str,
) -> Result<GrantId, AppError> {
    if let Ok(uuid) = uuid::Uuid::parse_str(text) {
        return Ok(GrantId::from_uuid(uuid));
    }
    let matches: Vec<GrantId> = app
        .runtime
        .get_task(task_id)?
        .grants
        .iter()
        .map(|g| g.grant_id)
        .filter(|g| text.len() >= 4 && g.to_string().starts_with(text))
        .collect();
    match matches.as_slice() {
        [one] => Ok(*one),
        [] => Err(AppError::TaskRef {
            detail: format!("no active grant starts with {text}"),
            not_found: true,
        }),
        many => Err(AppError::TaskRef {
            detail: format!(
                "{} grants start with {text}; give more characters",
                many.len()
            ),
            not_found: false,
        }),
    }
}
