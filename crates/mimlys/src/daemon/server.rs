//! The daemon (RFC 014, Amendment 1; external design §19.5; REQ-ARCH-009, REQ-PERS-014,
//! REQ-SEC-016, REQ-SCHED-008, REQ-CLI-008): the process that owns a store and serves its
//! clients over the socket.
//!
//! **One owner, one scheduler.** The daemon is an [`App`] (which holds the store's lock for its
//! life) whose driver runs the steps of every client's tasks, in one line under LIM-X1. Its
//! clients open no store for writing and take no registry slots: the daemon takes its own, as any
//! runtime does, and is recorded in the registry as a daemon.
//!
//! **One connection, one thread.** Each connection is checked (the peer's user, the peer's tree),
//! greets (the hellos), and then carries requests, blocking where the request blocks, as the
//! in-process transport does. A request never waits behind another's, so that a stop signal's
//! request is not held by a request that waits for the step in flight.

use crate::app::{App, AppError, os_user};
use crate::backends::Backend;
use crate::client::Transport as _;
use crate::daemon::plat::{self, Peer};
use crate::daemon::wire::{
    self, ClientHello, DaemonHello, DaemonStatus, HelloAnswer, ProbeFailure, Refusal, RefusalKind,
    Reply, Request,
};
use crate::embedded::InProcess;
use crate::probe;
use crate::probe::drive::Stop;
use crate::probe::run::probe_backend;
use crate::registry::{Bounds, DaemonEntry, Registry};
use mimlys_core::api::{Envelope, Outcome, Response};
use mimlys_core::clock::{Clock, SystemClock};
use mimlys_core::event::Event;
use mimlys_core::ids::{RequestId, SystemIds, TaskId};
use mimlys_core::limits::{LIM_O1_STOP_MS, LIM_O14_DAEMON_CONNECT_MS, LIM_O24_DAEMON_STOP_MS};
use mimlys_core::runtime::{RuntimeError, Session};
use mimlys_core::task::TaskState;
use mimlys_exec::frame::FrameError;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

/// The reason `daemon.refused` records for a peer of the daemon's own tree (§17.1).
const REFUSED_OWN_TREE: &str = "own_tree";
/// The reason it records for a peer whose process id the system did not give (REQ-SEC-016).
const REFUSED_NO_PEER_PID: &str = "no_peer_pid";

/// How often an idle subscription writes to its client anyway, so that a client that has gone is
/// found by the failed write and its connection does not keep the daemon alive.
const SUBSCRIPTION_BEAT: Duration = Duration::from_secs(2);

/// How often a wait on the daemon's side (for a probe, for a task to pause) looks again.
const WAIT_POLL: Duration = Duration::from_millis(50);

/// What the daemon is started with.
pub struct Options {
    /// The idle time after which it exits; none for `never` (LIM-O13).
    pub idle: Option<Duration>,
    /// The log file, for `daemon status`.
    pub log: PathBuf,
}

/// The test seam of LIM-O13: `MIMLYS_DAEMON_IDLE_MS` replaces the configured idle time, in a
/// build with `debug_assertions` on only (as `MIMLYS_RUN_DIR` is): a test cannot wait a minute.
pub const IDLE_ENV: &str = "MIMLYS_DAEMON_IDLE_MS";

/// The idle time in force: the configured one, or, in a debug build, the seam's.
pub fn idle_in_force(configured: Option<Duration>) -> Option<Duration> {
    if cfg!(debug_assertions)
        && let Some(ms) = std::env::var(IDLE_ENV)
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
    {
        return Some(Duration::from_millis(ms));
    }
    configured
}

/// Everything the connection threads share.
pub struct Served {
    app: App,
    /// The runtime of the app, which the daemon owns.
    runtime: Arc<mimlys_core::runtime::Runtime>,
    transport: Arc<InProcess>,
    socket: PathBuf,
    config_hash: String,
    started: Instant,
    started_at: String,
    options: Options,
    /// The open connections, by the pid of the peer's process (none for a peer the system gave
    /// no pid for, which is refused at once): how many each process holds.
    connections: Connections,
    stopping: AtomicBool,
    last_activity: Mutex<Instant>,
    /// Counts what the driver's host said changed; each subscriber waits for it to move.
    seen: (Mutex<u64>, Condvar),
    shutdown: tokio::sync::Notify,
    /// What the first `stop` answered, for the ones after it.
    stopped: Mutex<Option<(Vec<String>, Vec<String>)>>,
    /// The stop handles of the probes that run now (`model probe` through the daemon): a stop of
    /// the daemon signals each, as a stop signal ends a probe of a command.
    probes: Mutex<Vec<Arc<Stop>>>,
    /// The test seam of the repeat after a break ([`CUT_ANSWER_ENV`]): the request whose first
    /// answer is not sent. Always none in a release.
    cut_answer: Mutex<Option<String>>,
}

