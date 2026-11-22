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
    fx_with(mimlys_exec::sandbox::Limits::default())
}

/// The runtime side of a worker with these limits: `output_bytes` is LIM-S4 as configured, which
/// both sides derive the frame limit from.
fn fx_with(limits: mimlys_exec::sandbox::Limits) -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let workspace = dir.path().join("tree");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::create_dir_all(dir.path().join("tmp")).unwrap();
    let worker = Worker::new(WorkerConfig {
        program: worker_bin(),
        interpreters: vec![],
        confinement: platform(),
        limits,
        spec: None,
        host_tools: vec![],
        record: None,
    });
    // These tests share one worker between tasks and count its deaths; a worker
    // made per task (the default on macOS) is tested on its own.
    worker.bind_to_task(false);
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
    assert_eq!(facts.engine_version, mimlys_exec::sandbox::ENGINE_VERSION);
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
        allow_paths: false,
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

/// REQ-CONF-009 (handoff 011p, item 1): where each worker's profile names a
/// marker (macOS by default, here by the handle's switch), every worker the
/// runtime makes has a marker of its own, valid, and the record of the live
/// worker holds it: the same task keeps its worker and its marker, another
/// task's worker, a replacement, has another. Two workers never share one.
#[cfg(target_os = "linux")]
#[test]
fn req_conf_009_each_worker_has_a_marker_of_its_own_and_the_record_holds_it() {
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
    worker.make_markers(true);
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
        value["marker"]
            .as_str()
            .expect("the record holds the worker's marker")
            .to_owned()
    };
    let first = run(0);
    assert!(
        mimlys_exec::confine::macos::marker_is_valid(&first),
        "{first}"
    );
    assert_eq!(
        run(0),
        first,
        "the same task keeps its worker and its marker"
    );
    let second = run(1);
    assert!(
        mimlys_exec::confine::macos::marker_is_valid(&second),
        "{second}"
    );
    assert_ne!(second, first, "another worker has a marker of its own");
    worker.shutdown();
    assert!(
        !record.exists(),
        "the record of a worker that ended is removed"
    );
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
    /// The frame limit both sides derive from `--output-bytes`.
    limit: usize,
}

impl RawWorker {
    fn start() -> Self {
        Self::start_with(1_048_576)
    }

    /// A worker whose LIM-S4 is `output_bytes`, as the runtime passes it.
    fn start_with(output_bytes: usize) -> Self {
        let mut cmd = std::process::Command::new(worker_bin());
        cmd.args([
            "--serve",
            "--fuel",
            "10000000000",
            "--memory-bytes",
            "268435456",
            "--output-bytes",
            &output_bytes.to_string(),
        ])
        .env_clear()
        .env("MIMLYS_WORKER", "1")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
        if let Some(root) = std::env::var_os("SystemRoot") {
            cmd.env("SystemRoot", root);
        }
        let limit = mimlys_exec::protocol::frame_limit(output_bytes);
        let mut child = cmd.spawn().unwrap();
        let stdin = child.stdin.take().unwrap();
        let mut stdout = child.stdout.take().unwrap();
        let (tx, frames) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            loop {
                let frame = mimlys_exec::protocol::read_frame(&mut stdout, limit)
                    .map_err(|e| e.to_string());
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
            limit,
        }
    }

