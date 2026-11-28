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

/// A pipe or its peer that cannot be used: refused before anything was sent or read (§20: "a peer
/// of another user").
#[cfg(windows)]
pub fn daemon_socket_refused(error: &crate::daemon::pipe::SocketError) -> String {
    use crate::daemon::pipe::SocketError;
    match error {
        SocketError::Peer { user } => format!(
            "the process at the other end of the daemon's pipe belongs to another user ({user}); nothing was sent or read"
        ),
        SocketError::Io { what, kind } => {
            format!("the daemon's pipe cannot be used: {what} failed ({kind})")
        }
    }
}

/// A client and a daemon of different runtime versions refuse each other at the hello (§20).
pub fn daemon_version_refused(client: &str, daemon: &str) -> String {
    format!(
        "this mimlys is {client} and the daemon runs {daemon}; they do not talk to each other: run `mimlys daemon restart` to restart it under this version"
    )
}

/// A client whose configuration file hashes differently: what changes state is refused (§20).
pub fn daemon_configuration_differs() -> String {
    "the daemon runs another configuration than this file: a request that changes state is refused until `mimlys daemon restart` makes it read this one; what only reads is not affected".to_owned()
}

/// A client whose secret differs from the daemon's: the reference is named, never the value (§20).
pub fn daemon_secret_differs(reference: &str) -> String {
    format!(
        "the value of `{reference}` is not the one the daemon read at its start: a request that changes state is refused until `mimlys daemon restart` makes it read the current one; what only reads is not affected"
    )
}

/// A request from a process of the daemon's own tree (§20, INV-13).
pub fn daemon_own_tree_refused(pid: u32) -> String {
    format!(
        "refused: process {pid} is in the daemon's own tree (the worker, or a tool server it started); only a client outside it may approve, answer, grant or cancel"
    )
}

/// A client whose process id the system did not give (REQ-SEC-016): without it the daemon cannot
/// tell its own tree (the worker's session, a tool server's) from a client, so it serves none.
pub fn daemon_no_peer_pid_refused() -> String {
    "refused: the system gave no process id for this client, and the daemon cannot tell its own tree (the worker, or a tool server it started) from it; nothing was applied".to_owned()
}

/// A refusal whose word this client does not know (a newer daemon's): the daemon's own message,
/// and the word when it is one a person can read (a short word of letters, digits and `_`; the
/// daemon is the user's own, but what it sends is not echoed unclean).
pub fn daemon_refused_for_another_reason(word: &str, message: &str) -> String {
    let readable = !word.is_empty()
        && word.len() <= 40
        && word
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    if readable {
        format!("{message} (the daemon's reason: {word})")
    } else {
        message.to_owned()
    }
}

/// The daemon is stopping, and takes no new request.
pub fn daemon_stopping() -> String {
    "the daemon is stopping".to_owned()
}

/// A socket whose daemon is gone: removed, and the command runs embedded (§20); said once.
pub const DAEMON_NOT_RUNNING: &str =
    "daemon not running (a stale socket was removed); this command runs embedded";

/// What answered at the socket did not greet as a daemon does.
pub fn daemon_hello_failed(why: &str) -> String {
    format!(
        "what answered at the daemon's socket did not greet as a mimlys daemon does ({why}); nothing was sent"
    )
}

/// `daemon start`: the daemon runs.
pub fn daemon_started(version: &str, pid: Option<u32>, socket: &str, was_running: bool) -> String {
    let who = pid.map_or_else(
        || "the daemon".to_owned(),
        |p| format!("the daemon (pid {p})"),
    );
    if was_running {
        format!("{who} is running already (mimlys {version}), at {socket}")
    } else {
        format!("{who} started (mimlys {version}), serving at {socket}")
    }
}

/// `daemon start`: the process ended before it answered.
pub fn daemon_did_not_start(code: Option<i32>, said: &str, log: &std::path::Path) -> String {
    let code = code.map_or_else(|| "a signal".to_owned(), |c| format!("code {c}"));
    if said.is_empty() {
        format!(
            "the daemon did not start: its process ended with {code}; its log is {}",
            log.display()
        )
    } else {
        format!("the daemon did not start: {said}")
    }
}

