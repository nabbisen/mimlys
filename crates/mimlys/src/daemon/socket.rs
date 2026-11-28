//! The socket, and who may reach it (external design §19.5; REQ-SEC-016; THR-39). Unix only:
//! Windows has a named pipe, which is 014d's.
//!
//! - **The socket** is `d-<hash>.sock` in the registry's directory, mode 0600. The hash is of the
//!   data directory's canonical path, so that a user with two stores has two daemons and the path
//!   fits the platform's limit for a socket.
//! - **The directory is checked by both ends** before use: owned by the user, mode 0700, not a
//!   link. So is the socket's own file.
//! - **The peer is checked by both ends:** the daemon refuses every uid but its own, and the
//!   client sends nothing to a daemon of another user. The credentials come from the system
//!   (`SO_PEERCRED` on Linux, `LOCAL_PEEREPID` and `getpeereid` on macOS, through tokio).
//! - **The daemon's own tree is refused:** a peer whose session is led by a child of the daemon
//!   (the worker, an MCP server) is its tree.

use sha2::{Digest, Sha256};
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

/// The mode of the socket's file (REQ-SEC-016).
pub const SOCKET_MODE: u32 = 0o600;
/// The mode of the socket's directory (REQ-SEC-016).
pub const DIRECTORY_MODE: u32 = 0o700;

/// The longest path `bind` accepts, in bytes: the platform's `sun_path` less its end. Not a limit
/// of the product: the system's (about 104 on macOS, 108 on Linux).
#[cfg(target_os = "macos")]
pub const SUN_PATH_MAX: usize = 103;
/// The longest path `bind` accepts, in bytes.
#[cfg(not(target_os = "macos"))]
pub const SUN_PATH_MAX: usize = 107;

/// What is wrong with the socket's directory or file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    /// It is not there.
    Missing,
    /// It is a symbolic link.
    Link,
    /// A directory was expected and is not one, or a socket and is not one.
    WrongKind,
    /// Another user owns it.
    OtherUser,
    /// Its mode is not the one required.
    Mode(u32),
}

/// Why a socket cannot be used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SocketError {
    /// The path does not fit the platform's limit for a socket.
    PathTooLong {
        /// The path.
        path: PathBuf,
        /// Its length in bytes.
        len: usize,
    },
    /// The directory is not the user's own, mode 0700, and not a link.
    Directory {
        /// The directory.
        path: PathBuf,
        /// What is wrong.
        fault: Fault,
    },
    /// The socket's file is not the user's own socket of mode 0600.
    File {
        /// The file.
        path: PathBuf,
        /// What is wrong.
        fault: Fault,
    },
    /// The process at the other end belongs to another user.
    Peer {
        /// Its uid.
        uid: u32,
    },
    /// The system failed.
    Io {
        /// What was being done.
        what: &'static str,
        /// The error's kind.
        kind: std::io::ErrorKind,
    },
}

impl std::fmt::Display for SocketError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&crate::messages::daemon_socket_refused(self))
    }
}

impl std::error::Error for SocketError {}

/// The user's effective uid.
pub fn own_uid() -> u32 {
    rustix::process::geteuid().as_raw()
}

/// The socket's file name for the data directory at `canonical` (its canonical path): `d-` and
/// twelve hexadecimal digits of the path's SHA-256, then `.sock`.
pub fn socket_name(canonical: &Path) -> String {
    let digest = Sha256::digest(canonical.as_os_str().as_encoded_bytes());
    let hash: String = digest.iter().take(6).map(|b| format!("{b:02x}")).collect();
    format!("d-{hash}.sock")
}

/// The socket's path for the data directory `data_dir`, in the registry's directory `run_dir`.
/// `None` when the data directory does not exist: no daemon can serve it. A path that does not
/// fit the platform's limit is refused, never shortened.
pub fn socket_path(run_dir: &Path, data_dir: &Path) -> Result<Option<PathBuf>, SocketError> {
    let Ok(canonical) = std::fs::canonicalize(data_dir) else {
        return Ok(None);
    };
    let path = run_dir.join(socket_name(&canonical));
    let len = path.as_os_str().as_encoded_bytes().len();
    if len > SUN_PATH_MAX {
        return Err(SocketError::PathTooLong { path, len });
    }
    Ok(Some(path))
}

