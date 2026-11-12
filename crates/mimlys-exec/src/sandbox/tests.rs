//! The security suite of REQ-TEST-008, with hand-written modules: each guard
//! alone, output limits, filesystem denial, clocks, malformed modules,
//! replay determinism.

use super::*;
use mimlys_core::action::{HostFaultDetail, Termination};
use mimlys_core::limits::LIM_S2_DEADLINE_MS;

fn sandbox() -> Sandbox {
    Sandbox::new().unwrap()
}

struct Dirs {
    _root: tempfile::TempDir,
    workspace: std::path::PathBuf,
    tmp: std::path::PathBuf,
}

fn dirs() -> Dirs {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("tree");
    let tmp = root.path().join("tmp");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::create_dir_all(&tmp).unwrap();
    std::fs::write(workspace.join("hello.txt"), "hi").unwrap();
    Dirs {
        _root: root,
        workspace,
        tmp,
    }
}

fn request(d: &Dirs, limits: Limits) -> Request {
    Request {
        args: vec!["prog".into()],
        stdin: Vec::new(),
        workspace: d.workspace.clone(),
        read_only: false,
        tmp: d.tmp.clone(),
        replay: None,
        limits,
    }
}

fn run(sb: &Sandbox, wat: &str, req: &Request) -> Outcome {
    let module = sb.compile(wat.as_bytes()).unwrap();
    let interrupt = sb.interrupt(req.limits.deadline_ms);
    sb.run(&module, req, &interrupt)
}

const LOOP: &str =
    r#"(module (memory 1) (export "memory" (memory 0)) (func (export "_start") (loop br 0)))"#;

/// REQ-SBX-002 / REQ-SBX-003 / INV-04 (external design §21.2): an infinite
/// loop ends `fuel_exhausted` under a small fuel budget and `deadline` under
/// a short deadline; never conflated. INV-04's fuel and deadline clauses: a
/// sandbox execution that would never end is ended by each limit alone.
#[test]
fn req_sbx_002_fuel_and_deadline_alone() {
    let sb = sandbox();
    let d = dirs();
    let fuel = run(
        &sb,
        LOOP,
        &request(
            &d,
            Limits {
                fuel: 1_000_000,
                deadline_ms: LIM_S2_DEADLINE_MS as u64,
                ..Limits::default()
            },
        ),
    );
    assert_eq!(fuel.termination, Termination::FuelExhausted);
    assert!(fuel.metrics.fuel.unwrap() >= 1_000_000);
    let deadline = run(
        &sb,
        LOOP,
        &request(
            &d,
            Limits {
                fuel: u64::MAX / 2,
                deadline_ms: 100,
                ..Limits::default()
            },
        ),
    );
    assert_eq!(deadline.termination, Termination::Deadline);
    // The loop never ends by itself (the fuel is out of reach): a deadline that
    // did not fire would hang this test, and the termination above says that it
    // fired. Not before the 100 ms it was given; and not later than 30 s, a
    // margin far above the deadline that no stall of a slow machine reaches.
    assert!(
        deadline.metrics.wall_ms >= 90 && deadline.metrics.wall_ms < 30_000,
        "{}",
        deadline.metrics.wall_ms
    );
}

/// REQ-SBX-002 / INV-04 (external design §21.2): growing memory past the
/// ceiling ends `memory_limit`, and the recorded peak stays within it (the
/// memory clause of INV-04).
#[test]
fn req_sbx_002_memory_alone() {
    let sb = sandbox();
    let d = dirs();
    let bomb = r#"(module (memory 1) (export "memory" (memory 0))
        (func (export "_start") (loop (if (i32.eq (memory.grow (i32.const 16)) (i32.const -1)) (then unreachable)) br 0)))"#;
    let out = run(
        &sb,
        bomb,
        &request(
            &d,
            Limits {
                memory_bytes: 8 * 1024 * 1024,
                ..Limits::default()
            },
        ),
    );
    assert_eq!(out.termination, Termination::MemoryLimit);
    assert!(out.metrics.memory_peak_bytes.unwrap() <= 8 * 1024 * 1024);
}

