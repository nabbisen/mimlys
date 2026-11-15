//! A task's paths are opened, not compared (handoff 011n item 9; external
//! design §13.3; REQ-CONF-006, REQ-WS-005): the tree and the temporary
//! directory that a host tool's layer (Linux), a worker made per task (macOS)
//! or the container's entries (Windows) are given are taken apart into their
//! components, every component below the data directory must be a plain name
//! (no `..`, no `.`, no root or prefix, no separator, no drive or stream
//! mark, no trailing dot or space), and each level is **opened without
//! following a link**: on Unix `openat` with `O_NOFOLLOW | O_DIRECTORY`
//! one component at a time from a descriptor of the data directory (on Linux
//! with `O_PATH`, so that the descriptor can be handed to Landlock); on
//! Windows a handle opened with `FILE_FLAG_OPEN_REPARSE_POINT`, whose
//! attributes are checked for the reparse flag. A link anywhere below the data
//! directory is refused, naming it. The data directory itself is opened as it
//! is spelled: it is the administrator's choice, and what lies beneath is
//! mimlys's own.
//!
//! The names and the components are decided by pure functions, tested on every
//! platform.

use std::ffi::{OsStr, OsString};
use std::path::{Component, Path, PathBuf};

/// One directory, opened without following a link.
#[derive(Debug)]
pub struct OpenedDir {
    /// The path it was opened by.
    pub path: PathBuf,
    /// The descriptor (on Linux an `O_PATH` one, which Landlock takes).
    #[cfg(unix)]
    pub fd: std::os::fd::OwnedFd,
    /// The handle.
    #[cfg(windows)]
    pub handle: std::fs::File,
}

/// A task's two directories, opened.
#[derive(Debug)]
pub struct OpenedTask {
    /// `<data>/workspaces/<task>/tree`.
    pub tree: OpenedDir,
    /// A directory inside `<data>/workspaces/<task>/`: its temporary directory.
    pub tmp: OpenedDir,
}

/// Whether a path component is a plain name: something a task id, `tree` or
/// `tmp` can be, and nothing that names another place. The same on every
/// platform, so that a path is not plain on one machine and a trap on another.
pub fn is_plain_name(name: &OsStr) -> bool {
    let Some(text) = name.to_str() else {
        return false;
    };
    !text.is_empty()
        && text != "."
        && text != ".."
        && !text.contains(['/', '\\', ':', '\0'])
        && !text.ends_with(['.', ' '])
}

/// The components of `path` below `data_dir`, each a plain name, or why not.
pub fn plain_components_beneath(data_dir: &Path, path: &Path) -> Result<Vec<OsString>, String> {
    let relative = path.strip_prefix(data_dir).map_err(|_| {
        format!(
            "{} is not beneath the data directory {}",
            path.display(),
            data_dir.display()
        )
    })?;
    let mut names = Vec::new();
    for component in relative.components() {
        match component {
            Component::Normal(name) if is_plain_name(name) => names.push(name.to_owned()),
            other => {
                return Err(format!(
                    "{} holds {:?}, which is not a plain name: a task's paths are opened level by level, never compared",
                    path.display(),
                    other.as_os_str()
                ));
            }
        }
    }
    Ok(names)
}

/// What a task's two paths must be, as names below the data directory (pure):
/// the tree is `workspaces/<task>/tree`, the temporary directory is inside
/// `workspaces/<task>/`. Anything else is refused, never widened.
pub fn task_names(
    data_dir: &Path,
    tree: &Path,
    tmp: &Path,
) -> Result<(Vec<OsString>, Vec<OsString>), String> {
    let tree_names = plain_components_beneath(data_dir, tree)?;
    if tree_names.len() != 3 || tree_names[0] != "workspaces" || tree_names[2] != "tree" {
        return Err(format!("{} is not the tree of a task", tree.display()));
    }
    let tmp_names = plain_components_beneath(data_dir, tmp)?;
    if tmp_names.len() < 3 || tmp_names[..2] != tree_names[..2] {
        return Err(format!(
            "{} and {} do not belong to one task",
            tree.display(),
            tmp.display()
        ));
    }
    Ok((tree_names, tmp_names))
}