/// The connections that are open, by the pid of the peer's process: a client **process** is what
/// `daemon status` counts (a follower holds one connection for as long as it follows, each request
/// its own for a moment, so the connections alone say little of how many terminals there are).
#[derive(Default)]
struct Connections(Mutex<BTreeMap<Option<u32>, u32>>);

impl Connections {
    /// A connection of the process `pid` is open until the returned guard is dropped.
    fn open(&self, pid: Option<u32>) -> OpenConnection<'_> {
        *self
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(pid)
            .or_insert(0) += 1;
        OpenConnection { of: self, pid }
    }

    /// How many processes hold a connection, and how many connections they hold.
    fn counts(&self) -> (u32, u32) {
        let open = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        (
            u32::try_from(open.len()).unwrap_or(u32::MAX),
            open.values().sum(),
        )
    }
}

/// One open connection, counted under its peer's process until it is dropped.
struct OpenConnection<'a> {
    of: &'a Connections,
    pid: Option<u32>,
}

impl Drop for OpenConnection<'_> {
    fn drop(&mut self) {
        let mut open = self.of.0.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(count) = open.get_mut(&self.pid) {
            *count -= 1;
            if *count == 0 {
                open.remove(&self.pid);
            }
        }
    }
}

impl Served {
    fn touch(&self) {
        *self
            .last_activity
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Instant::now();
    }

    /// Whether the answer to a request of this name is the one the seam cuts (once).
    fn cut_answer_of(&self, name: &str) -> bool {
        let mut cut = self
            .cut_answer
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if cut.as_deref() == Some(name) {
            *cut = None;
            return true;
        }
        false
    }

    fn session(&self) -> Session {
        Session {
            id: format!("pid:{}", std::process::id()),
            os_user: os_user(),
        }
    }

    /// The processes that lead a session the daemon made: the worker, the dependency step's
    /// worker, and every MCP server that runs. A peer in one of those sessions is the daemon's
    /// own tree. Asked at each connection of the live objects, so it is current by construction:
    /// a server that has just started is in it and one that has ended is not.
    #[cfg(unix)]
    fn session_leaders(&self) -> Vec<plat::Leader> {
        let mut leaders = Vec::new();
        leaders.extend(self.app.worker.process_id());
        if let Some(fetch) = &self.app.fetch_worker {
            leaders.extend(fetch.process_id());
        }
        leaders.extend(self.app.servers.process_ids());
        leaders
    }

    /// The Job Objects the daemon made for its own tree: the worker's, the dependency step's, and
    /// the one it put each tool server in. A peer in one of them is the daemon's own tree. Asked of
    /// the live objects at each connection, as on Unix.
    #[cfg(windows)]
    fn session_leaders(&self) -> Vec<plat::Leader> {
        let mut leaders = Vec::new();
        leaders.extend(self.app.worker.tree_job());
        if let Some(fetch) = &self.app.fetch_worker {
            leaders.extend(fetch.tree_job());
        }
        leaders.extend(crate::daemon::marks::jobs());
        leaders
    }

    /// The refusal of a peer that is the daemon's own tree, or that the daemon cannot tell from
    /// it, and the reason the audit records. A peer with no pid is refused (REQ-SEC-016): the
    /// check below needs the pid, and serving a peer it cannot check would fail open.
    fn refuse_peer(&self, peer: Peer) -> Option<(Refusal, &'static str)> {
        let tree = plat::judge_tree(peer, &self.session_leaders());
        let pid = peer.pid.unwrap_or_default();
        match tree {
            plat::Tree::Outside => None,
            plat::Tree::NoPid => Some((
                Refusal {
                    kind: RefusalKind::NoPeerPid,
                    message: crate::messages::daemon_no_peer_pid_refused(),
                    reference: None,
                },
                REFUSED_NO_PEER_PID,
            )),
            plat::Tree::Own | plat::Tree::Unreadable => {
                if tree == plat::Tree::Unreadable {
                    // A session that cannot be read is refused, never served.
                    tracing::warn!(pid, "the session of a client could not be read: refused");
                }
                Some((
                    Refusal {
                        kind: RefusalKind::OwnTree,
                        message: crate::messages::daemon_own_tree_refused(pid),
                        reference: None,
                    },
                    REFUSED_OWN_TREE,
                ))
            }
        }
    }

    /// What the hellos say about the client: the refusal that ends the connection, and the one
    /// that only refuses what changes state.
    fn judge(&self, salt: &str, client: &ClientHello) -> (Option<Refusal>, Option<Refusal>) {
        let mine = crate::daemon::wire::runtime_version();
        if client.runtime_version != mine {
            return (
                Some(Refusal {
                    kind: RefusalKind::Version,
                    message: crate::messages::daemon_version_refused(&client.runtime_version, mine),
                    reference: None,
                }),
                None,
            );
        }
        if client.config_hash != self.config_hash {
            return (
                None,
                Some(Refusal {
                    kind: RefusalKind::Configuration,
                    message: crate::messages::daemon_configuration_differs(),
                    reference: None,
                }),
            );
        }
        // A secret of the client's, or of the daemon's, whose keyed hash is not the other's.
        let ours = &self.app.loaded.secrets;
        let mut names: Vec<&str> = client.secrets.keys().map(String::as_str).collect();
        names.extend(ours.iter().map(|(n, _)| n.as_str()));
        names.sort_unstable();
        names.dedup();
        for name in names {
            let mine = ours
                .iter()
                .find(|(n, _)| n == name)
                .map(|(n, v)| wire::keyed_hash(salt, n, v));
            if mine.as_deref() != client.secrets.get(name).map(String::as_str) {
                return (
                    None,
                    Some(Refusal {
                        kind: RefusalKind::Secret,
                        message: crate::messages::daemon_secret_differs(name),
                        reference: Some(name.to_owned()),
                    }),
                );
            }
        }
        (None, None)
    }

