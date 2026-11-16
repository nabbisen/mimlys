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
//!
//! The removal of a deleted task's files ([`remove_below`], REQ-WS-012) is the same design
//! bottom-up: one handle, down by one name at a time, up by `..` (checked) or from the root, so
//! that a tree of any depth is removed.

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
        /// The permission bits, masked to 0o777 (Unix).
        mode: Option<u32>,
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
                    mode: super::snapshot::file_mode(&meta),
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

/// How a directory is told apart from the one it is in (Unix: the device and the inode).
type Identity = Option<(u64, u64)>;

/// The identity of the directory behind a handle, where the system has one.
#[cfg(unix)]
#[allow(clippy::unnecessary_cast)]
fn identity(dir: &Dir) -> Identity {
    let stat = rustix::fs::fstat(dir).ok()?;
    Some((stat.st_dev as u64, stat.st_ino as u64))
}

#[cfg(not(unix))]
fn identity(_dir: &Dir) -> Identity {
    None
}

/// A directory on the way down of [`remove_below`].
struct Level {
    /// Its name in the directory above (empty for the directory the removal starts in).
    name: OsString,
    id: Identity,
    /// Its subdirectories not yet emptied.
    subdirs: Vec<OsString>,
    /// Whether something in it or below it could not be removed (so that it is not named again
    /// when it cannot be removed because of that).
    left: bool,
}

/// What was found in a directory: its subdirectories, and the names that could not be removed
/// (empty for the directory itself, when it could not be read).
struct Scan {
    subdirs: Vec<OsString>,
    stayed: Vec<OsString>,
}

/// A name for a message: as it is, or written out byte by byte when it is not valid UTF-8.
fn shown(name: &std::ffi::OsStr) -> String {
    match name.to_str() {
        Some(text) => text.to_owned(),
        None => format!("{name:?}").trim_matches('"').to_owned(),
    }
}

/// The path of `leaf` (the directory itself when empty) in the levels, from where the removal
/// starts, `/`-separated.
fn path_in(levels: &[Level], leaf: &std::ffi::OsStr) -> String {
    let mut path = String::new();
    for name in levels
        .iter()
        .skip(1)
        .map(|l| l.name.as_os_str())
        .chain(std::iter::once(leaf))
        .filter(|n| !n.is_empty())
    {
        if !path.is_empty() {
            path.push('/');
        }
        path.push_str(&shown(name));
    }
    path
}

/// Makes the directory `name` of `dir` readable, writable and searchable by its owner, so that
/// it can be opened, emptied and removed (a toolchain may leave read-only ones). A failure is
/// not an error here: the open that follows says what could not be done.
fn make_dir_accessible(dir: &Dir, name: &std::ffi::OsStr) {
    #[cfg(unix)]
    {
        use cap_std::fs::PermissionsExt;
        let Ok(meta) = dir.symlink_metadata(name) else {
            return;
        };
        let mode = meta.permissions().mode();
        if meta.is_dir() && mode & 0o700 != 0o700 {
            let wanted =
                rustix::fs::Mode::from_bits_truncate((mode | 0o700) as rustix::fs::RawMode);
            let _ = rustix::fs::chmodat(dir, name, wanted, rustix::fs::AtFlags::empty());
        }
    }
    #[cfg(not(unix))]
    {
        if let Ok(meta) = dir.symlink_metadata(name) {
            let mut perms = meta.permissions();
            if meta.is_dir() && perms.readonly() {
                #[allow(clippy::permissions_set_readonly_false)]
                perms.set_readonly(false);
                let _ = dir.set_permissions(name, perms);
            }
        }
    }
}

/// Removes one entry that is not a directory (a file, a link, a named pipe, a socket, a device),
/// never following a link. A read-only file is made writable once and tried again (Windows).
fn remove_entry(
    dir: &Dir,
    name: &std::ffi::OsStr,
    kind: &cap_std::fs::FileType,
) -> std::io::Result<()> {
    let first = match dir.remove_file(name) {
        Ok(()) => return Ok(()),
        Err(e) => e,
    };
    if dir.symlink_metadata(name).is_err() {
        return Ok(()); // already gone
    }
    if kind.is_symlink() {
        // A link to a directory is removed as a directory on Windows.
        return dir.remove_dir(name).or(Err(first));
    }
    #[cfg(not(unix))]
    if let Ok(meta) = dir.symlink_metadata(name) {
        let mut perms = meta.permissions();
        if perms.readonly() {
            #[allow(clippy::permissions_set_readonly_false)]
            perms.set_readonly(false);
            if dir.set_permissions(name, perms).is_ok() && dir.remove_file(name).is_ok() {
                return Ok(());
            }
        }
    }
    Err(first)
}

