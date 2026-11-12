//! The M2 demonstration (external design §23; RFC 005): JavaScript through
//! the worker's sandbox; an infinite loop, an allocation bomb and a print
//! flood each end with the right termination; `run_wasm` from the
//! workspace; a read-write script writes only inside the workspace; the
//! status shows the tier and the clauses; the overhead report (REQ-SBX-009).
//!
//! The QuickJS build comes from `MIMLYS_QJS_WASM` when set, else from the
//! pinned artifact through `mimlys interpreters install` (network).

mod common;

use common::run_bounded;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

fn script(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/m2")
        .join(name)
}

/// The QuickJS module, fetched once per test run.
fn qjs() -> PathBuf {
    static MODULE: OnceLock<PathBuf> = OnceLock::new();
    MODULE
        .get_or_init(|| {
            if let Some(p) = std::env::var_os("MIMLYS_QJS_WASM") {
                return PathBuf::from(p);
            }
            let cache = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/interpreters");
            let data = cache.join("data");
            let pinned = mimlys_exec::interpreters::pinned("javascript").unwrap();
            let path = mimlys_exec::interpreters::default_module_path(&data, pinned.sha256);
            if !path.is_file() {
                std::fs::create_dir_all(&data).unwrap();
                let config = cache.join("config.toml");
                std::fs::write(
                    &config,
                    format!(
                        "config_version = 1\n[runtime]\ndata_dir = {:?}\n",
                        data.to_string_lossy()
                    ),
                )
                .unwrap();
                let out = Command::new(env!("CARGO_BIN_EXE_mimlys"))
                    .args([
                        "--config",
                        config.to_str().unwrap(),
                        "interpreters",
                        "install",
                        "javascript",
                    ])
                    .env("MIMLYS_CONFIG", "")
                    .output()
                    .unwrap();
                assert!(
                    out.status.success(),
                    "{}",
                    String::from_utf8_lossy(&out.stderr)
                );
            }
            path
        })
        .clone()
}

fn scope() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("tools")).unwrap();
    let hello = r#"(module (import "wasi_snapshot_preview1" "fd_write" (func $fd_write (param i32 i32 i32 i32) (result i32)))
        (import "wasi_snapshot_preview1" "proc_exit" (func $exit (param i32)))
        (memory 1) (export "memory" (memory 0)) (data (i32.const 0) "wasm\n")
        (func (export "_start")
            (i32.store (i32.const 100) (i32.const 0)) (i32.store (i32.const 104) (i32.const 5))
            (drop (call $fd_write (i32.const 1) (i32.const 100) (i32.const 1) (i32.const 200)))
            (call $exit (i32.const 7))))"#;
    std::fs::write(
        dir.path().join("tools/hello.wasm"),
        wat::parse_str(hello).unwrap(),
    )
    .unwrap();
    // Creates `/workspace/out.txt` (fd 3 is the first preopen) and writes into it; exits with the errno of path_open.
    let write = r#"(module
        (import "wasi_snapshot_preview1" "path_open" (func $open (param i32 i32 i32 i32 i32 i64 i64 i32 i32) (result i32)))
        (import "wasi_snapshot_preview1" "fd_write" (func $fd_write (param i32 i32 i32 i32) (result i32)))
        (import "wasi_snapshot_preview1" "proc_exit" (func $exit (param i32)))
        (memory 1) (export "memory" (memory 0))
        (data (i32.const 0) "out.txt") (data (i32.const 16) "written by the sandbox")
        (func (export "_start")
            (local $err i32)
            (local.set $err (call $open (i32.const 3) (i32.const 0) (i32.const 0) (i32.const 7) (i32.const 1) (i64.const 0x40) (i64.const 0) (i32.const 0) (i32.const 100)))
            (if (i32.ne (local.get $err) (i32.const 0)) (then (call $exit (local.get $err))))
            (i32.store (i32.const 200) (i32.const 16)) (i32.store (i32.const 204) (i32.const 22))
            (drop (call $fd_write (i32.load (i32.const 100)) (i32.const 200) (i32.const 1) (i32.const 300)))))"#;
    std::fs::write(
        dir.path().join("tools/write.wasm"),
        wat::parse_str(write).unwrap(),
    )
    .unwrap();
    dir
}

