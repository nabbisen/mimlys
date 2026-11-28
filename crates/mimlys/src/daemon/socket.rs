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

/// One connection, either end.
pub type Stream = std::os::unix::net::UnixStream;
/// What a check of the daemon's own tree is asked against: the process that leads a session the
/// daemon made.
pub type Leader = u32;

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

/// Whether `error` is what macOS answers to a socket option of a socket whose peer has gone
/// (`EINVAL`, "Invalid argument"): the peer's end, not a fault of the socket. Linux answers
/// nothing of the kind; the words mean the same there, so a test can make the error.
pub fn is_peer_gone(error: &std::io::Error) -> bool {
    error.raw_os_error() == Some(rustix::io::Errno::INVAL.raw_os_error())
}

/// The outcome of setting an option of a socket, with the peer's end read as no fault: an option
/// that macOS refuses with `EINVAL` because the peer has closed is `Ok`. What the peer sent
/// before it went is still there to read, as it is on Linux; a peer that sent nothing is read as
/// the end of the stream. **Every option of a daemon's socket is set through this** (external
/// design §19.5): a user never sees `Invalid argument` for a daemon that answered and closed.
pub fn peer_end_is_not_a_fault(result: std::io::Result<()>) -> std::io::Result<()> {
    match result {
        Err(e) if is_peer_gone(&e) => Ok(()),
        other => other,
    }
}

/// Sets the read and write timeouts of `stream` ([`peer_end_is_not_a_fault`]).
pub fn set_timeouts(
    stream: &Stream,
    read: Option<std::time::Duration>,
    write: Option<std::time::Duration>,
) -> std::io::Result<()> {
    // Both are tried, so that one that the peer's end made refuse does not leave the other unset.
    let read = peer_end_is_not_a_fault(stream.set_read_timeout(read));
    let write = peer_end_is_not_a_fault(stream.set_write_timeout(write));
    read.and(write)
}

/// Sets the blocking mode of `stream` ([`peer_end_is_not_a_fault`]).
pub fn set_nonblocking(stream: &Stream, nonblocking: bool) -> std::io::Result<()> {
    peer_end_is_not_a_fault(stream.set_nonblocking(nonblocking))
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

/// The test seam of the peer check: `MIMLYS_PRETEND_UID` names the user the process takes
/// itself for **when it judges a peer** (a debug build only, as `MIMLYS_RUN_DIR` is), so that a
/// test makes a peer of "another user" without a second account. Nothing else reads it: the
/// directory's and the socket's owner are always checked against the real user.
pub const PRETEND_UID_ENV: &str = "MIMLYS_PRETEND_UID";

/// The test seam of the peer's pid: when set, **in a debug build only** (as `MIMLYS_RUN_DIR` is),
/// a peer is taken as one whose pid the system did not give, so that a test makes the case that
/// neither Linux nor macOS has (REQ-SEC-016: such a peer is refused).
pub const PRETEND_NO_PID_ENV: &str = "MIMLYS_PRETEND_NO_PID";

/// The user a peer must be: this process's own, or, in a debug build, the seam's.
pub fn user_for_peers() -> u32 {
    if cfg!(debug_assertions)
        && let Some(uid) = std::env::var(PRETEND_UID_ENV)
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
    {
        return uid;
    }
    own_uid()
}

impl Peer {
    /// The peer a connection's credentials name, **if it is the user's own**: the one way to
    /// get a peer from a connection, for the daemon (who may connect) and for the client (whose
    /// daemon it is), so that neither can use a peer it did not check.
    pub fn admit(cred: &tokio::net::unix::UCred) -> Result<Self, SocketError> {
        let hidden = cfg!(debug_assertions) && std::env::var_os(PRETEND_NO_PID_ENV).is_some();
        Self {
            uid: cred.uid(),
            pid: cred
                .pid()
                .and_then(|p| u32::try_from(p).ok())
                .filter(|_| !hidden),
        }
        .check(user_for_peers())
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

/// Where a peer stands in relation to the daemon's own tree: what the daemon does with it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tree {
    /// Its session is none the daemon made: it is served.
    Outside,
    /// Its session is one the daemon made (the worker's, an MCP server's): refused.
    Own,
    /// Its session could not be read (it ended, or the system refused): refused, never served.
    Unreadable,
    /// The system gave no process id for it: the daemon cannot tell its own tree from it, and
    /// refuses it (REQ-SEC-016). Without this arm the check would fail open. Linux and macOS both
    /// give a pid, so nothing works less; a platform that did not would fail closed.
    NoPid,
}

/// Judges `peer` against the sessions `leaders` lead. The one place that decides who is served: a
/// peer with no pid is [`Tree::NoPid`] **whatever the leaders are**, the empty set included.
pub fn judge_tree(peer: Peer, leaders: &[Leader]) -> Tree {
    let Some(pid) = peer.pid else {
        return Tree::NoPid;
    };
    match in_own_tree(pid, leaders) {
        Ok(false) => Tree::Outside,
        Ok(true) => Tree::Own,
        Err(_) => Tree::Unreadable,
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
