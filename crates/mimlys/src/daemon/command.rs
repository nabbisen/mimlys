//! `mimlys daemon start | stop | restart | status` (RFC 014, Amendment 1, item 10; external design
//! §19.5; REQ-CLI-008): the lifecycle of the daemon, as the user sees it.
//!
//! - **`start`** detaches: the process it starts leads a session of its own, has its standard
//!   streams closed once it listens, and is waited for until it answers a hello (LIM-O23), or
//!   fails with the reason it gave. `--foreground` keeps it attached, for a service manager or a
//!   test.
//! - **`stop`** pauses the running tasks at their boundary, lists them and how to resume them,
//!   and ends the daemon. It resumes nothing.
//! - **`restart`** is a stop, a start, and the resumption of exactly the tasks the stop paused.
//! - **`status`** says what the daemon is, and, on request, the tail of its log.

use crate::app::{App, AppError};
use crate::config::Loaded;
use crate::daemon::connect::{self, Daemon, Reach};
use crate::daemon::plat;
use crate::daemon::wire::{self, DaemonStatus, Reply, Request};
use crate::messages;
use crate::output::Output;
use mimlys_core::api::{Envelope, Outcome, Reply as ControlReply, Request as Control};
use mimlys_core::ids::{RequestId, SystemIds, TaskId};
use mimlys_core::limits::{LIM_O23_DAEMON_START_MS, LIM_O24_DAEMON_STOP_MS};
use mimlys_core::runtime::Session;
use serde_json::json;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// How often a wait for the daemon looks again.
const POLL: Duration = Duration::from_millis(50);

fn io(detail: impl Into<String>) -> AppError {
    AppError::Io {
        detail: detail.into(),
    }
}

/// The registry's directory and the data directory a command is about.
pub fn places(loaded: &Loaded) -> Result<(PathBuf, PathBuf), AppError> {
    let run_dir = crate::registry::default_dir().map_err(|reason| {
        io(format!(
            "the registry of the user's runtimes cannot be found: {reason}"
        ))
    })?;
    Ok((run_dir, loaded.config.data_dir()?))
}

/// Looks for the daemon, and says `daemon not running` once when the socket of a dead one is
/// removed (external design §20).
pub fn find(loaded: &Loaded, quiet: bool) -> Result<Option<Box<Daemon>>, AppError> {
    let (run_dir, data_dir) = places(loaded)?;
    match connect::reach(&run_dir, &data_dir, loaded, true).map_err(|e| io(e.to_string()))? {
        Reach::Daemon(daemon) => Ok(Some(daemon)),
        Reach::Dead => {
            if !quiet {
                crate::output::diag(messages::DAEMON_NOT_RUNNING);
            }
            Ok(None)
        }
        Reach::None | Reach::Unready => Ok(None),
    }
}

/// Sends one control request over a greeted connection and reads its answer, passing the notices
/// the daemon sends before it to `notice`.
pub fn control_once(
    daemon: &mut Daemon,
    envelope: Envelope,
    notice: &dyn Fn(mimlys_core::api::Notice),
) -> Result<mimlys_core::api::Response, AppError> {
    wire::write(
        &mut daemon.stream,
        &Request::Control {
            envelope: Box::new(envelope),
        },
    )
    .map_err(|e| io(e.to_string()))?;
    loop {
        match wire::read::<Reply>(&mut daemon.stream).map_err(|e| io(e.to_string()))? {
            Reply::Notice { notice: n } => notice(n),
            Reply::Control { answer } => return Ok(*answer),
            Reply::Refused { refusal } => return Err(io(refusal.shown())),
            other => return Err(io(format!("the daemon answered {other:?}, not a response"))),
        }
    }
}

