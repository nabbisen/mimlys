//! The daemon's words (external design §19.5, §20; REQ-SEC-016, REQ-CLI-008): each line of §20's
//! rows for the daemon, and the lines of `mimlys daemon`. They name the thing and what to do next.

#[cfg(unix)]
use crate::daemon::socket::{Fault, SocketError};

/// A socket, its directory or its peer that cannot be used: refused before anything was sent or
/// read (§20: "a peer of another user, or a socket directory that is not the user's own, mode
/// 0700"; "a path past the platform's limit").
#[cfg(unix)]
pub fn daemon_socket_refused(error: &SocketError) -> String {
    let what = |fault: &Fault, thing: &str| match fault {
        Fault::Missing => format!("{thing} is not there"),
        Fault::Link => format!("{thing} is a symbolic link"),
        Fault::WrongKind => format!("{thing} is not what it should be (a directory, or a socket)"),
        Fault::OtherUser => format!("{thing} belongs to another user"),
        Fault::Mode(mode) => format!(
            "{thing} has mode {mode:04o}, not the {} the daemon's socket needs",
            if thing.starts_with("the socket's directory") {
                "0700"
            } else {
                "0600"
            }
        ),
    };
    match error {
        SocketError::PathTooLong { path, len } => format!(
            "the daemon's socket path {} is {len} bytes, past the {} this platform allows for a Unix socket; it is never shortened. Use a shorter runtime directory (`XDG_RUNTIME_DIR` on Linux)",
            path.display(),
            crate::daemon::socket::SUN_PATH_MAX
        ),
        SocketError::Directory { path, fault } => format!(
            "{}: {}; nothing was sent or read",
            path.display(),
            what(fault, "the socket's directory")
        ),
        SocketError::File { path, fault } => format!(
            "{}: {}; nothing was sent or read",
            path.display(),
            what(fault, "the socket")
        ),
        SocketError::Peer { uid } => format!(
            "the process at the other end of the daemon's socket belongs to another user (uid {uid}); nothing was sent or read"
        ),
        SocketError::Io { what, kind } => {
            format!("the daemon's socket cannot be used: {what} failed ({kind})")
        }
    }
}
