//! The runtime's side of the worker (external design §13.1; REQ-ARCH-008,
//! REQ-TOOL-007, REQ-CONF-009): spawn through the confinement interface,
//! probe once, send one execution at a time, cancel within LIM-O2, treat
//! the worker's death as an execution failure and restart it lazily at most
//! LIM-X3 times per task.

use crate::confine::{ConfinedChild, Confinement, WorkerCommand};
use crate::interpreters::Configured;
use crate::protocol::{FrameError, Replay, Request, Response, read_frame, write_frame};
use mimlys_core::action::{
    EffectView, ExecutionMetrics, FailureCause, HostFaultDetail, Termination,
};
use mimlys_core::confine::{Clause, ClauseState, Probe};
use mimlys_core::ids::{ActionId, ExecutionId, TaskId};
use mimlys_core::limits::{LIM_O2_CANCEL_MS, LIM_X3_RESTARTS, LIM_X4_WORKER_START_MS};
use mimlys_core::spi::{Cancellation, ExecutionOutcome};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How to start the worker.
pub struct WorkerConfig {
    /// The worker binary.
    pub program: PathBuf,
    /// Interpreters to load and verify at start.
    pub interpreters: Vec<Configured>,
    /// The confinement to spawn through.
    pub confinement: Box<dyn Confinement>,
    /// The guards' values (`[sandbox]`; the deadline is per execution).
    pub limits: crate::sandbox::Limits,
    /// What the worker confines itself with (RFC 011 §1); `None` only in tests of the sandbox alone.
    pub spec: Option<crate::confine::ConfinementSpec>,
    /// The declared host tools the worker may run (RFC 011 §6).
    pub host_tools: Vec<mimlys_core::host_tool::HostToolDeclaration>,
    /// Where the live worker's record is kept for orphan reaping (REQ-CONF-009).
    pub record: Option<PathBuf>,
}

/// The record of a live worker, for a later runtime to reap it (REQ-CONF-009).
#[derive(Clone, Debug, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct WorkerRecord {
    /// The process id.
    pub pid: u32,
    /// Its start time (`/proc/<pid>/stat` field 22), to tell a reused pid apart.
    #[serde(default)]
    pub start_time: Option<u64>,
    /// The delegated cgroup, when one holds the tree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cgroup: Option<PathBuf>,
}

/// Terminates what a previous runtime left behind, from its record; returns how many trees were killed.
pub fn reap_orphans(record: &std::path::Path) -> u32 {
    let Ok(text) = std::fs::read_to_string(record) else {
        return 0;
    };
    let _ = std::fs::remove_file(record);
    let Ok(r) = serde_json::from_str::<WorkerRecord>(&text) else {
        return 0;
    };
    #[cfg(target_os = "linux")]
    {
        let mut killed = 0;
        if let Some(c) = &r.cgroup
            && crate::confine::linux::kill_cgroup(c)
        {
            killed += 1;
            // The kill is asynchronous; the directory goes once it is empty.
            let deadline = std::time::Instant::now() + Duration::from_millis(LIM_O2_CANCEL_MS);
            while !crate::confine::linux::remove_cgroup(c) && std::time::Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        if start_time_of(r.pid) == r.start_time
            && r.start_time.is_some()
            && let Some(pid) = rustix::process::Pid::from_raw(r.pid as i32)
        {
            let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
            crate::confine::linux::kill_descendants(r.pid, Duration::from_millis(LIM_O2_CANCEL_MS));
            let _ = rustix::process::kill_process(pid, rustix::process::Signal::KILL);
            if killed == 0 {
                killed += 1;
            }
        }
        killed
    }
    // Windows: the job's kill-on-close ends the tree when the runtime's
    // handle closes, crash included; nothing can be left to reap.
    #[cfg(not(target_os = "linux"))]
    {
        let _ = r;
        0
    }
}

/// `/proc/<pid>/stat` field 22, when the process exists.
#[cfg(target_os = "linux")]
fn start_time_of(pid: u32) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let after = stat.rsplit_once(')')?.1;
    after.split_whitespace().nth(19)?.parse().ok()
}

/// What the worker reported at start.
#[derive(Clone, Debug, Default)]
pub struct WorkerFacts {
    /// Engine version.
    pub engine_version: String,
    /// Loaded interpreters: language → hash.
    pub interpreters: BTreeMap<String, String>,
    /// Interpreters that failed to load: language → reason.
    pub interpreter_errors: BTreeMap<String, String>,
}

