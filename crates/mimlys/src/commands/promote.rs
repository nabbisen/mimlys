//! `mimlys promote <task> [--yes] [--summary] [--patch]` (REQ-WS-007,
//! REQ-SEC-015): the plan, the diff, the user's confirmation, the backup,
//! the writes. `--patch` prints a unified patch and writes nothing.

use crate::app::{App, AppError};
use crate::messages;
use crate::output::Output;
use crate::prompt::Interaction;
use mimlys_core::snapshot::ChangeKind;

/// Plans and, when confirmed, applies a promotion.
pub fn run(
    app: &App,
    task: &str,
    yes: bool,
    summary: bool,
    patch: bool,
    interaction: &mut dyn Interaction,
    out: &Output,
) -> Result<(), AppError> {
    let id = app.resolve_task(task)?;
    let plan = app.runtime.promotion_plan(id)?;
    if patch {
        if plan.changes.is_empty() {
            out.line(messages::PROMOTE_NOTHING);
        }
        for (_, diff) in &plan.text_diffs {
            out.line(diff.trim_end_matches('\n'));
        }
        out.value(&plan);
        return Ok(());
    }
    if plan.changes.is_empty() {
        out.line(messages::PROMOTE_NOTHING);
        out.value(&serde_json::json!({ "plan": plan, "applied": false }));
        return Ok(());
    }
    out.line(messages::promote_header(
        &id.to_string(),
        plan.changes.len(),
    ));
    for c in &plan.changes {
        out.line(messages::promote_change(
            match c.kind {
                ChangeKind::Added => "added",
                ChangeKind::Modified => "modified",
                ChangeKind::Deleted => "deleted",
            },
            &c.path,
            c.bytes,
        ));
    }
    if !plan.excluded.is_empty() {
        out.line(messages::promote_excluded(&plan.excluded));
    }
    if !summary {
        for (_, diff) in &plan.text_diffs {
            out.line("");
            out.line(diff.trim_end_matches('\n'));
        }
    }
    let confirmed = yes
        || (interaction.interactive()
            && !out.json()
            && interaction.confirm(messages::PROMOTE_CONFIRM));
    if !confirmed {
        out.line(messages::PROMOTE_NOT_CONFIRMED);
        out.value(&serde_json::json!({ "plan": plan, "applied": false }));
        return Ok(());
    }
    let promotion = app.runtime.promote_workspace(&app.session, id, &plan)?;
    out.line(messages::promoted(
        &promotion.promotion_id.to_string(),
        promotion.files.len(),
        &promotion.backup_snapshot.to_string(),
    ));
    out.value(&serde_json::json!({ "plan": plan, "applied": true, "promotion": promotion }));
    Ok(())
}