    /// One connection, from its hello to its end. It is counted under its peer's process while it
    /// is open, and its beginning and its end are activity (LIM-O13).
    fn connection(&self, stream: &mut plat::Stream, peer: Peer) -> Result<(), FrameError> {
        let open = self.connections.open(peer.pid);
        self.touch();
        let result = self.serve_connection(stream, peer);
        drop(open);
        self.touch();
        result
    }

    fn serve_connection(&self, stream: &mut plat::Stream, peer: Peer) -> Result<(), FrameError> {
        let timeout = Some(Duration::from_millis(LIM_O14_DAEMON_CONNECT_MS));
        plat::set_timeouts(stream, timeout, timeout)?;
        let salt = wire::new_salt()?;
        wire::write(
            stream,
            &DaemonHello {
                protocol: wire::PROTOCOL.to_owned(),
                hello_version: wire::HELLO_VERSION,
                runtime_version: crate::daemon::wire::runtime_version().to_owned(),
                config_hash: self.config_hash.clone(),
                salt: salt.clone(),
            },
        )?;
        let hello: ClientHello = wire::read(stream)?;
        // The daemon's own tree is refused before anything it says is read, and the refusal is
        // recorded; so is a daemon that is stopping.
        let (mut refusal, writes_refused) = match self.refuse_peer(peer) {
            Some((refused, reason)) => {
                tracing::warn!(pid = ?peer.pid, reason, "a client was refused");
                if let Err(e) = self.runtime.record_server_events(&[Event::DaemonRefused {
                    reason: reason.into(),
                    peer_pid: peer.pid,
                }]) {
                    tracing::warn!("the refusal could not be recorded: {e}");
                }
                (Some(refused), None)
            }
            None => self.judge(&salt, &hello),
        };
        if refusal.is_none() && self.stopping.load(Ordering::SeqCst) {
            refusal = Some(Refusal {
                kind: RefusalKind::Stopping,
                message: crate::messages::daemon_stopping(),
                reference: None,
            });
        }
        let accepted = refusal.is_none();
        wire::write(
            stream,
            &HelloAnswer {
                accepted,
                refusal: refusal.clone(),
                writes_refused: writes_refused.clone(),
            },
        )?;
        // A peer of the own tree, or one that cannot be told from it, is read no further: not even
        // `stop`.
        if refusal
            .as_ref()
            .is_some_and(|r| matches!(r.kind, RefusalKind::OwnTree | RefusalKind::NoPeerPid))
        {
            return Ok(());
        }
        tracing::info!(pid = ?peer.pid, accepted, "client connected");
        plat::set_timeouts(stream, None, None)?;
        loop {
            let request: Request = match wire::read(stream) {
                Ok(request) => request,
                Err(FrameError::Eof) => return Ok(()),
                Err(e) => return Err(e),
            };
            if let (Some(refusal), false) = (&refusal, matches!(request, Request::Stop)) {
                wire::write(
                    stream,
                    &Reply::Refused {
                        refusal: refusal.clone(),
                    },
                )?;
                return Ok(());
            }
            match request {
                Request::Stop => {
                    let (paused, queued) = self.stop();
                    wire::write(
                        stream,
                        &Reply::Stopped {
                            pid: std::process::id(),
                            paused,
                            queued,
                        },
                    )?;
                    self.shutdown.notify_one();
                    return Ok(());
                }
                Request::Control { envelope } => {
                    let name = envelope.request.name();
                    let task_of_request = envelope.request.task_id();
                    let answer = {
                        let cell = RefCell::new(&mut *stream);
                        self.control(*envelope, peer, writes_refused.as_ref(), &|notice| {
                            let _ =
                                wire::write(&mut **cell.borrow_mut(), &Reply::Notice { notice });
                        })
                    };
                    // The seam: the request was applied, and the connection ends before the answer.
                    if self.cut_answer_of(name) {
                        // With `MIMLYS_DAEMON_DELETE_AFTER_CUT`, the task is deleted before the
                        // client asks again: a request that took effect, whose task is gone.
                        if let (true, Some(task_id)) = (
                            cfg!(debug_assertions)
                                && std::env::var_os(crate::daemon::transport::DELETE_AFTER_CUT_ENV)
                                    .is_some(),
                            task_of_request,
                        ) {
                            let _ = crate::client::Transport::send(
                                &*self.transport,
                                Envelope {
                                    request_id: RequestId::new(&SystemIds),
                                    session: self.session(),
                                    request: mimlys_core::api::Request::DeleteTask {
                                        task_id,
                                        audit: false,
                                    },
                                },
                                &|_| {},
                            );
                        }
                        return Ok(());
                    }
                    wire::write(
                        stream,
                        &Reply::Control {
                            answer: Box::new(answer),
                        },
                    )?;
                }
                Request::Attach { task_id } => {
                    crate::client::Transport::attach(&*self.transport, task_id);
                    wire::write(stream, &Reply::Attached)?;
                }
                Request::Failure { task_id } => {
                    let error = crate::client::Transport::failure(&*self.transport, task_id);
                    wire::write(stream, &Reply::Failure { error })?;
                }
                Request::Queue { task_id } => {
                    let line = self.transport.driver().queued(task_id);
                    wire::write(stream, &Reply::Queue { line })?;
                }
                Request::Status => {
                    wire::write(
                        stream,
                        &Reply::Status {
                            status: Box::new(self.status()),
                        },
                    )?;
                }
                Request::Subscribe => {
                    wire::write(stream, &Reply::Subscribed)?;
                    return self.subscription(stream);
                }
                Request::Probe { backend } => {
                    // The probe writes to the store (a task of its own, deleted when it ends): a
                    // client whose configuration or secrets differ may not ask for it, as for
                    // any request that changes state.
                    if let Some(refusal) = &writes_refused {
                        wire::write(
                            stream,
                            &Reply::Refused {
                                refusal: refusal.clone(),
                            },
                        )?;
                        return Ok(());
                    }
                    let reply = self.probe(stream, &backend)?;
                    wire::write(stream, &reply)?;
                    // The connection is the probe's alone: the thread that watched it for a stop
                    // request ends with it.
                    let _ = stream.shutdown(std::net::Shutdown::Both);
                    return Ok(());
                }
                // A stop request with no probe to stop is nothing.
                Request::StopProbe => {}
            }
        }
    }