/// One execution request.
#[derive(Clone, Debug)]
pub struct Execution {
    /// Owning task (for LIM-X3 accounting).
    pub task_id: TaskId,
    /// Ids.
    pub execution_id: ExecutionId,
    /// Action.
    pub action_id: ActionId,
    /// Tool name.
    pub tool: String,
    /// Canonical arguments.
    pub arguments: serde_json::Value,
    /// Effect.
    pub effect: EffectView,
    /// Deadline.
    pub deadline_ms: i64,
    /// Workspace root.
    pub workspace_root: String,
    /// Read-only workspace.
    pub read_only: bool,
    /// Replay.
    pub replay: Option<Replay>,
}

struct Live {
    child: ConfinedChild,
    stdin: Box<dyn std::io::Write + Send>,
    frames: Receiver<Result<Response, FrameError>>,
    facts: WorkerFacts,
    /// The worker's own cgroup, recorded for a later runtime and removed at its end.
    cgroup: Option<PathBuf>,
}

/// The worker handle. One execution at a time (LIM-X2).
pub struct Worker {
    config: WorkerConfig,
    live: Mutex<Option<Live>>,
    probe_cache: Mutex<Option<Probe>>,
    restarts: Mutex<BTreeMap<TaskId, u32>>,
    facts: Mutex<WorkerFacts>,
    /// The execution in flight, for the span of the log thread's lines.
    reader_span: mimlys_core::log_span::ReaderSpan,
}

impl Worker {
    /// A handle; the process starts at the first execution (or `start`).
    pub fn new(config: WorkerConfig) -> Arc<Self> {
        Arc::new(Self {
            config,
            live: Mutex::new(None),
            probe_cache: Mutex::new(None),
            restarts: Mutex::new(BTreeMap::new()),
            facts: Mutex::new(WorkerFacts::default()),
            reader_span: mimlys_core::log_span::ReaderSpan::new(),
        })
    }

    /// The probe (REQ-CONF-003, RFC 011 §1): a real worker started with the
    /// specification and `--probe` reports what its self-confinement held;
    /// the result is cached for the process. Without a specification, or when
    /// the worker cannot be started, every clause is unavailable with the reason.
    pub fn probe_confined(&self) -> Probe {
        if let Some(p) = self.probe_cache.lock().expect("probe cache").as_ref() {
            return p.clone();
        }
        let probe = self
            .run_probe()
            .unwrap_or_else(|reason| Probe::unavailable(&reason));
        *self.probe_cache.lock().expect("probe cache") = Some(probe.clone());
        probe
    }

    fn run_probe(&self) -> Result<Probe, String> {
        let spec = self
            .config
            .spec
            .as_ref()
            .ok_or_else(|| "no confinement specification".to_owned())?;
        let mut args = vec![
            "--probe".to_owned(),
            "--confine".to_owned(),
            serde_json::to_string(spec).expect("serializes"),
        ];
        for d in &self.config.host_tools {
            args.push("--host-tool".into());
            args.push(serde_json::to_string(d).expect("serializes"));
        }
        let command = WorkerCommand {
            program: self.config.program.clone(),
            args,
            env: worker_env(),
            current_dir: None,
        };
        let mut child = self
            .config
            .confinement
            .spawn(&command, self.config.spec.as_ref())
            .map_err(|e| e.to_string())?;
        drop(child.take_stdin());
        // Both streams are drained by threads: a worker that neither writes
        // nor exits must end in the timed wait, never in a blocking read.
        let out_rx = drain(child.take_stdout());
        let err_rx = drain(child.take_stderr());
        if !child.wait_within(Duration::from_millis(LIM_X4_WORKER_START_MS)) {
            let activity = child
                .activity()
                .map(|a| format!("; {a}"))
                .unwrap_or_default();
            child.kill_tree();
            let err = err_rx
                .recv_timeout(Duration::from_millis(LIM_O2_CANCEL_MS))
                .unwrap_or_default();
            return Err(format!(
                "worker probe timed out after {LIM_X4_WORKER_START_MS} ms{activity}{}",
                stderr_note(&err)
            ));
        }
        let status = child.try_wait().ok().flatten();
        let out = out_rx
            .recv_timeout(Duration::from_millis(LIM_O2_CANCEL_MS))
            .unwrap_or_default();
        let err = err_rx
            .recv_timeout(Duration::from_millis(LIM_O2_CANCEL_MS))
            .unwrap_or_default();
        let value: serde_json::Value = serde_json::from_str(out.trim()).map_err(|_| {
            format!(
                "worker probe unreadable (exit {}){}",
                status.map_or("unknown".to_owned(), |s| s.to_string()),
                stderr_note(&err)
            )
        })?;
        // The probe process has exited; its own cgroup is empty and goes now.
        #[cfg(target_os = "linux")]
        if let Some(dir) = value["cgroup"].as_str() {
            let _ = crate::confine::linux::remove_cgroup(std::path::Path::new(dir));
        }
        let letters: BTreeMap<String, ClauseState> =
            serde_json::from_value(value["clauses"].clone())
                .map_err(|_| "worker probe unreadable".to_owned())?;
        let mut clauses = BTreeMap::new();
        for c in Clause::ALL {
            let state = letters
                .get(&c.letter().to_string())
                .cloned()
                .unwrap_or_else(|| ClauseState::unavailable("not reported"));
            clauses.insert(c, state);
        }
        Ok(Probe { clauses })
    }

