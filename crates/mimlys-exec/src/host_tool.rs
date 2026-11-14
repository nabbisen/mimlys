//! A declared host tool run by the worker (RFC 011 §6; REQ-CONF-006,
//! REQ-CONF-009, REQ-TOOL-008): the declaration's command with its fixed
//! and validated arguments, in the task's workspace, with an explicit
//! environment, both streams captured to LIM-H4, the deadline and the
//! interrupt honoured by terminating the whole tree, and, where no cgroup
//! bounds memory, `RLIMIT_AS` set for the child around the spawn. On
//! Windows the tool starts suspended inside a job of its own, nested in the
//! worker's, so that its tree is killed as one (clause K). On macOS the tree
//! is the sandbox's members: killed by the membership loop after a cancel, a
//! deadline and the tool's end, and watched for memory and count every LIM-H6.

use crate::sandbox::{Interrupt, InterruptReason};
use mimlys_core::action::{ExecutionMetrics, HostFaultDetail, Termination};
use mimlys_core::host_tool::HostToolDeclaration;
use mimlys_core::limits::LIM_H4_HOST_OUTPUT_BYTES;
#[cfg(target_os = "macos")]
use mimlys_core::limits::LIM_H6_TRACK_INTERVAL_MS;
use mimlys_core::limits::LIM_O2_CANCEL_MS;
use mimlys_core::observation::{Notice, Stream};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The ceilings the macOS watchdogs hold the tool's tree to (clauses M and C,
/// RFC 011 Amendment 2, settled, point 7), polled every LIM-H6. A field is
/// `Some` only for a clause the worker reported `best_effort`; always `None`
/// elsewhere, where the kernel holds the limits.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TreeLimits {
    /// LIM-H2: the members' summed resident size.
    pub memory_bytes: Option<u64>,
    /// LIM-H3: the members' count.
    pub max_processes: Option<u32>,
}

/// One host tool run.
pub struct HostRun<'a> {
    /// The declaration (its `env` already holds the resolved `env_allow` values).
    pub declaration: &'a HostToolDeclaration,
    /// The model's arguments, already validated at stage 7.
    pub extra_args: Vec<String>,
    /// The workspace tree: the working directory and the only writable project path.
    pub workspace: &'a Path,
    /// A private temporary directory inside the write set (`HOME`, `TMPDIR`, `TEMP`, the profile paths).
    pub tmp: &'a Path,
    /// The effective deadline.
    pub deadline_ms: i64,
    /// The execution's interrupt (cancel, deadline).
    pub interrupt: &'a Interrupt,
    /// `RLIMIT_AS` for the child when no cgroup bounds the tree (clause M best-effort).
    pub rlimit_memory: Option<u64>,
    /// The `tool` sub-cgroup the child joins before it runs, when clause K holds through `cgroup.kill`.
    pub tool_cgroup: Option<&'a Path>,
    /// The watchdogs' ceilings (macOS; empty elsewhere).
    pub tree_limits: TreeLimits,
}

/// What a run produced.
pub struct HostOutcome {
    /// How it ended.
    pub termination: Termination,
    /// Standard output, bounded.
    pub stdout: Vec<u8>,
    /// Standard error, bounded.
    pub stderr: Vec<u8>,
    /// Measurements.
    pub metrics: ExecutionMetrics,
    /// Notices.
    pub notices: Vec<Notice>,
}

/// Resolves the command: an absolute path as it is; a name against the
/// `bin` directories of the declared read paths, never the inherited `PATH`
/// (on Windows the name may omit `.exe`).
pub fn resolve_command(declaration: &HostToolDeclaration) -> Result<PathBuf, String> {
    let command = Path::new(&declaration.command);
    if command.is_absolute() {
        return match std::fs::metadata(command) {
            Ok(m) if m.is_file() => Ok(command.to_path_buf()),
            Ok(_) => Err(format!("{} is not a file", command.display())),
            Err(e) => Err(format!("{}: {e}", command.display())),
        };
    }
    bin_dirs(&declaration.read_paths)
        .into_iter()
        .flat_map(|dir| {
            let base = dir.join(command);
            let mut candidates = vec![base.clone()];
            if cfg!(windows) && base.extension().is_none() {
                candidates.push(base.with_extension("exe"));
            }
            candidates
        })
        .find(|p| p.is_file())
        .ok_or_else(|| {
            format!(
                "{} is under no declared bin directory (a read path that is a bin directory or holds one)",
                command.display()
            )
        })
}

