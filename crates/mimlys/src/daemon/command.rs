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
            Reply::Refused { refusal } => return Err(io(refusal.message)),
            other => return Err(io(format!("the daemon answered {other:?}, not a response"))),
        }
    }
}

/// The daemon of `mimlys daemon start --foreground`: the one process that serves. With
/// `detached` (the process `start` made) it leads a session of its own and closes its standard
/// streams once it listens.
pub async fn serve_here(loaded: Loaded, detached: bool) -> Result<(), AppError> {
    if detached {
        // A new session: closing the terminal does not end the daemon. The process was made with
        // no group of its own, so this succeeds.
        let _ = rustix::process::setsid();
    }
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

/// Points the standard streams at `/dev/null`: the daemon has no terminal and writes nothing to
/// one. Its log is a file.
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
    let (run_dir, data_dir) = places(loaded)?;
    if let Some(daemon) = find(loaded, false)? {
        if !daemon.usable() {
            return Err(io(messages::daemon_version_refused(
                env!("CARGO_PKG_VERSION"),
                &daemon.hello.runtime_version,
            )));
        }
        say_started(
            out,
            &daemon.hello.runtime_version,
            daemon.peer.pid,
            &daemon.socket,
            true,
        );
        return Ok(());
    }
    let mut command = std::process::Command::new(
        std::env::current_exe()
            .map_err(|e| io(format!("cannot find this program: {}", e.kind())))?,
    );
    command.args(["daemon", "start", "--foreground", "--detached"]);
    if let Some(path) = &loaded.path {
        command.arg("--config").arg(path);
    }
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
    let begun = Instant::now();
    let limit = Duration::from_millis(LIM_O23_DAEMON_START_MS);
    loop {
        if let Ok(Some(status)) = child.try_wait() {
            let said = reader.join().unwrap_or_default();
            // The process said it as a command does (`error (kind): …`): the reason is the rest.
            let said = said
                .trim()
                .split_once("): ")
                .filter(|(kind, _)| kind.starts_with("error ("))
                .map_or(said.trim(), |(_, reason)| reason)
                .to_owned();
            return Err(io(messages::daemon_did_not_start(
                status.code(),
                &said,
                &crate::daemon::log::log_path(&data_dir),
            )));
        }
        if let Ok(Reach::Daemon(daemon)) = connect::reach(&run_dir, &data_dir, loaded, false)
            && daemon.hello.runtime_version == env!("CARGO_PKG_VERSION")
        {
            say_started(
                out,
                &daemon.hello.runtime_version,
                daemon.peer.pid,
                &daemon.socket,
                false,
            );
            // The child is the daemon's own now: it is not waited for.
            std::mem::forget(child);
            return Ok(());
        }
        if begun.elapsed() >= limit {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io(messages::daemon_start_timed_out(
                LIM_O23_DAEMON_START_MS,
                &crate::daemon::log::log_path(&data_dir),
            )));
        }
        std::thread::sleep(POLL);
    }
}

fn say_started(out: &Output, version: &str, pid: Option<u32>, socket: &std::path::Path, was: bool) {
    out.line(messages::daemon_started(
        version,
        pid,
        &socket.to_string_lossy(),
        was,
    ));
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

/// Sends `stop` to the daemon, waits for its answer (LIM-O24) and for its process to end.
fn stop_daemon(mut daemon: Box<Daemon>, loaded: &Loaded) -> Result<Stopped, AppError> {
    wire::write(&mut daemon.stream, &Request::Stop).map_err(|e| io(e.to_string()))?;
    daemon
        .stream
        .set_read_timeout(Some(Duration::from_millis(LIM_O24_DAEMON_STOP_MS)))
        .map_err(|e| io(e.to_string()))?;
    let reply: Reply = wire::read(&mut daemon.stream).map_err(|e| io(e.to_string()))?;
    let Reply::Stopped {
        pid,
        paused,
        queued,
    } = reply
    else {
        return Err(io(format!("the daemon answered {reply:?} to stop")));
    };
    // The daemon ends after it has answered: its lock is free when it has.
    let (_, data_dir) = places(loaded)?;
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
    start(loaded, out)?;
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
    for id in &stopped.paused {
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
        match answer.outcome {
            Outcome::Done(ControlReply::Task(_) | ControlReply::Queued { .. })
            | Outcome::AlreadyApplied(_) => {
                out.line(messages::daemon_resumed(id));
                resumed.push(id.clone());
            }
            Outcome::Failed(error) => {
                crate::output::diag(messages::daemon_not_resumed(id, &error.to_string()));
            }
            Outcome::Done(_) => {}
        }
    }
    out.value(&json!({ "daemon": { "restarted": true, "resumed": resumed } }));
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