    /// A connection that is told, by a frame, that something changed. It ends when the client
    /// goes (a write fails) or the daemon stops.
    fn subscription(&self, stream: &mut plat::Stream) -> Result<(), FrameError> {
        let (lock, changed) = &self.seen;
        let mut seen = *lock.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            let moved = {
                let guard = lock.lock().unwrap_or_else(PoisonError::into_inner);
                let (guard, _) = changed
                    .wait_timeout_while(guard, SUBSCRIPTION_BEAT, |now| *now == seen)
                    .unwrap_or_else(PoisonError::into_inner);
                let now = *guard;
                let moved = now != seen;
                seen = now;
                moved
            };
            // A beat with nothing changed is also a frame: a follower looks again, and a client
            // that has gone is found here.
            let _ = moved;
            wire::write(stream, &Reply::Changed)?;
            if self.stopping.load(Ordering::SeqCst) {
                return Ok(());
            }
        }
    }

    /// A request of the control API, as a client of this connection sends it: under the session
    /// the daemon verified (the peer's process, as the system reports it, and the daemon's own
    /// user), never the one the client wrote; refused while the daemon stops, and, when the
    /// client's configuration or a secret differs, refused if it changes state.
    fn control(
        &self,
        envelope: Envelope,
        peer: Peer,
        writes_refused: Option<&Refusal>,
        notice: &dyn Fn(mimlys_core::api::Notice),
    ) -> Response {
        let refuse = |error: RuntimeError| Response {
            request_id: envelope.request_id,
            outcome: Outcome::Failed(error),
        };
        if self.stopping.load(Ordering::SeqCst) {
            return refuse(RuntimeError::Unavailable {
                component: crate::messages::daemon_stopping(),
            });
        }
        if envelope.request.changes_state()
            && let Some(refusal) = writes_refused
        {
            return refuse(RuntimeError::Denied {
                detail: refusal.message.clone(),
            });
        }
        let envelope = Envelope {
            session: Session {
                id: peer.session_id(),
                os_user: os_user(),
            },
            ..envelope
        };
        let request_id = envelope.request_id;
        crate::client::Transport::send(&*self.transport, envelope, notice).unwrap_or_else(
            // An in-process transport does not break; said anyway, never panicked.
            |broken| Response {
                request_id,
                outcome: Outcome::Failed(RuntimeError::Unavailable {
                    component: broken.detail,
                }),
            },
        )
    }

    fn status(&self) -> DaemonStatus {
        let runtime = &self.runtime;
        let waiting_user = runtime
            .persistence()
            .tasks_in_state("waiting_user")
            .unwrap_or_default();
        // Client processes, by the peer's pid, and the connections they hold (the connection that
        // asks for this is one of them).
        let (clients, connections) = self.connections.counts();
        DaemonStatus {
            pid: std::process::id(),
            runtime_version: crate::daemon::wire::runtime_version().to_owned(),
            started_at: self.started_at.clone(),
            uptime_secs: self.started.elapsed().as_secs(),
            socket: self.socket.to_string_lossy().into_owned(),
            config_hash: self.config_hash.clone(),
            clients,
            connections,
            running: runtime.running_tasks().unwrap_or_default(),
            queued: self.transport.driver().waiting(),
            waiting_user,
            log: self.options.log.to_string_lossy().into_owned(),
            idle_secs: self.options.idle.map(|d| d.as_secs()),
        }
    }

    /// Signals every probe that runs, and waits for each to end (within LIM-O24).
    fn stop_probes(&self) {
        let signal = |stops: &[Arc<Stop>]| stops.iter().for_each(|s| s.signal());
        let begun = Instant::now();
        let cap = Duration::from_millis(LIM_O24_DAEMON_STOP_MS);
        let grace = Duration::from_millis(LIM_O1_STOP_MS.unsigned_abs());
        let mut signalled = 0;
        loop {
            let running = self
                .probes
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone();
            if running.is_empty() || begun.elapsed() >= cap {
                return;
            }
            // One signal at once, and a second after the grace of the first (it interrupts
            // anyway; no tool of the probe's has an effect that cannot be undone).
            let due = if begun.elapsed() >= grace { 2 } else { 1 };
            while signalled < due {
                signal(&running);
                signalled += 1;
            }
            std::thread::sleep(WAIT_POLL);
        }
    }

    /// Stops: new requests are refused (`stopping`), the line starts nothing more, every running
    /// task pauses at its boundary (REQ-CLI-005: an execution within LIM-O2, an action that
    /// cannot be undone waited for to its own deadline), and the daemon lists what it paused and
    /// what waited in line. **Nothing resumes**: that is the user's, or `restart`'s, for exactly
    /// the tasks this paused. Called by a second stop, it waits for the first and answers the same.
    fn stop(&self) -> (Vec<String>, Vec<String>) {
        let mut done = self.stopped.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(answer) = done.as_ref() {
            return answer.clone();
        }
        self.stopping.store(true, Ordering::SeqCst);
        // A probe that runs is stopped as a stop signal stops one, and waited for before the
        // tasks are looked at: its task is the probe's, deleted when it ends, never a task the
        // user paused.
        self.stop_probes();
        let queued: Vec<String> = self
            .transport
            .driver()
            .drain_waiting()
            .into_iter()
            .map(|id| id.to_string())
            .collect();
        let runtime = &self.runtime;
        let running = runtime.running_tasks().unwrap_or_default();
        let session = self.session();
        let send = |request: mimlys_core::api::Request| {
            let _ = crate::client::Transport::send(
                &*self.transport,
                Envelope {
                    request_id: RequestId::new(&SystemIds),
                    session: session.clone(),
                    request,
                },
                &|_| {},
            );
        };
        for task_id in &running {
            send(mimlys_core::api::Request::StopAtBoundary { task_id: *task_id });
        }
        // The first stop signal's rules: a pause is taken at the boundary; what has not paused a
        // second before LIM-O1 is asked to stop what can be undone; an action that cannot be
        // undone is waited for, up to its own deadline.
        let begun = Instant::now();
        let watchdog = Duration::from_millis(LIM_O1_STOP_MS.unsigned_abs().saturating_sub(1000));
        let cap = Duration::from_millis(LIM_O24_DAEMON_STOP_MS);
        let mut interrupted = false;
        loop {
            let still: Vec<TaskId> = runtime
                .running_tasks()
                .unwrap_or_default()
                .into_iter()
                .filter(|id| running.contains(id))
                .collect();
            if still.is_empty() || begun.elapsed() >= cap {
                break;
            }
            if !interrupted && begun.elapsed() >= watchdog {
                interrupted = true;
                for task_id in &still {
                    send(mimlys_core::api::Request::InterruptStep { task_id: *task_id });
                }
            }
            std::thread::sleep(WAIT_POLL);
        }
        let paused: Vec<String> = running
            .iter()
            .filter(|id| {
                matches!(
                    runtime.persistence().load_task(**id),
                    Ok(Some(task)) if task.state == TaskState::Paused
                )
            })
            .map(ToString::to_string)
            .collect();
        tracing::info!(
            paused = paused.len(),
            queued = queued.len(),
            "the daemon stops"
        );
        *done = Some((paused.clone(), queued.clone()));
        (paused, queued)
    }

    /// `mimlys model probe` for one backend, run here: the probe builds a runtime of its own over
    /// the store, and the daemon is the process that holds it (REQ-PERS-014). The same code runs
    /// as in embedded mode ([`probe_backend`]), over the daemon's own store handle; nothing of it
    /// is the daemon's own task, and no second process owns the store. A stop request on the
    /// connection (or the connection's end) stops the probe as a signal stops one, and so does a
    /// stop of the daemon ([`Served::stop`]).
    fn probe(&self, stream: &mut plat::Stream, name: &str) -> Result<Reply, FrameError> {
        if self.stopping.load(Ordering::SeqCst) {
            return Ok(Reply::ProbeFailed {
                failure: ProbeFailure::Runtime {
                    error: RuntimeError::Unavailable {
                        component: crate::messages::daemon_stopping(),
                    },
                },
            });
        }
        let failed = |e: AppError| Reply::ProbeFailed {
            failure: match e {
                AppError::Runtime(error) => ProbeFailure::Runtime { error },
                other => ProbeFailure::Other {
                    message: other.to_string(),
                },
            },
        };
        // One task runs at a time (LIM-X1), the probe's own included: with one running, the
        // runtime would refuse the probe's task with no name to give. Said here, naming it.
        if let Some(running) = self
            .runtime
            .running_tasks()
            .ok()
            .and_then(|r| r.first().copied())
        {
            return Ok(Reply::ProbeFailed {
                failure: ProbeFailure::Runtime {
                    error: RuntimeError::Conflict {
                        detail: crate::messages::daemon_probe_refused_for_task(
                            &running.to_string(),
                        ),
                    },
                },
            });
        }
        // The same plan as the command's: a remote backend is probed only when it is named and the
        // administrator allowed it, and that is what lets its probe's profile send the fixed
        // mini-task. The client's word is the name alone.
        let plan = match probe::plan(self.app.config(), Some(name)) {
            Ok(plan) => plan,
            Err(e) => return Ok(failed(e)),
        };
        let backend = match Backend::make(&self.app.loaded, name, None).and_then(|b| {
            b.ready()?;
            Ok(b)
        }) {
            Ok(backend) => backend,
            Err(e) => return Ok(failed(e)),
        };
        let stop = Stop::new();
        self.probes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(Arc::clone(&stop));
        // What the client says on this connection while the probe runs: a stop request, or its end.
        let watcher = stream.try_clone().map(|mut reader| {
            let stop = Arc::clone(&stop);
            std::thread::spawn(move || {
                loop {
                    match wire::read::<Request>(&mut reader) {
                        Ok(Request::StopProbe) => stop.signal(),
                        Ok(_) => {}
                        Err(_) => {
                            stop.signal();
                            return;
                        }
                    }
                }
            })
        });
        let clock: Arc<dyn Clock> = Arc::new(SystemClock);
        let ended = tokio::runtime::Handle::current().block_on(probe_backend(
            &self.app,
            &backend,
            plan.remote_named.is_some(),
            &stop,
            clock,
        ));
        self.probes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|s| !Arc::ptr_eq(s, &stop));
        self.touch();
        // The watcher ends when the connection does (the caller shuts it down after the answer).
        drop(watcher);
        Ok(match ended {
            Ok(ended) => Reply::Probed {
                ended: Box::new(ended),
            },
            Err(e) => failed(e),
        })
    }

    /// What keeps the daemon from idling out, in words for its log: a client is connected, or a
    /// task is running, queued or waiting for the user (LIM-O13). `None` when it has nothing to do.
    fn busy_reason(&self) -> Option<&'static str> {
        if self.connections.counts().0 > 0 {
            return Some("a client is connected");
        }
        let runtime = &self.runtime;
        let busy = |state: &str| {
            runtime
                .persistence()
                .tasks_in_state(state)
                .map(|t| !t.is_empty())
                // A store that cannot be read is not a reason to leave tasks unserved.
                .unwrap_or(true)
        };
        if busy("running") {
            Some("a task is running")
        } else if busy("waiting_user") {
            Some("a task waits for the user")
        } else if busy("recovering") {
            Some("a task is recovering")
        } else if !self.transport.driver().waiting().is_empty()
            || !self.transport.driver().owned_tasks().is_empty()
        {
            Some("a task is in line or owned by the daemon")
        } else {
            None
        }
    }
}

