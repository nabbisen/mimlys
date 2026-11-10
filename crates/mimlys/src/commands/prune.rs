//! `mimlys task prune [--state …] [--older-than …] [--audit] [--yes]`
//! (REQ-WS-014, RFC 013): lists the terminal tasks that match with their
//! footprints and the total, asks once, then deletes each as `task delete`
//! does, one at a time, reporting each result. It never touches a task that
//! is not terminal; a task whose promotion is still reversible is listed as
//! `kept: promotion reversible` and skipped; audits stay unless `--audit`; a
//! failure stops the run, leaving the remaining tasks as they were and naming
//! them.
//!
//! The selection, the plan and the run are plain functions over data, so that
//! the rules are tested without a store; `run` joins them to the runtime.

use crate::app::{App, AppError};
use crate::commands::parse_duration_ms;
use crate::messages;
use crate::output::Output;
use crate::prompt::Interaction;
use mimlys_core::clock::{Clock, Instant, SystemClock};
use mimlys_core::ids::TaskId;
use mimlys_core::snapshot::Footprint;
use serde::Serialize;

/// The states a task can be pruned from: the terminal ones (external design §5).
pub const TERMINAL_STATES: [&str; 3] = ["completed", "failed", "cancelled"];

/// Why a task that matched is not deleted.
pub const KEPT_PROMOTION_REVERSIBLE: &str = "promotion reversible";

/// What the command line asked for.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Filter {
    /// The states to prune; every terminal state when empty.
    pub states: Vec<String>,
    /// Only tasks whose last change is at least this long ago, in milliseconds.
    pub older_than_ms: Option<i64>,
}

impl Filter {
    /// Parses `--state` (terminal states only) and `--older-than` (the
    /// duration syntax of the other commands: `30s`, `15m`, `2h`, `1d`).
    pub fn parse(states: &[String], older_than: Option<&str>) -> Result<Self, AppError> {
        for state in states {
            if !TERMINAL_STATES.contains(&state.as_str()) {
                return Err(AppError::Io {
                    detail: format!(
                        "`{state}` is not a state `task prune` takes: it deletes terminal tasks only ({})",
                        TERMINAL_STATES.join(", ")
                    ),
                });
            }
        }
        let older_than_ms = older_than.map(parse_duration_ms).transpose()?;
        Ok(Self {
            states: states.to_vec(),
            older_than_ms,
        })
    }

    /// Whether a task in `state`, last changed at `updated_at`, matches at `now`.
    /// A task that is not terminal never does, whatever was asked.
    pub fn matches(&self, state: &str, updated_at: Instant, now: Instant) -> bool {
        if !TERMINAL_STATES.contains(&state) {
            return false;
        }
        if !self.states.is_empty() && !self.states.iter().any(|s| s == state) {
            return false;
        }
        match self.older_than_ms {
            Some(ms) => updated_at.millis_until(now) >= ms,
            None => true,
        }
    }
}

/// A task that matched, with what deleting it frees.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Candidate {
    /// The task.
    pub task_id: String,
    /// Its state.
    pub state: String,
    /// Seconds since its last change.
    pub age_seconds: i64,
    /// What it holds.
    pub footprint: Footprint,
    /// Why it is kept, when it is.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kept: Option<&'static str>,
}

impl Candidate {
    /// A candidate, kept when a promotion of the task can still be reversed
    /// (its backup would be lost).
    pub fn new(task_id: TaskId, state: &str, age_seconds: i64, footprint: Footprint) -> Self {
        let kept = (footprint.promotions > 0).then_some(KEPT_PROMOTION_REVERSIBLE);
        Self {
            task_id: task_id.to_string(),
            state: state.to_owned(),
            age_seconds,
            footprint,
            kept,
        }
    }

    /// What deleting the task frees.
    pub fn bytes(&self) -> u64 {
        self.footprint.workspace_bytes
            + self.footprint.object_bytes_exclusive
            + self.footprint.blob_bytes
    }
}

/// The tasks to delete, in the order given.
pub fn to_delete(candidates: &[Candidate]) -> Vec<&Candidate> {
    candidates.iter().filter(|c| c.kept.is_none()).collect()
}

/// What a run did.
#[derive(Debug, Default)]
pub struct Outcome {
    /// The tasks deleted, with what each freed.
    pub deleted: Vec<(String, Footprint)>,
    /// The task whose deletion failed, and why.
    pub failed: Option<(String, String)>,
    /// The tasks after the failed one, which were not touched.
    pub untouched: Vec<String>,
}

