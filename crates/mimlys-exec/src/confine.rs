//! The confinement interface (external design §13.2, §13.4; REQ-CONF-002,
//! REQ-CONF-003, REQ-CONF-009, REQ-CONF-010). One `Confinement` per platform
//! family spawns the worker with an explicit environment inside a process
//! tree it can terminate, and reports per clause what it enforces. The
//! backends for the filesystem, network, memory and count clauses are the
//! Linux one (M6a, `linux`) and the Windows one (M6b, `windows`); on macOS
//! (M6c, not yet shipped) those clauses are `unavailable` and say so.
//!
//! Foreign-function code lives only in this module's platform files, each
//! `unsafe` block justified (REQ-CONF-010).

use mimlys_core::confine::{Clause, ClauseState, Probe};
use serde::{Deserialize, Serialize};

pub mod dacl_wait;
#[cfg(target_os = "linux")]
pub mod linux;

/// What the worker confines itself with (RFC 011 §1): the read and write
/// sets, whether network is denied, and the tree's limits.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ConfinementSpec {
    /// Directories and files the tree may read (and execute).
    pub read_paths: Vec<PathBuf>,
    /// Directories and files the tree may write.
    pub write_paths: Vec<PathBuf>,
    /// Clause N: deny network. `false` only for the dependency step (REQ-CONF-007).
    pub deny_network: bool,
    /// LIM-H2.
    pub memory_bytes: u64,
    /// LIM-H3.
    pub max_processes: u32,
    /// The data directory the tree belongs to: Windows derives the
    /// AppContainer profile from it and finds the permission record there.
    #[serde(default)]
    pub data_dir: PathBuf,
}

/// What a backend reports per clause after applying itself.
pub type Report = BTreeMap<Clause, ClauseState>;

/// What the worker's self-confinement produced: the report and the
/// handles the runtime needs for termination (RFC 011 §2, §9).
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Applied {
    /// Per clause.
    pub report: BTreeMap<String, ClauseState>,
    /// The delegated cgroup the worker moved into, when one exists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cgroup: Option<PathBuf>,
    /// Its `tool` sub-cgroup, when the worker can attach and kill through it (clause K).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_cgroup: Option<PathBuf>,
    /// Whether a memory limit must be applied per host tool (no cgroup limit).
    #[serde(default)]
    pub rlimit_memory: bool,
}
#[cfg(windows)]
use mimlys_core::limits::LIM_O2_CANCEL_MS;
use std::collections::BTreeMap;
use std::path::PathBuf;
#[cfg(unix)]
use std::process::{Child, Command, Stdio};
use std::time::Duration;

#[cfg(unix)]
mod posix;
#[cfg(windows)]
pub mod windows;

/// What to spawn.
#[derive(Clone, Debug)]
pub struct WorkerCommand {
    /// The worker binary.
    pub program: PathBuf,
    /// Its arguments.
    pub args: Vec<String>,
    /// The explicit environment (clause E); nothing else is inherited.
    pub env: Vec<(String, String)>,
    /// Working directory.
    pub current_dir: Option<PathBuf>,
}

/// A confinement failure.
#[derive(Debug, thiserror::Error)]
pub enum ConfineError {
    /// The worker could not be started.
    #[error("cannot spawn the worker: {0}")]
    Spawn(std::io::Error),
    /// The tree primitive could not be set up; nothing was left running.
    #[error("cannot confine the worker tree: {0}")]
    Tree(String),
}

/// A spawned worker inside its tree.
pub struct ConfinedChild {
    proc: Proc,
    tree: Tree,
}

/// The process behind a confined child: the standard library's on POSIX,
/// the backend's own on Windows (the container and the token are attributes
/// of process creation the standard library does not expose).
enum Proc {
    #[cfg(unix)]
    Std(Child),
    #[cfg(windows)]
    Raw(windows::RawChild),
}

enum Tree {
    #[cfg(unix)]
    ProcessGroup(posix::Group),
    #[cfg(windows)]
    Job(windows::Job),
}

impl ConfinedChild {
    /// The child's standard input, once. On Windows it is the backend's
    /// own synchronous pipe end ([`windows::PipeEnd`]), hence the trait object.
    pub fn take_stdin(&mut self) -> Option<Box<dyn std::io::Write + Send>> {
        match &mut self.proc {
            #[cfg(unix)]
            Proc::Std(c) => c.stdin.take().map(|s| Box::new(s) as _),
            #[cfg(windows)]
            Proc::Raw(c) => c.stdin.take().map(|s| Box::new(s) as _),
        }
    }