    fn send(&mut self, request: &mimlys_exec::protocol::Request) {
        mimlys_exec::protocol::write_frame(&mut self.stdin, request, self.limit).unwrap();
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

/// A module that writes `out_kib` KiB of `byte` (a WAT data escape: `x`, `\ff`) to standard
/// output and `err_kib` to standard error, a KiB at a time: exactly what a bound is tested with.
fn printing(out_kib: u32, err_kib: u32, byte: &str) -> String {
    let chunk = byte.repeat(1024);
    let writes = |fd: u32, count: u32| {
        if count == 0 {
            return String::new();
        }
        format!(
            "(local.set $i (i32.const 0))
             (block $done{fd} (loop $again{fd}
                (br_if $done{fd} (i32.ge_u (local.get $i) (i32.const {count})))
                (drop (call $fd_write (i32.const {fd}) (i32.const 2048) (i32.const 1) (i32.const 3000)))
                (local.set $i (i32.add (local.get $i) (i32.const 1)))
                (br $again{fd})))"
        )
    };
    format!(
        r#"(module
        (import "wasi_snapshot_preview1" "fd_write" (func $fd_write (param i32 i32 i32 i32) (result i32)))
        (memory 1) (export "memory" (memory 0))
        (data (i32.const 0) "{chunk}")
        (func (export "_start") (local $i i32)
            (i32.store (i32.const 2048) (i32.const 0)) (i32.store (i32.const 2052) (i32.const 1024))
            {out} {err}))"#,
        out = writes(1, out_kib),
        err = writes(2, err_kib),
    )
}

fn run_printing(
    fx: &Fx,
    out_kib: u32,
    err_kib: u32,
    byte: &str,
) -> mimlys_core::spi::ExecutionOutcome {
    write_module(fx, "print.wasm", &printing(out_kib, err_kib, byte));
    fx.worker.execute(
        execution(
            fx,
            "run_wasm",
            serde_json::json!({"module": "print.wasm"}),
            120_000,
        ),
        Cancellation::new(),
    )
}

/// REQ-SBX-005, external design §13.1 (handoff 004d-4): through the real worker, a script that
/// prints exactly its bound on both streams, at the default LIM-S4 and at a configured 4 MiB
/// and 8 MiB, is carried whole: the record is not refused, nothing is cut, and no limit is
/// reported. Version 1 of the protocol could not carry more than about 600 KB of text.
#[test]
fn req_sbx_005_a_script_that_prints_exactly_its_bound_is_carried_whole() {
    for bound_kib in [1024u32, 4096, 8192] {
        let fx = fx_with(mimlys_exec::sandbox::Limits {
            output_bytes: bound_kib as usize * 1024,
            ..Default::default()
        });
        let out = run_printing(&fx, bound_kib, bound_kib, "x");
        assert_eq!(
            out.termination,
            Termination::Exited { exit_code: 0 },
            "{bound_kib} KiB"
        );
        assert_eq!(out.stdout.len(), bound_kib as usize * 1024);
        assert_eq!(out.stderr.len(), bound_kib as usize * 1024);
        assert!(out.stdout.iter().chain(&out.stderr).all(|b| *b == b'x'));
        assert!(out.notices.is_empty(), "nothing was cut: {:?}", out.notices);
    }
}

/// The worker itself lives on after a record of its bound: the same process (not a restart)
/// answers the next request, at the default and at a configured 4 MiB.
#[test]
fn req_sbx_005_the_worker_lives_on_after_a_record_of_its_bound() {
    use mimlys_exec::protocol::{Request, Response};
    for bound_kib in [1024u32, 4096] {
        let fx = fx();
        write_module(&fx, "print.wasm", &printing(bound_kib, bound_kib, "x"));
        let e = execution(
            &fx,
            "run_wasm",
            serde_json::json!({"module": "print.wasm"}),
            120_000,
        );
        let mut worker = RawWorker::start_with(bound_kib as usize * 1024);
        worker.send(&Request::Probe);
        assert!(matches!(worker.next(), Response::ProbeResult { .. }));
        let id = e.execution_id;
        worker.send(&Request::Execute {
            execution_id: id,
            action_id: e.action_id,
            tool: e.tool,
            arguments: e.arguments,
            effect: e.effect,
            deadline_ms: e.deadline_ms,
            workspace_root: e.workspace_root,
            read_only: false,
            replay: None,
        });
        match worker.next() {
            Response::ExecutionRecord {
                execution_id,
                termination,
                stdout,
                stderr,
                ..
            } => {
                assert_eq!(execution_id, id);
                assert_eq!(termination, Termination::Exited { exit_code: 0 });
                assert_eq!(stdout.len(), bound_kib as usize * 1024);
                assert_eq!(stderr.len(), bound_kib as usize * 1024);
            }
            other => panic!("{bound_kib} KiB: the record was expected: {other:?}"),
        }
        worker.send(&Request::Probe);
        assert!(
            matches!(worker.next(), Response::ProbeResult { .. }),
            "{bound_kib} KiB: the same worker answers again"
        );
        assert!(
            worker.child.try_wait().unwrap().is_none(),
            "{bound_kib} KiB: the worker did not exit"
        );
    }
}

/// Bytes that are no text travel as they are: the frame carries bytes, not characters.
#[test]
fn req_sbx_005_bytes_that_are_no_text_are_carried_exactly() {
    let fx = fx();
    let out = run_printing(&fx, 1024, 0, "\\ff");
    assert_eq!(out.termination, Termination::Exited { exit_code: 0 });
    assert_eq!(out.stdout, vec![0xffu8; 1024 * 1024]);
}

/// External design §13.1 (handoff 004d-4): a worker refuses a request of another protocol
/// version by name and ends (status 2), before it takes any work; it does not answer in a
/// version the runtime cannot read.
#[test]
fn ed_13_1_a_worker_refuses_a_request_of_another_protocol_version() {
    use std::io::Write as _;
    let mut cmd = std::process::Command::new(worker_bin());
    cmd.args(["--serve"])
        .env_clear()
        .env("MIMLYS_WORKER", "1")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    if let Some(root) = std::env::var_os("SystemRoot") {
        cmd.env("SystemRoot", root);
    }
    let mut child = cmd.spawn().unwrap();
    let body = br#"{"version":1,"request":"probe"}"#;
    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(&(body.len() as u32).to_be_bytes()).unwrap();
    stdin.write_all(body).unwrap();
    stdin.flush().unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(2), "{:?}", out.status);
    assert!(out.stdout.is_empty(), "it answers nothing");
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(
        said.contains("protocol version 1") && said.contains("same build"),
        "{said}"
    );
}