struct Run {
    report: Value,
    stderr: String,
}

fn testloop(script_name: &str, scope: &Path, data: &Path, extra: &[&str]) -> Run {
    let qjs = qjs();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_mimlys-testloop"));
    cmd.args([
        "--script",
        script(script_name).to_str().unwrap(),
        "--data-dir",
        data.to_str().unwrap(),
        "--scope",
        scope.to_str().unwrap(),
        "--interpreter-module",
        qjs.to_str().unwrap(),
    ]);
    if !extra.contains(&"--sandbox-deadline-ms") {
        cmd.args(["--sandbox-deadline-ms", "3000"]);
    }
    // The runtime log (the worker's standard error included) is in `Run.stderr` for the messages of failing assertions.
    cmd.args(["--log", "info,mimlys_worker=debug"]);
    cmd.args(extra);
    cmd.env_clear();
    for key in ["PATH", "SYSTEMROOT", "TEMP", "TMP", "HOME", "USERPROFILE"] {
        if let Ok(v) = std::env::var(key) {
            cmd.env(key, v);
        }
    }
    cmd.env("MIMLYS_WORKER_BIN", env!("CARGO_BIN_EXE_mimlys-worker"));
    let out = run_bounded(&mut cmd, data);
    assert!(out.status.success(), "{}", out.stderr);
    Run {
        report: serde_json::from_str(&out.stdout)
            .unwrap_or_else(|e| panic!("{e}: {}\n{}", out.stdout, out.stderr)),
        stderr: out.stderr,
    }
}

fn observations(report: &Value) -> Vec<Value> {
    let obs = report["observations"].as_array().unwrap().clone();
    for o in &obs {
        if o["status"] != "succeeded" {
            eprintln!(
                "observation: status={} cause={} payload={}",
                o["status"], o["cause"], o["payload"]
            );
        }
    }
    obs
}

/// RFC 005 §5: `run_script` and `run_wasm` run in the worker; stdout and the
/// exit code come back in the observation; the task completes.
#[test]
fn m2_scripts_run_in_the_worker() {
    let scope = scope();
    let data = tempfile::tempdir().unwrap();
    let run = testloop("hello.json", scope.path(), data.path(), &[]);
    assert_eq!(run.report["state"]["state"], "completed", "{}", run.stderr);
    let obs = observations(&run.report);
    assert_eq!(obs.len(), 2);
    assert_eq!(obs[0]["status"], "succeeded");
    assert_eq!(obs[0]["payload"]["termination"], "exited");
    assert_eq!(obs[0]["payload"]["exit_code"], 0);
    assert!(
        obs[0]["stdout"].as_str().unwrap().contains("sum 2"),
        "{}",
        obs[0]
    );
    assert!(obs[0]["metrics"]["fuel"].as_u64().unwrap() > 0);
    assert!(obs[0]["metrics"]["setup_ms"].is_number());
    assert_eq!(obs[1]["status"], "failed", "exit code 7 is a tool error");
    assert_eq!(obs[1]["payload"]["exit_code"], 7);
    assert!(obs[1]["stdout"].as_str().unwrap().contains("wasm"));
    // RFC 006 §9 ended the M2 stopgap of RFC 005 §11: with rollback in place the shipped
    // profiles no longer ask before `run_script`; nothing was approved.
    assert_eq!(run.report["approvals"].as_array().unwrap().len(), 0);
}

