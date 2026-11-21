//! What a probe a crash cut short left behind, and its removal (REQ-MDL-011, THR-40).
//!
//! **How a probe that a crash left is found.** The probe makes its scope, `<data_dir>/tmp/probe-<id>/`
//! (`<id>` a UUID), before anything else, and removes it last: the task and every record of it,
//! then its workspace, then the scope. So a directory of that name under `<data_dir>/tmp` is the
//! sign that a probe began and did not end, and a task of the store whose scope **is** such a
//! directory is its task. A user's task cannot have that scope: a scope inside the data directory
//! is refused for every provider but the probe's own, which takes only the directory the probe
//! just made. The next start that writes ([`remove`], called by `App::open` before recovery, which
//! therefore never reports a probe's task as a recovered one) removes, for each such directory:
//!
//! 1. the tasks whose scope it is, whatever their state, with their records and events, as
//!    `task delete --audit` removes them;
//! 2. the workspace copy of a creation that the crash cut between the copy and the task's record
//!    (a directory of `<data_dir>/workspaces` that no task has), when it holds exactly the probe's
//!    one file as the probe wrote it: nothing the user made is identified that way, and a copy
//!    that holds anything else is left to `task prune`;
//! 3. the directory.
//!
//! A start with no such directory reads `<data_dir>/tmp` and nothing else.

use super::{FILE, FILE_START, GOAL, SCOPE_PREFIX};
use mimlys_core::clock::Clock;
use mimlys_core::event::{Actor, Event, EventRecord};
use mimlys_core::ids::{EventId, SystemIds, TaskId};
use mimlys_core::spi::{Persistence, WorkspaceError, WorkspaceProvider};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// What [`remove`] did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Removed {
    /// The probe directories found (their names).
    pub scopes: Vec<String>,
    /// The tasks of probes removed.
    pub tasks: usize,
    /// The workspace copies of probes' unfinished creations removed.
    pub copies: usize,
    /// What could not be removed, and why (the next start tries again).
    pub left: Vec<String>,
}

impl Removed {
    /// Whether a probe's leftover was found at all.
    pub fn found(&self) -> bool {
        !self.scopes.is_empty()
    }
}

/// The directories `probe-<id>` of `tmp`: real directories (never a link), named as the probe
/// names its scope.
pub fn scope_dirs(tmp: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(tmp) else {
        return Vec::new();
    };
    let mut found: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .filter(|entry| {
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                return false;
            };
            is_scope_name(name) && entry.file_type().is_ok_and(|t| t.is_dir())
        })
        .map(|entry| entry.path())
        .collect();
    found.sort();
    found
}

/// Whether `name` is `probe-` and a hyphenated UUID, as the probe writes it.
pub fn is_scope_name(name: &str) -> bool {
    name.strip_prefix(SCOPE_PREFIX)
        .is_some_and(|id| uuid::Uuid::parse_str(id).is_ok_and(|u| u.hyphenated().to_string() == id))
}

/// Deletes the probe's task with every record and event of it (as `task delete --audit` does) and
/// its files; the paths and the reasons that could not be removed. The task may be in any state:
/// a probe's task is not the user's, and nothing of it is kept.
pub fn discard_task(
    store: &dyn Persistence,
    workspaces: &dyn WorkspaceProvider,
    task_id: TaskId,
    clock: &dyn Clock,
) -> Vec<String> {
    // With `audit` the rows go and no event is written; the record is the deletion's argument.
    let event = EventRecord {
        event_id: EventId::new(&SystemIds),
        at: clock.now(),
        actor: Actor::Runtime,
        task_id: Some(task_id),
        step_id: None,
        action_id: None,
        policy_hash: mimlys_core::policy::Policy::shipped().hash(),
        event: Event::TaskDeleted { audit: true },
    };
    match store.delete_task_data(task_id, true, &event) {
        Err(e) => vec![format!("the records of the probe's task {task_id}: {e}")],
        Ok(rows) => match workspaces.delete_files(task_id, &rows) {
            Ok(()) => Vec::new(),
            Err(WorkspaceError::NotRemoved { paths, .. }) => paths,
            Err(e) => vec![format!("the files of the probe's task {task_id}: {e}")],
        },
    }
}

