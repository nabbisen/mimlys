//! The one walk of a tree (external design §14.2 to §14.4; REQ-WS-002, REQ-WS-004, REQ-WS-006,
//! REQ-WS-008): the snapshot of a task's tree, the restore, the quota check, the measure and the
//! copy of a scope, and the look at what a promotion would remove all read a tree through it.
//!
//! The walk is iterative: it keeps the directories still to read in a list, never on the stack
//! of the thread, so a tree of any depth is read in the same stack. It sees every kind of entry
//! (a named pipe, a socket and a device are entries, named by [`Node::Special`], and are never
//! opened), it refuses a name that is not valid UTF-8 and names it, and it states a failure to
//! read with the path and the kind of the failure. A link's target is kept byte for byte.
//!
//! A directory is read through the handle of its parent where one is held (at most
//! [`LIVE_HANDLES`] at a time), so that the cost of a level does not grow with its depth and the
//! length of a path (the system's limit on one) does not stop the walk of a deep tree; a
//! directory whose parent is not held is opened from the root by its path.

use super::snapshot::{special_name, unreadable};
use super::{is_excluded, join};
use cap_std::fs::Dir;
use mimlys_core::panic_text::bounded_message;
use mimlys_core::policy::ExclusionMatcher;
use mimlys_core::spi::WorkspaceError;
use std::cell::Cell;
use std::ffi::OsString;
use std::rc::Rc;

/// How many directory handles a walk keeps open at once. A directory beyond that is opened from
/// the root by its path when its turn comes, which costs the length of the path and no
/// descriptor.
const LIVE_HANDLES: usize = 64;

/// What an entry is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Node {
    /// A regular file.
    File {
        /// Its size in bytes.
        len: u64,
        /// Whether any execute bit is set (Unix).
        executable: bool,
    },
    /// A directory.
    Dir,
    /// A link, with its target as the system holds it, byte for byte.
    Link {
        /// The target.
        target: String,
    },
    /// A named pipe, a socket, a device or anything else that is not a file, a directory or a
    /// link: named, never opened. The word says what it is.
    Special(&'static str),
}

/// An entry found by the walk: its path relative to the walked root, `/`-separated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Found {
    /// The path.
    pub path: String,
    /// What it is.
    pub node: Node,
}

/// What the walk does with a directory the visitor was shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Flow {
    /// Read it.
    Descend,
    /// Leave it unread.
    Skip,
}

/// A directory handle, counted while it is held.
struct Held {
    dir: Dir,
    live: Rc<Cell<usize>>,
}

impl Held {
    fn new(dir: Dir, live: &Rc<Cell<usize>>) -> Rc<Self> {
        live.set(live.get() + 1);
        Rc::new(Self {
            dir,
            live: Rc::clone(live),
        })
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        self.live.set(self.live.get() - 1);
    }
}

/// A directory to read: its name in the parent, whose handle is held while there is room, and
/// its path from the root.
struct Pending {
    parent: Option<Rc<Held>>,
    name: String,
    path: String,
}

/// The refusal of a name that is not valid UTF-8: it names the path, the name written out
/// byte by byte, and the reason; nothing is changed.
fn not_utf8(parent: &str, raw: &OsString, what: &'static str) -> WorkspaceError {
    let written = format!("{raw:?}");
    let written = written.trim_matches('"');
    WorkspaceError::ReadFailed {
        path: bounded_message(&join(parent, written), None),
        kind: what.to_owned(),
    }
}

/// Reads a link's target, byte for byte; on Windows, where a backslash is a separator and
/// nothing else, it is written with `/` as the manifest holds it everywhere.
pub(super) fn target_of(dir: &Dir, name: &str, path: &str) -> Result<String, WorkspaceError> {
    let target = dir.read_link_contents(name).map_err(unreadable(path))?;
    let target = target
        .into_os_string()
        .into_string()
        .map_err(|raw| not_utf8(path, &raw, "the target of the link is not valid UTF-8"))?;
    #[cfg(windows)]
    let target = target.replace('\\', "/");
    Ok(target)
}

/// Walks `dir` below `start` ("" for the root), showing every entry to `visit`, parents before
/// their children; an excluded path is passed over, and a directory the visitor answers
/// [`Flow::Skip`] to is not read. The first failure ends the walk: it names the path.
pub(super) fn walk(
    dir: &Dir,
    start: &str,
    matcher: Option<&ExclusionMatcher>,
    mut visit: impl FnMut(Found) -> Result<Flow, WorkspaceError>,
) -> Result<(), WorkspaceError> {
    let live = Rc::new(Cell::new(0));
    let mut pending = vec![Pending {
        parent: None,
        name: start.to_owned(),
        path: start.to_owned(),
    }];
    while let Some(next) = pending.pop() {
        let handle = match (&next.parent, next.path.is_empty()) {
            (_, true) => dir.try_clone(),
            (Some(parent), false) => parent.dir.open_dir(&next.name),
            (None, false) => dir.open_dir(&next.path),
        }
        .map_err(unreadable(&next.path))?;
        let handle = Held::new(handle, &live);
        let mut children = Vec::new();
        for entry in handle.dir.entries().map_err(unreadable(&next.path))? {
            let entry = entry.map_err(unreadable(&next.path))?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|raw| not_utf8(&next.path, &raw, "the name is not valid UTF-8"))?;
            children.push((name, entry));
        }
        // The order of a directory is the system's; sorted, the errors and the visits repeat.
        children.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
        for (name, entry) in children {
            let path = join(&next.path, &name);
            if matcher.is_some_and(|m| is_excluded(m, &path)) {
                continue;
            }
            let file_type = entry.file_type().map_err(unreadable(&path))?;
            let node = if file_type.is_symlink() {
                Node::Link {
                    target: target_of(&handle.dir, &name, &path)?,
                }
            } else if file_type.is_dir() {
                Node::Dir
            } else if file_type.is_file() {
                let meta = entry.metadata().map_err(unreadable(&path))?;
                Node::File {
                    len: meta.len(),
                    executable: super::snapshot::is_executable(&meta),
                }
            } else {
                Node::Special(special_name(&file_type))
            };
            let is_dir = node == Node::Dir;
            let flow = visit(Found {
                path: path.clone(),
                node,
            })?;
            if is_dir && flow == Flow::Descend {
                pending.push(Pending {
                    parent: (live.get() < LIVE_HANDLES).then(|| Rc::clone(&handle)),
                    name,
                    path,
                });
            }
        }
    }
    Ok(())
}