/// Runs the daemon until it is stopped: opens nothing itself (the app holds the store's lock),
/// binds the socket, records itself in the registry, serves, and ends by `stop`, by its idle
/// time, or by a stop signal, each of which pauses what runs first.
pub async fn serve(app: App, options: Options, ready: impl FnOnce()) -> Result<(), AppError> {
    let data_dir = app.data_dir.clone();
    let run_dir = app
        .registry_dir
        .clone()
        .map_err(|detail| AppError::Io { detail })?;
    mimlys_exec::confine::create_private_dir(&run_dir).map_err(|e| AppError::Io {
        detail: format!(
            "cannot make the registry's directory {}: {}",
            run_dir.display(),
            e.kind()
        ),
    })?;
    let (listener, path) = bind(&run_dir, &data_dir)?;
    let registry = Registry::new(run_dir, Bounds::default(), Arc::new(SystemClock));
    let started_at = SystemClock.now();
    let registered = registry
        .register_daemon(DaemonEntry {
            v: crate::registry::DAEMON_ENTRY_VERSION,
            pid: std::process::id(),
            data_dir: data_dir.to_string_lossy().into_owned(),
            socket: path.to_string_lossy().into_owned(),
            version: crate::daemon::wire::runtime_version().to_owned(),
            since: started_at,
        })
        .map_err(|detail| AppError::Io {
            detail: format!("the daemon cannot be recorded in the registry: {detail}"),
        })?;
    let runtime = Arc::clone(app.runtime()?);
    let transport = InProcess::with_line(Arc::clone(&runtime));
    transport.start_driving();
    let served = Arc::new(Served {
        config_hash: wire::config_hash(app.loaded.path.as_deref()),
        runtime,
        app,
        transport,
        socket: path.clone(),
        started: Instant::now(),
        started_at: started_at.to_rfc3339(),
        options,
        connections: Connections::default(),
        stopping: AtomicBool::new(false),
        last_activity: Mutex::new(Instant::now()),
        seen: (Mutex::new(0), Condvar::new()),
        shutdown: tokio::sync::Notify::new(),
        stopped: Mutex::new(None),
        probes: Mutex::new(Vec::new()),
        cut_answer: Mutex::new(
            std::env::var(crate::daemon::transport::CUT_ANSWER_ENV)
                .ok()
                .filter(|_| cfg!(debug_assertions)),
        ),
    });
    // What the driver's host says changed reaches every subscriber.
    {
        let served = Arc::clone(&served);
        let changes = served.transport.changes();
        tokio::spawn(async move {
            loop {
                changes.notified().await;
                let (lock, changed) = &served.seen;
                *lock.lock().unwrap_or_else(PoisonError::into_inner) += 1;
                changed.notify_all();
            }
        });
    }
    // The idle watch.
    if let Some(idle) = served.options.idle {
        let served = Arc::clone(&served);
        tokio::spawn(async move {
            let tick = (idle / 4).clamp(Duration::from_millis(50), Duration::from_secs(5));
            // Said once each time the idle time passes with something to do: the log's line is
            // the state a reader (and a test) waits for, rather than a time chosen to be long
            // enough (handoff 014d, item 0).
            let mut said_busy = false;
            loop {
                tokio::time::sleep(tick).await;
                let quiet = served
                    .last_activity
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .elapsed();
                if quiet < idle {
                    said_busy = false;
                    continue;
                }
                match served.busy_reason() {
                    None => {
                        tracing::info!(
                            idle_secs = idle.as_secs(),
                            "the daemon has had nothing to do for its idle time and exits"
                        );
                        served.shutdown.notify_one();
                        return;
                    }
                    Some(why) if !said_busy => {
                        said_busy = true;
                        tracing::info!(
                            idle_secs = idle.as_secs(),
                            why,
                            "the daemon is past its idle time and stays: it has something to do"
                        );
                    }
                    Some(_) => {}
                }
            }
        });
    }
    // A stop signal pauses what runs, as `stop` does, and ends the daemon.
    if let Ok(mut signals) = crate::follow::StopSignals::new() {
        let served = Arc::clone(&served);
        tokio::spawn(async move {
            if signals.recv().await.is_ok() {
                tracing::info!("the daemon was asked to stop by a signal");
                let for_stop = Arc::clone(&served);
                let _ = tokio::task::spawn_blocking(move || for_stop.stop()).await;
                served.shutdown.notify_one();
            }
        });
    }
    tracing::info!(
        pid = std::process::id(),
        socket = %path.display(),
        "the daemon serves {}",
        data_dir.display()
    );
    // Listening and recorded: whoever started the daemon may now reach it.
    ready();
    accept_until_stopped(&served, listener).await;
    // (A socket's file is the daemon's to remove; a pipe goes with the process.)
    #[cfg(unix)]
    let _ = std::fs::remove_file(&path);
    drop(registered);
    let closed = served.app.close();
    tracing::info!("the daemon has ended");
    closed
}