/// The directories a tool's commands live in, from the declared read paths
/// (external design §13.3, §13.5): on Linux and macOS a read path that is
/// itself a `bin` directory (`~/.cargo/bin`) and a read path that holds one
/// (`<path>/bin`), in declaration order; on Windows `<path>\bin` for every
/// read path, as before.
pub fn bin_dirs(read_paths: &[String]) -> Vec<PathBuf> {
    bin_dirs_with(read_paths, |dir| dir.is_dir())
}

/// As [`bin_dirs`], with the directory test given (the rule is tested without a filesystem).
pub fn bin_dirs_with(read_paths: &[String], is_dir: impl Fn(&Path) -> bool) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    for path in read_paths {
        let path = Path::new(path);
        if cfg!(windows) {
            dirs.push(path.join("bin"));
            continue;
        }
        if path.file_name().is_some_and(|name| name == "bin") {
            dirs.push(path.to_path_buf());
        }
        let held = path.join("bin");
        if is_dir(&held) {
            dirs.push(held);
        }
    }
    dirs.dedup();
    dirs
}

/// The notice of a tool that never started, also traced.
fn not_started(tool: &str, reason: String) -> Notice {
    crate::trace::line(|| format!("host tool {tool}: not started: {reason}"));
    Notice::HostToolNotStarted { reason }
}

/// The `PATH` the child sees, unless the declaration's `env` sets one: the
/// declared read paths that are a `bin` directory or hold one ([`bin_dirs`])
/// and the system's, so that a toolchain finds its own helpers (`cc`, `ld`,
/// `link.exe`).
fn child_path(declaration: &HostToolDeclaration) -> String {
    let separator = if cfg!(windows) { ";" } else { ":" };
    let mut dirs: Vec<String> = bin_dirs(&declaration.read_paths)
        .iter()
        .map(|d| d.to_string_lossy().into_owned())
        .collect();
    if cfg!(windows) {
        if let Ok(root) = std::env::var("SystemRoot") {
            dirs.push(format!("{root}\\System32"));
            dirs.push(root);
        }
    } else {
        dirs.extend(["/usr/bin".to_owned(), "/bin".to_owned()]);
    }
    dirs.join(separator)
}

/// The explicit environment of the tool (clause E): the system's `PATH`
/// shape, every profile and temporary location inside the workspace's
/// temporary directory, then the declaration's variables.
fn child_env(r: &HostRun<'_>) -> Vec<(String, String)> {
    env_for(r.declaration, r.tmp)
}

/// [`child_env`] from what it depends on: the declaration and the temporary directory.
fn env_for(declaration: &HostToolDeclaration, tmp: &Path) -> Vec<(String, String)> {
    let tmp = tmp.to_string_lossy().into_owned();
    let mut env = vec![
        ("PATH".to_owned(), child_path(declaration)),
        ("HOME".to_owned(), tmp.clone()),
        ("TMPDIR".to_owned(), tmp.clone()),
        ("TMP".to_owned(), tmp.clone()),
    ];
    if cfg!(windows) {
        for name in ["TEMP", "USERPROFILE", "LOCALAPPDATA", "APPDATA"] {
            env.push((name.to_owned(), tmp.clone()));
        }
        if let Ok(root) = std::env::var("SystemRoot") {
            env.push(("SystemRoot".to_owned(), root));
        }
    }
    for (k, v) in &declaration.env {
        env.retain(|(name, _)| !name.eq_ignore_ascii_case(k));
        env.push((k.clone(), v.clone()));
    }
    env
}

/// The tool's process, with its tree primitive.
enum ToolProc {
    #[cfg(unix)]
    Std(std::process::Child),
    #[cfg(windows)]
    Win(
        crate::confine::windows::RawChild,
        crate::confine::windows::Job,
    ),
}

impl ToolProc {
    fn take_stdout(&mut self) -> Option<Box<dyn Read + Send>> {
        match self {
            #[cfg(unix)]
            ToolProc::Std(c) => c.stdout.take().map(|s| Box::new(s) as _),
            #[cfg(windows)]
            ToolProc::Win(c, _) => c.stdout.take().map(|s| Box::new(s) as _),
        }
    }

