//! The workspaces no task refers to (handoff 013c, item 2; REQ-WS-014): a copy of
//! a scope that a refused creation left behind (0.10 and earlier did not remove
//! it when the provider, the store or the clock failed after the copy was made).
//! `mimlys task prune` lists each one with its footprint and removes it under the
//! confirmation it asks for the tasks.
//!
//! What is looked at, and what is not:
//! - only the entries of `<data>/workspaces` that are directories, read without
//!   following a link (a link or a junction there is never listed, followed or
//!   removed), and whose name is a task id in the form this program writes (a
//!   lower-case hyphenated UUID): a name that is anything else was not made by
//!   a creation and is not this command's to remove;
//! - not one a task refers to (every task the store lists, whatever its state),
//!   nor one a deletion that is not finished still names (the next start
//!   removes it: it is not a refused creation's).
//!
//! The removal is the one `task delete` uses (`remove_tree`): every removal is
//! attempted, nothing is followed, and each failure is named by its path.

use super::snapshot::remove_tree;
use super::walk::{Flow, Node, walk};
use cap_std::ambient_authority;
use cap_std::fs::Dir;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Why a workspace is listed: the words of the plan and of `--json`.
pub const REASON: &str = "no task: left by a refused creation";

/// A directory of `<data>/workspaces` that no task refers to.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Orphan {
    /// The directory's name: the id of the task that was never recorded.
    pub name: String,
    /// What it holds, in bytes: the regular files below it, not following a link. A part
    /// that cannot be read is not counted.
    pub bytes: u64,
    /// Why it is listed ([`REASON`]), as a task that is kept says why in `kept`.
    pub reason: &'static str,
}

/// `<data>/workspaces`.
fn root(data_dir: &Path) -> PathBuf {
    data_dir.join("workspaces")
}

/// Whether `name` is a task id as this program writes it.
fn is_task_id(name: &str) -> bool {
    uuid::Uuid::parse_str(name).is_ok_and(|id| id.hyphenated().to_string() == name)
}

/// The directories of `<data>/workspaces` that no task refers to: `referenced` holds the
/// names (task ids) the tasks of the store and the unfinished deletions refer to. They are
/// returned in name order. A data directory with no `workspaces` is no error: there are none.
pub fn find(data_dir: &Path, referenced: &BTreeSet<String>) -> std::io::Result<Vec<Orphan>> {
    let entries = match std::fs::read_dir(root(data_dir)) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut found = BTreeSet::new();
    for entry in entries {
        let entry = entry?;
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        if !is_task_id(&name) || referenced.contains(&name) {
            continue;
        }
        // Read without following: the type of a directory entry is the entry's own, so a link
        // (a symbolic link, or a junction on Windows) is not a directory here.
        let kind = entry.file_type()?;
        if kind.is_dir() {
            found.insert(name);
        }
    }
    Ok(found
        .into_iter()
        .map(|name| {
            let bytes = size(&root(data_dir).join(&name));
            Orphan {
                name,
                bytes,
                reason: REASON,
            }
        })
        .collect())
}

/// The bytes of the regular files below `dir`, by the one walk, which follows no link. A part
/// that cannot be read ends the count there: it is a footprint to show, not a guarantee.
fn size(dir: &Path) -> u64 {
    let Ok(handle) = Dir::open_ambient_dir(dir, ambient_authority()) else {
        return 0;
    };
    let mut total = 0;
    let _ = walk(&handle, "", None, |found| {
        if let Node::File { len, .. } = found.node {
            total += len;
        }
        Ok(Flow::Descend)
    });
    total
}

/// Removes the workspace `name` of `<data>/workspaces`, as it was listed: when what is there
/// is no longer a directory with the name of a task id (or is a link), nothing is removed.
/// Returns the paths that could not be removed, relative to the data directory.
pub fn remove(data_dir: &Path, name: &str) -> Vec<String> {
    let mut failed = Vec::new();
    if !is_task_id(name) {
        return failed;
    }
    let path = root(data_dir).join(name);
    match std::fs::symlink_metadata(&path) {
        Ok(meta) if meta.is_dir() => {
            remove_tree(&path, data_dir, &mut failed);
        }
        _ => {}
    }
    failed
}

#[cfg(test)]
mod tests;
