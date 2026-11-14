//! The worker through its binary (REQ-ARCH-008, REQ-CONF-002, REQ-TEST-008,
//! REQ-TEST-013): the explicit environment, the honest probe, a module run
//! end to end, cancellation, the worker's death and its restart budget.

use mimlys_core::action::{HostFaultDetail, Termination};
use mimlys_core::ids::{ActionId, ExecutionId, ScriptedIds, TaskId};
use mimlys_core::spi::Cancellation;
use mimlys_exec::confine::platform;
use mimlys_exec::worker::{Execution, Worker, WorkerConfig};
use std::path::PathBuf;
use std::sync::Arc;

fn worker_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_mimlys-worker"))
}

/// REQ-CONF-002 / clause E: the child sees exactly the environment it was given.
#[test]
fn req_conf_002_explicit_environment() {
    let out = std::process::Command::new(worker_bin())
        .arg("--print-env-names")
        .env_clear()
        .env("MIMLYS_WORKER_ALLOWED", "1")
        .output()
        .unwrap();
    let names = String::from_utf8_lossy(&out.stdout);
    let names: Vec<&str> = names.lines().collect();
    assert_eq!(names, ["MIMLYS_WORKER_ALLOWED"], "{names:?}");
}

/// REQ-TEST-013: an injected probe failure is reported.
#[test]
fn req_test_013_probe_honesty() {
    let out = std::process::Command::new(worker_bin())
        .arg("--probe")
        .env_clear()
        .env("MIMLYS_PROBE_FAIL", "E")
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    let json: serde_json::Value =
        serde_json::from_str(text.trim()).unwrap_or_else(|_| panic!("{text}"));
    assert_eq!(json["clauses"]["E"]["state"], "unavailable");
    assert!(
        json["clauses"]["E"]["reason"]
            .as_str()
            .unwrap()
            .contains("injected")
    );
    let out = std::process::Command::new(worker_bin())
        .arg("--probe")
        .env_clear()
        .output()
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["clauses"]["E"]["state"], "enforced");
    if cfg!(target_os = "linux") {
        assert_eq!(
            json["clauses"]["P"]["state"], "enforced",
            "no_new_privs is set by the worker itself"
        );
    }
}

struct Fx {
    _dir: tempfile::TempDir,
    workspace: PathBuf,
    worker: Arc<Worker>,
    ids: ScriptedIds,
}

fn fx() -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let workspace = dir.path().join("tree");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::create_dir_all(dir.path().join("tmp")).unwrap();
    let worker = Worker::new(WorkerConfig {
        program: worker_bin(),
        interpreters: vec![],
        confinement: platform(),
        limits: mimlys_exec::sandbox::Limits::default(),
        spec: None,
        host_tools: vec![],
        record: None,
    });
    Fx {
        _dir: dir,
        workspace,
        worker,
        ids: ScriptedIds::default(),
    }
}

fn execution(fx: &Fx, tool: &str, arguments: serde_json::Value, deadline_ms: i64) -> Execution {
    Execution {
        task_id: TaskId::new(&fx.ids),
        execution_id: ExecutionId::new(&fx.ids),
        action_id: ActionId::new(&fx.ids),
        tool: tool.into(),
        arguments,
        effect: mimlys_core::action::EffectView {
            kind: mimlys_core::action::EffectKind::WorkspaceWrite,
            reversible: true,
            idempotent: false,
        },
        deadline_ms,
        workspace_root: fx.workspace.to_string_lossy().into_owned(),
        read_only: false,
        replay: None,
    }
}

fn write_module(fx: &Fx, name: &str, wat: &str) {
    let bytes = wat::parse_str(wat).unwrap();
    std::fs::write(fx.workspace.join(name), bytes).unwrap();
}

const HELLO: &str = r#"(module (import "wasi_snapshot_preview1" "fd_write" (func $fd_write (param i32 i32 i32 i32) (result i32)))
    (import "wasi_snapshot_preview1" "proc_exit" (func $exit (param i32)))
    (memory 1) (export "memory" (memory 0)) (data (i32.const 0) "hello\n")
    (func (export "_start")
        (i32.store (i32.const 100) (i32.const 0)) (i32.store (i32.const 104) (i32.const 6))
        (drop (call $fd_write (i32.const 1) (i32.const 100) (i32.const 1) (i32.const 200)))
        (call $exit (i32.const 7))))"#;