    fn take_stderr(&mut self) -> Option<Box<dyn Read + Send>> {
        match self {
            #[cfg(unix)]
            ToolProc::Std(c) => c.stderr.take().map(|s| Box::new(s) as _),
            #[cfg(windows)]
            ToolProc::Win(c, _) => c.stderr.take().map(|s| Box::new(s) as _),
        }
    }

    /// Whether the tool's own process has ended. On Unix it is not reaped
    /// yet: its process group still names its tree until the tree is ended
    /// (a reaped pid can be given to another process), and
    /// [`kill_tree`](Self::kill_tree) reaps it and returns its status.
    fn has_exited(&mut self) -> std::io::Result<bool> {
        let exited = self.poll_exit()?;
        // Test builds: `MIMLYS_PROBE_WAIT_ERROR` answers an error, instead of
        // "ended", once the tool has ended, so that the path of a wait that
        // fails is run on a real tree (REQ-TEST-013).
        if exited && cfg!(debug_assertions) && std::env::var_os("MIMLYS_PROBE_WAIT_ERROR").is_some()
        {
            return Err(std::io::Error::other("injected (MIMLYS_PROBE_WAIT_ERROR)"));
        }
        Ok(exited)
    }

    fn poll_exit(&mut self) -> std::io::Result<bool> {
        match self {
            #[cfg(unix)]
            ToolProc::Std(child) => {
                use rustix::process::{WaitId, WaitIdOptions, waitid};
                let pid = rustix::process::Pid::from_child(child);
                let options = WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT;
                waitid(WaitId::Pid(pid), options)
                    .map(|status| status.is_some())
                    .map_err(std::io::Error::from)
            }
            #[cfg(windows)]
            ToolProc::Win(c, _) => c.try_wait().map(|status| status.is_some()),
        }
    }

    /// Terminates the tree (clause K; REQ-CONF-009), at every end of the
    /// tool: the `tool` cgroup as one where it exists, then the child's
    /// process group and every descendant that re-parented to the worker; on
    /// macOS the membership sweep; on Windows the tool's job. The result is
    /// checked: a process that survives within LIM-O2 is logged at `warn` as
    /// `tree_kill_incomplete`, on every platform. Returns the tool's own
    /// exit status, reaped here.
    fn kill_tree(&mut self, tool_cgroup: Option<&Path>) -> Option<std::process::ExitStatus> {
        #[cfg(not(target_os = "linux"))]
        let _ = tool_cgroup;
        #[cfg(any(target_os = "linux", windows))]
        let grace = Duration::from_millis(LIM_O2_CANCEL_MS);
        let (status, incomplete) = match self {
            #[cfg(unix)]
            ToolProc::Std(child) => {
                // A test build can leave the tree alone (`MIMLYS_PROBE_NO_TREE_KILL`) to
                // show that a survivor is reported; the tool's own process is always ended.
                let kill = !tree_kill_disabled();
                #[cfg(target_os = "linux")]
                if kill && let Some(dir) = tool_cgroup {
                    // A cgroup that cannot be killed is not the end: the walk below
                    // reaches the same processes, and what it leaves is reported.
                    let _ = crate::confine::linux::kill_cgroup(dir);
                }
                let pid = rustix::process::Pid::from_child(child);
                if kill {
                    let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
                }
                let _ = child.kill();
                let status = child.wait().ok();
                #[cfg(target_os = "linux")]
                let incomplete = {
                    let remaining =
                        crate::confine::linux::tool_tree_remaining(tool_cgroup, kill, grace);
                    // The killed descendants that re-parented to the worker are zombies
                    // until reaped, and a zombie still counts against the process limit.
                    crate::confine::linux::reap_ended_children();
                    match remaining {
                        Ok(0) => None,
                        Ok(n) => Some(format!(
                            "{n} process(es) remained after {LIM_O2_CANCEL_MS} ms"
                        )),
                        Err(reason) => Some(reason),
                    }
                };
                #[cfg(target_os = "macos")]
                let incomplete = sweep_members();
                #[cfg(not(any(target_os = "linux", target_os = "macos")))]
                let incomplete: Option<String> = None;
                (status, incomplete)
            }
            #[cfg(windows)]
            ToolProc::Win(child, job) => {
                job.kill();
                let _ = child.kill();
                let status = child.wait().ok();
                // The job is asked what is left of it, until none remains or LIM-O2 passes.
                let started = Instant::now();
                let mut active = job.active_processes();
                while active.is_some_and(|n| n > 0) && started.elapsed() < grace {
                    std::thread::sleep(Duration::from_millis(10));
                    active = job.active_processes();
                }
                let incomplete = match active {
                    Some(0) => None,
                    Some(n) => Some(format!(
                        "{n} process(es) remained after {LIM_O2_CANCEL_MS} ms"
                    )),
                    None => Some("the job cannot be queried".to_owned()),
                };
                (status, incomplete)
            }
        };
        if let Some(why) = incomplete {
            // The worker has no log of its own: the runtime logs this line at `warn`.
            crate::trace::warn(&format!("tree_kill_incomplete: {why}"));
        }
        status
    }
}