/// Makes the daemon's listener: the socket, in the registry's directory, mode 0600, after the
/// directory is checked (Unix); and its path.
#[cfg(unix)]
fn bind(
    run_dir: &std::path::Path,
    data_dir: &std::path::Path,
) -> Result<(tokio::net::UnixListener, PathBuf), AppError> {
    plat::check_directory(run_dir).map_err(|e| AppError::Io {
        detail: e.to_string(),
    })?;
    let path = plat::socket_path(run_dir, data_dir)
        .map_err(|e| AppError::Io {
            detail: e.to_string(),
        })?
        .ok_or_else(|| AppError::Io {
            detail: "the data directory is not there".to_owned(),
        })?;
    // The store's lock is ours: no daemon of this store lives, and what is at the socket's path
    // is what a daemon that was killed left.
    if std::fs::symlink_metadata(&path).is_ok() {
        std::fs::remove_file(&path).map_err(|e| AppError::Io {
            detail: format!(
                "cannot remove the stale socket {}: {}",
                path.display(),
                e.kind()
            ),
        })?;
    }
    let listener = tokio::net::UnixListener::bind(&path).map_err(|e| AppError::Io {
        detail: format!("cannot bind the socket {}: {}", path.display(), e.kind()),
    })?;
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(plat::SOCKET_MODE))
            .map_err(|e| AppError::Io {
                detail: format!("cannot set the mode of {}: {}", path.display(), e.kind()),
            })?;
    }
    plat::check_socket_file(&path).map_err(|e| AppError::Io {
        detail: e.to_string(),
    })?;
    Ok((listener, path))
}