/// A module that counts to `n`, grows its memory from one page to four,
/// writes `hello` (five bytes) to standard output and `hel` (three) to
/// standard error, and exits normally.
fn measured(n: u32) -> String {
    format!(
        r#"(module (import "wasi_snapshot_preview1" "fd_write" (func $fd_write (param i32 i32 i32 i32) (result i32)))
        (memory 1) (export "memory" (memory 0))
        (data (i32.const 0) "hello")
        (func (export "_start") (local $i i32)
            (loop $count
                (local.set $i (i32.add (local.get $i) (i32.const 1)))
                (br_if $count (i32.lt_u (local.get $i) (i32.const {n}))))
            (drop (memory.grow (i32.const 3)))
            (i32.store (i32.const 100) (i32.const 0))
            (i32.store (i32.const 104) (i32.const 5))
            (drop (call $fd_write (i32.const 1) (i32.const 100) (i32.const 1) (i32.const 120)))
            (i32.store (i32.const 104) (i32.const 3))
            (drop (call $fd_write (i32.const 2) (i32.const 100) (i32.const 1) (i32.const 120)))))"#
    )
}

/// REQ-SBX-011 (external design §7.4, §12): every execution records the fuel
/// it consumed, its peak memory, its wall time and the bytes of output it
/// produced. A module that counts to 1 000 and one that counts to 100 000
/// consume different fuel (the larger count, more); the peak is the four
/// pages the module grew to; the output bytes are the five and the three
/// the module wrote; a run ended by its deadline records the wall time it
/// had (at least the deadline), not zero.
#[test]
fn req_sbx_011_resource_accounting_per_execution() {
    let sb = sandbox();
    let d = dirs();
    let small = run(&sb, &measured(1_000), &request(&d, Limits::default()));
    let large = run(&sb, &measured(100_000), &request(&d, Limits::default()));
    for out in [&small, &large] {
        assert_eq!(out.termination, Termination::Exited { exit_code: 0 });
        assert_eq!(out.stdout, b"hello");
        assert_eq!(out.stderr, b"hel");
        assert_eq!(out.metrics.stdout_bytes, 5);
        assert_eq!(out.metrics.stderr_bytes, 3);
        assert!(
            out.metrics.memory_peak_bytes.unwrap() >= 4 * 65_536,
            "the module grew to four pages: {:?}",
            out.metrics.memory_peak_bytes
        );
    }
    let (few, many) = (small.metrics.fuel.unwrap(), large.metrics.fuel.unwrap());
    assert!(few >= 1_000, "at least one unit per iteration: {few}");
    assert!(many >= 100_000, "at least one unit per iteration: {many}");
    assert!(many > few, "more work, more fuel: {few} against {many}");
    let ended = run(
        &sb,
        LOOP,
        &request(
            &d,
            Limits {
                fuel: 1_000_000_000_000,
                deadline_ms: 100,
                ..Limits::default()
            },
        ),
    );
    assert_eq!(ended.termination, Termination::Deadline);
    assert!(ended.metrics.wall_ms >= 90, "{}", ended.metrics.wall_ms);
}

/// REQ-SBX-012 (external design §12, §7.4), fail closed: when the sandbox's
/// filesystem boundary cannot be established (the workspace to preopen, or
/// the temporary directory, does not exist), the execution ends
/// `host_fault{unavailable}`, the module does not run (it writes `hello` to
/// standard output first thing, and nothing is captured), and nothing is
/// measured. The same module runs when both directories exist.
#[test]
fn req_sbx_012_an_unestablished_boundary_fails_closed() {
    let sb = sandbox();
    let d = dirs();
    let module = measured(10);
    let ran = run(&sb, &module, &request(&d, Limits::default()));
    assert_eq!(ran.termination, Termination::Exited { exit_code: 0 });
    assert_eq!(ran.stdout, b"hello", "the control: the module writes");

    let mut no_workspace = request(&d, Limits::default());
    no_workspace.workspace = d.workspace.join("absent");
    let mut no_tmp = request(&d, Limits::default());
    no_tmp.tmp = d.tmp.join("absent");
    for (what, req) in [("workspace", no_workspace), ("tmp", no_tmp)] {
        let out = run(&sb, &module, &req);
        assert_eq!(
            out.termination,
            Termination::HostFault {
                detail: HostFaultDetail::Unavailable
            },
            "{what}"
        );
        assert!(
            out.stdout.is_empty() && out.stderr.is_empty(),
            "{what}: nothing ran"
        );
        assert_eq!(out.metrics.fuel, None, "{what}: no fuel was spent");
    }
}