/// Test builds: `MIMLYS_PROBE_NO_TREE_KILL` leaves a tool's descendants
/// alone at the end of the tool, so that a survivor is real and the warning
/// for it can be seen (REQ-TEST-013). Read only with `debug_assertions` on.
#[cfg(unix)]
fn tree_kill_disabled() -> bool {
    cfg!(debug_assertions) && std::env::var_os("MIMLYS_PROBE_NO_TREE_KILL").is_some()
}

/// Clause K on macOS (RFC 011 Amendment 2, settled, point 6): the sandbox's
/// members other than the worker, found and killed until none is left or
/// LIM-O2 passes; re-parented orphans are members like any other. Does
/// nothing unless the worker verified the mechanism after applying its profile.
/// Returns why the tree is not known to be gone, when it is not.
#[cfg(target_os = "macos")]
fn sweep_members() -> Option<String> {
    let report = crate::confine::macos::sweep(Duration::from_millis(LIM_O2_CANCEL_MS));
    crate::trace::line(|| {
        format!(
            "membership sweep: {} signals in {} rounds, complete: {}",
            report.killed, report.rounds, report.complete
        )
    });
    (!report.complete)
        .then(|| format!("members of the sandbox remained after {LIM_O2_CANCEL_MS} ms"))
}

#[cfg(unix)]
fn spawn_tool(program: &Path, r: &HostRun<'_>) -> Result<ToolProc, String> {
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};
    let mut cmd = Command::new(program);
    cmd.args(&r.declaration.args)
        .args(&r.extra_args)
        .current_dir(r.workspace)
        .env_clear()
        .envs(child_env(r))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    cmd.process_group(0);
    #[cfg(target_os = "linux")]
    if let Some(dir) = r.tool_cgroup {
        // Fail closed: a tree the kernel cannot kill as one is not started.
        let procs = std::fs::OpenOptions::new()
            .write(true)
            .open(dir.join("cgroup.procs"))
            .map_err(|e| format!("tool cgroup: {e}"))?;
        crate::confine::linux::attach_child_to_cgroup(&mut cmd, procs);
    }
    // REQ-CONF-011, REQ-CONF-006: the tool's process is bound by the worker's
    // sets without what only the worker reads (`/proc`), and writes, of the
    // data directory, only its own task's workspace tree and the execution's
    // temporary directory. A layer that cannot be made is a tool that does
    // not start. The descriptor stays open until the process is spawned.
    #[cfg(target_os = "linux")]
    let layer = crate::confine::linux::restrict_child(
        &mut cmd,
        crate::confine::linux::TaskPaths {
            tree: r.workspace,
            tmp: r.tmp,
        },
    )
    .map_err(|e| format!("tool layer: {e}"))?;
    let previous = r.rlimit_memory.and_then(set_address_space_limit);
    let spawned = cmd.spawn();
    if let Some(prev) = previous {
        restore_address_space_limit(prev);
    }
    #[cfg(target_os = "linux")]
    drop(layer);
    spawned.map(ToolProc::Std).map_err(|e| e.to_string())
}