/// External design §23, M2: an infinite loop ends `fuel_exhausted` or `deadline`.
#[test]
fn m2_infinite_loop() {
    let scope = scope();
    let data = tempfile::tempdir().unwrap();
    let run = testloop("loop.json", scope.path(), data.path(), &[]);
    let obs = observations(&run.report);
    let termination = obs[0]["payload"]["termination"]
        .as_str()
        .unwrap_or_else(|| panic!("{}\n{}", obs[0], run.stderr))
        .to_owned();
    assert!(
        termination == "fuel_exhausted" || termination == "deadline",
        "{termination}"
    );
    assert_eq!(obs[0]["status"], "failed");
    let cause = obs[0]["cause"]["cause"].as_str().unwrap();
    assert_eq!(
        cause,
        if termination == "deadline" {
            "timeout"
        } else {
            "resource_exhausted"
        }
    );
    // With a tiny fuel budget it is fuel; with a large one and a short deadline it is the deadline.
    let data2 = tempfile::tempdir().unwrap();
    let run = testloop(
        "loop.json",
        scope.path(),
        data2.path(),
        &["--sandbox-fuel", "1000000"],
    );
    let obs = observations(&run.report);
    assert_eq!(
        obs[0]["payload"]["termination"], "fuel_exhausted",
        "{}",
        obs[0]
    );
    let data3 = tempfile::tempdir().unwrap();
    let run = testloop(
        "loop.json",
        scope.path(),
        data3.path(),
        &[
            "--sandbox-fuel",
            "1000000000000",
            "--sandbox-deadline-ms",
            "1000",
        ],
    );
    let obs = observations(&run.report);
    assert_eq!(
        obs[0]["payload"]["termination"], "deadline",
        "{}\n{}",
        obs[0], run.stderr
    );
    assert!(obs[0]["metrics"]["wall_ms"].as_i64().unwrap() < 5_000);
}