/// Makes the daemon's listener: its pipe, created as the first instance of its name with a DACL
/// for the user's SID alone (Windows); and its name. A pipe that something else already holds is
/// not the daemon's to take, and the daemon says so.
#[cfg(windows)]
fn bind(
    _run_dir: &std::path::Path,
    data_dir: &std::path::Path,
) -> Result<(mimlys_exec::daemon_pipe::PipeListener, PathBuf), AppError> {
    let io = |e: &dyn std::fmt::Display| AppError::Io {
        detail: e.to_string(),
    };
    let name = plat::pipe_name(data_dir)
        .map_err(|e| io(&e))?
        .ok_or_else(|| AppError::Io {
            detail: "the data directory is not there".to_owned(),
        })?;
    let sid = plat::own_sid().map_err(|e| io(&e))?;
    let listener = mimlys_exec::daemon_pipe::PipeListener::bind(&name, &sid).map_err(|e| {
        AppError::Io {
            detail: format!(
                "cannot create the pipe {name}: {e} (the name is held by a process that is not this daemon, or the system refused)"
            ),
        }
    })?;
    Ok((listener, PathBuf::from(name)))
}

/// Starts the thread of one connection that was admitted: the runtime's verbs reach the executor
/// (a server is started, a cancel waits), so it runs inside the daemon's own; a defect in the
/// handling of one request ends that connection, and never the daemon, which owns every client's
/// tasks.
fn start_connection(served: &Arc<Served>, stream: plat::Stream, peer: Peer) {
    let served = Arc::clone(served);
    let executor = tokio::runtime::Handle::current();
    let spawned = std::thread::Builder::new()
        .name("daemon-connection".into())
        .spawn(move || {
            let _inside = executor.enter();
            let mut stream = stream;
            let ended = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                served.connection(&mut stream, peer)
            }));
            match ended {
                Ok(Err(e)) => {
                    tracing::debug!(pid = ?peer.pid, "connection ended: {e}");
                }
                Ok(Ok(())) => {}
                Err(_) => {
                    tracing::error!(pid = ?peer.pid, "a request panicked; its connection was closed");
                }
            }
        });
    if let Err(e) = spawned {
        tracing::warn!("a thread for a client could not be started: {e}");
    }
}