/// REQ-ARCH-008: `run_wasm` runs in the worker and reports its record; the
/// probe carries the engine version.
#[test]
fn req_arch_008_run_wasm_in_the_worker() {
    let fx = fx();
    write_module(&fx, "hello.wasm", HELLO);
    let facts = fx.worker.start().unwrap();
    assert_eq!(facts.engine_version, "50.0.1");
    let out = fx.worker.execute(
        execution(
            &fx,
            "run_wasm",
            serde_json::json!({"module": "hello.wasm"}),
            5_000,
        ),
        Cancellation::new(),
    );
    assert_eq!(out.termination, Termination::Exited { exit_code: 7 });
    assert_eq!(out.stdout, b"hello\n");
    assert_eq!(out.payload["exit_code"], 7);
    assert!(out.failure.is_some(), "a non-zero exit is a tool error");
    assert!(out.metrics.setup_ms.is_some());
    let missing = fx.worker.execute(
        execution(
            &fx,
            "run_wasm",
            serde_json::json!({"module": "nope.wasm"}),
            5_000,
        ),
        Cancellation::new(),
    );
    assert_eq!(
        missing.termination,
        Termination::HostFault {
            detail: HostFaultDetail::InvalidModule
        }
    );
}

const LOOP: &str =
    r#"(module (memory 1) (export "memory" (memory 0)) (func (export "_start") (loop br 0)))"#;

/// REQ-TOOL-007 / REQ-CONF-009: a cancel reaches the worker within LIM-O2.
#[test]
fn req_tool_007_cancel_reaches_the_worker() {
    let fx = fx();
    write_module(&fx, "loop.wasm", LOOP);
    let cancel = Cancellation::new();
    let c = cancel.clone();
    // The instant of the cancel: the time to end is measured from the state
    // change, not from the start of the run (the worker's start included).
    let cancelled_at = std::sync::Arc::new(std::sync::Mutex::new(None));
    let at = std::sync::Arc::clone(&cancelled_at);
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(300));
        *at.lock().unwrap() = Some(std::time::Instant::now());
        c.cancel();
    });
    let out = fx.worker.execute(
        execution(
            &fx,
            "run_wasm",
            serde_json::json!({"module": "loop.wasm"}),
            60_000,
        ),
        cancel,
    );
    assert_eq!(out.termination, Termination::Cancelled);
    assert!(out.failure.is_none());
    // The loop's own deadline is 60 s, so `Cancelled` is what proves that the
    // cancel reached the worker. LIM-O2 is the contract; the time allowed is five
    // times the limit: a cancel that needs a multiple of it is a defect, a stall of
    // a slow machine shorter than that is not.
    let took = cancelled_at
        .lock()
        .unwrap()
        .expect("the cancel was sent")
        .elapsed();
    assert!(
        took < std::time::Duration::from_millis(5 * mimlys_core::limits::LIM_O2_CANCEL_MS),
        "the cancel took {took:?} to reach the worker"
    );
}

/// REQ-SBX-002 through the worker: the deadline ends a loop as `deadline`.
#[test]
fn req_sbx_002_deadline_through_the_worker() {
    let fx = fx();
    write_module(&fx, "loop.wasm", LOOP);
    let out = fx.worker.execute(
        execution(
            &fx,
            "run_wasm",
            serde_json::json!({"module": "loop.wasm"}),
            200,
        ),
        Cancellation::new(),
    );
    assert_eq!(out.termination, Termination::Deadline);
}

/// REQ-SBX-012 (external design §12, §7.4), fail closed: a worker that cannot
/// be established (its program is not there) ends the execution
/// `host_fault{unavailable}`; the module does not run (no output, no exit
/// code, no fuel spent), and nothing falls back to running it in the
/// runtime's own process. The same execution through the real worker runs
/// (the control).
#[test]
fn req_sbx_012_a_worker_that_cannot_be_established_fails_closed() {
    let fx = fx();
    write_module(&fx, "hello.wasm", HELLO);
    let run = |worker: &Worker| {
        worker.execute(
            execution(
                &fx,
                "run_wasm",
                serde_json::json!({"module": "hello.wasm"}),
                5_000,
            ),
            Cancellation::new(),
        )
    };
    let control = run(&fx.worker);
    assert_eq!(control.termination, Termination::Exited { exit_code: 7 });
    assert_eq!(control.stdout, b"hello\n");

    let absent = Worker::new(WorkerConfig {
        program: fx._dir.path().join("no-such-worker"),
        interpreters: vec![],
        confinement: platform(),
        limits: mimlys_exec::sandbox::Limits::default(),
        spec: None,
        host_tools: vec![],
        record: None,
    });
    let out = run(&absent);
    assert_eq!(
        out.termination,
        Termination::HostFault {
            detail: HostFaultDetail::Unavailable
        }
    );
    assert!(
        out.stdout.is_empty() && out.stderr.is_empty(),
        "nothing ran"
    );
    assert_eq!(out.metrics.fuel, None);
}