/// REQ-SBX-002 in process, at two code-generation levels: the deadline
/// interrupts the interpreter's `while (true) {}` within a bounded time.
/// Diagnoses the macOS runner, where the worker's guest ran past its
/// deadline while a plain wasm loop was interrupted. Each variant runs on
/// its own thread with a bound, so a guest that never returns fails the
/// test instead of hanging it.
#[test]
fn m2_deadline_interrupts_the_interpreter_in_process() {
    use mimlys_core::action::Termination;
    use mimlys_exec::sandbox::{Limits, OptLevel, Request, Sandbox};
    let bytes = std::fs::read(qjs()).unwrap();
    let pinned = mimlys_exec::interpreters::pinned("javascript").unwrap();
    let mut failures = Vec::new();
    for (name, level) in [("speed", OptLevel::Speed), ("none", OptLevel::None)] {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("ws");
        let tmp = dir.path().join("tmp");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(tmp.join(pinned.script_name), "while (true) {}").unwrap();
        let mut args = vec!["javascript".to_owned()];
        args.extend(pinned.args.iter().map(|a| (*a).to_owned()));
        args.push(format!("/tmp/{}", pinned.script_name));
        let request = Request {
            args,
            stdin: Vec::new(),
            workspace,
            read_only: true,
            tmp,
            replay: None,
            limits: Limits {
                fuel: 1_000_000_000_000,
                deadline_ms: 500,
                ..Limits::default()
            },
        };
        let bytes = bytes.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let sb = Sandbox::with_opt_level(level).unwrap();
            let module = sb.compile(&bytes).unwrap();
            let interrupt = sb.interrupt(500);
            let started = std::time::Instant::now();
            let out = sb.run(&module, &request, &interrupt);
            let _ = tx.send((out.termination, started.elapsed().as_millis()));
        });
        match rx.recv_timeout(std::time::Duration::from_secs(20)) {
            Ok((Termination::Deadline, ms)) => println!("opt level {name}: deadline after {ms} ms"),
            Ok((other, ms)) => failures.push(format!("opt level {name}: {other:?} after {ms} ms")),
            Err(_) => failures.push(format!(
                "opt level {name}: the guest did not return within 20 s (deadline 500 ms)"
            )),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// External design §23, M2: an allocation bomb ends `memory_limit`.
#[test]
fn m2_allocation_bomb() {
    let scope = scope();
    let data = tempfile::tempdir().unwrap();
    let run = testloop(
        "bomb.json",
        scope.path(),
        data.path(),
        &["--sandbox-memory-mib", "32"],
    );
    let obs = observations(&run.report);
    assert_eq!(
        obs[0]["payload"]["termination"], "memory_limit",
        "{}\n{}",
        obs[0], run.stderr
    );
    assert!(obs[0]["metrics"]["memory_peak_bytes"].as_u64().unwrap() <= 32 * 1024 * 1024);
}

/// External design §23, M2: a print flood ends `output_limit` with the bounded capture.
#[test]
fn m2_print_flood() {
    let scope = scope();
    let data = tempfile::tempdir().unwrap();
    let run = testloop(
        "flood.json",
        scope.path(),
        data.path(),
        &["--sandbox-output-bytes", "65536"],
    );
    let obs = observations(&run.report);
    assert_eq!(
        obs[0]["payload"]["termination"], "output_limit",
        "{}\n{}",
        obs[0], run.stderr
    );
    assert_eq!(obs[0]["metrics"]["stdout_bytes"], 65536);
    let notices = obs[0]["notices"].as_array().unwrap();
    assert!(
        notices.iter().any(|n| n["code"] == "output_truncated"),
        "{notices:?}"
    );
}

/// REQ-SBX-004 / INV-10: a read-write execution writes inside the workspace
/// only and the project is untouched; the same module in `read_only` mode
/// is refused by the WASI boundary; the ephemeral tmp is removed.
#[test]
fn m2_read_write_stays_in_the_workspace() {
    let scope = scope();
    let data = tempfile::tempdir().unwrap();
    let run = testloop("write.json", scope.path(), data.path(), &[]);
    let obs = observations(&run.report);
    assert_eq!(
        obs[0]["payload"]["termination"], "exited",
        "{} {}",
        obs[0], run.stderr
    );
    assert_eq!(obs[0]["payload"]["exit_code"], 0, "{}", obs[0]);
    let task_id = run.report["task_id"].as_str().unwrap();
    let written = data
        .path()
        .join("workspaces")
        .join(task_id)
        .join("tree/out.txt");
    assert_eq!(
        std::fs::read_to_string(&written).unwrap(),
        "written by the sandbox"
    );
    assert!(!scope.path().join("out.txt").exists());
    assert_eq!(obs[1]["payload"]["termination"], "exited", "{}", obs[1]);
    assert_ne!(
        obs[1]["payload"]["exit_code"], 0,
        "read_only refuses the create: {}",
        obs[1]
    );
    assert_eq!(
        std::fs::read_to_string(&written).unwrap(),
        "written by the sandbox",
        "unchanged by the refused run"
    );
    assert!(
        std::fs::read_dir(data.path().join("workspaces").join(task_id).join("tmp"))
            .unwrap()
            .next()
            .is_none(),
        "the ephemeral tmp is removed"
    );
}

/// REQ-CONF-003 / REQ-OBS-006: `status` shows the tier, every clause and the engine.
/// REQ-PLAT-003 (every platform): the binary reports the tier it runs in, `core`
/// or `full` (never none), with each clause's state and the engine, on each of
/// the four platforms of the CI matrix; the core tier (built-in tools and the
/// sandbox) is what the other tests of this file run on all of them.
#[test]
fn m2_status_shows_tier_and_clauses() {
    let data = tempfile::tempdir().unwrap();
    let config = data.path().join("config.toml");
    std::fs::write(
        &config,
        format!(
            "config_version = 1\n[runtime]\ndata_dir = {:?}\n",
            data.path().join("d").to_string_lossy()
        ),
    )
    .unwrap();
    let out = run_bounded(
        Command::new(env!("CARGO_BIN_EXE_mimlys"))
            .args(["--config", config.to_str().unwrap(), "--json", "status"])
            .env("MIMLYS_CONFIG", ""),
        data.path(),
    );
    let json: Value = serde_json::from_str(&out.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}\n{}", out.stdout, out.stderr));
    let tier = json["tier"].as_str().unwrap();
    assert!(tier == "core" || tier == "full", "{tier}");
    assert_eq!(json["engine_version"], "50.0.1");
    assert_eq!(
        json["clauses"]["environment"]["state"], "enforced",
        "{}",
        json["clauses"]
    );
    // K: the process group on Linux, the job on Windows, sandbox membership
    // on macOS (RFC 011 Amendment 2, settled); on a macOS without
    // `sandbox_init` the worker reports it unavailable, never enforced on faith.
    let seatbelt_missing =
        cfg!(target_os = "macos") && json["clauses"]["filesystem"]["reason"] == "seatbelt_missing";
    assert_eq!(
        json["clauses"]["termination"]["state"],
        if seatbelt_missing {
            "unavailable"
        } else {
            "enforced"
        },
        "{}",
        json["clauses"]
    );
    assert!(json["clauses"]["network"]["state"].is_string());
    assert_eq!(json["interpreters"][0]["language"], "javascript");
    let out = run_bounded(
        Command::new(env!("CARGO_BIN_EXE_mimlys"))
            .args(["--config", config.to_str().unwrap(), "--json", "status"])
            .env("MIMLYS_CONFIG", "")
            .env("MIMLYS_PROBE_FAIL", "K"),
        data.path(),
    );
    let json: Value = serde_json::from_str(&out.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}\n{}", out.stdout, out.stderr));
    assert_eq!(
        json["clauses"]["termination"]["state"], "unavailable",
        "the probe reports an injected failure"
    );
}

/// REQ-SBX-009 / REQ-SCHED-003: the overhead report. Prints setup and wall
/// times for a trivial script over several executions; the review package
/// records them and fixes LIM-S8.
#[test]
fn m2_overhead_report() {
    let scope = scope();
    let data = tempfile::tempdir().unwrap();
    let script = data.path().join("many.json");
    let decisions: Vec<Value> = (0..10)
        .map(|i| serde_json::json!({"decision": "act", "tool": "run_script", "arguments": {"language": "javascript", "source": format!("console.log({i});"), "mode": "read_only"}}))
        .chain(std::iter::once(serde_json::json!({"decision": "complete", "summary": "done"})))
        .collect();
    std::fs::write(&script, serde_json::to_string(&decisions).unwrap()).unwrap();
    let qjs = qjs();
    let started = std::time::Instant::now();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_mimlys-testloop"));
    cmd.args([
        "--script",
        script.to_str().unwrap(),
        "--data-dir",
        data.path().to_str().unwrap(),
        "--scope",
        scope.path().to_str().unwrap(),
        "--interpreter-module",
        qjs.to_str().unwrap(),
        "--steps",
        "20",
    ])
    .env("MIMLYS_WORKER_BIN", env!("CARGO_BIN_EXE_mimlys-worker"))
    .env("MIMLYS_CONFIG", "");
    let out = run_bounded(&mut cmd, data.path());
    assert!(out.status.success(), "{}", out.stderr);
    let total_ms = started.elapsed().as_millis();
    let report: Value = serde_json::from_str(&out.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}\n{}", out.stdout, out.stderr));
    let obs = observations(&report);
    assert_eq!(obs.len(), 10, "{}", out.stderr);
    println!(
        "M2 overhead report: whole run {total_ms} ms for 10 executions, including the worker start and the interpreter compilation"
    );
    let setup: Vec<i64> = obs
        .iter()
        .map(|o| o["metrics"]["setup_ms"].as_i64().unwrap())
        .collect();
    let wall: Vec<i64> = obs
        .iter()
        .map(|o| o["metrics"]["wall_ms"].as_i64().unwrap())
        .collect();
    println!(
        "M2 overhead report (REQ-SBX-009): first execution wall {} ms (worker start and compile); steady state: setup {:?} ms, wall {:?} ms",
        wall[0],
        &setup[1..],
        &wall[1..]
    );
    let steady_setup: i64 = setup[1..].iter().sum();
    let steady_wall: i64 = wall[1..].iter().sum();
    println!(
        "steady-state setup share: {:.1}%",
        100.0 * steady_setup as f64 / steady_wall.max(1) as f64
    );
    assert!(
        setup[1..].iter().all(|s| *s < 1000),
        "setup stays well under a second: {setup:?}"
    );
}
