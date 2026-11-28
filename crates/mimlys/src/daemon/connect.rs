//! Reaching the daemon, from a client (external design §19.5; REQ-SEC-016, REQ-CLI-008).
//!
//! A client finds the daemon by its socket (on Windows, its pipe), and sends nothing before it has
//! checked: on Unix the registry's directory (the user's own, mode 0700, not a link) and the
//! socket's file (the user's own socket, mode 0600); on both the process at the other end (the
//! user's own: a daemon of another user, or a server that squats the pipe's name, is refused before
//! the client says a word). Then the hellos: the daemon's first, the client's
//! after it, with the keyed hash of each secret under the daemon's salt, and the daemon's answer.
//!
//! **A dead daemon.** A socket that refuses the connection, or does not answer within LIM-O14,
//! while the store's lock is free, is a daemon that is gone: the client removes the socket and
//! says so once. With the lock held the daemon is starting, or is another runtime, and nothing
//! is removed.

use crate::config::Loaded;
use crate::daemon::plat::{self, Peer, SocketError, Stream};
use crate::daemon::wire::{self, ClientHello, DaemonHello, HelloAnswer};
use mimlys_core::limits::LIM_O14_DAEMON_CONNECT_MS;
use mimlys_exec::frame::FrameError;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// A connection that has greeted: the daemon's hello and its answer to the client's.
pub struct Daemon {
    /// The connection, with no timeout set.
    pub stream: Stream,
    /// What the daemon said first.
    pub hello: DaemonHello,
    /// What it answered to this client's hello.
    pub answer: HelloAnswer,
    /// The daemon's process, as the system reports it.
    pub peer: Peer,
    /// The socket (on Windows, the pipe's name).
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
    Stream(Stream, Peer),
    /// Refused, or not answered within LIM-O14: nobody is there.
    Silent,
    /// There is no pipe of that name (Windows: a pipe lasts as long as its process, so a name that
    /// is not there is no daemon, and there is no stale socket to remove).
    #[cfg(windows)]
    Absent,
    Refused(SocketError),
}

/// Connects to the pipe within LIM-O14 and reads the server's process: the client sends nothing to a
/// server that is not the user's own.
#[cfg(windows)]
fn connect(path: &Path) -> Connected {
    let within = Duration::from_millis(LIM_O14_DAEMON_CONNECT_MS);
    match Stream::connect(&path.to_string_lossy(), within) {
        Ok(stream) => {
            let peer = Peer {
                pid: stream.peer_process_id().ok(),
            };
            Connected::Stream(stream, peer)
        }
        Err(e) => match e.kind() {
            std::io::ErrorKind::NotFound => Connected::Absent,
            // Every instance stayed busy for LIM-O14: nobody answers.
            std::io::ErrorKind::TimedOut => Connected::Silent,
            kind => Connected::Refused(SocketError::Io {
                what: "connecting to the daemon",
                kind,
            }),
        },
    }
}

/// Connects to the socket within LIM-O14 and reads the peer's credentials. The tokio runtime of
/// one thread makes the connection, so that the call is made from anywhere (inside an executor,
/// or none).
#[cfg(unix)]
fn connect(path: &Path) -> Connected {
    let path = path.to_owned();
    let work =
        std::thread::Builder::new()
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
                    let stream =
                        match tokio::time::timeout(wait, tokio::net::UnixStream::connect(&path))
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
                        // A daemon that has gone already (macOS answers `EINVAL`) is not there.
                        Err(e) if plat::is_peer_gone(&e) => return Connected::Silent,
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
                        Ok(std) if plat::set_nonblocking(&std, false).is_ok() => {
                            Connected::Stream(std, peer)
                        }
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
    mut stream: Stream,
    peer: Peer,
    socket: PathBuf,
    loaded: &Loaded,
) -> Result<Daemon, ReachError> {
    let timeout = Some(Duration::from_millis(LIM_O14_DAEMON_CONNECT_MS));
    let io = |e: std::io::Error| ReachError::Hello(e.to_string());
    plat::set_timeouts(&stream, timeout, timeout).map_err(io)?;
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
    // already (a peer that has gone is no fault of the option: `socket::set_timeouts`).
    let _ = plat::set_timeouts(&stream, None, None);
    Ok(Daemon {
        stream,
        hello,
        answer,
        peer,
        socket,
    })
}

/// Where the daemon of `data_dir` would be, checked as far as a client may before it connects:
/// `None` when there can be no daemon (no data directory; on Unix, no socket, and a path that does
/// not fit the platform's limit holds none either).
#[cfg(unix)]
fn locate(run_dir: &Path, data_dir: &Path) -> Result<Option<PathBuf>, ReachError> {
    // Where the socket would be. A path that does not fit the platform's limit holds no socket
    // (the daemon could not bind one there): there is no daemon, and a command that only looks
    // for one goes on; `daemon start` says why it cannot start one there.
    let path = match plat::socket_path(run_dir, data_dir) {
        Ok(Some(path)) => path,
        Ok(None) | Err(SocketError::PathTooLong { .. }) => return Ok(None),
        Err(e) => return Err(ReachError::Socket(e)),
    };
    // No socket, no daemon, and nothing of the directory is used. A socket that is there is
    // looked at, and so is its directory, before anything is sent (REQ-SEC-016).
    match std::fs::symlink_metadata(&path) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(ReachError::Socket(SocketError::Io {
                what: "looking at the socket",
                kind: e.kind(),
            }));
        }
    }
    plat::check_directory(run_dir).map_err(ReachError::Socket)?;
    plat::check_socket_file(&path).map_err(ReachError::Socket)?;
    Ok(Some(path))
}

/// Where the daemon of `data_dir` would be: its pipe, named from the user's SID and the data
/// directory. There is no file to look at: the connect says whether the pipe is there.
#[cfg(windows)]
fn locate(_run_dir: &Path, data_dir: &Path) -> Result<Option<PathBuf>, ReachError> {
    plat::pipe_name(data_dir)
        .map(|name| name.map(PathBuf::from))
        .map_err(ReachError::Socket)
}

/// Looks for the daemon of `data_dir` in the registry's directory `run_dir`, and greets it. With
/// `remove_dead`, the socket of a dead daemon is removed.
pub fn reach(
    run_dir: &Path,
    data_dir: &Path,
    loaded: &Loaded,
    remove_dead: bool,
) -> Result<Reach, ReachError> {
    let Some(path) = locate(run_dir, data_dir)? else {
        return Ok(Reach::None);
    };
    match connect(&path) {
        Connected::Stream(stream, peer) => {
            greet(stream, peer, path, loaded).map(|d| Reach::Daemon(Box::new(d)))
        }
        Connected::Refused(e) => Err(ReachError::Socket(e)),
        #[cfg(windows)]
        Connected::Absent => Ok(Reach::None),
        Connected::Silent => {
            // With the store's lock free nobody holds the store: the daemon is gone.
            let free = mimlys_persist::open_read_only(data_dir)
                .map(|store| store.probe_lock() == mimlys_persist::LockState::Free)
                .unwrap_or(false);
            if !free {
                return Ok(Reach::Unready);
            }
            // (A pipe lasts as long as its process: there is no file of a dead daemon to remove.)
            #[cfg(unix)]
            if remove_dead {
                let _ = std::fs::remove_file(&path);
            }
            #[cfg(windows)]
            let _ = remove_dead;
            Ok(Reach::Dead)
        }
    }
}

#[cfg(all(test, unix))]
mod tests;
