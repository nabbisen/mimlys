//! The execution worker's entry point (REQ-ARCH-008): every sandbox
//! execution happens in this process, never in the runtime's. The
//! `mimlys-worker` binary of the `mimlys` crate calls [`main`]. Frames arrive on standard
//! input and records leave on standard output (external design §13.1);
//! the log goes to standard error. Each execution runs under panic
//! containment (REQ-ENG-004); a failure of the sandbox is reported as
//! `host_fault`, never as a process failure (REQ-SBX-012).

use crate::interpreters::{Configured, load_verified};
use crate::protocol::{ClauseState, FrameError, Request, Response, read_frame, write_frame};
use crate::sandbox::{
    ENGINE_VERSION, Interrupt, InterruptReason, Limits, Request as RunRequest, Sandbox,
};
use mimlys_core::action::{HostFaultDetail, Termination};
use mimlys_core::limits::LIM_S5_MODULE_BYTES;
use mimlys_core::observation::Notice;
use mimlys_core::path::WorkspacePath;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};

struct Interpreter {
    module: wasmtime::Module,
    sha256: String,
    script_name: String,
}

struct State {
    sandbox: Sandbox,
    interpreters: BTreeMap<String, Interpreter>,
    interpreter_errors: BTreeMap<String, String>,
    limits: Limits,
    /// The declared host tools, by name (RFC 011 §6).
    host_tools: BTreeMap<String, mimlys_core::host_tool::HostToolDeclaration>,
    /// `RLIMIT_AS` for host tools when no cgroup bounds the tree.
    host_memory: Option<u64>,
    /// The `tool` sub-cgroup every host tool's tree is attached to (clause K).
    tool_cgroup: Option<std::path::PathBuf>,
}

/// What the worker's self-confinement produced (RFC 011 §1), set once at start.
static APPLIED: std::sync::OnceLock<crate::confine::Applied> = std::sync::OnceLock::new();

/// Applies the confinement specification before anything else runs.
fn confine_self(args: &[String]) -> crate::confine::Applied {
    let spec = args
        .iter()
        .position(|a| a == "--confine")
        .and_then(|i| args.get(i + 1))
        .and_then(|json| serde_json::from_str::<crate::confine::ConfinementSpec>(json).ok());
    let applied = match spec {
        #[cfg(target_os = "linux")]
        Some(spec) => crate::confine::linux::apply(&spec),
        // Windows: the parent confined this process at creation; what holds
        // is read from the own token and job (RFC 011 §3).
        #[cfg(windows)]
        Some(spec) => crate::confine::Applied {
            report: crate::confine::windows::inspect(&spec),
            cgroup: None,
            tool_cgroup: None,
            rlimit_memory: false,
        },
        #[cfg(not(any(target_os = "linux", windows)))]
        Some(_) => {
            let mut report = BTreeMap::new();
            let milestone = "M6c";
            for letter in ["F", "N", "K", "M", "C"] {
                report.insert(
                    letter.to_owned(),
                    ClauseState::unavailable(&format!("backend arrives in {milestone}")),
                );
            }
            crate::confine::Applied {
                report,
                cgroup: None,
                tool_cgroup: None,
                rlimit_memory: false,
            }
        }
        None => {
            let mut report = BTreeMap::new();
            for letter in ["F", "N", "K", "M", "C"] {
                report.insert(
                    letter.to_owned(),
                    ClauseState::unavailable("no confinement specification"),
                );
            }
            crate::confine::Applied {
                report,
                cgroup: None,
                tool_cgroup: None,
                rlimit_memory: false,
            }
        }
    };
    let _ = APPLIED.set(applied.clone());
    applied
}

fn parse_host_tools(
    args: &[String],
) -> BTreeMap<String, mimlys_core::host_tool::HostToolDeclaration> {
    let mut out = BTreeMap::new();
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--host-tool"
            && let Some(json) = args.get(i + 1)
            && let Ok(d) = serde_json::from_str::<mimlys_core::host_tool::HostToolDeclaration>(json)
        {
            out.insert(d.name.clone(), d);
            i += 1;
        }
        i += 1;
    }
    out
}

