//! `mimlys task prune [--state …] [--older-than …] [--audit] [--yes]`
//! (REQ-WS-014, RFC 013): lists the terminal tasks that match with their
//! footprints and the total, asks once, then deletes each as `task delete`
//! does, one at a time, reporting each result. It never touches a task that
//! is not terminal; a task whose promotion is still reversible is listed as
//! `kept: promotion reversible` and skipped; audits stay unless `--audit`; a
//! failure stops the run, leaving the remaining tasks as they were and naming
//! them. A task deleted with files left (the next start removes them) is not a
//! failure: it counts as deleted, its paths are listed, the run goes on, and
//! the command finishes `Partial` (exit 1) so that a script sees that files
//! remain.
//!
//! The plan also lists each directory of `<data>/workspaces` that no task refers to
//! (REQ-WS-014; handoff 013c: a copy of a scope that a refused creation left behind) as
//! `no task: left by a refused creation`, with its footprint, and removes it under the same
//! confirmation. Those are not tasks: the filters do not apply to them, and they are removed
//! before the tasks, so that a task that fails to delete does not leave them.
//!
//! The selection, the plan and the run are plain functions over data, so that
//! the rules are tested without a store; `run` joins them to the runtime.

use crate::app::{App, AppError};
use crate::commands::parse_duration_ms;
use crate::messages;
use crate::output::Output;
use crate::prompt::Interaction;
use crate::workspace::orphans::{self, Orphan};
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

/// The paths a deletion left, when that is what its error says: the task is deleted, and the
/// next start removes these.
fn files_left(error: &AppError) -> Option<&[String]> {
    match error {
        AppError::Runtime(mimlys_core::runtime::RuntimeError::Workspace(
            mimlys_core::spi::WorkspaceError::NotRemoved { paths, .. },
        ))
        | AppError::Workspace(mimlys_core::spi::WorkspaceError::NotRemoved { paths, .. }) => {
            Some(paths)
        }
        _ => None,
    }
}

/// The line for a task whose deletion returned an error. A deletion that left files behind has
/// deleted the task: its message says so and names the task, so it is not labelled `failed`.
pub fn failure_line(task_id: &str, error: &AppError) -> String {
    if let Some(paths) = files_left(error) {
        return messages::prune_task_left(task_id, paths);
    }
    messages::prune_failed(task_id, &error.to_string())
}

/// The tasks to delete, in the order given.
pub fn to_delete(candidates: &[Candidate]) -> Vec<&Candidate> {
    candidates.iter().filter(|c| c.kept.is_none()).collect()
}

/// What a run did.
#[derive(Debug, Default)]
pub struct Outcome {
    /// The tasks deleted, with what each freed (a task deleted with files left is among them).
    pub deleted: Vec<(String, Footprint)>,
    /// The tasks deleted with files left, with the paths: the next start removes them.
    pub left: Vec<(String, Vec<String>)>,
    /// The task whose deletion failed, and why.
    pub failed: Option<(String, String)>,
    /// The tasks after the failed one, which were not touched.
    pub untouched: Vec<String>,
}

/// Deletes the candidates not kept, one at a time, through `delete`, calling
/// `report` after each result. A deletion that left files behind has deleted the task: the run
/// goes on, and the paths are kept in `left`. Any other failure stops the run: the tasks after
/// it are not touched and are named.
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
            Err(e) if files_left(&e).is_some() => {
                out.deleted.push((c.task_id.clone(), c.footprint.clone()));
                out.left.push((
                    c.task_id.clone(),
                    files_left(&e).map(<[String]>::to_vec).unwrap_or_default(),
                ));
            }
            Err(e) => {
                out.failed = Some((c.task_id.clone(), e.to_string()));
                out.untouched = todo[i + 1..].iter().map(|c| c.task_id.clone()).collect();
                break;
            }
        }
    }
    out
}

/// What a run did with the workspaces no task refers to.
#[derive(Debug, Default)]
pub struct OrphanOutcome {
    /// The workspaces removed, with what each held (one with paths left is among them: the
    /// next prune finds the rest).
    pub removed: Vec<Orphan>,
    /// The workspaces with paths that could not be removed, with the paths.
    pub left: Vec<(String, Vec<String>)>,
}

/// Removes the workspaces, one at a time, through `remove`, which returns the paths it could not
/// remove; `report` is called after each one. A workspace with paths left does not stop the
/// run: every one is tried, and a next prune lists the one that remains.
pub fn remove_orphans(
    found: &[Orphan],
    mut remove: impl FnMut(&Orphan) -> Vec<String>,
    mut report: impl FnMut(&Orphan, &[String]),
) -> OrphanOutcome {
    let mut out = OrphanOutcome::default();
    for orphan in found {
        let left = remove(orphan);
        report(orphan, &left);
        if left.is_empty() {
            out.removed.push(orphan.clone());
        } else {
            out.left.push((orphan.name.clone(), left));
        }
    }
    out
}