/// REQ-SEC-003 (an error in confinement means no execution): a worker whose
/// own report of its confinement is short refuses `run_host_tool` when it is
/// asked, whatever the runtime checked: this worker was given no confinement
/// specification, so it reports none of the mandatory clauses, and the tool
/// it is asked to run does nothing. The refusal carries the reason.
#[cfg(unix)]
#[test]
fn req_sec_003_a_worker_with_a_short_report_refuses_run_host_tool() {
    let dir = tempfile::tempdir().unwrap();
    let workspace = dir.path().join("tree");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::create_dir_all(dir.path().join("tmp")).unwrap();
    let declaration = mimlys_core::host_tool::HostToolDeclaration {
        name: "writes".into(),
        command: "/bin/sh".into(),
        args: vec!["-c".into(), "echo ran > ran.txt".into()],
        allowed_args: vec![],
        arg_pattern: None,
        max_args: 0,
        effect: mimlys_core::action::EffectKind::WorkspaceWrite,
        reversible: true,
        timeout_ms: 5_000,
        read_paths: vec![],
        env: Default::default(),
        env_allow: vec![],
        network: false,
    };
    let worker = Worker::new(WorkerConfig {
        program: worker_bin(),
        interpreters: vec![],
        confinement: platform(),
        limits: mimlys_exec::sandbox::Limits::default(),
        spec: None,
        host_tools: vec![declaration],
        record: None,
    });
    let ids = ScriptedIds::default();
    let out = worker.execute(
        Execution {
            task_id: TaskId::new(&ids),
            execution_id: ExecutionId::new(&ids),
            action_id: ActionId::new(&ids),
            tool: "run_host_tool".into(),
            arguments: serde_json::json!({"tool": "writes"}),
            effect: mimlys_core::action::EffectView {
                kind: mimlys_core::action::EffectKind::WorkspaceWrite,
                reversible: true,
                idempotent: false,
            },
            deadline_ms: 5_000,
            workspace_root: workspace.to_string_lossy().into_owned(),
            read_only: false,
            replay: None,
        },
        Cancellation::new(),
    );
    assert_eq!(
        out.termination,
        Termination::HostFault {
            detail: HostFaultDetail::Unavailable
        }
    );
    assert!(!workspace.join("ran.txt").exists(), "the tool did not run");
    let reasons: Vec<String> = out
        .notices
        .iter()
        .filter_map(|n| match n {
            mimlys_core::observation::Notice::HostToolNotStarted { reason } => Some(reason.clone()),
            _ => None,
        })
        .collect();
    assert!(
        reasons.len() == 1 && reasons[0].contains("report of its confinement is short: F:"),
        "{:?}",
        out.notices
    );
    worker.shutdown();
}