fn io(what: &'static str, e: &std::io::Error) -> SocketError {
    SocketError::Io {
        what,
        kind: e.kind(),
    }
}

/// Checks the socket's directory as it is now: the user's own, mode 0700, not a link. A link is
/// looked at, not followed. Both ends do it before they use the directory.
pub fn check_directory(dir: &Path) -> Result<(), SocketError> {
    check_directory_for(dir, own_uid())
}

/// As [`check_directory`], for a user `uid` (the credentials a test injects).
pub fn check_directory_for(dir: &Path, uid: u32) -> Result<(), SocketError> {
    let fault = |fault| SocketError::Directory {
        path: dir.to_owned(),
        fault,
    };
    let meta = match std::fs::symlink_metadata(dir) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(fault(Fault::Missing)),
        Err(e) => return Err(io("looking at the socket's directory", &e)),
    };
    if meta.file_type().is_symlink() {
        return Err(fault(Fault::Link));
    }
    if !meta.is_dir() {
        return Err(fault(Fault::WrongKind));
    }
    if meta.uid() != uid {
        return Err(fault(Fault::OtherUser));
    }
    let mode = meta.permissions().mode() & 0o777;
    if mode != DIRECTORY_MODE {
        return Err(fault(Fault::Mode(mode)));
    }
    Ok(())
}

/// Checks the socket's file: a socket (not a link), the user's own, mode 0600.
pub fn check_socket_file(path: &Path) -> Result<(), SocketError> {
    check_socket_file_for(path, own_uid())
}

/// As [`check_socket_file`], for a user `uid`.
pub fn check_socket_file_for(path: &Path, uid: u32) -> Result<(), SocketError> {
    let fault = |fault| SocketError::File {
        path: path.to_owned(),
        fault,
    };
    let meta = match std::fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(fault(Fault::Missing)),
        Err(e) => return Err(io("looking at the socket", &e)),
    };
    if meta.file_type().is_symlink() {
        return Err(fault(Fault::Link));
    }
    if !meta.file_type().is_socket() {
        return Err(fault(Fault::WrongKind));
    }
    if meta.uid() != uid {
        return Err(fault(Fault::OtherUser));
    }
    let mode = meta.permissions().mode() & 0o777;
    if mode != SOCKET_MODE {
        return Err(fault(Fault::Mode(mode)));
    }
    Ok(())
}

/// The process at the other end of a connection, as the system reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Peer {
    /// Its user.
    pub uid: u32,
    /// Its process, where the system gives one: Linux and macOS both do for a connected stream
    /// socket (`SO_PEERCRED`; `LOCAL_PEEREPID`).
    pub pid: Option<u32>,
}

impl Peer {
    /// The peer a connection's credentials name.
    pub fn of(cred: &tokio::net::unix::UCred) -> Self {
        Self {
            uid: cred.uid(),
            pid: cred.pid().and_then(|p| u32::try_from(p).ok()),
        }
    }

    /// Whether the peer is the user's own: its uid is the one the process runs as.
    pub fn check(self, own: u32) -> Result<Self, SocketError> {
        if self.uid == own {
            Ok(self)
        } else {
            Err(SocketError::Peer { uid: self.uid })
        }
    }

    /// The session the daemon records for this client: the system's word for its process, never
    /// the client's own claim. Where the system gives none, the record says that it is not known.
    pub fn session_id(self) -> String {
        match self.pid {
            Some(pid) => format!("pid:{pid}"),
            None => "pid:unknown".to_owned(),
        }
    }
}

/// Whether the process `peer` is in the daemon's own tree: its session is led by one of
/// `leaders`, the processes the daemon made leaders of sessions of their own (the worker, each
/// MCP server). `Ok(false)` for a peer of no such session; an error when the peer's session
/// cannot be read (it ended, or the system refused), which the caller refuses, never serves.
pub fn in_own_tree(peer: u32, leaders: &[u32]) -> Result<bool, std::io::Error> {
    if leaders.is_empty() {
        return Ok(false);
    }
    let pid = i32::try_from(peer)
        .ok()
        .and_then(rustix::process::Pid::from_raw)
        .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    let session = rustix::process::getsid(Some(pid)).map_err(std::io::Error::from)?;
    Ok(leaders.contains(&session.as_raw_nonzero().get().cast_unsigned()))
}

#[cfg(test)]
mod tests;