/// The one executable script of the stand-in workers, written once for the whole binary.
///
/// A test that wrote an executable script of its own, and ran it a moment later, while another
/// test of the binary forked, met `ETXTBSY` ("Text file busy"): the forked child holds the write
/// handle of the file until it execs, and no program can be executed while one is open for
/// writing. It failed `ed_13_1_a_frame_the_runtime_refuses_ends_as_worker_died` once on
/// `ubuntu-24.04-arm` (the spawn's failure reached the test only as `host_fault{unavailable}`).
/// `xcrun`'s tests of the same crate avoid it the same way: the executable is written once and
/// never again, and what differs between tests is **data** the runner reads: the file
/// `<its own path>.body`, run by `sh`. A file that is only read is never held open for writing.
///
/// The one remaining window is the write of the runner itself, which a child forked by another
/// test during it could still hold: so the runner is executed once here, until the system lets it
/// be, before any test uses it.
#[cfg(unix)]
fn shared_runner() -> &'static (tempfile::TempDir, PathBuf) {
    use std::os::unix::fs::PermissionsExt as _;
    static RUNNER: std::sync::OnceLock<(tempfile::TempDir, PathBuf)> = std::sync::OnceLock::new();
    RUNNER.get_or_init(|| {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("runner");
        std::fs::write(&path, "#!/bin/sh\nexec sh \"$0.body\" \"$@\"\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        // Its body is not there, so it ends at once: only whether it could be executed matters.
        let mut busy = 0;
        loop {
            match std::process::Command::new(&path)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
            {
                Ok(_) => break,
                // ETXTBSY: a child forked during the write still holds it, until it execs.
                Err(e) if e.raw_os_error() == Some(26) && busy < 1000 => {
                    busy += 1;
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                Err(e) => panic!("the stand-in workers' runner cannot be executed: {e}"),
            }
        }
        (dir, path)
    })
}

/// A stand-in for a worker, on Unix: it answers the probe (in `version`) and then does what
/// `after` says. The runtime starts it as it starts the worker, with the same arguments, which it
/// ignores. It is a link to [`shared_runner`] with its body beside it, as data; nothing executable
/// is written here.
#[cfg(unix)]
fn fake_worker(fx: &Fx, version: u32, after: &str) -> Arc<Worker> {
    let body = format!(
        r#"{{"version":{version},"response":"probe_result","clauses":{{}},"engine_version":"fake","interpreters":{{}},"interpreter_errors":{{}}}}"#
    );
    let len = (body.len() as u32).to_be_bytes();
    let script = format!(
        "printf '\\{:03o}\\{:03o}\\{:03o}\\{:03o}'\nprintf '%s' '{body}'\n{after}\n",
        len[0], len[1], len[2], len[3]
    );
    let path = fx._dir.path().join(format!("fake-worker-{version}"));
    std::os::unix::fs::symlink(&shared_runner().1, &path).unwrap();
    std::fs::write(path.with_extension("body"), script).unwrap();
    let worker = Worker::new(WorkerConfig {
        program: path,
        interpreters: vec![],
        confinement: platform(),
        limits: mimlys_exec::sandbox::Limits::default(),
        spec: None,
        host_tools: vec![],
        record: None,
    });
    worker.bind_to_task(false);
    worker
}

/// External design §13.1 (handoff 004d-4): a frame the runtime refuses (here one that declares
/// more than the limit in force) ends the execution as `host_fault{worker_died}`, as any broken
/// frame does; the runtime does not wait for the deadline and does not hang. The test decides
/// by the outcome: a wait to the deadline would be `deadline`, not `worker_died`.
#[cfg(unix)]
#[test]
fn ed_13_1_a_frame_the_runtime_refuses_ends_as_worker_died() {
    let fx = fx();
    // After the probe: a frame that declares 0x7fffffff bytes, then nothing, and no exit.
    let worker = fake_worker(&fx, 2, "printf '\\177\\377\\377\\377'\nexec sleep 600");
    // Started first, so that a worker that cannot be started says why (the execution below would
    // say only `host_fault{unavailable}`, the spawn's reason being a log line).
    worker
        .start()
        .unwrap_or_else(|reason| panic!("the stand-in worker did not start: {reason}"));
    let out = worker.execute(
        execution(
            &fx,
            "run_wasm",
            serde_json::json!({"module": "hello.wasm"}),
            600_000,
        ),
        Cancellation::new(),
    );
    assert_eq!(
        out.termination,
        Termination::HostFault {
            detail: HostFaultDetail::WorkerDied
        }
    );
    worker.shutdown();
}

/// External design §13.1 (handoff 004d-4): a worker of another protocol version is refused at
/// the handshake, by name, before any work is sent to it: the start fails and says both versions
/// and that the runtime and the worker must be of the same build.
#[cfg(unix)]
#[test]
fn ed_13_1_a_worker_of_another_protocol_version_is_refused_at_the_handshake() {
    let fx = fx();
    let worker = fake_worker(&fx, 1, "exec sleep 600");
    let error = worker
        .start()
        .expect_err("a worker of version 1 is refused");
    assert!(
        error.contains("protocol version 1") && error.contains("same build"),
        "{error}"
    );
    assert!(
        error.contains(&mimlys_exec::protocol::PROTOCOL_VERSION.to_string()),
        "{error}"
    );
    worker.shutdown();
}

/// REQ-SBX-005 (handoff 004d-4): through the real worker, the notice that says a capture was cut
/// is on the stream that was cut, with the bytes kept, and only there: a stream of exactly its
/// bound is whole even when the other one overflows and ends the run (it was reported cut when
/// the notice followed the run's `output_limit`, not the stream).
#[test]
fn req_sbx_005_only_the_stream_that_overflowed_is_reported_cut() {
    use mimlys_core::observation::{Notice, Stream};
    let fx = fx();
    let bound = 1024u32;
    let out = run_printing(&fx, bound, bound + 1, "x");
    assert_eq!(out.termination, Termination::OutputLimit);
    assert_eq!(
        out.stdout.len(),
        bound as usize * 1024,
        "whole, at its bound"
    );
    assert_eq!(out.stderr.len(), bound as usize * 1024, "cut at its bound");
    assert_eq!(
        out.notices,
        vec![Notice::OutputTruncated {
            stream: Stream::Stderr,
            total_bytes: bound as u64 * 1024,
            captured_bytes: Some(bound as u64 * 1024),
            copy_cut: false,
        }]
    );
}