/// REQ-CONF-006: where workers are made per task (macOS by default, here by
/// the handle's switch), one worker serves one task: its executions of the same
/// task share it, and the first execution of another task ends it and gets a
/// worker of its own. Read from the worker record's process id: the first
/// worker is gone when the second answers.
#[cfg(target_os = "linux")]
#[test]
fn req_conf_006_a_worker_made_per_task_is_replaced_by_the_next_tasks() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let ids = ScriptedIds::default();
    let mut tasks = Vec::new();
    for _ in 0..2 {
        let task = TaskId::new(&ids);
        let root = data.join("workspaces").join(task.to_string());
        std::fs::create_dir_all(root.join("tree")).unwrap();
        std::fs::create_dir_all(root.join("tmp")).unwrap();
        std::fs::write(
            root.join("tree").join("hello.wasm"),
            wat::parse_str(HELLO).unwrap(),
        )
        .unwrap();
        tasks.push((task, root.join("tree")));
    }
    for sub in ["tmp", "interpreters"] {
        std::fs::create_dir_all(data.join(sub)).unwrap();
    }
    let record = dir.path().join("worker.json");
    let worker = Worker::new(WorkerConfig {
        program: worker_bin(),
        interpreters: vec![],
        confinement: platform(),
        limits: mimlys_exec::sandbox::Limits::default(),
        spec: Some(mimlys::app::confinement_spec(&data, &[], &[], true)),
        host_tools: vec![],
        record: Some(record.clone()),
    });
    worker.bind_to_task(true);
    let run = |task: usize| {
        let (id, tree) = &tasks[task];
        let out = worker.execute(
            Execution {
                task_id: *id,
                execution_id: ExecutionId::new(&ids),
                action_id: ActionId::new(&ids),
                tool: "run_wasm".into(),
                arguments: serde_json::json!({"module": "hello.wasm"}),
                effect: mimlys_core::action::EffectView {
                    kind: mimlys_core::action::EffectKind::WorkspaceWrite,
                    reversible: true,
                    idempotent: false,
                },
                deadline_ms: 20_000,
                workspace_root: tree.to_string_lossy().into_owned(),
                read_only: false,
                replay: None,
            },
            Cancellation::new(),
        );
        assert_eq!(out.termination, Termination::Exited { exit_code: 7 });
        let text = std::fs::read_to_string(&record).expect("the live worker's record");
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        value["pid"].as_u64().unwrap()
    };
    let first = run(0);
    assert_eq!(run(0), first, "the same task keeps its worker");
    let second = run(1);
    assert_ne!(second, first, "another task gets a worker of its own");
    assert!(
        !std::path::Path::new(&format!("/proc/{first}")).exists(),
        "the first task's worker has ended"
    );
    worker.shutdown();
}

/// REQ-ARCH-008 / REQ-TEST-008: the worker's death is `host_fault{worker_died}`,
/// the runtime survives, the next execution restarts the worker; the budget
/// LIM-X3 ends in `host_fault{unavailable}`.
/// REQ-SEC-011 (external design §12, §21), the worker clause: the worker's
/// death is an execution failure, never a runtime failure: the test process,
/// which is the runtime side, survives every death.
/// REQ-SBX-012 (external design §12), fail closed, the budget half: once
/// LIM-X3 is spent a worker cannot be established for the task, and its next
/// execution is `host_fault{unavailable}` (the module is not run elsewhere);
/// another task still gets a worker.
#[test]
fn req_arch_008_worker_death_and_restart_budget() {
    let fx = fx();
    write_module(&fx, "hello.wasm", HELLO);
    let task_id = TaskId::new(&fx.ids);
    let mut deaths = 0;
    for _ in 0..=mimlys_core::limits::LIM_X3_RESTARTS {
        fx.worker.start().unwrap();
        fx.worker.kill_for_test();
        let mut e = execution(
            &fx,
            "run_wasm",
            serde_json::json!({"module": "hello.wasm"}),
            5_000,
        );
        e.task_id = task_id;
        let out = fx.worker.execute(e, Cancellation::new());
        assert_eq!(
            out.termination,
            Termination::HostFault {
                detail: HostFaultDetail::WorkerDied
            }
        );
        deaths += 1;
    }
    assert_eq!(deaths, mimlys_core::limits::LIM_X3_RESTARTS + 1);
    let mut e = execution(
        &fx,
        "run_wasm",
        serde_json::json!({"module": "hello.wasm"}),
        5_000,
    );
    e.task_id = task_id;
    let out = fx.worker.execute(e, Cancellation::new());
    assert_eq!(
        out.termination,
        Termination::HostFault {
            detail: HostFaultDetail::Unavailable
        },
        "beyond LIM-X3 the worker is not restarted for this task"
    );
    // Another task still gets a worker.
    let out = fx.worker.execute(
        execution(
            &fx,
            "run_wasm",
            serde_json::json!({"module": "hello.wasm"}),
            5_000,
        ),
        Cancellation::new(),
    );
    assert_eq!(out.termination, Termination::Exited { exit_code: 7 });
}

/// A worker process spoken to in the protocol's own frames, killed when the
/// test ends however it ends.
struct RawWorker {
    child: std::process::Child,
    stdin: std::process::ChildStdin,
    frames: std::sync::mpsc::Receiver<Result<mimlys_exec::protocol::Response, String>>,
}