    /// The child's standard output, once.
    pub fn take_stdout(&mut self) -> Option<Box<dyn std::io::Read + Send>> {
        match &mut self.proc {
            #[cfg(unix)]
            Proc::Std(c) => c.stdout.take().map(|s| Box::new(s) as _),
            #[cfg(windows)]
            Proc::Raw(c) => c.stdout.take().map(|s| Box::new(s) as _),
        }
    }

    /// The child's standard error, once.
    pub fn take_stderr(&mut self) -> Option<Box<dyn std::io::Read + Send>> {
        match &mut self.proc {
            #[cfg(unix)]
            Proc::Std(c) => c.stderr.take().map(|s| Box::new(s) as _),
            #[cfg(windows)]
            Proc::Raw(c) => c.stderr.take().map(|s| Box::new(s) as _),
        }
    }

    /// The exit status when the child has ended, else `None`.
    pub fn try_wait(&mut self) -> std::io::Result<Option<std::process::ExitStatus>> {
        match &mut self.proc {
            #[cfg(unix)]
            Proc::Std(c) => c.try_wait(),
            #[cfg(windows)]
            Proc::Raw(c) => c.try_wait(),
        }
    }

    fn kill_process(&mut self) {
        match &mut self.proc {
            #[cfg(unix)]
            Proc::Std(c) => {
                let _ = c.kill();
                let _ = c.wait();
            }
            #[cfg(windows)]
            Proc::Raw(c) => {
                let pid = c.id();
                let _ = c.kill();
                // Bounded: a process the system will not end must not hold the runtime.
                let start = std::time::Instant::now();
                while c.try_wait().is_ok_and(|s| s.is_none())
                    && start.elapsed() < Duration::from_millis(LIM_O2_CANCEL_MS)
                {
                    std::thread::sleep(Duration::from_millis(10));
                }
                if c.try_wait().is_ok_and(|s| s.is_none()) {
                    tracing::warn!(
                        target: "mimlys_worker",
                        "process {pid} survives termination{}",
                        c.cpu_times_ms()
                            .map(|(k, u)| format!(" (kernel {k} ms, user {u} ms)"))
                            .unwrap_or_default()
                    );
                }
            }
        }
    }

    /// Terminates the whole tree (clause K).
    pub fn kill_tree(&mut self) {
        let pid = self.id();
        tracing::debug!(target: "mimlys_worker", "terminating the tree of {pid}");
        match &mut self.tree {
            #[cfg(unix)]
            Tree::ProcessGroup(g) => g.kill(),
            #[cfg(windows)]
            Tree::Job(j) => j.kill(),
        }
        self.kill_process();
        tracing::debug!(target: "mimlys_worker", "the tree of {pid} is ended");
    }

    /// Waits up to `timeout` for the child to exit; `false` when it is still running.
    pub fn wait_within(&mut self, timeout: Duration) -> bool {
        let start = std::time::Instant::now();
        loop {
            match self.try_wait() {
                Ok(Some(_)) => return true,
                Ok(None) if start.elapsed() < timeout => {
                    std::thread::sleep(Duration::from_millis(20))
                }
                _ => return false,
            }
        }
    }

    /// What a child that does not answer has done so far, for the log: its
    /// CPU times on Windows (none at all means it never ran); nothing elsewhere.
    pub fn activity(&self) -> Option<String> {
        match &self.proc {
            #[cfg(unix)]
            Proc::Std(_) => None,
            #[cfg(windows)]
            Proc::Raw(c) => c
                .cpu_times_ms()
                .map(|(k, u)| format!("kernel {k} ms, user {u} ms")),
        }
    }

    /// Process id, for the orphan marker.
    pub fn id(&self) -> u32 {
        match &self.proc {
            #[cfg(unix)]
            Proc::Std(c) => c.id(),
            #[cfg(windows)]
            Proc::Raw(c) => c.id(),
        }
    }
}

impl Drop for ConfinedChild {
    fn drop(&mut self) {
        if matches!(self.try_wait(), Ok(None)) {
            self.kill_tree();
        }
    }
}

