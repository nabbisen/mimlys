//! `mimlys rollback <task> [--snapshot ID] [--promotion ID]` (REQ-WS-006, REQ-WS-007).

use crate::app::{App, AppError};
use crate::messages;
use crate::output::Output;
use mimlys_core::ids::{PromotionId, SnapshotId};

/// Restores the task's workspace to a snapshot (the latest by default), or
/// reverses a promotion from its backup.
pub fn run(
    app: &App,
    task: &str,
    snapshot: Option<&str>,
    promotion: Option<&str>,
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
        let reversed = app
            .runtime
            .revert_promotion(&app.session, id, promotion_id)?;
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
