//! A declared host tool run by the worker (RFC 011 §6; REQ-CONF-006,
//! REQ-CONF-009, REQ-TOOL-008): the declaration's command with its fixed
//! and validated arguments, in the task's workspace, with an explicit
//! environment, both streams captured to LIM-H4, the deadline and the
//! interrupt honoured by terminating the whole tree, and, where no cgroup
//! bounds memory, `RLIMIT_AS` set for the child around the spawn. On
//! Windows the tool starts suspended inside a job of its own, nested in the
//! worker's, so that its tree is killed as one (clause K).

use crate::sandbox::{Interrupt, InterruptReason};
use mimlys_core::action::{ExecutionMetrics, HostFaultDetail, Termination};
use mimlys_core::host_tool::HostToolDeclaration;
use mimlys_core::limits::LIM_H4_HOST_OUTPUT_BYTES;
use mimlys_core::limits::LIM_O2_CANCEL_MS;
use mimlys_core::observation::{Notice, Stream};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

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

/// D1 DIAGNOSTIC (throwaway branch only): after a start failure, retry reading the
/// tool for up to 3 s and trace when (if ever) access returns.
fn d1_retry(path: &Path) {
    let t0 = Instant::now();
    let mut last = String::new();
    for i in 0..60 {
        let open = std::fs::File::open(path).map(|_| "ok".to_string()).unwrap_or_else(|e| e.to_string());
        let meta = std::fs::metadata(path).map(|_| "ok".to_string()).unwrap_or_else(|e| e.to_string());
        let dir = path
            .parent()
            .map(|d| std::fs::read_dir(d).map(|_| "ok".to_string()).unwrap_or_else(|e| e.to_string()))
            .unwrap_or_default();
        let state = format!("open={open} metadata={meta} read_dir_parent={dir}");
        if state != last || i == 59 {
            let ms = t0.elapsed().as_millis();
            crate::trace::line(|| format!("D1 retry {i} +{ms}ms {state}"));
            last = state;
        }
        if open == "ok" && meta == "ok" {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
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

    fn try_wait(&mut self) -> std::io::Result<Option<std::process::ExitStatus>> {
        match self {
            #[cfg(unix)]
            ToolProc::Std(c) => c.try_wait(),
            #[cfg(windows)]
            ToolProc::Win(c, _) => c.try_wait(),
        }
    }

    /// Terminates the tree (clause K; REQ-CONF-009): the `tool` cgroup as
    /// one where it exists, then the child's process group and every
    /// descendant that re-parented to the worker; on Windows the tool's job.
    fn kill_tree(&mut self, tool_cgroup: Option<&Path>) {
        #[cfg(not(target_os = "linux"))]
        let _ = tool_cgroup;
        match self {
            #[cfg(unix)]
            ToolProc::Std(child) => {
                #[cfg(target_os = "linux")]
                if let Some(dir) = tool_cgroup {
                    let _ = crate::confine::linux::kill_cgroup(dir);
                }
                let pid = rustix::process::Pid::from_child(child);
                let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
                let _ = child.kill();
                let _ = child.wait();
                #[cfg(target_os = "linux")]
                crate::confine::linux::kill_descendants(
                    std::process::id(),
                    Duration::from_millis(LIM_O2_CANCEL_MS),
                );
            }
            #[cfg(windows)]
            ToolProc::Win(child, job) => {
                job.kill();
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
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
    // REQ-CONF-011: the tool's process is bound by the worker's sets without
    // what only the worker reads (`/proc`).
    #[cfg(target_os = "linux")]
    crate::confine::linux::restrict_child(&mut cmd);
    let previous = r.rlimit_memory.and_then(set_address_space_limit);
    let spawned = cmd.spawn();
    if let Some(prev) = previous {
        restore_address_space_limit(prev);
    }
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
            d1_retry(Path::new(&r.declaration.command));
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
            d1_retry(&program);
            return fault(
                HostFaultDetail::Unavailable,
                vec![not_started(name, reason)],
            );
        }
    };
    let stdout = capture(child.take_stdout(), Stream::Stdout);
    let stderr = capture(child.take_stderr(), Stream::Stderr);
    let deadline = Duration::from_millis(r.deadline_ms.max(1) as u64);
    let termination = loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                break Termination::Exited {
                    exit_code: exit_code(status),
                };
            }
            Ok(None) => {}
            Err(_) => {
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