/// Reads `dir` and removes every entry in it that is not a directory.
fn scan(dir: &Dir) -> Scan {
    let mut found = Scan {
        subdirs: Vec::new(),
        stayed: Vec::new(),
    };
    let Ok(entries) = dir.entries() else {
        found.stayed.push(OsString::new());
        return found;
    };
    for entry in entries {
        let Ok(entry) = entry else {
            found.stayed.push(OsString::new());
            continue;
        };
        let name = entry.file_name();
        match entry.file_type() {
            Ok(kind) if kind.is_dir() => found.subdirs.push(name),
            Ok(kind) => {
                if remove_entry(dir, &name, &kind).is_err() {
                    found.stayed.push(name);
                }
            }
            Err(_) => found.stayed.push(name),
        }
    }
    found.subdirs.sort();
    found
}

/// The directory above `cur`; `levels` are the levels from the root down to it, `cur`'s own
/// removed, so the last of them is the one to reach. Through `..` where the system has it,
/// checked to be the directory that was left, and otherwise from the root, name by name. Never
/// by a path, so that its length is never a limit.
fn climb(root: &Dir, cur: &Dir, levels: &[Level]) -> std::io::Result<Dir> {
    #[cfg(unix)]
    {
        use rustix::fs::{Mode, OFlags, openat};
        if let Ok(fd) = openat(
            cur,
            "..",
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        ) {
            let above = Dir::from_std_file(std::fs::File::from(fd));
            if let Some(last) = levels.last()
                && identity(&above) == last.id
            {
                return Ok(above);
            }
        }
    }
    let _ = cur;
    let mut above = root.try_clone()?;
    for level in levels.iter().skip(1) {
        above = above.open_dir(&level.name)?;
    }
    Ok(above)
}

/// Removes everything below `root`, which stays, without following a link and attempting every
/// removal: what could not be removed is added to `stayed` by its path from `root` (empty for
/// `root` itself, when it could not be read), `/`-separated. A directory is made accessible
/// before it is emptied. A directory that stays because something in it stayed is not named
/// again; the cause is.
///
/// The walk is iterative, like [`walk`], and reads each directory through the handle of the one
/// above it, so that a tree deeper than the stack, or than a path may be, is removed: it holds
/// one handle, goes down by `open_dir` of one name, and goes up by [`climb`] after the
/// directory is empty. Unlike [`walk`] it is bottom-up (a directory goes after its contents), it
/// goes on past a failure, and it takes a name that is not valid UTF-8 as it is.
pub(super) fn remove_below(root: &Dir, stayed: &mut Vec<String>) {
    let Ok(mut cur) = root.try_clone() else {
        stayed.push(String::new());
        return;
    };
    let first = scan(&cur);
    let mut levels = vec![Level {
        name: OsString::new(),
        id: identity(&cur),
        subdirs: first.subdirs,
        left: !first.stayed.is_empty(),
    }];
    for name in &first.stayed {
        stayed.push(path_in(&levels, name));
    }
    loop {
        let next = levels
            .last_mut()
            .expect("the root level stays")
            .subdirs
            .pop();
        if let Some(name) = next {
            make_dir_accessible(&cur, &name);
            match cur.open_dir(&name) {
                Ok(child) => {
                    let found = scan(&child);
                    levels.push(Level {
                        name,
                        id: identity(&child),
                        subdirs: found.subdirs,
                        left: !found.stayed.is_empty(),
                    });
                    for name in &found.stayed {
                        stayed.push(path_in(&levels, name));
                    }
                    cur = child;
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => {
                    stayed.push(path_in(&levels, &name));
                    levels.last_mut().expect("the root level stays").left = true;
                }
            }
            continue;
        }
        if levels.len() == 1 {
            return;
        }
        let done = levels.pop().expect("a level above the root");
        let Ok(above) = climb(root, &cur, &levels) else {
            // Neither `..` nor the names from the root lead back: what is still to do above
            // cannot be reached. Name the directory we are in, and what is waiting.
            stayed.push(path_in(&levels, &done.name));
            for at in 0..levels.len() {
                for waiting in &levels[at].subdirs {
                    stayed.push(path_in(&levels[..=at], waiting));
                }
            }
            return;
        };
        cur = above;
        if cur.remove_dir(&done.name).is_err() && cur.symlink_metadata(&done.name).is_ok() {
            if !done.left {
                stayed.push(path_in(&levels, &done.name));
            }
            levels.last_mut().expect("a level above").left = true;
        } else if done.left {
            levels.last_mut().expect("a level above").left = true;
        }
    }
}