/// The daemon of `mimlys daemon start --foreground`: the one process that serves. With
/// `detached` (the process `start` made) it leads a session of its own and closes its standard
/// streams once it listens.
pub async fn serve_here(loaded: Loaded, detached: bool) -> Result<(), AppError> {
    // A new session: closing the terminal does not end the daemon. The process was made with
    // no group of its own, so this succeeds. (On Windows `start` made it with no console and in a
    // process group of its own: there is nothing to do here.)
    #[cfg(unix)]
    if detached {
        let _ = rustix::process::setsid();
    }
    // The seam of a start that fails (a debug build only): the daemon ends at once, saying why.
    if cfg!(debug_assertions)
        && let Ok(why) = std::env::var(START_FAILS_ENV)
    {
        return Err(io(why));
    }
    // The tool servers this daemon starts are marked as its own tree (Windows).
    #[cfg(windows)]
    crate::daemon::marks::enable();
    let data_dir = loaded.config.data_dir()?;
    let log_path = crate::daemon::log::log_path(&data_dir);
    mimlys_exec::confine::create_private_dir(&data_dir)
        .map_err(|e| io(format!("cannot create the data directory: {}", e.kind())))?;
    let log = std::sync::Arc::new(
        crate::daemon::log::RotatingLog::open(&log_path)
            .map_err(|e| io(format!("cannot open the daemon's log: {}", e.kind())))?,
    );
    crate::logging::init_daemon(&loaded.config.logging, &loaded.secrets, log);
    let idle = crate::daemon::server::idle_in_force(loaded.config.runtime.daemon_idle.duration());
    let app = App::open(loaded)?;
    crate::daemon::server::serve(
        app,
        crate::daemon::server::Options {
            idle,
            log: log_path,
        },
        move || {
            if detached {
                close_standard_streams();
            }
        },
    )
    .await
}

/// Points the standard streams at `NUL`: the daemon has no terminal and writes nothing to one. Its
/// log is a file.
#[cfg(windows)]
fn close_standard_streams() {
    let _ = mimlys_exec::daemon_pipe::detach_standard_streams();
}

/// Points the standard streams at `/dev/null`: the daemon has no terminal and writes nothing to
/// one. Its log is a file.
#[cfg(unix)]
fn close_standard_streams() {
    let Ok(null) = rustix::fs::open(
        "/dev/null",
        rustix::fs::OFlags::RDWR,
        rustix::fs::Mode::empty(),
    ) else {
        return;
    };
    let _ = rustix::stdio::dup2_stdin(&null);
    let _ = rustix::stdio::dup2_stdout(&null);
    let _ = rustix::stdio::dup2_stderr(&null);
}

/// `daemon start`: the daemon, detached, or already running.
pub fn start(loaded: &Loaded, out: &Output) -> Result<(), AppError> {
    start_with(loaded, out, true)
}

/// The process `start` made, and the thread that reads what it says on its standard error until
/// it listens (so that a start that fails says why).
struct Spawned {
    child: std::process::Child,
    reader: std::thread::JoinHandle<String>,
}

/// Starts the process that will be the daemon: this program with `daemon start --foreground
/// --detached`, in this process's environment (a daemon started by a client has that client's, and
/// so its secrets, external design §19.5), detached from the terminal.
fn spawn_daemon(loaded: &Loaded) -> Result<Spawned, AppError> {
    let mut command = std::process::Command::new(
        std::env::current_exe()
            .map_err(|e| io(format!("cannot find this program: {}", e.kind())))?,
    );
    // The daemon is given its three standard streams and nothing else of this process's.
    #[cfg(windows)]
    mimlys_exec::daemon_pipe::keep_standard_handles_from_children();
    command.args(["daemon", "start", "--foreground", "--detached"]);
    if let Some(path) = &loaded.path {
        command.arg("--config").arg(path);
    }
    detach(&mut command);
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|e| io(format!("cannot start the daemon: {}", e.kind())))?;
    let mut stderr = child.stderr.take();
    let reader = std::thread::spawn(move || {
        let mut text = String::new();
        if let Some(stderr) = stderr.as_mut() {
            let _ = std::io::Read::read_to_string(stderr, &mut text);
        }
        text
    });
    Ok(Spawned { child, reader })
}

impl Spawned {
    /// Why the process ended, as the user is told: what it said (the reason, without the
    /// `error (kind): ` a command puts before it), or its exit code.
    fn failure(self, status: std::process::ExitStatus, data_dir: &std::path::Path) -> String {
        let said = self.reader.join().unwrap_or_default();
        let said = said
            .trim()
            .split_once("): ")
            .filter(|(kind, _)| kind.starts_with("error ("))
            .map_or(said.trim(), |(_, reason)| reason)
            .to_owned();
        messages::daemon_did_not_start(
            status.code(),
            &said,
            &crate::daemon::log::log_path(data_dir),
        )
    }
}

