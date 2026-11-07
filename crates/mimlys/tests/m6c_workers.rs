//! The macOS workers' sandboxes against each other and against their own
//! death (handoff 011c-5; RFC 011 Amendment 2, implementation decisions,
//! point 6; REQ-CONF-009): two workers started as the runtime starts them
//! (the main one and the dependency step's), each with a real host tool, and
//! the sweeper that cleans up after a worker that is killed. macOS only.
//!
//! The workers are driven through `mimlys_exec::worker::Worker`, in this
//! process, so that the test can kill one and observe the other.
#![cfg(target_os = "macos")]

use mimlys_core::action::{EffectKind, EffectView, HostFaultDetail, Termination};
use mimlys_core::host_tool::HostToolDeclaration;
use mimlys_core::ids::{ActionId, ExecutionId, ScriptedIds, TaskId};
use mimlys_core::limits::{LIM_H6_TRACK_INTERVAL_MS, LIM_O2_CANCEL_MS};
use mimlys_core::spi::Cancellation;
use mimlys_exec::confine::platform;
use mimlys_exec::worker::{Execution, Worker, WorkerConfig};
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

mod common;

fn worker_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_mimlys-worker"))
}

fn probe_tool() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_mimlys-probe-tool"))
}

/// One worker as the runtime builds it: its own data directory, a
/// specification from `confinement_spec`, the probe tool declared.
struct Side {
    _data: tempfile::TempDir,
    tree: PathBuf,
    worker: Arc<Worker>,
    name: &'static str,
    network: bool,
}

fn side(name: &'static str, network: bool) -> Side {
    let data = tempfile::tempdir().unwrap();
    let data_dir = data.path().join("data");
    let tree = data_dir.join("workspaces/t/tree");
    std::fs::create_dir_all(&tree).unwrap();
    std::fs::create_dir_all(data_dir.join("workspaces/t/tmp")).unwrap();
    std::fs::create_dir_all(data_dir.join("tmp")).unwrap();
    let tree = tree.canonicalize().unwrap();
    let bin_dir = probe_tool()
        .parent()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let declaration = HostToolDeclaration {
        name: name.into(),
        command: probe_tool().to_string_lossy().into_owned(),
        args: vec![],
        allowed_args: vec!["visible".into(), "setsid-sleep".into()],
        arg_pattern: Some("[A-Za-z0-9_.-]+".into()),
        max_args: 2,
        effect: if network {
            EffectKind::ExternalRead
        } else {
            EffectKind::WorkspaceWrite
        },
        reversible: true,
        timeout_ms: 120_000,
        read_paths: vec![bin_dir],
        env: Default::default(),
        env_allow: vec![],
        network,
    };
    let spec =
        mimlys::app::confinement_spec(&data_dir, &[], std::slice::from_ref(&declaration), !network);
    let worker = Worker::new(WorkerConfig {
        program: worker_bin(),
        interpreters: vec![],
        confinement: platform(),
        limits: mimlys_exec::sandbox::Limits::default(),
        spec: Some(spec),
        host_tools: vec![declaration],
        record: None,
    });
    Side {
        _data: data,
        tree,
        worker,
        name,
        network,
    }
}

impl Side {
    /// Starts `run_host_tool <name> <args>` on a thread; the handle returns the outcome.
    fn run(
        &self,
        args: &[&str],
        cancel: &Cancellation,
    ) -> std::thread::JoinHandle<mimlys_core::spi::ExecutionOutcome> {
        let ids = ScriptedIds::default();
        let execution = Execution {
            task_id: TaskId::new(&ids),
            execution_id: ExecutionId::new(&ids),
            action_id: ActionId::new(&ids),
            tool: "run_host_tool".into(),
            arguments: serde_json::json!({"tool": self.name, "args": args}),
            effect: EffectView {
                kind: if self.network {
                    EffectKind::ExternalRead
                } else {
                    EffectKind::WorkspaceWrite
                },
                reversible: true,
                idempotent: false,
            },
            deadline_ms: 120_000,
            workspace_root: self.tree.to_string_lossy().into_owned(),
            read_only: false,
            replay: None,
        };
        let worker = Arc::clone(&self.worker);
        let cancel = cancel.clone();
        std::thread::spawn(move || worker.execute(execution, cancel))
    }

    fn numbers(&self, file: &str) -> Option<Vec<i32>> {
        let text = std::fs::read_to_string(self.tree.join(file)).ok()?;
        Some(
            text.split_whitespace()
                .filter_map(|t| t.parse().ok())
                .collect(),
        )
    }
}

fn wait_for(limit: Duration, mut done: impl FnMut() -> bool) -> bool {
    let started = Instant::now();
    while started.elapsed() < limit {
        if done() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    done()
}

/// Live processes whose command line matches `pattern` (pgrep -f).
fn matching(pattern: &str) -> Vec<u32> {
    let out = Command::new("pgrep")
        .args(["-f", pattern])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.trim().parse().ok())
        .collect()
}

fn skip_without_seatbelt(test: &str, side: &Side) -> bool {
    let probe = side.worker.probe_confined();
    let missing = probe
        .unavailable_lines()
        .iter()
        .any(|l| l.contains("seatbelt_missing"));
    if missing {
        common::skip(test, "sandbox_init is not on this macOS (seatbelt_missing)");
    }
    missing
}