    /// The probe: the spawn side's clauses, merged with what a running worker
    /// reports about itself (REQ-CONF-003).
    pub fn probe(&self) -> Probe {
        self.config.confinement.probe()
    }

    /// What the last started worker reported.
    pub fn facts(&self) -> WorkerFacts {
        self.facts.lock().expect("facts").clone()
    }

    /// Starts the worker now and probes it.
    pub fn start(&self) -> Result<WorkerFacts, String> {
        let mut live = self.live.lock().expect("worker");
        if live.is_none() {
            *live = Some(self.spawn()?);
        }
        Ok(live.as_ref().map(|l| l.facts.clone()).unwrap_or_default())
    }

    fn spawn(&self) -> Result<Live, String> {
        let mut args = vec![
            "--serve".to_owned(),
            "--fuel".to_owned(),
            self.config.limits.fuel.to_string(),
            "--memory-bytes".to_owned(),
            self.config.limits.memory_bytes.to_string(),
            "--output-bytes".to_owned(),
            self.config.limits.output_bytes.to_string(),
        ];
        for i in &self.config.interpreters {
            args.push("--interpreter".into());
            args.push(format!(
                "{}={}:{}:{}",
                i.language,
                i.module.to_string_lossy(),
                i.sha256,
                i.script_name
            ));
        }
        if let Some(spec) = &self.config.spec {
            args.push("--confine".into());
            args.push(serde_json::to_string(spec).expect("serializes"));
        }
        if tracing::enabled!(target: "mimlys_worker", tracing::Level::DEBUG) {
            args.push("--trace".into());
        }
        for d in &self.config.host_tools {
            args.push("--host-tool".into());
            args.push(serde_json::to_string(d).expect("serializes"));
        }
        let command = WorkerCommand {
            program: self.config.program.clone(),
            args,
            env: worker_env(),
            current_dir: None,
        };
        let mut child = self
            .config
            .confinement
            .spawn(&command, self.config.spec.as_ref())
            .map_err(|e| e.to_string())?;
        let stdin = child.take_stdin().ok_or("worker stdin")?;
        let stdout = child.take_stdout().ok_or("worker stdout")?;
        let stderr = child.take_stderr().ok_or("worker stderr")?;
        let (tx, frames) = channel();
        std::thread::Builder::new()
            .name("mimlys-worker-frames".into())
            .spawn(move || {
                let mut reader = BufReader::new(stdout);
                loop {
                    let frame = read_frame::<Response>(&mut reader);
                    let eof = matches!(frame, Err(FrameError::Eof));
                    if tx.send(frame).is_err() || eof {
                        return;
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        // This thread outlives the step that started the worker, which serves
        // many: each line is written in the span of the execution in flight
        // when it is read (none between executions), not in a span captured here.
        let reader_span = self.reader_span.clone();
        std::thread::Builder::new()
            .name("mimlys-worker-log".into())
            .spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    reader_span.current().in_scope(|| {
                        match line.strip_prefix(crate::trace::PREFIX) {
                            Some(t) => {
                                tracing::debug!(target: "mimlys_worker", "worker trace: {t}")
                            }
                            None => tracing::info!(target: "mimlys_worker", "{line}"),
                        }
                    });
                }
            })
            .map_err(|e| e.to_string())?;
        let mut live = Live {
            child,
            stdin,
            frames,
            facts: WorkerFacts::default(),
            cgroup: None,
        };
        tracing::debug!(target: "mimlys_worker", "worker {} started, probing", live.child.id());
        let probing = Instant::now();
        write_frame(&mut live.stdin, &Request::Probe).map_err(|e| e.to_string())?;
        match live
            .frames
            .recv_timeout(Duration::from_millis(LIM_X4_WORKER_START_MS))
        {
            Ok(Ok(Response::ProbeResult {
                engine_version,
                interpreters,
                interpreter_errors,
                cgroup,
                ..
            })) => {
                tracing::debug!(
                    target: "mimlys_worker",
                    "worker probe answered in {} ms",
                    probing.elapsed().as_millis()
                );
                live.cgroup = cgroup;
                live.facts = WorkerFacts {
                    engine_version,
                    interpreters,
                    interpreter_errors,
                };
                *self.facts.lock().expect("facts") = live.facts.clone();
                self.write_record(&live);
                Ok(live)
            }
            other => {
                let activity = live
                    .child
                    .activity()
                    .map(|a| format!("; {a}"))
                    .unwrap_or_default();
                live.child.kill_tree();
                Err(format!("worker probe failed: {other:?}{activity}"))
            }
        }
    }

    /// Runs one execution; never panics the runtime; the worker's death is
    /// `host_fault{worker_died}` (REQ-ARCH-008).
    pub fn execute(&self, execution: Execution, cancel: Cancellation) -> ExecutionOutcome {
        let started = Instant::now();
        let mut live = self.live.lock().expect("worker");
        let _ = started;
        // The lock makes this the one execution in flight; the log thread
        // writes the worker's lines in the span current here (the step's).
        let _in_flight = self.reader_span.begin();
        if live.is_none() {
            let count = *self
                .restarts
                .lock()
                .expect("restarts")
                .get(&execution.task_id)
                .unwrap_or(&0);
            if count > LIM_X3_RESTARTS {
                return fault(HostFaultDetail::Unavailable, started, None);
            }
            match self.spawn() {
                Ok(l) => *live = Some(l),
                Err(detail) => {
                    tracing::warn!(target: "mimlys_worker", "spawn failed: {detail}");
                    return fault(HostFaultDetail::Unavailable, started, None);
                }
            }
        }
        let outcome = {
            let l = live.as_mut().expect("spawned");
            // The execution's clock starts when the request leaves; the spawn above is not the execution's time.
            run_one(l, &execution, &cancel, Instant::now())
        };
        if outcome.died {
            // The worker is gone: drop it and count the restart for the task.
            if let Some(mut l) = live.take() {
                l.child.kill_tree();
            }
            *self
                .restarts
                .lock()
                .expect("restarts")
                .entry(execution.task_id)
                .or_insert(0) += 1;
        }
        outcome.outcome
    }

    /// Kills the worker without a shutdown, for the death tests.
    pub fn kill_for_test(&self) {
        if let Some(l) = self.live.lock().expect("worker").as_mut() {
            l.child.kill_tree();
        }
    }

    /// Ends the worker; the tree is terminated (REQ-CONF-009).
    pub fn shutdown(&self) {
        if let Some(mut l) = self.live.lock().expect("worker").take() {
            let _ = write_frame(&mut l.stdin, &Request::Shutdown);
            if !l.child.wait_within(Duration::from_millis(LIM_O2_CANCEL_MS)) {
                l.child.kill_tree();
            }
            if let Some(path) = &self.config.record {
                let _ = std::fs::remove_file(path);
            }
            #[cfg(target_os = "linux")]
            if let Some(dir) = &l.cgroup {
                // Empty now that the tree is gone; a populated cgroup refuses, and the next start reaps it.
                let _ = crate::confine::linux::kill_cgroup(dir);
                let _ = crate::confine::linux::remove_cgroup(dir);
            }
        }
    }

    /// Writes the live worker's record for a later runtime to reap it (REQ-CONF-009).
    fn write_record(&self, live: &Live) {
        let Some(path) = &self.config.record else {
            return;
        };
        let pid = live.child.id();
        #[cfg(target_os = "linux")]
        let start_time = start_time_of(pid);
        #[cfg(not(target_os = "linux"))]
        let start_time = None;
        let record = WorkerRecord {
            pid,
            start_time,
            cgroup: live.cgroup.clone(),
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(path, serde_json::to_string(&record).expect("serializes"));
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.shutdown();
    }
}

struct RunResult {
    outcome: ExecutionOutcome,
    died: bool,
}

fn run_one(
    live: &mut Live,
    execution: &Execution,
    cancel: &Cancellation,
    started: Instant,
) -> RunResult {
    let request = Request::Execute {
        execution_id: execution.execution_id,
        action_id: execution.action_id,
        tool: execution.tool.clone(),
        arguments: execution.arguments.clone(),
        effect: execution.effect,
        deadline_ms: execution.deadline_ms,
        workspace_root: execution.workspace_root.clone(),
        read_only: execution.read_only,
        replay: execution.replay.clone(),
    };
    if write_frame(&mut live.stdin, &request).is_err() {
        return RunResult {
            outcome: fault(HostFaultDetail::WorkerDied, started, None),
            died: true,
        };
    }
    let deadline = Duration::from_millis(execution.deadline_ms.max(0) as u64 + LIM_O2_CANCEL_MS);
    let mut cancel_sent_at: Option<Instant> = None;
    loop {
        if cancel.is_cancelled() && cancel_sent_at.is_none() {
            let _ = write_frame(
                &mut live.stdin,
                &Request::Cancel {
                    execution_id: execution.execution_id,
                },
            );
            cancel_sent_at = Some(Instant::now());
        }
        let overdue = started.elapsed() > deadline
            || cancel_sent_at
                .is_some_and(|t| t.elapsed() > Duration::from_millis(LIM_O2_CANCEL_MS));
        if overdue {
            // Exited already (a crash the frame reader has not reported yet)
            // or unresponsive: the log says which, with the status when there is one.
            let note = match live.child.try_wait() {
                Ok(Some(status)) => format!("the worker exited before answering: {status}"),
                _ => {
                    if let Some(sample) = stack_sample(live.child.id()) {
                        tracing::warn!(target: "mimlys_worker", "stack sample of the unresponsive worker:\n{sample}");
                    }
                    "the worker did not answer within LIM-O2".to_owned()
                }
            };
            live.child.kill_tree();
            return RunResult {
                outcome: fault(HostFaultDetail::WorkerDied, started, Some(&note)),
                died: true,
            };
        }
        match live.frames.recv_timeout(Duration::from_millis(50)) {
            Ok(Ok(Response::ExecutionRecord {
                execution_id,
                termination,
                payload,
                stdout,
                stderr,
                notices,
                metrics,
            })) if execution_id == execution.execution_id => {
                let failure = match &termination {
                    Termination::Cancelled => None,
                    other => FailureCause::from_termination(other),
                };
                return RunResult {
                    outcome: ExecutionOutcome {
                        termination,
                        failure,
                        payload,
                        stdout,
                        stderr,
                        notices,
                        metrics,
                    },
                    died: false,
                };
            }
            Ok(Ok(Response::Error { detail })) => {
                tracing::warn!(target: "mimlys_worker", "worker error: {detail}");
                return RunResult {
                    outcome: fault(HostFaultDetail::Unavailable, started, Some(&detail)),
                    died: false,
                };
            }
            Ok(Ok(_)) => continue,
            Ok(Err(FrameError::Eof)) | Err(RecvTimeoutError::Disconnected) => {
                // Its output closed: the exit status, when it ends within LIM-O2, says how.
                let note = if live
                    .child
                    .wait_within(Duration::from_millis(LIM_O2_CANCEL_MS))
                {
                    match live.child.try_wait() {
                        Ok(Some(status)) => format!("the worker exited: {status}"),
                        _ => "the worker closed its output".to_owned(),
                    }
                } else {
                    "the worker closed its output and did not exit within LIM-O2".to_owned()
                };
                return RunResult {
                    outcome: fault(HostFaultDetail::WorkerDied, started, Some(&note)),
                    died: true,
                };
            }
            Ok(Err(e)) => {
                tracing::warn!(target: "mimlys_worker", "protocol: {e}");
                live.child.kill_tree();
                return RunResult {
                    outcome: fault(HostFaultDetail::WorkerDied, started, Some(&e.to_string())),
                    died: true,
                };
            }
            Err(RecvTimeoutError::Timeout) => continue,
        }
    }
}

/// A stack sample of a worker that does not answer, for the log of debug
/// builds on macOS (`/usr/bin/sample`); nowhere else.
#[cfg(all(target_os = "macos", debug_assertions))]
fn stack_sample(pid: u32) -> Option<String> {
    let out = std::process::Command::new("/usr/bin/sample")
        .args([&pid.to_string(), "1", "-mayDie"])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let graph = text
        .split("\nTotal number in stack")
        .next()
        .unwrap_or(&text)
        .split("\nBinary Images:")
        .next()
        .unwrap_or(&text);
    // Rust's v0-mangled names are long; their readable part is at the end.
    let shortened: Vec<String> = graph
        .lines()
        .map(|line| {
            line.split(' ')
                .map(|tok| {
                    if tok.starts_with("_R") && tok.len() > 80 {
                        let tail: String = tok
                            .chars()
                            .rev()
                            .take(72)
                            .collect::<Vec<_>>()
                            .into_iter()
                            .rev()
                            .collect();
                        format!("…{tail}")
                    } else {
                        tok.to_owned()
                    }
                })
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect();
    Some(shortened.join("\n").chars().take(120_000).collect())
}

#[cfg(not(all(target_os = "macos", debug_assertions)))]
fn stack_sample(_pid: u32) -> Option<String> {
    None
}

fn fault(detail: HostFaultDetail, started: Instant, note: Option<&str>) -> ExecutionOutcome {
    if let Some(n) = note {
        tracing::warn!(target: "mimlys_worker", "{n}");
    }
    ExecutionOutcome {
        termination: Termination::HostFault { detail },
        failure: Some(FailureCause::SandboxFault { detail }),
        payload: serde_json::Value::Null,
        stdout: Vec::new(),
        stderr: Vec::new(),
        notices: Vec::new(),
        metrics: ExecutionMetrics {
            wall_ms: started.elapsed().as_millis() as i64,
            ..ExecutionMetrics::default()
        },
    }
}

/// Reads a child's stream to its end on a thread; the receiver gets the text once.
fn drain<R: std::io::Read + Send + 'static>(
    stream: Option<R>,
) -> std::sync::mpsc::Receiver<String> {
    let (tx, rx) = channel();
    if let Some(mut s) = stream {
        std::thread::spawn(move || {
            let mut text = String::new();
            let _ = s.read_to_string(&mut text);
            let _ = tx.send(text);
        });
    }
    rx
}

/// The tail of a worker's standard error for a reason string, bounded.
fn stderr_note(err: &str) -> String {
    let t = err.trim();
    if t.is_empty() {
        return String::new();
    }
    let tail: String = t
        .chars()
        .rev()
        .take(1_500)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("; worker stderr: {tail}")
}

/// The worker's environment (clause E): a marker and, in debug builds, the
/// probe-failure injection of REQ-TEST-013 forwarded from the runtime's.
fn worker_env() -> Vec<(String, String)> {
    let mut env = vec![("MIMLYS_WORKER".to_owned(), "1".to_owned())];
    // Windows: the system root is part of the platform, not of the user's
    // environment; without it the loader and `cmd`-free spawns still work,
    // but toolchains do not. Forwarded by name only.
    if cfg!(windows)
        && let Ok(root) = std::env::var("SystemRoot")
    {
        env.push(("SystemRoot".to_owned(), root));
    }
    if cfg!(debug_assertions) {
        for name in [
            "MIMLYS_PROBE_FAIL",
            "MIMLYS_PROBE_BEST_EFFORT",
            "MIMLYS_PROBE_NO_CGROUP",
        ] {
            if let Ok(v) = std::env::var(name) {
                env.push((name.to_owned(), v));
            }
        }
    }
    env
}

/// The clause a worker reports about itself, merged by the spawn side.
pub fn worker_side_clauses(
    no_new_privs: bool,
    env_explicit: bool,
) -> BTreeMap<Clause, ClauseState> {
    let mut m = BTreeMap::new();
    m.insert(
        Clause::Privilege,
        if no_new_privs {
            ClauseState::ENFORCED
        } else {
            ClauseState::Unavailable {
                reason: "no_new_privs not set".into(),
            }
        },
    );
    m.insert(
        Clause::Environment,
        if env_explicit {
            ClauseState::ENFORCED
        } else {
            ClauseState::Unavailable {
                reason: "environment inherited".into(),
            }
        },
    );
    m
}

/// Writes a line to the worker's log (its standard error).
pub fn log(line: &str) {
    let _ = writeln!(std::io::stderr(), "{line}");
}