/// Removes what a probe that a crash cut short left, as the module says. `workspaces` is the
/// provider that owns `<data_dir>/workspaces`.
pub fn remove(
    store: &dyn Persistence,
    workspaces: &dyn WorkspaceProvider,
    data_dir: &Path,
    clock: &dyn Clock,
) -> Removed {
    let mut removed = Removed::default();
    let dirs = scope_dirs(&data_dir.join("tmp"));
    if dirs.is_empty() {
        return removed;
    }
    removed.scopes = dirs
        .iter()
        .filter_map(|d| d.file_name().and_then(|n| n.to_str()).map(str::to_owned))
        .collect();
    // The text a task's scope has for each directory: the canonical path, as the probe gave it.
    let roots: BTreeSet<String> = dirs
        .iter()
        .flat_map(|dir| {
            let canonical = std::fs::canonicalize(dir).ok();
            [Some(dir.clone()), canonical].into_iter().flatten()
        })
        .map(|p| {
            let text = p.to_string_lossy().into_owned();
            text.strip_prefix(r"\\?\")
                .map(str::to_owned)
                .unwrap_or(text)
        })
        .collect();
    match store.list_tasks() {
        Err(e) => removed.left.push(format!("the tasks of the store: {e}")),
        Ok(summaries) => {
            for summary in summaries.iter().filter(|s| s.goal == GOAL) {
                let is_probes = match store.load_task(summary.task_id) {
                    Ok(Some(task)) => roots.contains(&task.scope.root),
                    Ok(None) => false,
                    Err(e) => {
                        removed
                            .left
                            .push(format!("the task {}: {e}", summary.task_id));
                        false
                    }
                };
                if is_probes {
                    removed
                        .left
                        .extend(discard_task(store, workspaces, summary.task_id, clock));
                    removed.tasks += 1;
                }
            }
        }
    }
    // The copy of a creation the crash cut after the copy and before the task's record.
    removed.copies = remove_unfinished_copies(store, data_dir, &mut removed.left);
    for dir in &dirs {
        removed
            .left
            .extend(crate::workspace::remove_made_dir(dir, data_dir));
    }
    removed
}

/// Removes the workspace copies that no task has and that hold exactly the probe's file as the
/// probe wrote it. Returns how many.
fn remove_unfinished_copies(
    store: &dyn Persistence,
    data_dir: &Path,
    left: &mut Vec<String>,
) -> usize {
    let mut referenced: BTreeSet<String> = match store.list_tasks() {
        Ok(tasks) => tasks.into_iter().map(|t| t.task_id.to_string()).collect(),
        Err(_) => return 0,
    };
    if let Ok(pending) =
        mimlys_persist::snapshots::SnapshotFiles::open_for_reading(data_dir).pending_deletions()
    {
        referenced.extend(pending.into_iter().map(|(task_id, _)| task_id.to_string()));
    }
    let Ok(orphans) = crate::workspace::orphans::find(data_dir, &referenced) else {
        return 0;
    };
    let mut count = 0;
    for orphan in orphans {
        let tree = data_dir.join("workspaces").join(&orphan.name).join("tree");
        if !holds_only_the_probes_file(&tree) {
            continue;
        }
        let failed = crate::workspace::orphans::remove(data_dir, &orphan.name);
        if failed.is_empty() {
            count += 1;
        }
        left.extend(failed);
    }
    count
}

/// Whether `tree` holds one entry, a regular file named as the probe's, with the content the
/// probe wrote.
fn holds_only_the_probes_file(tree: &Path) -> bool {
    let Ok(mut entries) = std::fs::read_dir(tree) else {
        return false;
    };
    let (Some(Ok(entry)), None) = (entries.next(), entries.next()) else {
        return false;
    };
    entry.file_name() == FILE
        && entry.file_type().is_ok_and(|t| t.is_file())
        && std::fs::read_to_string(entry.path()).is_ok_and(|text| text == FILE_START)
}

#[cfg(test)]
mod tests;