enum Incoming {
    Frame(Result<Request, FrameError>),
    Done(Box<Response>),
}

/// The worker's `main`.
pub fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--print-env-names") {
        for (k, _) in std::env::vars_os() {
            println!("{}", k.to_string_lossy());
        }
        return;
    }
    #[cfg(target_os = "linux")]
    {
        let _ = crate::confine::set_no_new_privs();
    }
    // RFC 011 §1: the worker confines itself before reading its first frame
    // (and before any thread exists); what held is what it reports.
    let applied = confine_self(&args);
    if args.iter().any(|a| a == "--probe") {
        let clauses = own_clauses();
        println!(
            "{}",
            serde_json::to_string(
                &serde_json::json!({ "clauses": clauses, "cgroup": applied.cgroup })
            )
            .expect("serializes")
        );
        return;
    }
    let configured = parse_interpreters(&args);
    let limits = parse_limits(&args);
    let host_tools = parse_host_tools(&args);
    let host_memory = args
        .iter()
        .position(|a| a == "--confine")
        .and_then(|i| args.get(i + 1))
        .and_then(|json| serde_json::from_str::<crate::confine::ConfinementSpec>(json).ok())
        .filter(|_| applied.rlimit_memory)
        .map(|s| s.memory_bytes);
    let state = match build_state(configured, limits, host_tools, host_memory) {
        Ok(s) => s,
        Err(detail) => {
            eprintln!("worker: cannot start the sandbox: {detail}");
            std::process::exit(1);
        }
    };
    serve(state);
}

fn own_clauses() -> BTreeMap<String, ClauseState> {
    let no_new_privs = crate::confine::no_new_privs();
    // What the runtime gives the worker (`worker_env`), plus what the
    // platform's own libraries set inside the process: on macOS,
    // CoreFoundation sets `__CF_USER_TEXT_ENCODING` at load from the
    // process's user id, not from the parent's environment.
    let mut allowed = vec![
        "MIMLYS_WORKER",
        "MIMLYS_PROBE_FAIL",
        "MIMLYS_PROBE_BEST_EFFORT",
        "SystemRoot",
        "SYSTEMROOT",
    ];
    if cfg!(target_os = "macos") {
        allowed.push("__CF_USER_TEXT_ENCODING");
    }
    let extras = crate::confine::environment_extras(&allowed);
    let mut m: BTreeMap<String, ClauseState> =
        APPLIED.get().map(|a| a.report.clone()).unwrap_or_default();
    m.insert(
        "E".into(),
        if extras.is_empty() {
            ClauseState::enforced("explicit_environment")
        } else {
            // The names (never the values), so that the report says what leaked.
            let shown = extras
                .iter()
                .take(8)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ");
            let more = extras.len().saturating_sub(8);
            ClauseState::unavailable(&if more > 0 {
                format!("environment inherited: {shown} (+{more} more)")
            } else {
                format!("environment inherited: {shown}")
            })
        },
    );
    if !m.contains_key("P") {
        m.insert(
            "P".into(),
            if no_new_privs {
                ClauseState::enforced("no_new_privs")
            } else if cfg!(target_os = "linux") {
                ClauseState::unavailable("no_new_privs not set")
            } else {
                ClauseState::unavailable("backend arrives with the platform's confinement")
            },
        );
    }
    if let Ok(letter) = std::env::var("MIMLYS_PROBE_FAIL")
        && cfg!(debug_assertions)
    {
        m.insert(
            letter.to_ascii_uppercase(),
            ClauseState::Unavailable {
                reason: "injected failure (MIMLYS_PROBE_FAIL)".into(),
            },
        );
    }
    if let Ok(letter) = std::env::var("MIMLYS_PROBE_BEST_EFFORT")
        && cfg!(debug_assertions)
    {
        m.insert(
            letter.to_ascii_uppercase(),
            ClauseState::best_effort("injected (MIMLYS_PROBE_BEST_EFFORT)"),
        );
    }
    m
}