/// REQ-SBX-003: a normal exit carries its code; a trap is named; `unreachable` is `trapped`.
#[test]
fn req_sbx_003_exit_and_trap() {
    let sb = sandbox();
    let d = dirs();
    let exit = r#"(module (import "wasi_snapshot_preview1" "proc_exit" (func $exit (param i32)))
        (memory 1) (export "memory" (memory 0)) (func (export "_start") (call $exit (i32.const 3))))"#;
    let out = run(&sb, exit, &request(&d, Limits::default()));
    assert_eq!(out.termination, Termination::Exited { exit_code: 3 });
    let trap =
        r#"(module (memory 1) (export "memory" (memory 0)) (func (export "_start") unreachable))"#;
    let out = run(&sb, trap, &request(&d, Limits::default()));
    assert_eq!(
        out.termination,
        Termination::Trapped {
            trap: "unreachable_code_reached".into()
        }
    );
    let clean = r#"(module (memory 1) (export "memory" (memory 0)) (func (export "_start")))"#;
    let out = run(&sb, clean, &request(&d, Limits::default()));
    assert_eq!(out.termination, Termination::Exited { exit_code: 0 });
    assert!(out.metrics.setup_ms.is_some());
}

/// A module that writes `n` bytes to fd 1 in a loop.
fn flood(bytes_per_write: usize) -> String {
    format!(
        r#"(module (import "wasi_snapshot_preview1" "fd_write" (func $fd_write (param i32 i32 i32 i32) (result i32)))
        (memory 2) (export "memory" (memory 0))
        (data (i32.const 0) "{}")
        (func (export "_start")
            (i32.store (i32.const 65000) (i32.const 0))
            (i32.store (i32.const 65004) (i32.const {n}))
            (loop (drop (call $fd_write (i32.const 1) (i32.const 65000) (i32.const 1) (i32.const 65008))) br 0)))"#,
        "x".repeat(bytes_per_write),
        n = bytes_per_write
    )
}

/// REQ-SBX-005 / REQ-TOOL-008 / INV-04 (external design §21.2): output past
/// LIM-S4 ends `output_limit`; the capture holds the bound (the output-limit
/// clause of INV-04).
#[test]
fn req_sbx_005_output_limit() {
    let sb = sandbox();
    let d = dirs();
    let out = run(
        &sb,
        &flood(1000),
        &request(
            &d,
            Limits {
                output_bytes: 10_000,
                ..Limits::default()
            },
        ),
    );
    assert_eq!(out.termination, Termination::OutputLimit);
    assert_eq!(out.stdout.len(), 10_000);
    assert_eq!(out.metrics.stdout_bytes, 10_000);
}

/// REQ-SBX-004 / REQ-SEC-006: a read-only workspace refuses writes; the module sees nothing else.
#[test]
fn req_sbx_004_filesystem_boundary() {
    let sb = sandbox();
    let d = dirs();
    // path_open(fd=3 the first preopen, dirflags, path, len, oflags=CREAT, rights..., fdflags, result) → exits with the errno.
    let create = r#"(module
        (import "wasi_snapshot_preview1" "path_open" (func $open (param i32 i32 i32 i32 i32 i64 i64 i32 i32) (result i32)))
        (import "wasi_snapshot_preview1" "proc_exit" (func $exit (param i32)))
        (memory 1) (export "memory" (memory 0))
        (data (i32.const 0) "new.txt")
        (func (export "_start")
            (call $exit (call $open (i32.const 3) (i32.const 0) (i32.const 0) (i32.const 7) (i32.const 1) (i64.const 0x40) (i64.const 0) (i32.const 0) (i32.const 100)))))"#;
    let mut req = request(&d, Limits::default());
    req.read_only = true;
    let out = run(&sb, create, &req);
    assert!(
        matches!(out.termination, Termination::Exited { exit_code } if exit_code != 0),
        "{:?}",
        out.termination
    );
    assert!(!d.workspace.join("new.txt").exists());
    req.read_only = false;
    let out = run(&sb, create, &req);
    assert_eq!(out.termination, Termination::Exited { exit_code: 0 });
    assert!(d.workspace.join("new.txt").exists());
}

