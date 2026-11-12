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

/// REQ-ARCH-008 / REQ-TEST-008: the worker's death is `host_fault{worker_died}`,
/// the runtime survives, the next execution restarts the worker; the budget
/// LIM-X3 ends in `host_fault{unavailable}`.
/// REQ-SEC-011 (external design §12, §21), the worker clause: the worker's
/// death is an execution failure, never a runtime failure: the test process,
/// which is the runtime side, survives every death.
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
