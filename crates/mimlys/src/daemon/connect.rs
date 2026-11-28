//! Reaching the daemon, from a client (external design §19.5; REQ-SEC-016, REQ-CLI-008).
//!
//! A client finds the daemon by its socket, and sends nothing before it has checked: the
//! registry's directory (the user's own, mode 0700, not a link), the socket's file (the user's own
//! socket, mode 0600), and the process at the other end (the user's own: a daemon of another user
//! is refused before the client says a word). Then the hellos: the daemon's first, the client's
//! after it, with the keyed hash of each secret under the daemon's salt, and the daemon's answer.
//!
//! **A dead daemon.** A socket that refuses the connection, or does not answer within LIM-O14,
//! while the store's lock is free, is a daemon that is gone: the client removes the socket and
//! says so once. With the lock held the daemon is starting, or is another runtime, and nothing
//! is removed.

use crate::config::Loaded;
use crate::daemon::socket::{self, Peer, SocketError};
use crate::daemon::wire::{self, ClientHello, DaemonHello, HelloAnswer};
use mimlys_core::limits::LIM_O14_DAEMON_CONNECT_MS;
use mimlys_exec::frame::FrameError;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// A connection that has greeted: the daemon's hello and its answer to the client's.
pub struct Daemon {
    /// The connection, with no timeout set.
    pub stream: UnixStream,
    /// What the daemon said first.
    pub hello: DaemonHello,
    /// What it answered to this client's hello.
    pub answer: HelloAnswer,
    /// The daemon's process, as the system reports it.
    pub peer: Peer,
    /// The socket.
    pub socket: PathBuf,
}

impl Daemon {
    /// Whether this client and this daemon talk: they are of one runtime version and the daemon
    /// accepted the connection. A connection that is not usable can still `stop` the daemon.
    pub fn usable(&self) -> bool {
        self.answer.accepted && self.hello.runtime_version == crate::daemon::wire::runtime_version()
    }
}

/// What looking for a daemon found.
pub enum Reach {
    /// There is none: no directory, no socket.
    None,
    /// There was a socket and no daemon behind it: the socket was removed (when the caller asked
    /// for that), and the client says `daemon not running` once.
    Dead,
    /// A socket that does not answer, the store's lock held: a daemon that is starting, or
    /// another runtime. Nothing was removed.
    Unready,
    /// A daemon, greeted.
    Daemon(Box<Daemon>),
}

/// Why a daemon could not be reached though there may be one.
#[derive(Debug)]
pub enum ReachError {
    /// The directory, the socket or the peer is not the user's own, or the path does not fit.
    Socket(SocketError),
    /// What answered did not greet as a mimlys daemon does.
    Hello(String),
}

impl std::fmt::Display for ReachError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReachError::Socket(e) => write!(f, "{e}"),
            ReachError::Hello(why) => write!(f, "{}", crate::messages::daemon_hello_failed(why)),
        }
    }
}

impl std::error::Error for ReachError {}

/// How a connect ended.
enum Connected {
    Stream(UnixStream, Peer),
    /// Refused, or not answered within LIM-O14: nobody is there.
    Silent,
    Refused(SocketError),
}

/// Connects to the socket within LIM-O14 and reads the peer's credentials. The tokio runtime of
/// one thread makes the connection, so that the call is made from anywhere (inside an executor,
/// or none).
fn connect(path: &Path) -> Connected {
    let path = path.to_owned();
    let work = std::thread::Builder::new()
        .name("daemon-connect".into())
        .spawn(move || {
            let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                return Connected::Silent;
            };
            runtime.block_on(async {
                let wait = Duration::from_millis(LIM_O14_DAEMON_CONNECT_MS);
                let stream = match tokio::time::timeout(
                    wait,
                    tokio::net::UnixStream::connect(&path),
                )
                .await
                {
                    Ok(Ok(stream)) => stream,
                    Ok(Err(e)) => {
                        return match e.kind() {
                            std::io::ErrorKind::ConnectionRefused
                            | std::io::ErrorKind::NotFound => Connected::Silent,
                            kind => Connected::Refused(SocketError::Io {
                                what: "connecting to the daemon",
                                kind,
                            }),
                        };
                    }
                    Err(_) => return Connected::Silent,
                };
                let cred = match stream.peer_cred() {
                    Ok(cred) => cred,
                    Err(e) => {
                        return Connected::Refused(SocketError::Io {
                            what: "reading the daemon's credentials",
                            kind: e.kind(),
                        });
                    }
                };
                // The client sends nothing to a daemon of another user.
                let peer = match Peer::admit(&cred) {
                    Ok(peer) => peer,
                    Err(e) => return Connected::Refused(e),
                };
                match stream.into_std() {
                    Ok(std) if std.set_nonblocking(false).is_ok() => Connected::Stream(std, peer),
                    _ => Connected::Silent,
                }
            })
        });
    match work.map(std::thread::JoinHandle::join) {
        Ok(Ok(connected)) => connected,
        _ => Connected::Silent,
    }
}