/// REQ-SBX-004 / REQ-SBX-008: the clock has LIM-S6 granularity and is fixed in replay; random is replayable.
#[test]
fn req_sbx_008_clocks_and_replay() {
    let sb = sandbox();
    let d = dirs();
    // Writes the wall clock (ms) and 4 random bytes to stdout as raw bytes.
    let probe = r#"(module
        (import "wasi_snapshot_preview1" "clock_time_get" (func $clock (param i32 i64 i32) (result i32)))
        (import "wasi_snapshot_preview1" "random_get" (func $random (param i32 i32) (result i32)))
        (import "wasi_snapshot_preview1" "fd_write" (func $fd_write (param i32 i32 i32 i32) (result i32)))
        (memory 1) (export "memory" (memory 0))
        (func (export "_start")
            (drop (call $clock (i32.const 0) (i64.const 1) (i32.const 0)))
            (drop (call $random (i32.const 8) (i32.const 4)))
            (i32.store (i32.const 100) (i32.const 0))
            (i32.store (i32.const 104) (i32.const 12))
            (drop (call $fd_write (i32.const 1) (i32.const 100) (i32.const 1) (i32.const 200)))))"#;
    let mut req = request(&d, Limits::default());
    req.replay = Some(crate::protocol::Replay {
        seed: vec![7, 8, 9],
        clock_ms: 1_700_000_000_123,
    });
    let a = run(&sb, probe, &req);
    let b = run(&sb, probe, &req);
    assert_eq!(a.termination, Termination::Exited { exit_code: 0 });
    assert_eq!(a.stdout, b.stdout, "replay is byte-identical");
    let ns = u64::from_le_bytes(a.stdout[..8].try_into().unwrap());
    assert_eq!(
        ns,
        1_700_000_000_123 * 1_000_000,
        "the fixed wall clock in nanoseconds"
    );
    assert_eq!(&a.stdout[8..12], &[7, 8, 9, 7]);
    req.replay = None;
    let live = run(&sb, probe, &req);
    let ns = u64::from_le_bytes(live.stdout[..8].try_into().unwrap());
    assert_eq!(
        ns % 1_000_000,
        0,
        "LIM-S6: the live clock is a whole number of milliseconds"
    );
}

/// REQ-TOOL-007: a cancel interrupts a running loop within LIM-O2 and ends `cancelled`.
#[test]
fn req_tool_007_cancel() {
    let sb = sandbox();
    let d = dirs();
    let module = sb.compile(LOOP.as_bytes()).unwrap();
    let req = request(
        &d,
        Limits {
            fuel: u64::MAX / 2,
            ..Limits::default()
        },
    );
    let interrupt = sb.interrupt(req.limits.deadline_ms);
    let canceller = interrupt.clone();
    // The instant of the cancel, so that the time to end is measured from the
    // state change and not from the start of the run (instantiation included).
    let fired_at = std::sync::Arc::new(std::sync::Mutex::new(None));
    let fired = std::sync::Arc::clone(&fired_at);
    let t = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(100));
        *fired.lock().unwrap() = Some(std::time::Instant::now());
        canceller.fire(InterruptReason::Cancelled);
    });
    let out = sb.run(&module, &req, &interrupt);
    t.join().unwrap();
    assert_eq!(out.termination, Termination::Cancelled);
    // LIM-O2 is the contract; the loop only ends by the cancel (its deadline is
    // LIM-S2's), so the outcome is what proves that it was seen. The time allowed
    // is five times the limit: a cancel that needs a multiple of LIM-O2 is a
    // defect, a stall of a slow machine shorter than that is not.
    let took = fired_at
        .lock()
        .unwrap()
        .expect("the cancel was fired")
        .elapsed();
    assert!(
        took < std::time::Duration::from_millis(5 * mimlys_core::limits::LIM_O2_CANCEL_MS),
        "the cancel took {took:?} to end the run"
    );
}
