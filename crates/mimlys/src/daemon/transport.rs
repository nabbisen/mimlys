//! The socket behind the same client (RFC 014, Amendment 1, item 1; external design §4, §19.5;
//! REQ-ARCH-009): a [`Transport`] that sends the client's envelopes to the daemon of the store.
//!
//! **One connection per request**, made, greeted and closed for it: a stop signal's request is
//! never held behind a request that waits for the step in flight (a cancel waits for it, a grant
//! too), and a connection that breaks takes one request with it, which the client sends once
//! more. **One more connection follows**: a subscription, over which the daemon tells the
//! follower, by a frame, that something changed (readers stay direct: the follower reads the
//! store, REQ-PERS-013), with the poll of LIM-O15 as its safety.
//!
//! A connection that breaks while a request is in flight, or that cannot be made, is
//! [`ConnectionBroken`]: whether the request took effect is the store's to say, and the client
//! sends it again under its own id.

use crate::client::{ConnectionBroken, Transport};
use crate::config::Loaded;
use crate::daemon::connect::{self, Daemon, Reach};
use crate::daemon::wire::{self, Reply, Request};
use mimlys_core::api::{Envelope, Line, Notice, Outcome, Response};
use mimlys_core::ids::TaskId;
use mimlys_core::runtime::RuntimeError;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use tokio::sync::Notify;

/// The test seam of the repeat after a break: `MIMLYS_DAEMON_CUT_ANSWER=<request name>` makes a
/// **daemon** (in a debug build only) apply the first request of that name and close the
/// connection without answering, as a connection that broke after the request was applied and
/// before its answer was read. Nothing else reads it.
pub const CUT_ANSWER_ENV: &str = "MIMLYS_DAEMON_CUT_ANSWER";

/// What a connection attempt came to.
enum Attempt {
    Usable(Box<Daemon>),
    /// The daemon refuses this client for good (another version).
    Refused(String),
    /// No daemon answered.
    Broken(String),
}