/// As [`start`]; `value` says whether it prints its JSON value (a restart prints its own).
fn start_with(loaded: &Loaded, out: &Output, value: bool) -> Result<(), AppError> {
    let (run_dir, data_dir) = places(loaded)?;
    if let Some(daemon) = find(loaded, false)? {
        if !daemon.usable() {
            return Err(io(messages::daemon_version_refused(
                crate::daemon::wire::runtime_version(),
                &daemon.hello.runtime_version,
            )));
        }
        say_started(
            out,
            &daemon.hello.runtime_version,
            daemon.peer.pid,
            &daemon.socket,
            true,
            value,
        );
        return Ok(());
    }
    let mut spawned = spawn_daemon(loaded)?;
    let begun = Instant::now();
    let limit = Duration::from_millis(LIM_O23_DAEMON_START_MS);
    loop {
        if let Ok(Some(status)) = spawned.child.try_wait() {
            return Err(io(spawned.failure(status, &data_dir)));
        }
        if let Ok(Reach::Daemon(daemon)) = connect::reach(&run_dir, &data_dir, loaded, false)
            && daemon.hello.runtime_version == crate::daemon::wire::runtime_version()
        {
            say_started(
                out,
                &daemon.hello.runtime_version,
                daemon.peer.pid,
                &daemon.socket,
                false,
                value,
            );
            // The child is the daemon's own now: it is not waited for.
            std::mem::forget(spawned.child);
            return Ok(());
        }
        if begun.elapsed() >= limit {
            let _ = spawned.child.kill();
            let _ = spawned.child.wait();
            return Err(io(messages::daemon_start_timed_out(
                LIM_O23_DAEMON_START_MS,
                &crate::daemon::log::log_path(&data_dir),
            )));
        }
        std::thread::sleep(POLL);
    }
}

/// The test seam of the on-demand start's race: `MIMLYS_ON_DEMAND_SPAWN_AT_MS=<epoch ms>` makes a
/// **client** (in a debug build only) wait for that instant before it starts a daemon, so that two
/// clients that began apart start theirs at the same moment, and the store's lock has to decide.
pub const SPAWN_AT_ENV: &str = "MIMLYS_ON_DEMAND_SPAWN_AT_MS";

/// The test seam of a start that fails: `MIMLYS_DAEMON_START_FAILS=<reason>` makes the daemon
/// process (in a debug build only) end at once with that reason, as a daemon that cannot start does.
pub const START_FAILS_ENV: &str = "MIMLYS_DAEMON_START_FAILS";

/// Whether nobody holds the store: its lock is free, or there is no store yet.
fn store_is_free(data_dir: &std::path::Path) -> bool {
    mimlys_persist::open_read_only(data_dir)
        .map(|store| store.probe_lock() == mimlys_persist::LockState::Free)
        .unwrap_or(true)
}