/// Opens a task's tree and temporary directory without following a link at any
/// level below the data directory (see the module's documentation). The paths
/// are checked first ([`task_names`]); both directories must exist.
pub fn open_task(data_dir: &Path, tree: &Path, tmp: &Path) -> Result<OpenedTask, String> {
    let (tree_names, tmp_names) = task_names(data_dir, tree, tmp)?;
    Ok(OpenedTask {
        tree: open_beneath(data_dir, &tree_names, tree)?,
        tmp: open_beneath(data_dir, &tmp_names, tmp)?,
    })
}

/// [`OFlags`](rustix::fs::OFlags) for one level: a directory, not a link, not
/// inherited by a child; on Linux an `O_PATH` descriptor (Landlock takes it).
#[cfg(unix)]
fn level_flags() -> rustix::fs::OFlags {
    use rustix::fs::OFlags;
    #[cfg(target_os = "linux")]
    let path = OFlags::PATH;
    #[cfg(not(target_os = "linux"))]
    let path = OFlags::empty();
    OFlags::DIRECTORY | OFlags::CLOEXEC | path
}

/// Opens `names` below `data_dir`, one component at a time, each by `openat`
/// from the descriptor of the one above with `O_NOFOLLOW`.
#[cfg(unix)]
fn open_beneath(data_dir: &Path, names: &[OsString], shown: &Path) -> Result<OpenedDir, String> {
    use rustix::fs::{AtFlags, FileType, Mode, OFlags, open, openat, statat};
    let mut dir = open(data_dir, level_flags(), Mode::empty())
        .map_err(|e| format!("cannot open the data directory {}: {e}", data_dir.display()))?;
    for name in names {
        let next = openat(
            &dir,
            name.as_os_str(),
            level_flags() | OFlags::NOFOLLOW,
            Mode::empty(),
        );
        dir = match next {
            Ok(fd) => fd,
            Err(e) => {
                let is_link = statat(&dir, name.as_os_str(), AtFlags::SYMLINK_NOFOLLOW)
                    .is_ok_and(|st| FileType::from_raw_mode(st.st_mode) == FileType::Symlink);
                return Err(if is_link {
                    format!(
                        "{} holds a link ({:?}): a task's paths are not followed",
                        shown.display(),
                        name
                    )
                } else {
                    format!(
                        "{} cannot be opened at {:?} as a directory: {e}",
                        shown.display(),
                        name
                    )
                });
            }
        };
    }
    Ok(OpenedDir {
        path: shown.to_path_buf(),
        fd: dir,
    })
}

/// Opens each level of `names` below `data_dir` as a handle that does not
/// follow a reparse point (`FILE_FLAG_OPEN_REPARSE_POINT`), and refuses a level
/// whose attributes say it is one (a junction, a symbolic link), or is not a
/// directory.
#[cfg(windows)]
fn open_beneath(data_dir: &Path, names: &[OsString], shown: &Path) -> Result<OpenedDir, String> {
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    const FILE_READ_ATTRIBUTES: u32 = 0x80;
    const FILE_SHARE_ALL: u32 = 0x7;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    // `follow` for the data directory alone: it is opened as it is spelled.
    let open = |path: &Path, follow: bool| -> Result<std::fs::File, String> {
        let flags = if follow {
            FILE_FLAG_BACKUP_SEMANTICS
        } else {
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT
        };
        let file = std::fs::OpenOptions::new()
            .access_mode(FILE_READ_ATTRIBUTES)
            .share_mode(FILE_SHARE_ALL)
            .custom_flags(flags)
            .open(path)
            .map_err(|e| format!("cannot open {}: {e}", path.display()))?;
        let attributes = file
            .metadata()
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?
            .file_attributes();
        if !follow && attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(format!(
                "{} holds a link ({}): a task's paths are not followed",
                shown.display(),
                path.display()
            ));
        }
        if attributes & FILE_ATTRIBUTE_DIRECTORY == 0 {
            return Err(format!("{} is not a directory", path.display()));
        }
        Ok(file)
    };
    let mut current = data_dir.to_path_buf();
    let mut last = open(&current, true)?;
    for name in names {
        current.push(name);
        last = open(&current, false)?;
    }
    Ok(OpenedDir {
        path: shown.to_path_buf(),
        handle: last,
    })
}

#[cfg(test)]
mod tests;