/// The transport to the daemon of one store.
pub struct Socket {
    run_dir: PathBuf,
    data_dir: PathBuf,
    loaded: Loaded,
    changed: Arc<Notify>,
    /// The follower's subscription, once begun.
    subscribed: AtomicBool,
    ended: Arc<AtomicBool>,
    thread: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl Socket {
    /// The transport to the daemon of `data_dir`, found in the registry's directory `run_dir`,
    /// speaking for the configuration `loaded`.
    pub fn new(run_dir: PathBuf, data_dir: PathBuf, loaded: Loaded) -> Arc<Self> {
        Arc::new(Self {
            run_dir,
            data_dir,
            loaded,
            changed: Arc::new(Notify::new()),
            subscribed: AtomicBool::new(false),
            ended: Arc::new(AtomicBool::new(false)),
            thread: Mutex::new(None),
        })
    }

    fn attempt(&self) -> Attempt {
        match connect::reach(&self.run_dir, &self.data_dir, &self.loaded, false) {
            Ok(Reach::Daemon(daemon)) if daemon.usable() => Attempt::Usable(daemon),
            Ok(Reach::Daemon(daemon)) => match daemon.answer.refusal {
                Some(refusal)
                    if daemon.hello.runtime_version == crate::daemon::wire::runtime_version() =>
                {
                    Attempt::Refused(refusal.message)
                }
                _ => Attempt::Refused(crate::messages::daemon_version_refused(
                    crate::daemon::wire::runtime_version(),
                    &daemon.hello.runtime_version,
                )),
            },
            Ok(_) => Attempt::Broken("no daemon answers".to_owned()),
            Err(e) => Attempt::Broken(e.to_string()),
        }
    }

    /// One request over a connection of its own.
    fn once(&self, request: Request) -> Result<Reply, ConnectionBroken> {
        let broken = |detail: String| ConnectionBroken { detail };
        let mut daemon = match self.attempt() {
            Attempt::Usable(daemon) => daemon,
            Attempt::Refused(message) | Attempt::Broken(message) => return Err(broken(message)),
        };
        wire::write(&mut daemon.stream, &request).map_err(|e| broken(e.to_string()))?;
        wire::read(&mut daemon.stream).map_err(|e| broken(e.to_string()))
    }

    /// Begins the subscription: a thread that holds a connection and notifies the follower of
    /// each frame the daemon sends over it.
    fn subscribe(&self) {
        if self.subscribed.swap(true, Ordering::SeqCst) {
            return;
        }
        let (run_dir, data_dir, loaded) = (
            self.run_dir.clone(),
            self.data_dir.clone(),
            self.loaded.clone(),
        );
        let (changed, ended) = (Arc::clone(&self.changed), Arc::clone(&self.ended));
        let spawned = std::thread::Builder::new()
            .name("daemon-subscription".into())
            .spawn(move || {
                // A subscription that breaks is made again for as long as the transport lives:
                // the follower's poll covers the moments between.
                while !ended.load(Ordering::SeqCst) {
                    let Ok(Reach::Daemon(mut daemon)) =
                        connect::reach(&run_dir, &data_dir, &loaded, false)
                    else {
                        std::thread::sleep(std::time::Duration::from_millis(250));
                        continue;
                    };
                    if !daemon.usable()
                        || wire::write(&mut daemon.stream, &Request::Subscribe).is_err()
                    {
                        std::thread::sleep(std::time::Duration::from_millis(250));
                        continue;
                    }
                    // The daemon beats every couple of seconds: a connection that is silent for
                    // longer than that is gone.
                    let _ = daemon
                        .stream
                        .set_read_timeout(Some(std::time::Duration::from_secs(10)));
                    while let Ok(reply) = wire::read::<Reply>(&mut daemon.stream) {
                        if matches!(reply, Reply::Changed) {
                            changed.notify_one();
                        }
                        if ended.load(Ordering::SeqCst) {
                            return;
                        }
                    }
                }
            });
        if let Ok(handle) = spawned {
            *self.thread.lock().unwrap_or_else(PoisonError::into_inner) = Some(handle);
        }
    }
}

impl Transport for Socket {
    fn send(
        &self,
        envelope: Envelope,
        notice: &dyn Fn(Notice),
    ) -> Result<Response, ConnectionBroken> {
        let request_id = envelope.request_id;
        let broken = |detail: String| ConnectionBroken { detail };
        let mut daemon = match self.attempt() {
            Attempt::Usable(daemon) => daemon,
            // A refusal is the daemon's answer, and no connection that broke: it is not repeated.
            Attempt::Refused(message) => {
                return Ok(Response {
                    request_id,
                    outcome: Outcome::Failed(RuntimeError::Denied { detail: message }),
                });
            }
            Attempt::Broken(message) => return Err(broken(message)),
        };
        wire::write(
            &mut daemon.stream,
            &Request::Control {
                envelope: Box::new(envelope),
            },
        )
        .map_err(|e| broken(e.to_string()))?;
        loop {
            match wire::read::<Reply>(&mut daemon.stream).map_err(|e| broken(e.to_string()))? {
                Reply::Notice { notice: n } => notice(n),
                Reply::Control { answer } => return Ok(*answer),
                Reply::Refused { refusal } => {
                    return Ok(Response {
                        request_id,
                        outcome: Outcome::Failed(RuntimeError::Denied {
                            detail: refusal.message,
                        }),
                    });
                }
                other => {
                    return Err(broken(format!(
                        "the daemon answered {other:?}, not a response"
                    )));
                }
            }
        }
    }

    fn attach(&self, task_id: TaskId) {
        self.subscribe();
        let _ = self.once(Request::Attach { task_id });
    }

    fn changes(&self) -> Arc<Notify> {
        self.subscribe();
        Arc::clone(&self.changed)
    }

    fn failure(&self, task_id: TaskId) -> Option<RuntimeError> {
        match self.once(Request::Failure { task_id }) {
            Ok(Reply::Failure { error }) => error,
            _ => None,
        }
    }

    fn queued(&self, task_id: TaskId) -> Option<Line> {
        match self.once(Request::Queue { task_id }) {
            Ok(Reply::Queue { line }) => line,
            _ => None,
        }
    }
}

impl Drop for Socket {
    fn drop(&mut self) {
        // The subscription ends with the transport; its thread is not waited for (it may be
        // reading), and the process ends with the command.
        self.ended.store(true, Ordering::SeqCst);
    }
}