fn hello_error(error: &FrameError) -> ReachError {
    ReachError::Hello(error.to_string())
}

/// The greeting, on a connection that was checked: the daemon's hello, the client's, the answer.
fn greet(
    mut stream: UnixStream,
    peer: Peer,
    socket: PathBuf,
    loaded: &Loaded,
) -> Result<Daemon, ReachError> {
    let timeout = Some(Duration::from_millis(LIM_O14_DAEMON_CONNECT_MS));
    let io = |e: std::io::Error| ReachError::Hello(e.to_string());
    stream.set_read_timeout(timeout).map_err(io)?;
    stream.set_write_timeout(timeout).map_err(io)?;
    let hello: DaemonHello = wire::read(&mut stream).map_err(|e| hello_error(&e))?;
    if hello.protocol != wire::PROTOCOL || hello.hello_version != wire::HELLO_VERSION {
        return Err(ReachError::Hello(format!(
            "it says {} {}, not {} {}",
            hello.protocol,
            hello.hello_version,
            wire::PROTOCOL,
            wire::HELLO_VERSION
        )));
    }
    let secrets = loaded
        .secrets
        .iter()
        .map(|(name, value)| (name.clone(), wire::keyed_hash(&hello.salt, name, value)))
        .collect();
    wire::write(
        &mut stream,
        &ClientHello {
            runtime_version: crate::daemon::wire::runtime_version().to_owned(),
            config_hash: wire::config_hash(loaded.path.as_deref()),
            secrets,
        },
    )
    .map_err(|e| hello_error(&e))?;
    let answer: HelloAnswer = wire::read(&mut stream).map_err(|e| hello_error(&e))?;
    // Clearing the timeouts is a courtesy: a daemon that refused this client has closed its end
    // already, and macOS answers `EINVAL` to a socket option of a socket whose peer has gone.
    let _ = stream.set_read_timeout(None);
    let _ = stream.set_write_timeout(None);
    Ok(Daemon {
        stream,
        hello,
        answer,
        peer,
        socket,
    })
}

/// Looks for the daemon of `data_dir` in the registry's directory `run_dir`, and greets it. With
/// `remove_dead`, the socket of a dead daemon is removed.
pub fn reach(
    run_dir: &Path,
    data_dir: &Path,
    loaded: &Loaded,
    remove_dead: bool,
) -> Result<Reach, ReachError> {
    // Where the socket would be. A path that does not fit the platform's limit holds no socket
    // (the daemon could not bind one there): there is no daemon, and a command that only looks
    // for one goes on; `daemon start` says why it cannot start one there.
    let path = match socket::socket_path(run_dir, data_dir) {
        Ok(Some(path)) => path,
        Ok(None) | Err(SocketError::PathTooLong { .. }) => return Ok(Reach::None),
        Err(e) => return Err(ReachError::Socket(e)),
    };
    // No socket, no daemon, and nothing of the directory is used. A socket that is there is
    // looked at, and so is its directory, before anything is sent (REQ-SEC-016).
    match std::fs::symlink_metadata(&path) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Reach::None),
        Err(e) => {
            return Err(ReachError::Socket(SocketError::Io {
                what: "looking at the socket",
                kind: e.kind(),
            }));
        }
    }
    socket::check_directory(run_dir).map_err(ReachError::Socket)?;
    socket::check_socket_file(&path).map_err(ReachError::Socket)?;
    match connect(&path) {
        Connected::Stream(stream, peer) => {
            greet(stream, peer, path, loaded).map(|d| Reach::Daemon(Box::new(d)))
        }
        Connected::Refused(e) => Err(ReachError::Socket(e)),
        Connected::Silent => {
            // With the store's lock free nobody holds the store: the daemon is gone.
            let free = mimlys_persist::open_read_only(data_dir)
                .map(|store| store.probe_lock() == mimlys_persist::LockState::Free)
                .unwrap_or(false);
            if !free {
                return Ok(Reach::Unready);
            }
            if remove_dead {
                let _ = std::fs::remove_file(&path);
            }
            Ok(Reach::Dead)
        }
    }
}

#[cfg(test)]
mod tests;