/// `daemon start`: no answer within LIM-O23.
pub fn daemon_start_timed_out(ms: u64, log: &std::path::Path) -> String {
    format!(
        "the daemon did not answer within {} s and was stopped; its log is {}",
        ms / 1000,
        log.display()
    )
}

/// `daemon stop`: the daemon ended.
pub fn daemon_stopped(pid: u32) -> String {
    format!("the daemon (pid {pid}) stopped")
}

/// `daemon stop`: a task it paused, and how to resume it.
pub fn daemon_paused(task_id: &str) -> String {
    format!("paused task {task_id} at its boundary; resume it with `mimlys task resume {task_id}`")
}

/// `daemon stop`: a task that waited in line and did not start.
pub fn daemon_queued_not_started(task_id: &str) -> String {
    format!(
        "task {task_id} waited in line and did not start; start it with `mimlys task resume {task_id}`"
    )
}

/// `daemon stop` waits for an action that cannot be undone (external design §19.5): which action
/// of which task, why it is waited for, and until when at the latest (its own deadline, within the
/// stop's own limit).
pub fn daemon_stop_waits(
    task_id: &str,
    tool: &str,
    effect: mimlys_core::action::EffectKind,
    until: &str,
) -> String {
    let why = super::why_it_cannot_be_undone(effect)
        .unwrap_or("mimlys cannot tell whether it can be undone");
    format!(
        "the daemon is stopping and waits for {tool}, an action of task {task_id}, to finish: {why}; it waits until {until} at the latest, and the task pauses when it is done"
    )
}

/// `daemon stop`: the wait for an action that cannot be undone ended, and the action finished.
pub fn daemon_stop_wait_finished(task_id: &str, tool: &str) -> String {
    format!("the wait for {tool}, an action of task {task_id}, is over: it finished")
}

/// `daemon stop`: the wait ended and the action did not finish: the daemon ended at its limit, so
/// what the action did is unknown (REQ-CLI-005).
pub fn daemon_stop_wait_gave_up(task_id: &str, tool: &str) -> String {
    format!(
        "the wait for {tool}, an action of task {task_id}, is over: it had not finished when the daemon ended, so what it did is unknown; check before you resume (`mimlys task show {task_id}`)"
    )
}

/// `model probe` through a daemon that runs a task: one task runs at a time (LIM-X1), the probe's
/// own included, so it is refused, naming the task that runs and what to do.
pub fn daemon_probe_refused_for_task(task_id: &str) -> String {
    format!(
        "task {task_id} is running in the daemon, and one task runs at a time (LIM-X1), the probe's included: run the probe when it has stopped (`mimlys task pause {task_id}` pauses it at its next boundary)"
    )
}

/// `daemon restart`: a task that waited in line, put back in it.
pub fn daemon_back_in_line(task_id: &str, behind: Option<&str>) -> String {
    match behind {
        Some(behind) => format!("task {task_id} is back in line, behind task {behind}"),
        None => format!("task {task_id} is back in line and starts now"),
    }
}

/// `daemon restart`: a task that waited in line that could not be put back.
pub fn daemon_not_back_in_line(task_id: &str, why: &str) -> String {
    format!(
        "task {task_id} was not put back in line: {why}; start it with `mimlys task resume {task_id}`"
    )
}

/// `daemon stop` with no daemon.
pub const DAEMON_NONE_TO_STOP: &str = "no daemon is running";

/// `daemon restart`: a task the stop paused, resumed.
pub fn daemon_resumed(task_id: &str) -> String {
    format!("resumed task {task_id}")
}

/// `daemon restart`: a task the stop paused that could not be resumed.
pub fn daemon_not_resumed(task_id: &str, why: &str) -> String {
    format!("task {task_id} was not resumed: {why}; resume it with `mimlys task resume {task_id}`")
}

/// `daemon status` with no daemon.
pub fn daemon_none_for(data_dir: &str) -> String {
    format!("no daemon is running for {data_dir}")
}