/// Deletes the candidates not kept, one at a time, through `delete`, calling
/// `report` after each result. The first failure stops the run: the tasks
/// after it are not touched and are named.
pub fn execute(
    candidates: &[Candidate],
    mut delete: impl FnMut(&Candidate) -> Result<Footprint, AppError>,
    mut report: impl FnMut(&Candidate, &Result<Footprint, AppError>),
) -> Outcome {
    let mut out = Outcome::default();
    let todo = to_delete(candidates);
    for (i, c) in todo.iter().enumerate() {
        let result = delete(c);
        report(c, &result);
        match result {
            Ok(footprint) => out.deleted.push((c.task_id.clone(), footprint)),
            Err(e) => {
                out.failed = Some((c.task_id.clone(), e.to_string()));
                out.untouched = todo[i + 1..].iter().map(|c| c.task_id.clone()).collect();
                break;
            }
        }
    }
    out
}

/// The matching terminal tasks of the store with their footprints, newest
/// change last (the order `task list` has them in, reversed: oldest first).
///
/// A task already deleted (its audit kept) has nothing left to prune, so it is
/// not listed again; with `--audit` it is, because its events still go.
fn candidates(
    app: &App,
    filter: &Filter,
    audit: bool,
    now: Instant,
) -> Result<Vec<Candidate>, AppError> {
    let mut out = Vec::new();
    let mut tasks = app.runtime.list_tasks()?;
    tasks.sort_by_key(|t| t.updated_at);
    for t in tasks {
        if !filter.matches(&t.state, t.updated_at, now) {
            continue;
        }
        if !audit
            && app.runtime.persistence().load_workspace_state(t.task_id)?
                == Some(mimlys_core::snapshot::WorkspaceState::Deleted)
        {
            continue;
        }
        let view = app.runtime.get_task(t.task_id)?;
        // The state is read again with the footprint: only a terminal task is listed.
        if !view.task.state.is_terminal() {
            continue;
        }
        out.push(Candidate::new(
            t.task_id,
            view.task.state.name(),
            t.updated_at.millis_until(now) / 1000,
            view.footprint,
        ));
    }
    Ok(out)
}

/// `task prune`.
pub fn run(
    app: &App,
    states: &[String],
    older_than: Option<&str>,
    audit: bool,
    yes: bool,
    interaction: &mut dyn Interaction,
    out: &Output,
) -> Result<(), AppError> {
    let filter = Filter::parse(states, older_than)?;
    let found = candidates(app, &filter, audit, SystemClock.now())?;
    let todo = to_delete(&found);
    if found.is_empty() {
        out.line(messages::PRUNE_NOTHING);
        out.value(&serde_json::json!({ "tasks": [], "total_bytes": 0, "deleted": [], "confirmed": false }));
        return Ok(());
    }
    // The plan is shown before anything goes, as `task delete` shows its own.
    for line in messages::prune_plan(&found, audit) {
        out.line(line);
    }
    let total: u64 = todo.iter().map(|c| c.bytes()).sum();
    if todo.is_empty() {
        out.value(&serde_json::json!({ "tasks": found, "total_bytes": 0, "deleted": [], "confirmed": false, "audit": audit }));
        return Ok(());
    }
    let confirmed = yes
        || (interaction.interactive()
            && !out.json()
            && interaction.confirm(messages::PRUNE_CONFIRM));
    if !confirmed {
        out.line(messages::PRUNE_NOT_CONFIRMED);
        out.value(&serde_json::json!({ "tasks": found, "total_bytes": total, "deleted": [], "confirmed": false, "audit": audit }));
        return Ok(());
    }
    let outcome = execute(
        &found,
        |c| {
            let id = TaskId::from_uuid(c.task_id.parse().map_err(|_| AppError::Io {
                detail: format!("`{}` is not a task id", c.task_id),
            })?);
            Ok(app.runtime.delete_task(&app.session, id, audit)?)
        },
        |c, result| match result {
            Ok(f) => out.line(messages::prune_deleted(&c.task_id, f)),
            Err(e) => out.line(messages::prune_failed(&c.task_id, &e.to_string())),
        },
    );
    let freed: u64 = outcome
        .deleted
        .iter()
        .map(|(_, f)| f.workspace_bytes + f.object_bytes_exclusive + f.blob_bytes)
        .sum();
    let report = serde_json::json!({
        "tasks": found,
        "total_bytes": total,
        "confirmed": true,
        "audit": audit,
        "deleted": outcome.deleted.iter().map(|(id, f)| serde_json::json!({ "task_id": id, "footprint": f })).collect::<Vec<_>>(),
        "freed_bytes": freed,
        "failed": outcome.failed.as_ref().map(|(id, why)| serde_json::json!({ "task_id": id, "error": why })),
        "untouched": outcome.untouched,
    });
    match outcome.failed {
        None => {
            out.line(messages::prune_done(outcome.deleted.len(), freed));
            out.value(&report);
            Ok(())
        }
        Some((id, why)) => Err(AppError::Partial {
            message: messages::prune_stopped(&id, &why, &outcome.untouched, outcome.deleted.len()),
            report,
        }),
    }
}

#[cfg(test)]
mod tests;