/// Serves the clients that connect, until the daemon is stopped.
#[cfg(unix)]
async fn accept_until_stopped(served: &Arc<Served>, listener: tokio::net::UnixListener) {
    loop {
        tokio::select! {
            () = served.shutdown.notified() => break,
            accepted = listener.accept() => {
                let Ok((stream, _)) = accepted else { continue };
                // Another user's process is refused before anything is sent or read; so is a
                // peer whose credentials cannot be read.
                let peer = match stream.peer_cred() {
                    Ok(cred) => match Peer::admit(&cred) {
                        Ok(peer) => peer,
                        Err(e) => {
                            tracing::warn!("{e}");
                            continue;
                        }
                    },
                    Err(e) => {
                        tracing::warn!("the credentials of a client could not be read ({e}): refused");
                        continue;
                    }
                };
                let Ok(stream) = stream.into_std() else { continue };
                if plat::set_nonblocking(&stream, false).is_err() {
                    continue;
                }
                start_connection(served, stream, peer);
            }
        }
    }
    drop(listener);
}

/// Serves the clients that connect, until the daemon is stopped: a thread accepts (a blocking
/// wait, which the stopper ends), and each client is checked, as on Unix, before anything is sent
/// or read.
#[cfg(windows)]
async fn accept_until_stopped(
    served: &Arc<Served>,
    listener: mimlys_exec::daemon_pipe::PipeListener,
) {
    let stopper = listener.stopper();
    let acceptor = {
        let served = Arc::clone(served);
        let executor = tokio::runtime::Handle::current();
        std::thread::Builder::new()
            .name("daemon-accept".into())
            .spawn(move || {
                let _inside = executor.enter();
                loop {
                    match listener.accept() {
                        Ok(stream) => match Peer::admit(&stream) {
                            Ok(peer) => start_connection(&served, stream, peer),
                            Err(e) => tracing::warn!("{e}"),
                        },
                        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => return,
                        Err(e) => {
                            tracing::warn!("the pipe could not accept a client ({e})");
                            std::thread::sleep(WAIT_POLL);
                        }
                    }
                }
            })
    };
    served.shutdown.notified().await;
    stopper.stop();
    if let Ok(acceptor) = acceptor {
        let _ = tokio::task::spawn_blocking(move || acceptor.join()).await;
    }
}

#[cfg(test)]
mod tests;
