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
use crate::client::Transport as _;
use crate::daemon::socket::{self, Peer};
use crate::daemon::wire::{
    self, ClientHello, DaemonHello, DaemonStatus, HelloAnswer, Refusal, RefusalKind, Reply, Request,
};
use crate::embedded::InProcess;
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
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

/// How often an idle subscription writes to its client anyway, so that a client that has gone is
/// found by the failed write and its connection does not keep the daemon alive.
const SUBSCRIPTION_BEAT: Duration = Duration::from_secs(2);

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
    clients: AtomicU32,
    stopping: AtomicBool,
    last_activity: Mutex<Instant>,
    /// Counts what the driver's host said changed; each subscriber waits for it to move.
    seen: (Mutex<u64>, Condvar),
    shutdown: tokio::sync::Notify,
    /// What the first `stop` answered, for the ones after it.
    stopped: Mutex<Option<(Vec<String>, Vec<String>)>>,
    /// The test seam of the repeat after a break ([`CUT_ANSWER_ENV`]): the request whose first
    /// answer is not sent. Always none in a release.
    cut_answer: Mutex<Option<String>>,
}

/// A client is connected for as long as this lives.
struct Connected<'a>(&'a Served);

impl<'a> Connected<'a> {
    fn new(served: &'a Served) -> Self {
        served.clients.fetch_add(1, Ordering::SeqCst);
        served.touch();
        Self(served)
    }
}

impl Drop for Connected<'_> {
    fn drop(&mut self) {
        self.0.clients.fetch_sub(1, Ordering::SeqCst);
        self.0.touch();
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
    fn session_leaders(&self) -> Vec<u32> {
        let mut leaders = Vec::new();
        leaders.extend(self.app.worker.process_id());
        if let Some(fetch) = &self.app.fetch_worker {
            leaders.extend(fetch.process_id());
        }
        leaders.extend(self.app.servers.process_ids());
        leaders
    }

    fn refuse_own_tree(&self, peer: Peer) -> Option<Refusal> {
        let pid = peer.pid?;
        let leaders = self.session_leaders();
        let own = match socket::in_own_tree(pid, &leaders) {
            Ok(own) => own,
            // A session that cannot be read is refused, never served.
            Err(e) => {
                tracing::warn!(
                    pid,
                    "the session of a client could not be read ({e}): refused"
                );
                true
            }
        };
        own.then(|| Refusal {
            kind: RefusalKind::OwnTree,
            message: crate::messages::daemon_own_tree_refused(pid),
            reference: None,
        })
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

    /// One connection, from its hello to its end.
    fn connection(&self, stream: &mut UnixStream, peer: Peer) -> Result<(), FrameError> {
        let _connected = Connected::new(self);
        let timeout = Some(Duration::from_millis(LIM_O14_DAEMON_CONNECT_MS));
        stream.set_read_timeout(timeout)?;
        stream.set_write_timeout(timeout)?;
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
        let (mut refusal, writes_refused) = match self.refuse_own_tree(peer) {
            Some(own_tree) => {
                tracing::warn!(pid = ?peer.pid, "a client of the daemon's own tree was refused");
                if let Err(e) = self.runtime.record_server_events(&[Event::DaemonRefused {
                    reason: "own_tree".into(),
                    peer_pid: peer.pid,
                }]) {
                    tracing::warn!("the refusal could not be recorded: {e}");
                }
                (Some(own_tree), None)
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
        if refusal
            .as_ref()
            .is_some_and(|r| r.kind == RefusalKind::OwnTree)
        {
            return Ok(());
        }
        tracing::info!(pid = ?peer.pid, accepted, "client connected");
        stream.set_read_timeout(None)?;
        stream.set_write_timeout(None)?;
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
            }
        }
    }

    /// A connection that is told, by a frame, that something changed. It ends when the client
    /// goes (a write fails) or the daemon stops.
    fn subscription(&self, stream: &mut UnixStream) -> Result<(), FrameError> {
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
        DaemonStatus {
            pid: std::process::id(),
            runtime_version: crate::daemon::wire::runtime_version().to_owned(),
            started_at: self.started_at.clone(),
            uptime_secs: self.started.elapsed().as_secs(),
            socket: self.socket.to_string_lossy().into_owned(),
            config_hash: self.config_hash.clone(),
            clients: self.clients.load(Ordering::SeqCst),
            running: runtime.running_tasks().unwrap_or_default(),
            queued: self.transport.driver().waiting(),
            waiting_user,
            log: self.options.log.to_string_lossy().into_owned(),
            idle_secs: self.options.idle.map(|d| d.as_secs()),
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
            std::thread::sleep(Duration::from_millis(50));
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

    /// Whether the daemon has nothing to do: no client, and no task running, queued or waiting
    /// for the user (LIM-O13).
    fn idle_now(&self) -> bool {
        if self.clients.load(Ordering::SeqCst) > 0 {
            return false;
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
        !(busy("running")
            || busy("waiting_user")
            || busy("recovering")
            || !self.transport.driver().waiting().is_empty()
            || !self.transport.driver().owned_tasks().is_empty())
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
    socket::check_directory(&run_dir).map_err(|e| AppError::Io {
        detail: e.to_string(),
    })?;
    let path = socket::socket_path(&run_dir, &data_dir)
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
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(socket::SOCKET_MODE))
            .map_err(|e| AppError::Io {
                detail: format!("cannot set the mode of {}: {}", path.display(), e.kind()),
            })?;
    }
    socket::check_socket_file(&path).map_err(|e| AppError::Io {
        detail: e.to_string(),
    })?;
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
        clients: AtomicU32::new(0),
        stopping: AtomicBool::new(false),
        last_activity: Mutex::new(Instant::now()),
        seen: (Mutex::new(0), Condvar::new()),
        shutdown: tokio::sync::Notify::new(),
        stopped: Mutex::new(None),
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
            loop {
                tokio::time::sleep(tick).await;
                let quiet = served
                    .last_activity
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .elapsed();
                if quiet >= idle && served.idle_now() {
                    tracing::info!(
                        idle_secs = idle.as_secs(),
                        "the daemon has had nothing to do for its idle time and exits"
                    );
                    served.shutdown.notify_one();
                    return;
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
                if stream.set_nonblocking(false).is_err() {
                    continue;
                }
                let served = Arc::clone(&served);
                let executor = tokio::runtime::Handle::current();
                let spawned = std::thread::Builder::new()
                    .name("daemon-connection".into())
                    .spawn(move || {
                        // The runtime's verbs reach the executor (a server is started, a cancel
                        // waits): the thread of a connection runs inside the daemon's own.
                        let _inside = executor.enter();
                        let mut stream = stream;
                        // A defect in the handling of one request ends that connection, and
                        // never the daemon, which owns every client's tasks.
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
        }
    }
    drop(listener);
    let _ = std::fs::remove_file(&path);
    drop(registered);
    let closed = served.app.close();
    tracing::info!("the daemon has ended");
    closed
}

#[cfg(test)]
mod tests;
