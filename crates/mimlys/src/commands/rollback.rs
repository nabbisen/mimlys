//! `mimlys rollback <task> [--snapshot ID] [--promotion ID]` (REQ-WS-006, REQ-WS-007).

use crate::app::{App, AppError};
use crate::messages;
use crate::output::Output;
use crate::prompt::Interaction;
use mimlys_core::ids::{PromotionId, SnapshotId};
use mimlys_core::snapshot::RevertAction;

/// Restores the task's workspace to a snapshot (the latest by default), or
/// reverses a promotion from its backup: the paths are listed first, and the
/// user confirms; paths edited since are conflicts, refused unless confirmed.
#[allow(clippy::too_many_arguments)]
pub fn run(
    app: &App,
    task: &str,
    snapshot: Option<&str>,
    promotion: Option<&str>,
    yes: bool,
    overwrite_edited: bool,
    interaction: &mut dyn Interaction,
    out: &Output,
) -> Result<(), AppError> {
    let id = app.resolve_task(task)?;
    if let Some(p) = promotion {
        let view = app.runtime.get_task(id)?;
        let promotion_id = resolve_id(
            p,
            view.promotions.iter().map(|p| p.promotion_id.to_string()),
        )
        .map(|s| PromotionId::from_uuid(uuid::Uuid::parse_str(&s).expect("listed id")))?;
        let plan = app.runtime.revert_plan(id, promotion_id)?;
        out.line(messages::revert_header(
            &plan.promotion_id.to_string(),
            plan.entries.len() + plan.conflicts.len(),
        ));
        for e in &plan.entries {
            out.line(messages::revert_entry(
                match e.action {
                    RevertAction::Restore => "restore",
                    RevertAction::Remove => "remove",
                },
                &e.path,
            ));
        }
        if !plan.conflicts.is_empty() {
            out.line(messages::revert_conflicts(&plan.conflicts));
        }
        if !plan.edits_detected {
            out.line(messages::REVERT_EDITS_UNKNOWN);
        }
        let interactive = interaction.interactive() && !out.json();
        let confirmed = yes || (interactive && interaction.confirm(messages::REVERT_CONFIRM));
        if !confirmed {
            out.line(messages::REVERT_NOT_CONFIRMED);
            out.value(&serde_json::json!({ "plan": plan, "reversed": false }));
            return Ok(());
        }
        // Edits are overwritten only when the user said so, apart from the first confirmation.
        let overwrite = !plan.conflicts.is_empty()
            && (overwrite_edited
                || (interactive && interaction.confirm(messages::REVERT_OVERWRITE_CONFIRM)));
        let reversed = app
            .runtime
            .revert_promotion(&app.session, id, promotion_id, overwrite)?;
        out.line(messages::promotion_reversed(
            &reversed.promotion_id.to_string(),
            reversed.files.len(),
        ));
        out.value(&reversed);
        return Ok(());
    }
    let snapshot_id = match snapshot {
        Some(s) => {
            let view = app.runtime.get_task(id)?;
            Some(
                resolve_id(s, view.snapshots.iter().map(|s| s.snapshot_id.to_string())).map(
                    |s| SnapshotId::from_uuid(uuid::Uuid::parse_str(&s).expect("listed id")),
                )?,
            )
        }
        None => None,
    };
    let record = app
        .runtime
        .rollback_workspace(&app.session, id, snapshot_id)?;
    out.line(messages::rolled_back(
        &record.snapshot_id.to_string(),
        record.reason.name(),
    ));
    out.value(&record);
    Ok(())
}

/// A full id or a unique prefix of at least four characters among `candidates`.
pub fn resolve_id(
    text: &str,
    candidates: impl Iterator<Item = String>,
) -> Result<String, AppError> {
    let text = text.trim();
    let matches: Vec<String> = candidates
        .filter(|c| c == text || (text.len() >= 4 && c.starts_with(text)))
        .collect();
    match matches.as_slice() {
        [one] => Ok(one.clone()),
        [] => Err(AppError::TaskRef {
            detail: format!("nothing starts with {text}"),
            not_found: true,
        }),
        many => Err(AppError::TaskRef {
            detail: format!("{} ids start with {text}; give more characters", many.len()),
            not_found: false,
        }),
    }
}