impl RawWorker {
    fn start() -> Self {
        let mut cmd = std::process::Command::new(worker_bin());
        cmd.args([
            "--serve",
            "--fuel",
            "10000000000",
            "--memory-bytes",
            "268435456",
            "--output-bytes",
            "1048576",
        ])
        .env_clear()
        .env("MIMLYS_WORKER", "1")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
        if let Some(root) = std::env::var_os("SystemRoot") {
            cmd.env("SystemRoot", root);
        }
        let mut child = cmd.spawn().unwrap();
        let stdin = child.stdin.take().unwrap();
        let mut stdout = child.stdout.take().unwrap();
        let (tx, frames) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            loop {
                let frame =
                    mimlys_exec::protocol::read_frame(&mut stdout).map_err(|e| e.to_string());
                let end = frame.is_err();
                if tx.send(frame).is_err() || end {
                    return;
                }
            }
        });
        Self {
            child,
            stdin,
            frames,
        }
    }

    fn send(&mut self, request: &mimlys_exec::protocol::Request) {
        mimlys_exec::protocol::write_frame(&mut self.stdin, request).unwrap();
    }

    /// The next response; a bound far longer than anything it waits for.
    fn next(&self) -> mimlys_exec::protocol::Response {
        self.frames
            .recv_timeout(std::time::Duration::from_secs(180))
            .expect("the worker answered within the bound")
            .expect("a well-formed frame")
    }
}

impl Drop for RawWorker {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// REQ-SCHED-002 (one execution at a time per task, LIM-X2), at the worker: a
/// second `execute` request that arrives while one is in flight is refused
/// with an error, without running, and the first execution is undisturbed:
/// it ends `cancelled` by its own cancel and its record is the first to
/// arrive after the refusal; the slot is then free, and the next execution
/// runs to its end. The test decides by the order of the worker's replies
/// (the first loops until it is cancelled), never by a time.
#[test]
fn req_sched_002_a_worker_refuses_a_second_execution_while_one_is_in_flight() {
    use mimlys_exec::protocol::{Request, Response};
    let fx = fx();
    write_module(&fx, "loop.wasm", LOOP);
    write_module(&fx, "hello.wasm", HELLO);
    let execute = |module: &str, execution_id: ExecutionId| {
        let e = execution(
            &fx,
            "run_wasm",
            serde_json::json!({"module": module}),
            600_000,
        );
        Request::Execute {
            execution_id,
            action_id: e.action_id,
            tool: e.tool,
            arguments: e.arguments,
            effect: e.effect,
            deadline_ms: e.deadline_ms,
            workspace_root: e.workspace_root,
            read_only: false,
            replay: None,
        }
    };
    let (first, second, third) = (
        ExecutionId::new(&fx.ids),
        ExecutionId::new(&fx.ids),
        ExecutionId::new(&fx.ids),
    );
    let mut worker = RawWorker::start();
    worker.send(&Request::Probe);
    assert!(matches!(worker.next(), Response::ProbeResult { .. }));

    worker.send(&execute("loop.wasm", first));
    worker.send(&execute("hello.wasm", second));
    // The first loops for ten minutes unless cancelled, so the first reply
    // to arrive is the refusal of the second.
    match worker.next() {
        Response::Error { detail } => assert!(
            detail.contains("already in flight"),
            "the refusal says why: {detail}"
        ),
        other => panic!("the second execution was not refused: {other:?}"),
    }
    worker.send(&Request::Cancel {
        execution_id: first,
    });
    match worker.next() {
        Response::ExecutionRecord {
            execution_id,
            termination,
            ..
        } => {
            assert_eq!(execution_id, first, "the record is the first execution's");
            assert_eq!(termination, Termination::Cancelled);
        }
        other => panic!("the first execution's record was expected: {other:?}"),
    }

    // The slot is free again.
    worker.send(&execute("hello.wasm", third));
    match worker.next() {
        Response::ExecutionRecord {
            execution_id,
            termination,
            ..
        } => {
            assert_eq!(execution_id, third);
            assert_eq!(termination, Termination::Exited { exit_code: 7 });
        }
        other => panic!("the third execution should have run: {other:?}"),
    }
    worker.send(&Request::Shutdown);
}
