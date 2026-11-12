//! REQ-SCHED-005 at the process (handoff 002d): a task that is terminal holds
//! no worker, lock or scheduler slot. The scheduler half is tested in the
//! core (`runtime/tests.rs`); the half that belongs to the process is tested
//! here through the binary, because only the binary has a worker, a store
//! lock and an exit: a run that used the worker ends with its task
//! `completed`, and then the worker's process is gone, the record the
//! runtime keeps of a live worker is gone, and the store is free for the next
//! command, which starts a worker of its own at once.

mod common;

use common::run_bounded;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

const HELLO: &str = r#"(module (import "wasi_snapshot_preview1" "proc_exit" (func $exit (param i32)))
    (memory 1) (export "memory" (memory 0))
    (func (export "_start") (call $exit (i32.const 0))))"#;

struct Env {
    dir: tempfile::TempDir,
    config: PathBuf,
}

impl Env {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("script.json");
        std::fs::write(
            &script,
            r#"[
  {"decision": "act", "tool": "run_wasm", "arguments": {"module": "tools/hello.wasm", "mode": "read_only"}},
  {"decision": "complete", "summary": "ran"}
]"#,
        )
        .unwrap();
        let config = dir.path().join("config.toml");
        std::fs::write(
            &config,
            format!(
                "config_version = 1\n[runtime]\ndata_dir = {:?}\n[backend]\nkind = \"fake\"\nscript = {:?}\n[logging]\nlevel = \"debug\"\n",
                dir.path().join("data").to_string_lossy(),
                script.to_string_lossy()
            ),
        )
        .unwrap();
        Self { dir, config }
    }

    /// A project holding a module that `run_wasm` can run.
    fn scope(&self, name: &str) -> String {
        let dir = self.dir.path().join(name);
        std::fs::create_dir_all(dir.join("tools")).unwrap();
        std::fs::write(dir.join("tools/hello.wasm"), wat::parse_str(HELLO).unwrap()).unwrap();
        dir.to_string_lossy().into_owned()
    }

    fn run(&self, args: &[&str]) -> (i32, String, String) {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_mimlys"));
        cmd.arg("--config")
            .arg(&self.config)
            .arg("--json")
            .args(args)
            .env("MIMLYS_CONFIG", "")
            .env("MIMLYS_WORKER_BIN", env!("CARGO_BIN_EXE_mimlys-worker"));
        let out = run_bounded(&mut cmd, self.dir.path());
        (out.code(), out.stdout, out.stderr)
    }

    /// The pids of the workers the runtime logged starting.
    fn worker_pids(stderr: &str) -> Vec<String> {
        stderr
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .filter_map(|v| v["fields"]["message"].as_str().map(str::to_owned))
            .filter_map(|m| {
                let rest = m.strip_prefix("worker ")?;
                let (pid, tail) = rest.split_once(' ')?;
                tail.starts_with("started").then(|| pid.to_owned())
            })
            .collect()
    }
}

/// Whether a process with this pid exists (Unix; elsewhere `None`).
fn alive(pid: &str) -> Option<bool> {
    if cfg!(unix) {
        let status = Command::new("kill").args(["-0", pid]).output().ok()?;
        Some(status.status.success())
    } else {
        None
    }
}

fn completed(stdout: &str, stderr: &str) -> String {
    let value: Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("not JSON ({e}): {stdout}\n{stderr}"));
    assert_eq!(value["task"]["state"], "completed", "{value}\n{stderr}");
    value["task"]["task_id"].as_str().unwrap().to_owned()
}

/// REQ-SCHED-005: a run that used the worker and completed leaves no worker
/// (its process is gone, its record `worker.json` is removed) and no lock (the
/// next command opens the store for writing and runs a task of its own, with a
/// worker of its own), while the task stays `completed`.
#[test]
fn req_sched_005_a_finished_run_leaves_no_worker_and_no_lock() {
    let env = Env::new();
    let (code, stdout, stderr) = env.run(&["run", "run the module", "--scope", &env.scope("a")]);
    assert_eq!(code, 0, "{stdout}\n{stderr}");
    let first = completed(&stdout, &stderr);
    let pids = Env::worker_pids(&stderr);
    assert_eq!(
        pids.len(),
        1,
        "the run started one worker (else this test shows nothing): {stderr}"
    );
    let record = env.dir.path().join("data").join("worker.json");
    assert!(
        !Path::new(&record).exists(),
        "the record of a live worker is removed with it"
    );
    if let Some(alive) = alive(&pids[0]) {
        assert!(!alive, "the worker {} is gone", pids[0]);
    }

    // No lock: the next command writes at once and runs a worker of its own.
    let (code, stdout, stderr) = env.run(&["run", "run it again", "--scope", &env.scope("b")]);
    assert_eq!(code, 0, "{stdout}\n{stderr}");
    let second = completed(&stdout, &stderr);
    assert_ne!(first, second);
    let pids = Env::worker_pids(&stderr);
    assert_eq!(pids.len(), 1, "{stderr}");
    assert!(!Path::new(&record).exists());

    // The first task is as it was left.
    let (code, stdout, stderr) = env.run(&["task", "show", &first]);
    assert_eq!(code, 0, "{stdout}\n{stderr}");
    let shown: Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(shown["task"]["state"], "completed");
}