/// The on-demand start (`[runtime] serve = "on_demand"`; external design §19.5; REQ-CLI-008): the
/// client that finds no daemon starts one, detached, waits for its hello (LIM-O23), says once that
/// it started one with its pid, and returns it for the request to go through.
///
/// - **Two clients at once:** each may start one; **the store's lock decides**. The daemon that
///   does not get it ends, naming the holder, and its client waits for the other's hello and goes
///   through that daemon (it says nothing of having started one: it did not).
/// - **A store that someone else holds** (a daemon another client is starting, or an embedded
///   runtime) is waited for, up to LIM-O23, for it to be a daemon that answers; a client that
///   waits longer than LIM-O14 says whose the store is.
/// - **A start that fails says why** (what the process said, or its exit code), when the store is
///   free: there is no other holder to wait for. The command then **does not run embedded**: a user
///   who asked for a daemon is not given another mode without being told.
pub fn start_on_demand(loaded: &Loaded) -> Result<Box<Daemon>, AppError> {
    let (run_dir, data_dir) = places(loaded)?;
    let begun = Instant::now();
    let limit = Duration::from_millis(LIM_O23_DAEMON_START_MS);
    let mut spawned: Option<Spawned> = None;
    let mut failure: Option<String> = None;
    let mut said_waiting = false;
    loop {
        // A daemon that answers: the one this client started, or the one another client did.
        match connect::reach(&run_dir, &data_dir, loaded, false) {
            Ok(Reach::Daemon(daemon)) => {
                if !daemon.usable() {
                    if let Some(mut s) = spawned.take() {
                        let _ = s.child.kill();
                    }
                    return Err(io(messages::daemon_version_refused(
                        crate::daemon::wire::runtime_version(),
                        &daemon.hello.runtime_version,
                    )));
                }
                if let Some(s) = spawned.take() {
                    if daemon.peer.pid == Some(s.child.id()) {
                        let idle = loaded.config.runtime.daemon_idle.text();
                        crate::output::diag(messages::daemon_started_on_demand(
                            s.child.id(),
                            &idle,
                        ));
                    }
                    // The child is the daemon's own (or has ended): it is not waited for.
                    std::mem::forget(s.child);
                }
                return Ok(daemon);
            }
            Ok(_) => {}
            Err(e) => {
                if let Some(mut s) = spawned.take() {
                    let _ = s.child.kill();
                }
                return Err(io(e.to_string()));
            }
        }
        // The process this client started has ended: if nobody holds the store, it could not
        // start, and says why; if someone does, it lost the lock to another daemon, which is what
        // this client waits for.
        if let Some(s) = spawned.as_mut()
            && let Ok(Some(status)) = s.child.try_wait()
        {
            let s = spawned.take().expect("just seen");
            let why = s.failure(status, &data_dir);
            if store_is_free(&data_dir) {
                return Err(io(why));
            }
            failure = Some(why);
        }
        // Nobody holds the store and this client has no process running: start one.
        if spawned.is_none() && store_is_free(&data_dir) {
            if cfg!(debug_assertions)
                && let Some(at) = std::env::var(SPAWN_AT_ENV)
                    .ok()
                    .and_then(|v| v.parse::<u64>().ok())
            {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(0));
                if at > now {
                    std::thread::sleep(Duration::from_millis(at - now));
                }
            }
            spawned = Some(spawn_daemon(loaded)?);
        }
        if begun.elapsed() >= limit {
            if let Some(mut s) = spawned.take() {
                let _ = s.child.kill();
                let _ = s.child.wait();
            }
            return Err(io(failure.unwrap_or_else(|| {
                if store_is_free(&data_dir) {
                    messages::daemon_start_timed_out(
                        LIM_O23_DAEMON_START_MS,
                        &crate::daemon::log::log_path(&data_dir),
                    )
                } else {
                    messages::daemon_on_demand_store_held(
                        &crate::app::store_in_use(&data_dir).to_string(),
                    )
                }
            })));
        }
        if !said_waiting
            && spawned.is_none()
            && begun.elapsed()
                >= Duration::from_millis(mimlys_core::limits::LIM_O14_DAEMON_CONNECT_MS)
        {
            said_waiting = true;
            crate::output::diag(messages::daemon_waiting_for_the_store(
                &crate::app::store_in_use(&data_dir).to_string(),
            ));
        }
        std::thread::sleep(POLL);
    }
}

/// What makes the process `start` spawns a daemon and no child of the terminal. On Unix it leads a
/// session of its own once it runs (`serve_here`). On Windows it is made with no console window and
/// in a process group of its own, so that the terminal's Ctrl-C and its closing do not reach it.
/// (`DETACHED_PROCESS` would leave it no console at all, and each console program it starts, a tool
/// server for one, would open a window of its own: `CREATE_NO_WINDOW` gives it a console that
/// has none, which its children share.)
#[cfg(windows)]
fn detach(command: &mut std::process::Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
}

#[cfg(unix)]
fn detach(_command: &mut std::process::Command) {}

fn say_started(
    out: &Output,
    version: &str,
    pid: Option<u32>,
    socket: &std::path::Path,
    was: bool,
    value: bool,
) {
    out.line(messages::daemon_started(
        version,
        pid,
        &socket.to_string_lossy(),
        was,
    ));
    if !value {
        return;
    }
    out.value(&json!({
        "daemon": {
            "running": true,
            "already_running": was,
            "pid": pid,
            "version": version,
            "socket": socket.to_string_lossy(),
        }
    }));
}

/// What a stop did.
pub struct Stopped {
    /// The daemon's process.
    pub pid: u32,
    /// The tasks it paused.
    pub paused: Vec<String>,
    /// The tasks that waited in line and did not start.
    pub queued: Vec<String>,
}

/// An action that cannot be undone, in a task the stop pauses, which the stop waits for.
struct Waited {
    task_id: mimlys_core::ids::TaskId,
    action_id: mimlys_core::ids::ActionId,
    tool: String,
}