#[cfg(windows)]
fn spawn_tool(program: &Path, r: &HostRun<'_>) -> Result<ToolProc, String> {
    use crate::confine::windows::{Job, SpawnOptions, spawn};
    let _ = r.rlimit_memory;
    // A job of its own, nested in the worker's: the tool's tree dies as one
    // on cancel or deadline while the worker lives on (RFC 011 §3).
    let job = Job::new()?;
    let mut args = r.declaration.args.clone();
    args.extend(r.extra_args.iter().cloned());
    let child = spawn(SpawnOptions {
        program,
        args: &args,
        env: &child_env(r),
        current_dir: Some(r.workspace),
        container: None,
        job: Some(&job),
        restricted_token: false,
    })?;
    Ok(ToolProc::Win(child, job))
}

/// Runs the tool to its end, its deadline or its cancellation.
pub fn run(r: HostRun<'_>) -> HostOutcome {
    let started = Instant::now();
    let fault = |detail: HostFaultDetail, notices: Vec<Notice>| HostOutcome {
        termination: Termination::HostFault { detail },
        stdout: Vec::new(),
        stderr: Vec::new(),
        metrics: ExecutionMetrics {
            wall_ms: started.elapsed().as_millis() as i64,
            ..ExecutionMetrics::default()
        },
        notices,
    };
    let name = r.declaration.name.as_str();
    let program = match resolve_command(r.declaration) {
        Ok(p) => p,
        Err(reason) => {
            return fault(
                HostFaultDetail::Unavailable,
                vec![not_started(name, reason)],
            );
        }
    };
    if let Err(e) = std::fs::create_dir_all(r.tmp) {
        let reason = format!("temporary directory {}: {e}", r.tmp.display());
        return fault(
            HostFaultDetail::Unavailable,
            vec![not_started(name, reason)],
        );
    }
    let mut child = match spawn_tool(&program, &r) {
        Ok(c) => c,
        Err(reason) => {
            return fault(
                HostFaultDetail::Unavailable,
                vec![not_started(name, reason)],
            );
        }
    };
    let stdout = capture(child.take_stdout(), Stream::Stdout);
    let stderr = capture(child.take_stderr(), Stream::Stderr);
    let deadline = Duration::from_millis(r.deadline_ms.max(1) as u64);
    // Clauses M and C on macOS: the watchdogs over the members, every LIM-H6.
    #[cfg(target_os = "macos")]
    let mut next_watch = Instant::now();
    let termination = loop {
        match child.has_exited() {
            Ok(true) => {
                // Clause K (REQ-CONF-009): the tool's own exit ends its tree
                // as a cancel does. What it left behind (a daemon, a process
                // in a session of its own) ends with it, before the
                // action's "after" snapshot is taken.
                break match child.kill_tree(r.tool_cgroup) {
                    Some(status) => Termination::Exited {
                        exit_code: exit_code(status),
                    },
                    None => Termination::HostFault {
                        detail: HostFaultDetail::Unavailable,
                    },
                };
            }
            Ok(false) => {}
            Err(_) => {
                // The state of the tool is not known; its tree ends all the same.
                child.kill_tree(r.tool_cgroup);
                break Termination::HostFault {
                    detail: HostFaultDetail::Unavailable,
                };
            }
        }
        if r.interrupt.fired() {
            child.kill_tree(r.tool_cgroup);
            break match r.interrupt.reason() {
                InterruptReason::Cancelled => Termination::Cancelled,
                _ => Termination::Deadline,
            };
        }
        if started.elapsed() >= deadline {
            r.interrupt.fire(InterruptReason::Deadline);
            child.kill_tree(r.tool_cgroup);
            break Termination::Deadline;
        }
        #[cfg(target_os = "macos")]
        if Instant::now() >= next_watch
            && (r.tree_limits.memory_bytes.is_some() || r.tree_limits.max_processes.is_some())
        {
            next_watch = Instant::now() + Duration::from_millis(LIM_H6_TRACK_INTERVAL_MS);
            if let Some(verdict) = crate::confine::macos::watch(
                r.tree_limits.memory_bytes.unwrap_or(0),
                r.tree_limits.max_processes.unwrap_or(0),
            ) {
                crate::trace::line(|| format!("host tool {name}: watchdog verdict {verdict:?}"));
                child.kill_tree(r.tool_cgroup);
                break match verdict {
                    crate::confine::macos::Verdict::Memory { .. } => Termination::MemoryLimit,
                    // REQ-SBX-003: a tree the watchdog ended for its count is
                    // `process_limit`, never a tool that exited by itself.
                    crate::confine::macos::Verdict::Count { .. } => Termination::ProcessLimit,
                };
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let (out, out_notice) = stdout.finish();
    let (err, err_notice) = stderr.finish();
    let mut notices = Vec::new();
    notices.extend(out_notice);
    notices.extend(err_notice);
    HostOutcome {
        termination,
        metrics: ExecutionMetrics {
            wall_ms: started.elapsed().as_millis() as i64,
            stdout_bytes: out.len() as u64,
            stderr_bytes: err.len() as u64,
            ..ExecutionMetrics::default()
        },
        stdout: out,
        stderr: err,
        notices,
    }
}

fn exit_code(status: std::process::ExitStatus) -> i32 {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        status
            .code()
            .or_else(|| status.signal().map(|s| 128 + s))
            .unwrap_or(-1)
    }
    #[cfg(not(unix))]
    {
        status.code().unwrap_or(-1)
    }
}

/// A stream captured on its own thread to the bound, draining past it.
struct Capture {
    handle: Option<std::thread::JoinHandle<()>>,
    buffer: Arc<Mutex<(Vec<u8>, u64)>>,
    stream: Stream,
}

fn capture<R: Read + Send + 'static>(reader: Option<R>, stream: Stream) -> Capture {
    let buffer = Arc::new(Mutex::new((Vec::new(), 0u64)));
    let handle = reader.map(|mut reader| {
        let buffer = Arc::clone(&buffer);
        std::thread::spawn(move || {
            let mut chunk = [0u8; 8192];
            loop {
                match reader.read(&mut chunk) {
                    Ok(0) | Err(_) => return,
                    Ok(n) => {
                        let mut b = buffer.lock().expect("capture");
                        b.1 += n as u64;
                        let room = LIM_H4_HOST_OUTPUT_BYTES.saturating_sub(b.0.len());
                        b.0.extend_from_slice(&chunk[..n.min(room)]);
                    }
                }
            }
        })
    });
    Capture {
        handle,
        buffer,
        stream,
    }
}

impl Capture {
    /// What was captured. The tool's process has ended or been terminated
    /// here; the reader is given LIM-O2 to reach the end of its stream, in
    /// case a survivor of the termination still holds the pipe, and is then
    /// left to end on its own: the worker never waits on a process it has
    /// already terminated.
    fn finish(self) -> (Vec<u8>, Option<Notice>) {
        if let Some(h) = self.handle {
            let start = Instant::now();
            while !h.is_finished() && start.elapsed() < Duration::from_millis(LIM_O2_CANCEL_MS) {
                std::thread::sleep(Duration::from_millis(10));
            }
            if h.is_finished() {
                let _ = h.join();
            }
        }
        let (bytes, total) = std::mem::take(&mut *self.buffer.lock().expect("capture"));
        let notice = (total as usize > bytes.len()).then_some(Notice::OutputTruncated {
            stream: self.stream,
            total_bytes: total,
        });
        (bytes, notice)
    }
}

#[cfg(target_os = "linux")]
fn set_address_space_limit(bytes: u64) -> Option<rustix::process::Rlimit> {
    use rustix::process::{Resource, Rlimit, getrlimit, setrlimit};
    let previous = getrlimit(Resource::As);
    let wanted = Rlimit {
        current: Some(bytes.min(previous.maximum.unwrap_or(u64::MAX))),
        maximum: previous.maximum,
    };
    setrlimit(Resource::As, wanted).ok().map(|()| previous)
}

#[cfg(all(unix, not(target_os = "linux")))]
fn set_address_space_limit(_bytes: u64) -> Option<()> {
    None
}

#[cfg(target_os = "linux")]
fn restore_address_space_limit(previous: rustix::process::Rlimit) {
    let _ = rustix::process::setrlimit(rustix::process::Resource::As, previous);
}

#[cfg(all(unix, not(target_os = "linux")))]
fn restore_address_space_limit(_previous: ()) {}

#[cfg(test)]
mod tests;