/// REQ-CONF-009 / RFC 011 Amendment 2, implementation decisions, point 6: two
/// sandboxes stay apart. The main worker and the dependency step's worker,
/// both alive and both with members (a tool and the sleeper it left in a
/// session of its own), do not see each other's: neither tool's listing holds
/// a pid of the other's, nor of the other's worker. If they did, the kill loop
/// of one would end the other's tools. The control: each sees its own sleeper.
#[test]
fn req_conf_009_two_workers_do_not_see_each_others_members() {
    let test = "req_conf_009_two_workers_do_not_see_each_others_members";
    let main = side("main_tool", false);
    let fetch = side("fetch_tool", true);
    if skip_without_seatbelt(test, &main) {
        return;
    }
    let cancel = Cancellation::new();
    let a = main.run(&["visible", "20"], &cancel);
    let b = fetch.run(&["visible", "20"], &cancel);
    let ready = wait_for(Duration::from_secs(60), || {
        main.numbers("seen.txt").is_some() && fetch.numbers("seen.txt").is_some()
    });
    assert!(ready, "both tools run and report what they see");
    // Let each tool list a few more times, now that both sandboxes are populated.
    std::thread::sleep(Duration::from_millis(1200));
    let (mine_a, mine_b) = (
        main.numbers("mine.txt").unwrap(),
        fetch.numbers("mine.txt").unwrap(),
    );
    let (seen_a, seen_b) = (
        main.numbers("seen.txt").unwrap(),
        fetch.numbers("seen.txt").unwrap(),
    );
    cancel.cancel();
    let _ = (a.join(), b.join());
    // The control: the listing works, and each tool sees its own sleeper.
    assert!(
        seen_a.contains(&mine_a[1]),
        "A sees its own sleeper: {seen_a:?} {mine_a:?}"
    );
    assert!(
        seen_b.contains(&mine_b[1]),
        "B sees its own sleeper: {seen_b:?} {mine_b:?}"
    );
    // The property: nothing of the other's, and nothing at all in common
    // (the workers and their sweepers included).
    for pid in &mine_b {
        assert!(!seen_a.contains(pid), "A sees B's pid {pid}: {seen_a:?}");
    }
    for pid in &mine_a {
        assert!(!seen_b.contains(pid), "B sees A's pid {pid}: {seen_b:?}");
    }
    let shared: Vec<&i32> = seen_a.iter().filter(|p| seen_b.contains(p)).collect();
    assert!(
        shared.is_empty(),
        "the two sandboxes share pids: {shared:?}"
    );
}

/// REQ-CONF-009 / RFC 011 Amendment 2, implementation decisions, point 6: a
/// worker that is killed leaves no member alive. A tool starts a sleeper in a
/// session of its own and sleeps; the worker is killed with `SIGKILL`; the
/// sweeper, which the worker forked after its verification, sees end of file
/// and ends the tool and the sleeper within LIM-O2 plus one poll. The control:
/// before the kill both live and the sleeper's parent is `launchd`.
#[test]
fn req_conf_009_a_killed_worker_leaves_no_member() {
    let test = "req_conf_009_a_killed_worker_leaves_no_member";
    let side = side("main_tool", false);
    if skip_without_seatbelt(test, &side) {
        return;
    }
    let secs = "2351";
    for pid in matching(&format!("^/bin/sleep {secs}$")) {
        let _ = Command::new("kill")
            .args(["-KILL", &pid.to_string()])
            .status();
    }
    let cancel = Cancellation::new();
    let run = side.run(&["setsid-sleep", secs], &cancel);
    let sleeper = format!("^/bin/sleep {secs}$");
    let tool = format!("mimlys-probe-tool setsid-sleep {secs}$");
    assert!(
        wait_for(Duration::from_secs(60), || {
            !matching(&sleeper).is_empty() && !matching(&tool).is_empty()
        }),
        "the tool and its sleeper run"
    );
    let orphan = matching(&sleeper)[0];
    let parent = Command::new("ps")
        .args(["-o", "ppid=", "-p", &orphan.to_string()])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&parent.stdout).trim(), "1");
    // The worker is the tool's parent. `Worker::kill_for_test` would wait for
    // the execution's lock, so the worker alone gets the signal, as a crash would.
    let tool_pid = matching(&tool)[0];
    let worker_pid = Command::new("ps")
        .args(["-o", "ppid=", "-p", &tool_pid.to_string()])
        .output()
        .unwrap();
    let worker_pid = String::from_utf8_lossy(&worker_pid.stdout)
        .trim()
        .to_owned();
    assert!(
        worker_pid.parse::<u32>().is_ok_and(|p| p > 1),
        "{worker_pid:?}"
    );
    let killed_at = Instant::now();
    assert!(
        Command::new("kill")
            .args(["-KILL", &worker_pid])
            .status()
            .unwrap()
            .success()
    );
    let bound = Duration::from_millis(LIM_O2_CANCEL_MS + LIM_H6_TRACK_INTERVAL_MS + 250);
    let gone = wait_for(bound + Duration::from_secs(5), || {
        matching(&sleeper).is_empty() && matching(&tool).is_empty()
    });
    let took = killed_at.elapsed();
    let outcome = run.join().unwrap();
    assert!(
        gone,
        "a member is left after the worker was killed: {took:?}"
    );
    assert!(
        took <= bound,
        "swept in {took:?}, more than LIM-O2 plus one poll ({bound:?})"
    );
    assert_eq!(
        outcome.termination,
        Termination::HostFault {
            detail: HostFaultDetail::WorkerDied
        }
    );
    side.worker.shutdown();
}