/// While a stop waits for the daemon's answer: says, once for each, which action of which task the
/// daemon waits for and until when at the latest (external design §19.5), so that a wait of up to
/// LIM-O24 for something that cannot be undone is never silent. The client reads the store as any
/// reader does (REQ-PERS-013): the daemon's answer is a single frame and is fixed for every
/// version, so it carries nothing of this. The deadline is the action's own, counted from when the
/// client first saw it in flight (it may have begun earlier, so this is a bound, never early), and
/// never past the stop's own limit.
fn watch_the_wait(
    data_dir: PathBuf,
    begun: Instant,
    done: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> std::thread::JoinHandle<Vec<Waited>> {
    std::thread::spawn(move || {
        use mimlys_core::clock::{Clock, SystemClock};
        use mimlys_core::spi::PersistenceRead;
        let mut waited: Vec<Waited> = Vec::new();
        let Ok(store) = mimlys_persist::open_read_only(&data_dir) else {
            return waited;
        };
        let poll = Duration::from_millis(crate::commands::events::EVENTS_POLL_MS);
        let mut last = Instant::now()
            .checked_sub(poll)
            .unwrap_or_else(Instant::now);
        while !done.load(std::sync::atomic::Ordering::SeqCst) {
            if last.elapsed() >= poll {
                last = Instant::now();
                for task_id in store.tasks_in_state("running").unwrap_or_default() {
                    let Ok(Some(action)) = store.load_running_action(task_id) else {
                        continue;
                    };
                    let effect = action.content().effect();
                    if mimlys_core::action::can_be_undone(effect)
                        || waited.iter().any(|w| w.action_id == action.action_id)
                    {
                        continue;
                    }
                    let left = Duration::from_millis(LIM_O24_DAEMON_STOP_MS)
                        .saturating_sub(begun.elapsed())
                        .as_millis();
                    let wait_ms = i64::try_from(left)
                        .unwrap_or(i64::MAX)
                        .min(action.content().deadline_ms().max(0));
                    let until = SystemClock
                        .now()
                        .plus_millis(wait_ms)
                        .map(|t| t.to_rfc3339())
                        .unwrap_or_default();
                    crate::output::diag(messages::daemon_stop_waits(
                        &task_id.to_string(),
                        action.content().tool(),
                        effect.kind,
                        &until,
                    ));
                    waited.push(Waited {
                        task_id,
                        action_id: action.action_id,
                        tool: action.content().tool().to_owned(),
                    });
                }
            }
            std::thread::sleep(POLL);
        }
        waited
    })
}

/// Says again, for each action the stop waited for, how the wait ended: the action finished, or the
/// daemon ended at its limit before it did and what it did is unknown (REQ-CLI-005).
fn say_the_wait_ended(data_dir: &std::path::Path, waited: &[Waited]) {
    use mimlys_core::action::ActionStatus;
    use mimlys_core::spi::PersistenceRead;
    let store = mimlys_persist::open_read_only(data_dir).ok();
    for w in waited {
        let status = store
            .as_ref()
            .and_then(|s| s.load_actions(w.task_id).ok())
            .and_then(|actions| actions.into_iter().find(|a| a.action_id == w.action_id))
            .map(|a| a.status);
        let finished = status
            .as_ref()
            .is_some_and(|s| s.is_terminal() && *s != ActionStatus::Unknown);
        let (task, tool) = (w.task_id.to_string(), w.tool.as_str());
        crate::output::diag(if finished {
            messages::daemon_stop_wait_finished(&task, tool)
        } else {
            messages::daemon_stop_wait_gave_up(&task, tool)
        });
    }
}

/// Sends `stop` to the daemon, waits for its answer (LIM-O24) and for its process to end. While it
/// waits it says what it waits for ([`watch_the_wait`]), and says again when that ends.
fn stop_daemon(mut daemon: Box<Daemon>, loaded: &Loaded) -> Result<Stopped, AppError> {
    let (_, data_dir) = places(loaded)?;
    let begun = Instant::now();
    // The timeout is set before `stop` is sent, while the daemon is certain to be there; and a
    // daemon that has answered and gone by the time it is set is its end, not an error
    // (`socket::set_timeouts`): what it answered is read.
    plat::set_timeouts(
        &daemon.stream,
        Some(Duration::from_millis(LIM_O24_DAEMON_STOP_MS)),
        None,
    )
    .map_err(|e| io(e.to_string()))?;
    wire::write(&mut daemon.stream, &Request::Stop).map_err(|e| io(e.to_string()))?;
    let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let watch = watch_the_wait(data_dir.clone(), begun, std::sync::Arc::clone(&done));
    let reply = wire::read::<Reply>(&mut daemon.stream);
    done.store(true, std::sync::atomic::Ordering::SeqCst);
    let waited = watch.join().unwrap_or_default();
    // The wait is over when the daemon has answered; a daemon that did not answer is the
    // command's error, and says nothing of what the action came to.
    if reply.is_ok() {
        say_the_wait_ended(&data_dir, &waited);
    }
    let reply = reply.map_err(|e| io(e.to_string()))?;
    let Reply::Stopped {
        pid,
        paused,
        queued,
    } = reply
    else {
        return Err(io(format!("the daemon answered {reply:?} to stop")));
    };
    // The daemon ends after it has answered: its lock is free when it has.
    let begun = Instant::now();
    while begun.elapsed() < Duration::from_millis(LIM_O23_DAEMON_START_MS) {
        let free = mimlys_persist::open_read_only(&data_dir)
            .map(|s| s.probe_lock() == mimlys_persist::LockState::Free)
            .unwrap_or(true);
        if free {
            break;
        }
        std::thread::sleep(POLL);
    }
    Ok(Stopped {
        pid,
        paused,
        queued,
    })
}

fn say_stopped(out: &Output, stopped: &Stopped) {
    out.line(messages::daemon_stopped(stopped.pid));
    for id in &stopped.paused {
        out.line(messages::daemon_paused(id));
    }
    for id in &stopped.queued {
        out.line(messages::daemon_queued_not_started(id));
    }
    out.value(&json!({
        "daemon": {
            "running": false,
            "pid": stopped.pid,
            "paused": stopped.paused,
            "queued": stopped.queued,
        }
    }));
}

/// `daemon stop`.
pub fn stop(loaded: &Loaded, out: &Output) -> Result<(), AppError> {
    let Some(daemon) = find(loaded, false)? else {
        out.line(messages::DAEMON_NONE_TO_STOP);
        out.value(&json!({ "daemon": { "running": false } }));
        return Ok(());
    };
    let stopped = stop_daemon(daemon, loaded)?;
    say_stopped(out, &stopped);
    Ok(())
}

/// `daemon restart`: a stop, a start, and the resumption of exactly the tasks the stop paused.
pub fn restart(loaded: &Loaded, out: &Output) -> Result<(), AppError> {
    let stopped = match find(loaded, false)? {
        Some(daemon) => {
            let stopped = stop_daemon(daemon, loaded)?;
            say_stopped_lines(out, &stopped);
            Some(stopped)
        }
        None => None,
    };
    start_with(loaded, out, stopped.is_none())?;
    let Some(stopped) = stopped else {
        return Ok(());
    };
    let Some(mut daemon) = find(loaded, true)? else {
        return Err(io("the daemon that was started does not answer"));
    };
    let session = Session {
        id: format!("pid:{}", std::process::id()),
        os_user: crate::app::os_user(),
    };
    let mut resumed = Vec::new();
    let mut back_in_line = Vec::new();
    // First what the stop paused, which ran ahead of the line; then the tasks that waited in it,
    // in their order, so that the new daemon's line is the old one (external design §19.5).
    for (id, queued) in stopped
        .paused
        .iter()
        .map(|id| (id, false))
        .chain(stopped.queued.iter().map(|id| (id, true)))
    {
        let Ok(task_id) = uuid::Uuid::parse_str(id).map(TaskId::from_uuid) else {
            continue;
        };
        let answer = control_once(
            &mut daemon,
            Envelope {
                request_id: RequestId::new(&SystemIds),
                session: session.clone(),
                request: Control::ResumeTask {
                    task_id,
                    extend: None,
                },
            },
            &|_| {},
        )?;
        // What the daemon did with it: started it, put it in line behind a task, or refused.
        let (started, behind) = match answer.outcome {
            Outcome::Done(ControlReply::Queued { line, .. }) => (false, Some(line.behind)),
            Outcome::Done(ControlReply::Task(_)) | Outcome::AlreadyApplied(_) => (true, None),
            Outcome::Failed(error) => {
                crate::output::diag(if queued {
                    messages::daemon_not_back_in_line(id, &error.to_string())
                } else {
                    messages::daemon_not_resumed(id, &error.to_string())
                });
                continue;
            }
            Outcome::Done(_) => continue,
        };
        let behind = behind.map(|b| b.to_string());
        if queued {
            out.line(messages::daemon_back_in_line(id, behind.as_deref()));
            back_in_line.push(id.clone());
        } else {
            out.line(if started {
                messages::daemon_resumed(id)
            } else {
                messages::daemon_back_in_line(id, behind.as_deref())
            });
            resumed.push(id.clone());
        }
    }
    out.value(&json!({
        "daemon": { "restarted": true, "resumed": resumed, "back_in_line": back_in_line }
    }));
    Ok(())
}

/// The lines of a stop, without its JSON: `restart` prints one JSON value at its end.
fn say_stopped_lines(out: &Output, stopped: &Stopped) {
    out.line(messages::daemon_stopped(stopped.pid));
    for id in &stopped.paused {
        out.line(messages::daemon_paused(id));
    }
    for id in &stopped.queued {
        out.line(messages::daemon_queued_not_started(id));
    }
}

/// `daemon status`: what the daemon is, and, with `log`, the tail of its log.
pub fn status(loaded: &Loaded, log: Option<usize>, out: &Output) -> Result<(), AppError> {
    let Some(mut daemon) = find(loaded, false)? else {
        out.line(messages::daemon_none_for(
            &places(loaded)?.1.to_string_lossy(),
        ));
        out.value(&json!({ "daemon": { "running": false } }));
        return Ok(());
    };
    if !daemon.usable() {
        return Err(io(messages::daemon_version_refused(
            wire::runtime_version(),
            &daemon.hello.runtime_version,
        )));
    }
    wire::write(&mut daemon.stream, &Request::Status).map_err(|e| io(e.to_string()))?;
    let Reply::Status { status } = wire::read(&mut daemon.stream).map_err(|e| io(e.to_string()))?
    else {
        return Err(io("the daemon did not answer with its status"));
    };
    let mine = wire::config_hash(loaded.path.as_deref());
    for line in messages::daemon_status_lines(&status, mine == status.config_hash) {
        out.line(line);
    }
    let tail = log.map(|n| crate::daemon::log::tail(std::path::Path::new(&status.log), n));
    for line in tail.iter().flatten() {
        out.line(line);
    }
    out.value(&json!({ "daemon": daemon_json(&status, mine == status.config_hash, tail) }));
    Ok(())
}

/// The status as `--json` shows it.
pub fn daemon_json(
    status: &DaemonStatus,
    same_configuration: bool,
    tail: Option<Vec<String>>,
) -> serde_json::Value {
    json!({
        "running": true,
        "pid": status.pid,
        "version": status.runtime_version,
        "started_at": status.started_at,
        "uptime_secs": status.uptime_secs,
        "socket": status.socket,
        "config_hash": status.config_hash,
        "same_configuration": same_configuration,
        "clients": status.clients,
        "connections": status.connections,
        "running_tasks": status.running,
        "queued": status.queued.iter().map(|(id, line)| json!({
            "task_id": id,
            "behind": line.behind,
            "running_since": line.running_since,
        })).collect::<Vec<_>>(),
        "waiting_user": status.waiting_user,
        "log": status.log,
        "idle_secs": status.idle_secs,
        "log_tail": tail,
    })
}

/// The mode `status` says (external design §19.5; REQ-ARCH-009): the daemon that serves this data
/// directory, with what `daemon status` says first, or none (embedded). A daemon that cannot be
/// reached for a reason that is not "there is none" (a directory that is not the user's own, a
/// refusal) is said, and the mode is embedded: `status` is a diagnosis and does not stop at it.
pub fn mode(loaded: &Loaded) -> (Option<DaemonStatus>, Option<String>) {
    let daemon = match find(loaded, false) {
        Ok(Some(daemon)) => daemon,
        Ok(None) => return (None, None),
        Err(e) => return (None, Some(e.to_string())),
    };
    let mut daemon = daemon;
    if !daemon.usable() {
        return (
            None,
            Some(messages::daemon_version_refused(
                crate::daemon::wire::runtime_version(),
                &daemon.hello.runtime_version,
            )),
        );
    }
    if wire::write(&mut daemon.stream, &Request::Status).is_err() {
        return (None, None);
    }
    match wire::read::<Reply>(&mut daemon.stream) {
        Ok(Reply::Status { status }) => (Some(*status), None),
        _ => (None, None),
    }
}