/// The platform's confinement.
pub trait Confinement: Send + Sync {
    /// What this machine enforces, before any worker runs (REQ-CONF-003).
    fn probe(&self) -> Probe;
    /// Spawns the worker inside its tree with the explicit environment and,
    /// where the platform confines at process creation (Windows), inside
    /// the container and under the limits the specification names.
    fn spawn(
        &self,
        command: &WorkerCommand,
        spec: Option<&ConfinementSpec>,
    ) -> Result<ConfinedChild, ConfineError>;
}

/// The confinement of this platform.
pub fn platform() -> Box<dyn Confinement> {
    Box::new(Platform)
}

/// The portable backend of M2: clauses E and K everywhere, P on Linux.
struct Platform;

/// Test builds: `MIMLYS_PROBE_FAIL=<letter>` makes the probe report that
/// clause unavailable, so the status can be shown to be honest (REQ-TEST-013).
fn injected_failure() -> Option<char> {
    if cfg!(debug_assertions) {
        std::env::var("MIMLYS_PROBE_FAIL")
            .ok()
            .and_then(|v| v.chars().next())
    } else {
        None
    }
}

fn base_probe() -> BTreeMap<Clause, ClauseState> {
    let mut clauses = BTreeMap::new();
    let later = |milestone: &str| ClauseState::Unavailable {
        reason: format!("backend arrives in {milestone}"),
    };
    let milestone = if cfg!(target_os = "linux") {
        "M6a"
    } else if cfg!(windows) {
        "M6b"
    } else {
        "M6c"
    };
    clauses.insert(Clause::Filesystem, later(milestone));
    clauses.insert(Clause::Network, later(milestone));
    clauses.insert(Clause::Environment, ClauseState::ENFORCED);
    clauses.insert(Clause::Termination, ClauseState::ENFORCED);
    clauses.insert(
        Clause::Privilege,
        if cfg!(target_os = "linux") {
            ClauseState::ENFORCED
        } else {
            later(milestone)
        },
    );
    clauses.insert(Clause::Memory, later(milestone));
    clauses.insert(Clause::Count, later(milestone));
    if let Some(letter) = injected_failure()
        && let Some(clause) = Clause::ALL
            .iter()
            .find(|c| c.letter() == letter.to_ascii_uppercase())
    {
        clauses.insert(
            *clause,
            ClauseState::Unavailable {
                reason: "injected failure (MIMLYS_PROBE_FAIL)".into(),
            },
        );
    }
    clauses
}

impl Confinement for Platform {
    fn probe(&self) -> Probe {
        Probe {
            clauses: base_probe(),
        }
    }