/// The workspaces no task refers to: the directories of `<data>/workspaces` named like a task
/// that no task of the store has, and no deletion that is not finished names (the next start
/// finishes it).
fn orphans(app: &App) -> Result<Vec<Orphan>, AppError> {
    let mut referenced: std::collections::BTreeSet<String> = app
        .runtime
        .list_tasks()?
        .into_iter()
        .map(|t| t.task_id.to_string())
        .collect();
    if let Ok(pending) = mimlys_persist::snapshots::SnapshotFiles::open_for_reading(&app.data_dir)
        .pending_deletions()
    {
        referenced.extend(pending.into_iter().map(|(task_id, _)| task_id.to_string()));
    }
    orphans::find(&app.data_dir, &referenced).map_err(|e| AppError::Io {
        detail: format!("<data>/workspaces could not be read: {e}"),
    })
}

/// The matching terminal tasks of the store with their footprints, newest
/// change last (the order `task list` has them in, reversed: oldest first).
///
/// A task already deleted (its audit kept) has nothing left to prune, so it is
/// not listed again; with `--audit` it is, because its events still go.
///
/// A task that holds a word a newer release wrote (REQ-MAIN-006) is not a candidate and does not
/// fail the run: it is returned apart, to be named. What deleting it would release is read from
/// its snapshots, and the state that makes it terminal from its record, which this release cannot
/// read whole.
fn candidates(
    app: &App,
    filter: &Filter,
    audit: bool,
    now: Instant,
) -> Result<(Vec<Candidate>, Vec<NewerTask>), AppError> {
    let mut out = Vec::new();
    let mut newer = Vec::new();
    let mut tasks = app.runtime.list_tasks()?;
    tasks.sort_by_key(|t| t.updated_at);
    for t in tasks {
        if !filter.matches(&t.state, t.updated_at, now) {
            continue;
        }
        let found = mimlys_core::newer::scan(app.runtime.persistence(), t.task_id)?;
        if let Some(first) = found.first() {
            newer.push(NewerTask {
                task_id: t.task_id.to_string(),
                record: first.kind.name().to_owned(),
                unknown_to_this_release: first.newer.words.clone(),
            });
            continue;
        }
        let deleted = match app.runtime.persistence().load_workspace_state(t.task_id) {
            Ok(state) => state == Some(mimlys_core::snapshot::WorkspaceState::Deleted),
            Err(mimlys_core::spi::PersistenceError::NewerRecord { field, word, .. }) => {
                newer.push(NewerTask {
                    task_id: t.task_id.to_string(),
                    record: "workspace".to_owned(),
                    unknown_to_this_release: vec![mimlys_core::newer::UnknownWord::new(
                        field, word,
                    )],
                });
                continue;
            }
            Err(e) => return Err(e.into()),
        };
        if !audit && deleted {
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
    Ok((out, newer))
}

/// A task `task prune` leaves alone because a record of it holds a word that a newer release
/// wrote (REQ-MAIN-006): the release that wrote it is the one that can delete it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct NewerTask {
    /// The task.
    pub task_id: String,
    /// The kind of record that holds the word.
    pub record: String,
    /// The word, with the field that holds it.
    pub unknown_to_this_release: Vec<mimlys_core::newer::UnknownWord>,
}

/// `value` with the tasks left alone added, when there are any.
fn with_newer(mut value: serde_json::Value, newer: &[NewerTask]) -> serde_json::Value {
    if !newer.is_empty() {
        value["newer_tasks"] = serde_json::to_value(newer).unwrap_or_default();
    }
    value
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
    let (found, newer) = candidates(app, &filter, audit, SystemClock.now())?;
    let no_task = orphans(app)?;
    let todo = to_delete(&found);
    for task in &newer {
        out.line(messages::prune_newer_task(task));
    }
    if found.is_empty() && no_task.is_empty() {
        out.line(messages::PRUNE_NOTHING);
        out.value(&with_newer(
            serde_json::json!({ "tasks": [], "no_task": [], "total_bytes": 0, "deleted": [], "confirmed": false }),
            &newer,
        ));
        return Ok(());
    }
    // The plan is shown before anything goes, as `task delete` shows its own.
    if !found.is_empty() {
        for line in messages::prune_plan(&found, audit) {
            out.line(line);
        }
    }
    if !no_task.is_empty() {
        for line in messages::prune_no_task_plan(&no_task, !found.is_empty()) {
            out.line(line);
        }
    }
    let no_task_bytes: u64 = no_task.iter().map(|o| o.bytes).sum();
    let total: u64 = todo.iter().map(|c| c.bytes()).sum::<u64>() + no_task_bytes;
    if todo.is_empty() && no_task.is_empty() {
        out.value(&with_newer(
            serde_json::json!({ "tasks": found, "no_task": [], "total_bytes": 0, "deleted": [], "confirmed": false, "audit": audit }),
            &newer,
        ));
        return Ok(());
    }
    let confirmed = yes
        || (interaction.interactive()
            && !out.json()
            && interaction.confirm(messages::prune_confirm(
                !todo.is_empty(),
                !no_task.is_empty(),
            )));
    if !confirmed {
        out.line(messages::PRUNE_NOT_CONFIRMED);
        out.value(&with_newer(
            serde_json::json!({ "tasks": found, "no_task": no_task, "total_bytes": total, "deleted": [], "confirmed": false, "audit": audit }),
            &newer,
        ));
        return Ok(());
    }
    // The workspaces no task refers to first: they are not tasks, and a task that cannot be
    // deleted stops the run after them, not before.
    let removed_no_task = remove_orphans(
        &no_task,
        |o| orphans::remove(&app.data_dir, &o.name),
        |o, left| out.line(messages::prune_no_task_removed(o, left)),
    );
    let outcome = execute(
        &found,
        |c| {
            let id = TaskId::from_uuid(c.task_id.parse().map_err(|_| AppError::Io {
                detail: format!("`{}` is not a task id", c.task_id),
            })?);
            Ok(app.client.delete_task(id, audit)?)
        },
        |c, result| match result {
            Ok(f) => out.line(messages::prune_deleted(&c.task_id, f)),
            Err(e) => out.line(failure_line(&c.task_id, e)),
        },
    );
    // What a task deleted with files left freed is not known: it is not counted.
    let freed_tasks: u64 = outcome
        .deleted
        .iter()
        .filter(|(id, _)| !outcome.left.iter().any(|(left, _)| left == id))
        .map(|(_, f)| f.workspace_bytes + f.object_bytes_exclusive + f.blob_bytes)
        .sum();
    let freed_no_task: u64 = removed_no_task.removed.iter().map(|o| o.bytes).sum();
    let freed = freed_tasks + freed_no_task;
    let report = with_newer(
        serde_json::json!({
            "tasks": found,
            "no_task": no_task,
            "total_bytes": total,
            "confirmed": true,
            "audit": audit,
            "deleted": outcome.deleted.iter().map(|(id, f)| serde_json::json!({ "task_id": id, "footprint": f })).collect::<Vec<_>>(),
            "no_task_removed": removed_no_task.removed,
            "freed_bytes": freed,
            "failed": outcome.failed.as_ref().map(|(id, why)| serde_json::json!({ "task_id": id, "error": why })),
            "untouched": outcome.untouched,
            "left": outcome.left.iter().map(|(id, paths)| serde_json::json!({ "task_id": id, "paths": paths })).collect::<Vec<_>>(),
            "no_task_left": removed_no_task.left.iter().map(|(name, paths)| serde_json::json!({ "name": name, "paths": paths })).collect::<Vec<_>>(),
        }),
        &newer,
    );
    let done = messages::prune_done(outcome.deleted.len(), removed_no_task.removed.len(), freed);
    // The paths that remain: of tasks (the next start removes them) and of workspaces with no
    // task (the next prune lists them again).
    let mut left_parts = Vec::new();
    if !outcome.left.is_empty() {
        left_parts.push(messages::prune_left(&outcome.left));
    }
    if !removed_no_task.left.is_empty() {
        left_parts.push(messages::prune_no_task_left(&removed_no_task.left));
    }
    let left_text = (!left_parts.is_empty()).then(|| left_parts.join("; "));
    match (outcome.failed, left_text) {
        (None, None) => {
            out.line(done);
            out.value(&report);
            Ok(())
        }
        // Every task was deleted, and files of some are left.
        (None, Some(message)) => {
            out.line(done);
            Err(AppError::Partial { message, report })
        }
        (Some((id, why)), left) => {
            let mut message =
                messages::prune_stopped(&id, &why, &outcome.untouched, outcome.deleted.len());
            if let Some(left) = left {
                message.push_str("; ");
                message.push_str(&left);
            }
            Err(AppError::Partial { message, report })
        }
    }
}

#[cfg(test)]
mod tests;