fn parse_interpreters(args: &[String]) -> Vec<Configured> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--interpreter" && i + 1 < args.len() {
            let spec = &args[i + 1];
            if let Some((language, rest)) = spec.split_once('=') {
                let parts: Vec<&str> = rest.rsplitn(3, ':').collect();
                if parts.len() == 3 {
                    out.push(Configured {
                        language: language.to_owned(),
                        module: PathBuf::from(parts[2]),
                        sha256: parts[1].to_owned(),
                        script_name: parts[0].to_owned(),
                    });
                }
            }
            i += 2;
        } else {
            i += 1;
        }
    }
    out
}

fn flag(args: &[String], name: &str) -> Option<u64> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
}

fn parse_limits(args: &[String]) -> Limits {
    let d = Limits::default();
    Limits {
        fuel: flag(args, "--fuel").unwrap_or(d.fuel),
        deadline_ms: d.deadline_ms,
        memory_bytes: flag(args, "--memory-bytes")
            .map(|v| v as usize)
            .unwrap_or(d.memory_bytes),
        output_bytes: flag(args, "--output-bytes")
            .map(|v| v as usize)
            .unwrap_or(d.output_bytes),
    }
}

fn build_state(
    configured: Vec<Configured>,
    limits: Limits,
    host_tools: BTreeMap<String, mimlys_core::host_tool::HostToolDeclaration>,
    host_memory: Option<u64>,
) -> Result<State, String> {
    let sandbox = Sandbox::new().map_err(|e| e.to_string())?;
    let mut interpreters = BTreeMap::new();
    let mut interpreter_errors = BTreeMap::new();
    for c in configured {
        match load_verified(&c)
            .map_err(|e| e.to_string())
            .and_then(|bytes| sandbox.compile(&bytes))
        {
            Ok(module) => {
                interpreters.insert(
                    c.language.clone(),
                    Interpreter {
                        module,
                        sha256: c.sha256.clone(),
                        script_name: c.script_name.clone(),
                    },
                );
            }
            Err(e) => {
                interpreter_errors.insert(c.language.clone(), e);
            }
        }
    }
    Ok(State {
        sandbox,
        interpreters,
        interpreter_errors,
        limits,
        host_tools,
        host_memory,
        tool_cgroup: APPLIED.get().and_then(|a| a.tool_cgroup.clone()),
    })
}