    #[cfg(unix)]
    fn spawn(
        &self,
        command: &WorkerCommand,
        _spec: Option<&ConfinementSpec>,
    ) -> Result<ConfinedChild, ConfineError> {
        let mut cmd = Command::new(&command.program);
        cmd.args(&command.args)
            .env_clear()
            .envs(command.env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(dir) = &command.current_dir {
            cmd.current_dir(dir);
        }
        let (child, group) = posix::spawn_in_group(cmd)?;
        Ok(ConfinedChild {
            proc: Proc::Std(child),
            tree: Tree::ProcessGroup(group),
        })
    }

    /// Windows (RFC 011 §3): the worker starts suspended inside a Job Object
    /// carrying the tree's limits and, with a specification, inside the
    /// installation's AppContainer (no network capability unless the
    /// specification allows network) under a restricted low-integrity token
    /// where the system accepts one.
    #[cfg(windows)]
    fn spawn(
        &self,
        command: &WorkerCommand,
        spec: Option<&ConfinementSpec>,
    ) -> Result<ConfinedChild, ConfineError> {
        let job = match spec {
            Some(s) => windows::Job::with_limits(Some(s.memory_bytes), Some(s.max_processes)),
            None => windows::Job::new(),
        }
        .map_err(ConfineError::Tree)?;
        let sid = match spec {
            Some(s) => Some(
                windows::ensure_profile(&windows::profile_name(&s.data_dir))
                    .map_err(ConfineError::Tree)?,
            ),
            None => None,
        };
        // External design §13.3: the profile and temporary locations point
        // into `<data>/tmp`; AppContainer creation also requires them.
        let mut env = command.env.clone();
        if let Some(s) = spec {
            let tmp = s.data_dir.join("tmp").to_string_lossy().into_owned();
            for name in ["TEMP", "TMP", "USERPROFILE", "LOCALAPPDATA", "APPDATA"] {
                if !env.iter().any(|(k, _)| k.eq_ignore_ascii_case(name)) {
                    env.push((name.to_owned(), tmp.clone()));
                }
            }
        }
        let child = windows::spawn(windows::SpawnOptions {
            program: &command.program,
            args: &command.args,
            env: &env,
            current_dir: command.current_dir.as_deref(),
            container: sid
                .as_deref()
                .map(|sid| (sid, spec.is_some_and(|s| !s.deny_network))),
            job: Some(&job),
            restricted_token: spec.is_some(),
        })
        .map_err(|e| ConfineError::Spawn(std::io::Error::other(e)))?;
        Ok(ConfinedChild {
            proc: Proc::Raw(child),
            tree: Tree::Job(job),
        })
    }
}

/// Marks the calling process non-dumpable (REQ-CONF-012; Linux, a no-op
/// elsewhere): a same-user process can then neither read its memory, its
/// environment nor its descriptors through `/proc/<pid>` (`environ`, `mem`,
/// `fd`) nor with `process_vm_readv`. The runtime holds `env:`-referenced
/// secrets in its environment, and `/proc/<pid>/environ` shows the
/// environment a process *started* with, so removing a variable afterwards
/// would change nothing. Called first thing in the runtime's `main` and in
/// the worker's, before any thread exists. The cost: no core dump of the
/// process and no same-user `ptrace` of it. An `exec` resets the flag, so a
/// host tool or an MCP server child is dumpable again; what protects their
/// environments is the host tools' read set (external design §13.3).
pub fn harden_process() -> std::io::Result<()> {
    #[cfg(target_os = "linux")]
    {
        rustix::process::set_dumpable_behavior(rustix::process::DumpableBehavior::NotDumpable)
            .map_err(std::io::Error::from)
    }
    #[cfg(not(target_os = "linux"))]
    {
        Ok(())
    }
}

/// Creates the data directory, owner-only from the start (REQ-WS-001,
/// external design §14.1). Unix: every directory it creates has mode 0700
/// (given to `mkdir`, not applied afterwards), and an existing one is
/// brought to 0700. Windows: the directory gets a protected DACL granting
/// the current user full control and nothing else, at every call. The
/// containers' entries on the subdirectories are added after this, never
/// before.
pub fn create_private_dir(path: &std::path::Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        posix::create_private_dir(path)
    }
    #[cfg(windows)]
    {
        windows::create_private_dir(path)
    }
}

/// Creates a directory inside the data directory: mode 0700 from the start
/// on Unix; on Windows an ordinary directory, which inherits the data
/// directory's owner-only DACL.
pub fn create_private_subdir(path: &std::path::Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        posix::create_private_dir(path)
    }
    #[cfg(windows)]
    {
        std::fs::create_dir_all(path)
    }
}

/// Clause P on Linux, from the worker's own process (no-op elsewhere).
pub fn set_no_new_privs() -> std::io::Result<()> {
    #[cfg(unix)]
    {
        posix::set_no_new_privs()
    }
    #[cfg(not(unix))]
    {
        Err(std::io::Error::other("not available on this platform"))
    }
}

/// Whether `no_new_privs` is set on this process (Linux).
pub fn no_new_privs() -> bool {
    #[cfg(unix)]
    {
        posix::no_new_privs()
    }
    #[cfg(not(unix))]
    {
        false
    }
}

/// Clause E as the worker sees it: the names in its environment that it
/// was not given, sorted; empty when the environment is explicit.
pub fn environment_extras(allowed: &[&str]) -> Vec<String> {
    let mut extras: Vec<String> = std::env::vars_os()
        .filter(|(k, _)| !allowed.iter().any(|a| k == *a))
        .map(|(k, _)| k.to_string_lossy().into_owned())
        .collect();
    extras.sort();
    extras
}

/// Whether the environment holds nothing but `allowed` (clause E).
pub fn environment_is_explicit(allowed: &[&str]) -> bool {
    environment_extras(allowed).is_empty()
}

#[cfg(test)]
mod tests;
