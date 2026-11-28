//! The pipe, and who may reach it (external design §19.5, "Windows"; REQ-SEC-016; THR-39): the
//! Windows counterpart of [`super::socket`], which it matches name for name where the rest of the
//! daemon uses it (`plat`). No `unsafe` here: every foreign call is `mimlys_exec::daemon_pipe`'s.
//!
//! - **The pipe** is `\\.\pipe\mimlys-d-<hash>` of the user's SID and the data directory's
//!   canonical path ([`pipe_name`]). The daemon creates it as its first instance, with a DACL for
//!   the user's SID alone (`mimlys_exec::daemon_pipe::PipeListener`).
//! - **The peer is checked by both ends:** the daemon reads the client's process from the pipe and
//!   refuses a user other than its own; the client reads the **server's** process before it sends a
//!   word and refuses a server that is not the user's (a squatter). A process that cannot be
//!   opened is refused.
//! - **The daemon's own tree is refused:** a peer in the worker's Job Object, or in the one the
//!   daemon made for a tool server, is its tree.

use mimlys_exec::daemon_pipe::{self, JobRef};
use std::path::Path;

/// One connection, either end.
pub type Stream = daemon_pipe::PipeStream;
/// What a check of the daemon's own tree is asked against: a Job Object the daemon made.
pub type Leader = JobRef;

/// The test seam of the peer check: `MIMLYS_PRETEND_SID` names the SID the process takes itself
/// for **when it judges a peer** (a debug build only, as `MIMLYS_RUN_DIR` is), so that a test makes
/// a peer of "another user" without a second account. Nothing else reads it: the pipe's name and
/// its DACL are always the real user's.
pub const PRETEND_SID_ENV: &str = "MIMLYS_PRETEND_SID";

/// The test seam of the peer's pid, as on Unix: a peer is taken as one whose pid the system did not
/// give (a debug build only).
pub const PRETEND_NO_PID_ENV: &str = "MIMLYS_PRETEND_NO_PID";

/// Why a pipe cannot be used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SocketError {
    /// The process at the other end belongs to another user.
    Peer {
        /// Its user's SID.
        user: String,
    },
    /// The system failed, or would not say.
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

fn io(what: &'static str, e: &std::io::Error) -> SocketError {
    SocketError::Io {
        what,
        kind: e.kind(),
    }
}

/// The user's SID.
pub fn own_sid() -> Result<String, SocketError> {
    daemon_pipe::current_user_sid().map_err(|e| io("reading the user's SID", &e))
}

/// The pipe's name for the data directory `data_dir`. `None` when it does not exist: no daemon can
/// serve it.
pub fn pipe_name(data_dir: &Path) -> Result<Option<String>, SocketError> {
    let Ok(canonical) = std::fs::canonicalize(data_dir) else {
        return Ok(None);
    };
    Ok(Some(crate::daemon::pipe_name::pipe_name(
        &own_sid()?,
        &canonical,
    )))
}

/// The user a peer must be: this process's own, or, in a debug build, the seam's.
pub fn user_for_peers() -> Result<String, SocketError> {
    if cfg!(debug_assertions)
        && let Ok(sid) = std::env::var(PRETEND_SID_ENV)
        && !sid.is_empty()
    {
        return Ok(sid);
    }
    own_sid()
}

/// The process at the other end of a connection, as the system reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Peer {
    /// Its process, where the system gives one: the pipe always does.
    pub pid: Option<u32>,
}

impl Peer {
    /// The peer a connection names, **if it is the user's own**: the one way to get a peer from a
    /// connection, for the daemon (who may connect) and for the client (whose daemon it is), so
    /// that neither can use a peer it did not check. The daemon's end reads the client's process,
    /// the client's end the server's.
    pub fn admit(stream: &Stream) -> Result<Self, SocketError> {
        let pid = stream
            .peer_process_id()
            .map_err(|e| io("reading the process at the other end of the pipe", &e))?;
        let user = daemon_pipe::process_user_sid(pid)
            .map_err(|e| io("reading the user of the process at the other end", &e))?;
        if user != user_for_peers()? {
            return Err(SocketError::Peer { user });
        }
        let hidden = cfg!(debug_assertions) && std::env::var_os(PRETEND_NO_PID_ENV).is_some();
        Ok(Self {
            pid: (!hidden).then_some(pid),
        })
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
    /// It is in none of the daemon's jobs: it is served.
    Outside,
    /// It is in the worker's job or a tool server's: refused.
    Own,
    /// Its jobs could not be read (it ended, or the system refused): refused, never served.
    Unreadable,
    /// No process id: the daemon cannot tell its own tree from it, and refuses it.
    NoPid,
}

/// Judges `peer` against the jobs `leaders` the daemon made. The one place that decides who is
/// served: a peer with no pid is [`Tree::NoPid`] whatever the leaders are.
pub fn judge_tree(peer: Peer, leaders: &[Leader]) -> Tree {
    let Some(pid) = peer.pid else {
        return Tree::NoPid;
    };
    match daemon_pipe::process_in_any_job(pid, leaders) {
        Ok(false) => Tree::Outside,
        Ok(true) => Tree::Own,
        Err(_) => Tree::Unreadable,
    }
}

/// Sets the read and write timeouts of `stream`.
pub fn set_timeouts(
    stream: &Stream,
    read: Option<std::time::Duration>,
    write: Option<std::time::Duration>,
) -> std::io::Result<()> {
    stream.set_read_timeout(read)?;
    stream.set_write_timeout(write)
}