fn serve(state: State) {
    let state = Arc::new(state);
    let (tx, rx): (std::sync::mpsc::Sender<Incoming>, Receiver<Incoming>) = channel();
    let reader_tx = tx.clone();
    std::thread::spawn(move || {
        let mut stdin = std::io::stdin().lock();
        loop {
            let frame = read_frame::<Request>(&mut stdin);
            let eof = matches!(frame, Err(FrameError::Eof));
            if reader_tx.send(Incoming::Frame(frame)).is_err() || eof {
                return;
            }
        }
    });
    let stdout = Arc::new(Mutex::new(std::io::stdout()));
    let mut in_flight: Option<(mimlys_core::ids::ExecutionId, Interrupt)> = None;
    let mut shutting_down = false;
    for incoming in rx {
        match incoming {
            Incoming::Frame(Err(FrameError::Eof)) => {
                if let Some((_, interrupt)) = &in_flight {
                    interrupt.fire(InterruptReason::Cancelled);
                }
                return;
            }
            Incoming::Frame(Err(e)) => {
                eprintln!("worker: protocol error: {e}");
                std::process::exit(2);
            }
            Incoming::Frame(Ok(Request::Probe)) => {
                let response = Response::ProbeResult {
                    clauses: own_clauses(),
                    cgroup: APPLIED.get().and_then(|a| a.cgroup.clone()),
                    engine_version: ENGINE_VERSION.into(),
                    interpreters: state
                        .interpreters
                        .iter()
                        .map(|(k, v)| (k.clone(), v.sha256.clone()))
                        .collect(),
                    interpreter_errors: state.interpreter_errors.clone(),
                };
                send(&stdout, &response);
            }
            Incoming::Frame(Ok(Request::Cancel { execution_id })) => {
                if let Some((id, interrupt)) = &in_flight
                    && *id == execution_id
                {
                    interrupt.fire(InterruptReason::Cancelled);
                }
            }
            Incoming::Frame(Ok(Request::Shutdown)) => {
                shutting_down = true;
                match &in_flight {
                    Some((_, interrupt)) => interrupt.fire(InterruptReason::Cancelled),
                    None => return,
                }
            }
            Incoming::Frame(Ok(Request::Execute {
                execution_id,
                tool,
                arguments,
                deadline_ms,
                workspace_root,
                read_only,
                replay,
                ..
            })) => {
                if in_flight.is_some() {
                    send(
                        &stdout,
                        &Response::Error {
                            detail: "an execution is already in flight (LIM-X2)".into(),
                        },
                    );
                    continue;
                }
                let interrupt = state.sandbox.interrupt(deadline_ms.max(1) as u64);
                in_flight = Some((execution_id, interrupt.clone()));
                let done_tx = tx.clone();
                let state = Arc::clone(&state);
                std::thread::spawn(move || {
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        execute(
                            &state,
                            &tool,
                            &arguments,
                            deadline_ms,
                            &workspace_root,
                            read_only,
                            replay,
                            &interrupt,
                        )
                    }));
                    let (termination, stdout_bytes, stderr_bytes, metrics, notices) = match result {
                        Ok(r) => r,
                        Err(_) => (
                            Termination::HostFault {
                                detail: HostFaultDetail::Unavailable,
                            },
                            Vec::new(),
                            Vec::new(),
                            Default::default(),
                            vec![],
                        ),
                    };
                    let mut payload = serde_json::json!({ "termination": serde_json::to_value(&termination).ok().and_then(|v| v.get("termination").cloned()).unwrap_or_default() });
                    if let Termination::Exited { exit_code } = &termination {
                        payload["exit_code"] = serde_json::json!(exit_code);
                    }
                    let _ = done_tx.send(Incoming::Done(Box::new(Response::ExecutionRecord {
                        execution_id,
                        termination,
                        payload,
                        stdout: stdout_bytes,
                        stderr: stderr_bytes,
                        notices,
                        metrics,
                    })));
                });
            }
            Incoming::Done(response) => {
                in_flight = None;
                send(&stdout, &response);
                if shutting_down {
                    return;
                }
            }
        }
    }
}

fn send(stdout: &Arc<Mutex<std::io::Stdout>>, response: &Response) {
    let mut out = stdout.lock().expect("stdout");
    if let Err(e) = write_frame(&mut *out, response) {
        eprintln!("worker: cannot write a frame: {e}");
        std::process::exit(2);
    }
}

type Executed = (
    Termination,
    Vec<u8>,
    Vec<u8>,
    mimlys_core::action::ExecutionMetrics,
    Vec<Notice>,
);