/// `daemon status`: the lines, and, in `status`, the first of them.
pub fn daemon_status_lines(
    status: &crate::daemon::wire::DaemonStatus,
    same_configuration: bool,
) -> Vec<String> {
    let mut lines = vec![
        format!(
            "daemon: pid {}, mimlys {}, up {} s (since {})",
            status.pid, status.runtime_version, status.uptime_secs, status.started_at
        ),
        format!(
            "{}: {}",
            if cfg!(windows) { "pipe" } else { "socket" },
            status.socket
        ),
        format!(
            "configuration: {}{}",
            &status.config_hash[..12.min(status.config_hash.len())],
            if same_configuration {
                ""
            } else {
                " (this file differs: `mimlys daemon restart` applies it)"
            }
        ),
        format!(
            "clients: {} (processes, counted by the peer's pid; {} connections open)",
            status.clients, status.connections
        ),
        format!(
            "tasks: {} running, {} queued, {} waiting for you",
            status.running.len(),
            status.queued.len(),
            status.waiting_user.len()
        ),
    ];
    for id in &status.running {
        lines.push(format!("  running {id}"));
    }
    for (id, line) in &status.queued {
        lines.push(format!(
            "  {}",
            daemon_queued_line(
                &id.to_string(),
                &line.behind.to_string(),
                &line.running_since.to_rfc3339()
            )
        ));
    }
    for id in &status.waiting_user {
        lines.push(format!("  waiting for you {id}"));
    }
    lines.push(format!(
        "idle exit: {}",
        status.idle_secs.map_or_else(
            || "never".to_owned(),
            |s| format!("after {s} s with nothing to do")
        )
    ));
    lines.push(format!("log: {}", status.log));
    lines
}

/// A task that waits in line (external design §19.5): what it waits behind.
pub fn daemon_queued_line(task_id: &str, behind: &str, since: &str) -> String {
    format!("task {task_id}: queued behind task {behind}, running since {since}")
}

/// `status`: the mode in force, served by a daemon.
pub fn mode_served(pid: u32, version: &str, socket: &str) -> String {
    format!("mode: served by the daemon (pid {pid}, mimlys {version}) at {socket}")
}

/// The connection to the daemon broke, and could not be made again within LIM-O14: the request was
/// sent once more under its own id and was not answered either.
pub fn daemon_connection_lost(detail: &str) -> String {
    format!("the daemon does not answer ({detail}); the connection was made again once and failed")
}

/// `--detach` without a daemon (external design §19.5): refused before anything is created.
pub const DETACH_NEEDS_A_DAEMON: &str = "--detach needs a daemon, and none runs for this data directory: start one with `mimlys daemon start`, or leave the command to follow the task";

/// `--detach`: the task goes on in the daemon.
pub fn detached_from(task_id: &str) -> String {
    format!(
        "task {task_id} goes on in the daemon: follow it with `mimlys events {task_id} --follow`, pause it with `mimlys task pause {task_id}`, cancel it with `mimlys task cancel {task_id}`"
    )
}

/// A request that took effect before, and whose task has been deleted since (external design §4).
pub fn applied_earlier_gone(verb: &str, task_id: &str) -> String {
    format!("{verb}: it took effect earlier, and task {task_id} has been deleted since")
}

/// A command that needs the store itself (`model probe`, `toolchain grant`), reached while the
/// daemon holds it.
pub const NEEDS_THE_STORE_ITSELF: &str =
    "this command needs the store itself, which the daemon holds: run `mimlys daemon stop` first";

/// `status`: the mode in force, embedded.
pub const MODE_EMBEDDED: &str = "mode: embedded (no daemon runs for this data directory)";

/// A command that writes, while a daemon holds the store (REQ-PERS-014): the holder is named as
/// the daemon, and what to do.
pub fn store_in_use_by_daemon(pid: u32, since: &str) -> String {
    format!(
        "the store is in use by the daemon (pid {pid}) since {since}: this command does not run beside it; the commands that run tasks go through it, and for this one run `mimlys daemon stop` first"
    )
}

/// `status`: a daemon that is there and cannot be used by this client.
pub fn daemon_unreachable(why: &str) -> String {
    format!("daemon: not used ({why})")
}
