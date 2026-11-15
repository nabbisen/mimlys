//! Opening a workspace path for a tool (external design §11; REQ-TOOL-003, REQ-TOOL-006).
//!
//! A tool runs on a blocking thread that the runtime cannot take back: an open that
//! waits (a named pipe with no writer, a device) would keep the process from exiting
//! long after the action's deadline. So every tool opens a path the same way:
//!
//! 1. the path is looked at first (`stat`), and anything that is not a regular file
//!    is refused without being opened;
//! 2. it is opened without blocking (`O_NONBLOCK` on Unix) and looked at again
//!    through the handle (`fstat`), which is what decides when the file changed
//!    in between;
//! 3. what is read from it is bounded by the caller.
//!
//! On Windows there is no flag to ask for: a workspace tree holds no named pipe
//! or device the tools could be given, and the path rules refuse the reserved
//! device names; the check through the handle (step 2) refuses anything that is
//! not a file all the same.

use super::ToolError;
use cap_std::fs::{Dir, File, FileType, OpenOptions};
#[cfg(unix)]
use cap_std::fs::{FileTypeExt, OpenOptionsExt};
use serde_json::json;
use std::io::Read;

/// What a tool does with the file it opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Access {
    /// Reads it; the path must exist.
    Read,
    /// Writes it, creating it when it is missing; its content is not touched by the open.
    Write,
}

/// Why a path was not opened.
#[derive(Debug)]
pub(crate) enum OpenError {
    /// A directory, or anything else that is not a regular file; the failure is final
    /// and has its words and its hint.
    Refused(ToolError),
    /// The operating system refused the open (or the look at the path).
    Io(std::io::Error),
}

impl OpenError {
    /// The tool's failure: a refusal as it is, an operating-system error as `io` maps it.
    pub(crate) fn into_tool(self, io: impl FnOnce(&std::io::Error) -> ToolError) -> ToolError {
        match self {
            OpenError::Refused(error) => error,
            OpenError::Io(error) => io(&error),
        }
    }
}

/// What a file type is, for a tool.
enum Class {
    Regular,
    Directory,
    /// A named pipe, a socket, a device, a link that was not followed, or anything else.
    Other(&'static str),
}

fn classify(file_type: &FileType) -> Class {
    if file_type.is_file() {
        return Class::Regular;
    }
    if file_type.is_dir() {
        return Class::Directory;
    }
    if file_type.is_symlink() {
        return Class::Other("symlink");
    }
    #[cfg(unix)]
    {
        if file_type.is_fifo() {
            return Class::Other("fifo");
        }
        if file_type.is_socket() {
            return Class::Other("socket");
        }
        if file_type.is_block_device() || file_type.is_char_device() {
            return Class::Other("device");
        }
    }
    Class::Other("other")
}

/// The refusal for a path that is a directory.
pub(crate) fn is_directory() -> ToolError {
    ToolError {
        detail: "is_directory",
        payload: json!({ "hint": "this is a directory; use list_dir to see what it holds" }),
    }
}

/// The refusal for a path that is neither a regular file nor a directory.
pub(crate) fn not_a_file(kind: &'static str) -> ToolError {
    ToolError {
        detail: "not_a_file",
        payload: json!({
            "type": kind,
            "hint": "only regular files can be read or written; use list_dir to see what the directory holds",
        }),
    }
}

fn refuse(class: Class) -> Result<(), OpenError> {
    match class {
        Class::Regular => Ok(()),
        Class::Directory => Err(OpenError::Refused(is_directory())),
        Class::Other(kind) => Err(OpenError::Refused(not_a_file(kind))),
    }
}

/// Opens the regular file at `rel` (a path relative to the workspace root) without blocking.
pub(crate) fn open_regular(dir: &Dir, rel: &str, access: Access) -> Result<File, OpenError> {
    match dir.metadata(rel) {
        Ok(meta) => refuse(classify(&meta.file_type()))?,
        // A file to be written may not exist yet; the open creates it.
        Err(error) if access == Access::Write && error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(OpenError::Io(error)),
    }
    let file = open_handle(dir, rel, access).map_err(OpenError::Io)?;
    let meta = file.metadata().map_err(OpenError::Io)?;
    refuse(classify(&meta.file_type()))?;
    Ok(file)
}

/// The open itself: it never waits for the other end of a named pipe (`O_NONBLOCK`), and
/// does not truncate; the caller looks at what it got.
pub(crate) fn open_handle(dir: &Dir, rel: &str, access: Access) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    match access {
        Access::Read => options.read(true),
        Access::Write => options.write(true).create(true),
    };
    #[cfg(unix)]
    options.custom_flags(rustix::fs::OFlags::NONBLOCK.bits() as i32);
    dir.open_with(rel, &options)
}

/// Why a bounded read stopped.
#[derive(Debug)]
pub(crate) enum ReadError {
    /// The file holds more than the bound.
    TooLarge,
    /// The operating system failed the read.
    Io(std::io::Error),
}

/// Reads at most `limit` bytes of `file`: a file that holds more is `TooLarge`, and the
/// read does not go on past `limit + 1` bytes, whatever the file grows to.
pub(crate) fn read_bounded(file: File, limit: usize) -> Result<Vec<u8>, ReadError> {
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(ReadError::Io)?;
    if bytes.len() > limit {
        return Err(ReadError::TooLarge);
    }
    Ok(bytes)
}