#[allow(clippy::too_many_arguments)]
fn execute(
    state: &State,
    tool: &str,
    arguments: &serde_json::Value,
    deadline_ms: i64,
    workspace_root: &str,
    read_only: bool,
    replay: Option<crate::protocol::Replay>,
    interrupt: &Interrupt,
) -> Executed {
    let fault = |detail: HostFaultDetail| -> Executed {
        (
            Termination::HostFault { detail },
            Vec::new(),
            Vec::new(),
            Default::default(),
            vec![],
        )
    };
    let workspace = PathBuf::from(workspace_root);
    let tmp_root = workspace
        .parent()
        .map(|p| p.join("tmp"))
        .unwrap_or_else(|| workspace.join(".tmp"));
    let tmp = tmp_root.join(format!("exec-{}", std::process::id()));
    if std::fs::create_dir_all(&tmp).is_err() {
        return fault(HostFaultDetail::Unavailable);
    }
    let args_obj = arguments.as_object().cloned().unwrap_or_default();
    if tool == "run_host_tool" {
        let name = args_obj
            .get("tool")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let Some(declaration) = state.host_tools.get(name) else {
            return fault(HostFaultDetail::Unavailable);
        };
        let extra: Vec<String> = args_obj
            .get("args")
            .and_then(|a| a.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        let outcome = crate::host_tool::run(crate::host_tool::HostRun {
            declaration,
            extra_args: extra,
            workspace: &workspace,
            tmp: &tmp,
            deadline_ms,
            interrupt,
            rlimit_memory: state.host_memory,
            tool_cgroup: state.tool_cgroup.as_deref(),
        });
        return (
            outcome.termination,
            outcome.stdout,
            outcome.stderr,
            outcome.metrics,
            outcome.notices,
        );
    }
    let mut argv: Vec<String> = Vec::new();
    let extra: Vec<String> = args_obj
        .get("args")
        .and_then(|a| a.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    let stdin = args_obj
        .get("stdin")
        .and_then(|s| s.as_str())
        .map(|s| s.as_bytes().to_vec())
        .unwrap_or_default();
    let (module, owned);
    match tool {
        "run_script" => {
            let language = args_obj
                .get("language")
                .and_then(|l| l.as_str())
                .unwrap_or("");
            let Some(interpreter) = state.interpreters.get(language) else {
                let _ = std::fs::remove_dir_all(&tmp);
                return fault(HostFaultDetail::Unavailable);
            };
            let source = args_obj
                .get("source")
                .and_then(|s| s.as_str())
                .unwrap_or("");
            if std::fs::write(tmp.join(&interpreter.script_name), source).is_err() {
                let _ = std::fs::remove_dir_all(&tmp);
                return fault(HostFaultDetail::Unavailable);
            }
            argv.push(language.to_owned());
            if let Some(p) = crate::interpreters::pinned(language) {
                argv.extend(p.args.iter().map(|a| (*a).to_owned()));
            }
            argv.push(format!("/tmp/{}", interpreter.script_name));
            module = &interpreter.module;
        }
        "run_wasm" => {
            let path = args_obj
                .get("module")
                .and_then(|m| m.as_str())
                .unwrap_or("");
            let Ok(canonical) = WorkspacePath::canonicalize(path) else {
                let _ = std::fs::remove_dir_all(&tmp);
                return fault(HostFaultDetail::InvalidModule);
            };
            let host_path = canonical
                .components()
                .fold(workspace.clone(), |p, c| p.join(c));
            match std::fs::metadata(&host_path) {
                Ok(m) if m.is_file() && m.len() <= LIM_S5_MODULE_BYTES => {}
                _ => {
                    let _ = std::fs::remove_dir_all(&tmp);
                    return fault(HostFaultDetail::InvalidModule);
                }
            }
            let Ok(bytes) = std::fs::read(&host_path) else {
                let _ = std::fs::remove_dir_all(&tmp);
                return fault(HostFaultDetail::InvalidModule);
            };
            match state.sandbox.compile(&bytes) {
                Ok(m) => owned = m,
                Err(_) => {
                    let _ = std::fs::remove_dir_all(&tmp);
                    return fault(HostFaultDetail::InvalidModule);
                }
            }
            argv.push(path.to_owned());
            module = &owned;
        }
        _ => {
            let _ = std::fs::remove_dir_all(&tmp);
            return fault(HostFaultDetail::Unavailable);
        }
    }
    argv.extend(extra);
    let request = RunRequest {
        args: argv,
        stdin,
        workspace: workspace.clone(),
        read_only,
        tmp: tmp.clone(),
        replay,
        limits: Limits {
            deadline_ms: deadline_ms.max(1) as u64,
            ..state.limits
        },
    };
    let outcome = state.sandbox.run(module, &request, interrupt);
    let _ = std::fs::remove_dir_all(&tmp);
    let mut notices = Vec::new();
    for (stream, bytes) in [
        (mimlys_core::observation::Stream::Stdout, &outcome.stdout),
        (mimlys_core::observation::Stream::Stderr, &outcome.stderr),
    ] {
        if bytes.len() >= state.limits.output_bytes
            && outcome.termination == Termination::OutputLimit
        {
            notices.push(Notice::OutputTruncated {
                stream,
                total_bytes: bytes.len() as u64,
            });
        }
    }
    let _ = Path::new("");
    (
        outcome.termination,
        outcome.stdout,
        outcome.stderr,
        outcome.metrics,
        notices,
    )
}
