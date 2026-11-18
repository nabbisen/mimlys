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
    /// The live worker's record (`worker.json`): its pid and its marker.
    record: PathBuf,
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
        allowed_args: vec![
            "visible".into(),
            "setsid-sleep".into(),
            "sleep".into(),
            "poll-time".into(),
            "kill-worker".into(),
        ],
        arg_pattern: Some("[A-Za-z0-9_.-]+".into()),
        max_args: 3,
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
        allow_paths: false,
    };
    let record = data.path().join("worker.json");
    let spec =
        mimlys::app::confinement_spec(&data_dir, &[], std::slice::from_ref(&declaration), !network);
    let worker = Worker::new(WorkerConfig {
        program: worker_bin(),
        interpreters: vec![],
        confinement: platform(),
        limits: mimlys_exec::sandbox::Limits::default(),
        spec: Some(spec),
        host_tools: vec![declaration],
        record: Some(record.clone()),
    });
    // The membership tests run workers on workspaces of their own, and compare
    // two workers of one specification: not workers made per task.
    worker.bind_to_task(false);
    Side {
        _data: data,
        tree,
        worker,
        name,
        network,
        record,
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
    // Let each tool list again, now that both sandboxes are populated: a state,
    // not a pause. Each tool rewrites `seen.txt` every 200 ms; the listing that
    // follows the second rewrite seen after this point began after it, and so
    // after both sleepers were there (the first rewrite may stem from a listing
    // that began earlier). Up to 60 s.
    let stamp = |side: &Side| {
        std::fs::metadata(side.tree.join("seen.txt"))
            .and_then(|m| m.modified())
            .ok()
    };
    let mut stamps = [stamp(&main), stamp(&fetch)];
    let mut rewrites = [0u32; 2];
    assert!(
        wait_for(Duration::from_secs(60), || {
            for (i, side) in [&main, &fetch].into_iter().enumerate() {
                let now = stamp(side);
                if now != stamps[i] {
                    stamps[i] = now;
                    rewrites[i] += 1;
                }
            }
            rewrites.iter().all(|n| *n >= 2)
        }),
        "each tool lists again once both sandboxes are populated"
    );
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
    // A tool that ends first: the worker's own sweep at its end must not
    // take the sweeper with it (the worker's loop excludes the sweeper).
    let first = side
        .run(&["sleep", "1"], &Cancellation::new())
        .join()
        .unwrap();
    assert_eq!(first.termination, Termination::Exited { exit_code: 0 });
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
    // The contract is LIM-O2 plus one poll; the time allowed is five times that,
    // a margin no stall of a slow machine reaches (a sweep that needs a multiple
    // of the limit is a defect; the members being gone at all is the state), and
    // the wait for the state is that and 30 s more.
    let bound = Duration::from_millis(LIM_O2_CANCEL_MS + LIM_H6_TRACK_INTERVAL_MS + 250);
    let allowed = bound * 5;
    let gone = wait_for(allowed + Duration::from_secs(30), || {
        matching(&sleeper).is_empty() && matching(&tool).is_empty()
    });
    let took = killed_at.elapsed();
    eprintln!("MEASURE members gone {took:?} after SIGKILL of the worker");
    let outcome = run.join().unwrap();
    assert!(
        gone,
        "a member is left after the worker was killed: {took:?}"
    );
    assert!(
        took <= allowed,
        "swept in {took:?}, more than five times LIM-O2 plus one poll ({allowed:?}; the limit is {bound:?})"
    );
    assert_eq!(
        outcome.termination,
        Termination::HostFault {
            detail: HostFaultDetail::WorkerDied
        }
    );
    side.worker.shutdown();
}

/// REQ-CONF-001 (clauses M and C, polled every LIM-H6): the cost of one poll,
/// measured from inside the sandbox with the calls the worker's watchdogs
/// make (list every pid, read the ones the profile lets it read). It is the
/// figure the architect sets LIM-H6 from (handoff 011c-5 item 4a). The
/// assertion is only that a poll is much cheaper than the interval, or the
/// watchdogs could not keep their period.
#[test]
fn req_conf_001_one_poll_costs_far_less_than_the_interval() {
    let test = "req_conf_001_one_poll_costs_far_less_than_the_interval";
    let side = side("main_tool", false);
    if skip_without_seatbelt(test, &side) {
        return;
    }
    let outcome = side
        .run(&["poll-time", "200"], &Cancellation::new())
        .join()
        .unwrap();
    let line = String::from_utf8_lossy(&outcome.stdout).trim().to_owned();
    eprintln!("MEASURE poll cost: {line}");
    let field = |name: &str| -> f64 {
        let words: Vec<&str> = line.split_whitespace().collect();
        let at = words
            .iter()
            .position(|w| *w == name)
            .unwrap_or_else(|| panic!("{name} in {line:?}"));
        words[at + 1].parse().unwrap()
    };
    assert!(field("polls") >= 200.0, "{line}");
    assert!(
        field("members_read") >= 1.0,
        "the control: the tool reads its own tree: {line}"
    );
    assert!(
        field("mean_ms") < LIM_H6_TRACK_INTERVAL_MS as f64 / 4.0,
        "a poll is a quarter of the interval at most: {line}"
    );
    side.worker.shutdown();
}

/// The CPU time of a process, in seconds, from `ps -o cputime=`.
fn cpu_seconds(pid: u32) -> f64 {
    let out = Command::new("ps")
        .args(["-o", "cputime=", "-p", &pid.to_string()])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    text.split(':')
        .try_fold(0.0, |acc, part| {
            part.parse::<f64>().ok().map(|v| acc * 60.0 + v)
        })
        .unwrap_or_else(|| panic!("not a cpu time: {text:?}"))
}

/// REQ-CONF-001 (handoff 011c-5 item 4a): the worker's CPU time while a tool
/// sleeps for ten seconds under the watchdogs, and while it is idle. Reported
/// (`MEASURE` lines), the architect fixes LIM-H6 from it; the assertion is
/// only that the worker does not burn a core.
#[test]
fn req_conf_001_the_worker_is_cheap_while_a_tool_sleeps() {
    let test = "req_conf_001_the_worker_is_cheap_while_a_tool_sleeps";
    let side = side("main_tool", false);
    if skip_without_seatbelt(test, &side) {
        return;
    }
    let tool = "mimlys-probe-tool sleep 14$";
    for pid in matching(tool) {
        let _ = Command::new("kill")
            .args(["-KILL", &pid.to_string()])
            .status();
    }
    let cancel = Cancellation::new();
    let run = side.run(&["sleep", "14"], &cancel);
    assert!(wait_for(Duration::from_secs(60), || !matching(tool).is_empty()));
    let tool_pid = matching(tool)[0];
    let parent = Command::new("ps")
        .args(["-o", "ppid=", "-p", &tool_pid.to_string()])
        .output()
        .unwrap();
    let worker: u32 = String::from_utf8_lossy(&parent.stdout)
        .trim()
        .parse()
        .unwrap();
    std::thread::sleep(Duration::from_millis(500));
    let (cpu0, wall0) = (cpu_seconds(worker), Instant::now());
    std::thread::sleep(Duration::from_secs(10));
    let (cpu1, wall1) = (cpu_seconds(worker), wall0.elapsed());
    cancel.cancel();
    let _ = run.join();
    let idle0 = cpu_seconds(worker);
    std::thread::sleep(Duration::from_secs(5));
    let idle1 = cpu_seconds(worker);
    eprintln!(
        "MEASURE worker cpu while a tool sleeps: {:.2} s of cpu in {:.1} s of wall ({:.2} %), idle: {:.2} s in 5 s",
        cpu1 - cpu0,
        wall1.as_secs_f64(),
        (cpu1 - cpu0) / wall1.as_secs_f64() * 100.0,
        idle1 - idle0
    );
    assert!(
        (cpu1 - cpu0) / wall1.as_secs_f64() < 0.25,
        "the worker uses under a quarter of a core while it watches a tool"
    );
    side.worker.shutdown();
}

/// The live worker's process id and marker, from its record.
fn record_of(side: &Side) -> (i32, String) {
    let text = std::fs::read_to_string(&side.record).expect("the live worker's record");
    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
    (
        value["pid"].as_i64().expect("pid") as i32,
        value["marker"]
            .as_str()
            .expect("the record holds the worker's marker")
            .to_owned(),
    )
}

/// An outsider the test owns: a process of the user, killed when the test ends.
struct Outsider(std::process::Child);

impl Drop for Outsider {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn bystander(secs: &str) -> Outsider {
    Outsider(
        Command::new("/bin/sleep")
            .arg(secs)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap(),
    )
}

/// A process in a sandbox of its own whose profile allows everything (as 25
/// system processes of a runner do): started outside any confinement by the
/// probe tool, awaited until `sandbox_check` says it is sandboxed.
fn permissive_outsider(outside: &mimlys_exec::confine::macos::outside::Outside) -> Outsider {
    use mimlys_exec::confine::macos::outside::Check;
    let child = Command::new(probe_tool())
        .args(["allow-all", "120"])
        .env_clear()
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let pid = child.id() as i32;
    let outsider = Outsider(child);
    assert!(
        wait_for(Duration::from_secs(60), || outside.sandboxed(pid)
            == Check::Yes),
        "the permissive outsider is in a sandbox"
    );
    outsider
}

/// REQ-CONF-009 / audit A-21 (handoff 011p, ROADMAP's "done when"): a tool that
/// kills the worker's sweeper and then the worker leaves its tree (a child, a
/// re-parented orphan and a writer of the workspace) alive in the sandbox, and
/// the **runtime**, outside it, sweeps that tree when the worker ends, within
/// LIM-O2 (the sweep runs inside `execute`, before it returns): afterwards
/// nothing of the tool runs and the workspace receives no write. No outsider is
/// touched: an unsandboxed process of the user, a sandboxed process that
/// allows everything and another worker's member all survive. The mutation: the
/// outside sweep not run leaves the tree, because the only thing that would
/// have ended it, the sweeper, is dead.
#[test]
fn req_conf_009_the_runtime_sweeps_a_tree_whose_sweeper_and_worker_a_tool_killed() {
    let test = "req_conf_009_the_runtime_sweeps_a_tree_whose_sweeper_and_worker_a_tool_killed";
    use mimlys_exec::confine::macos::outside::{Outside, sweeps_run};
    let main = side("main_tool", false);
    if skip_without_seatbelt(test, &main) {
        return;
    }
    let other = side("fetch_tool", true);
    let (secs, orphan, other_secs) = ("2361", "2362", "2364");
    for pattern in [
        format!("^/bin/sleep {secs}$"),
        format!("^/bin/sleep {orphan}$"),
        format!("^/bin/sleep {other_secs}$"),
        "echo x >> writer.log".to_owned(),
    ] {
        for pid in matching(&pattern) {
            let _ = Command::new("kill")
                .args(["-KILL", &pid.to_string()])
                .status();
        }
    }
    main.worker.start().expect("the worker starts");
    assert_eq!(
        main.worker.outside_sweep(),
        mimlys_exec::worker::OutsideSweep::Verified,
        "the test tells a member from an outsider on this machine"
    );
    let (_, marker) = record_of(&main);
    let outside = Outside::resolve().expect("sandbox_check");
    let user_process = bystander("2363");
    let permissive = permissive_outsider(&outside);
    let cancel = Cancellation::new();
    let other_run = other.run(&["setsid-sleep", other_secs], &cancel);
    let other_sleeper = format!("^/bin/sleep {other_secs}$");
    assert!(
        wait_for(Duration::from_secs(60), || !matching(&other_sleeper)
            .is_empty()),
        "the other worker's tool runs"
    );
    let sleeper = format!("^/bin/sleep {secs}$");
    let orphan_sleeper = format!("^/bin/sleep {orphan}$");
    let writer = "echo x >> writer.log";
    let run = main.run(&["kill-worker", secs, "writer.log"], &cancel);
    let outcome = run.join().unwrap();
    assert_eq!(
        outcome.termination,
        Termination::HostFault {
            detail: HostFaultDetail::WorkerDied
        },
        "the tool killed its worker"
    );
    // Within LIM-O2 and the runtime's own grace (five times the limit): the sweep
    // ran inside `execute`, so the tree is gone as soon as it returned.
    let bound = Duration::from_millis(LIM_O2_CANCEL_MS + LIM_H6_TRACK_INTERVAL_MS + 250);
    let gone = wait_for(bound * 5, || {
        matching(&sleeper).is_empty()
            && matching(&orphan_sleeper).is_empty()
            && matching(writer).is_empty()
            && matching("mimlys-probe-tool kill-worker").is_empty()
    });
    assert!(
        gone,
        "the tool's tree is left after its worker was killed: sleeper {:?}, orphan {:?}, writer {:?}, tool {:?}",
        matching(&sleeper),
        matching(&orphan_sleeper),
        matching(writer),
        matching("mimlys-probe-tool kill-worker")
    );
    assert!(
        sweeps_run(&marker) >= 1,
        "the runtime swept the worker's sandbox from outside"
    );
    // No write after the sweep: the writer is gone, so the file no longer grows.
    let log = main.tree.join("writer.log");
    let size = || std::fs::metadata(&log).map_or(0, |m| m.len());
    let before = size();
    assert!(
        before >= 6,
        "the writer wrote while the tool lived ({before})"
    );
    assert!(
        !wait_for(Duration::from_millis(400), || size() != before),
        "the workspace received a write after the sweep"
    );
    // Nobody else was touched.
    assert!(
        !matching(&other_sleeper).is_empty(),
        "another worker's member survived the sweep"
    );
    for (what, outsider) in [
        ("a process of the user", &user_process),
        ("a sandboxed process that allows everything", &permissive),
    ] {
        assert!(
            pid_alive(outsider.0.id() as i32),
            "{what} was signalled by the sweep"
        );
    }
    cancel.cancel();
    let _ = other_run.join();
    main.worker.shutdown();
    other.worker.shutdown();
}

/// Whether a process is alive (signal 0).
fn pid_alive(pid: i32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .status()
        .is_ok_and(|s| s.success())
}

/// REQ-CONF-009 (handoff 011p; the owner's condition): the test that finds a
/// worker's members from outside fails if `sandbox_check` stops telling a member
/// from an outsider. Two live workers, each with a marker of its own: a worker
/// and a tool of its own are members of its sandbox and of no other's; the test
/// process itself (the runtime) and a sandboxed process that allows every lookup
/// (marker and canary both allowed: what the marker alone cannot tell, spike
/// M1) are members of neither.
#[test]
fn req_conf_009_sandbox_check_tells_a_member_from_an_outsider() {
    use mimlys_exec::confine::macos::outside::{Check, Outside};
    let test = "req_conf_009_sandbox_check_tells_a_member_from_an_outsider";
    let a = side("main_tool", false);
    if skip_without_seatbelt(test, &a) {
        return;
    }
    let b = side("fetch_tool", true);
    a.worker.start().expect("worker A starts");
    b.worker.start().expect("worker B starts");
    let ((pid_a, marker_a), (pid_b, marker_b)) = (record_of(&a), record_of(&b));
    assert_ne!(marker_a, marker_b, "each worker has a marker of its own");
    let outside = Outside::resolve().expect("sandbox_check is in this macOS");
    let own = std::process::id() as i32;
    // The controls: the workers are in a sandbox, the test process is not.
    assert_eq!(outside.sandboxed(pid_a), Check::Yes);
    assert_eq!(outside.sandboxed(pid_b), Check::Yes);
    assert_eq!(outside.sandboxed(own), Check::No);
    // A member is a member of its own sandbox only.
    assert!(outside.is_member(&marker_a, pid_a), "A's worker is A's");
    assert!(outside.is_member(&marker_b, pid_b), "B's worker is B's");
    assert!(
        !outside.is_member(&marker_b, pid_a),
        "A's worker is not B's"
    );
    assert!(
        !outside.is_member(&marker_a, pid_b),
        "B's worker is not A's"
    );
    // The runtime is no member of either.
    assert!(!outside.is_member(&marker_a, own));
    assert!(!outside.is_member(&marker_b, own));
    // A tool of A, and the sleeper it leaves re-parented, are A's.
    let secs = "2365";
    let cancel = Cancellation::new();
    let run = a.run(&["setsid-sleep", secs], &cancel);
    let sleeper = format!("^/bin/sleep {secs}$");
    let tool = format!("mimlys-probe-tool setsid-sleep {secs}$");
    assert!(
        wait_for(Duration::from_secs(60), || {
            !matching(&sleeper).is_empty() && !matching(&tool).is_empty()
        }),
        "the tool and its sleeper run"
    );
    for pid in [matching(&sleeper)[0] as i32, matching(&tool)[0] as i32] {
        assert!(outside.is_member(&marker_a, pid), "{pid} is A's");
        assert!(!outside.is_member(&marker_b, pid), "{pid} is not B's");
    }
    // A sandboxed process that allows everything: marker and canary both allowed.
    let permissive = permissive_outsider(&outside);
    let pp = permissive.0.id() as i32;
    assert_eq!(
        outside.allows_lookup(pp, &marker_a),
        Check::Yes,
        "the control: its sandbox allows the marker"
    );
    assert_eq!(
        outside.allows_lookup(pp, &mimlys_exec::confine::macos::canary_of(&marker_a)),
        Check::Yes,
        "the control: and the canary"
    );
    assert!(
        !outside.is_member(&marker_a, pp) && !outside.is_member(&marker_b, pp),
        "a sandbox that allows every lookup is no worker's"
    );
    cancel.cancel();
    let _ = run.join();
    a.worker.shutdown();
    b.worker.shutdown();
}

/// REQ-CONF-009 (handoff 011p, item 4): the verification fails (the known
/// member's answer is made wrong): `outside_sweep` says why, the tier is not
/// touched, the worker's own sweeper still sweeps a worker that is killed, and
/// the sweep from outside is never run for it. The mutation: the verification
/// skipped, which would sweep from outside with a test nobody checked.
#[test]
fn req_conf_009_a_failed_verification_leaves_the_sweeper_alone_and_says_so() {
    let test = "req_conf_009_a_failed_verification_leaves_the_sweeper_alone_and_says_so";
    let side = side("main_tool", false);
    if skip_without_seatbelt(test, &side) {
        return;
    }
    side.worker.fail_outside_verification_for_test(true);
    side.worker.start().expect("the worker starts all the same");
    match side.worker.outside_sweep() {
        mimlys_exec::worker::OutsideSweep::Unavailable(why) => {
            assert!(why.contains("does not find the worker"), "{why}");
        }
        other => panic!("the verification should have failed: {other:?}"),
    }
    let (_, marker) = record_of(&side);
    let secs = "2366";
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
    let tool_pid = matching(&tool)[0];
    let worker_pid = Command::new("ps")
        .args(["-o", "ppid=", "-p", &tool_pid.to_string()])
        .output()
        .unwrap();
    let worker_pid = String::from_utf8_lossy(&worker_pid.stdout)
        .trim()
        .to_owned();
    assert!(
        Command::new("kill")
            .args(["-KILL", &worker_pid])
            .status()
            .unwrap()
            .success()
    );
    let outcome = run.join().unwrap();
    assert_eq!(
        outcome.termination,
        Termination::HostFault {
            detail: HostFaultDetail::WorkerDied
        }
    );
    // The in-sandbox sweeper, the first layer, ends the tree, as in 0.10.
    let bound = Duration::from_millis(LIM_O2_CANCEL_MS + LIM_H6_TRACK_INTERVAL_MS + 250);
    assert!(
        wait_for(bound * 5 + Duration::from_secs(30), || {
            matching(&sleeper).is_empty() && matching(&tool).is_empty()
        }),
        "the worker's own sweeper did not end the tree"
    );
    assert_eq!(
        mimlys_exec::confine::macos::outside::sweeps_run(&marker),
        0,
        "nothing was swept from outside with a test that did not hold"
    );
    side.worker.shutdown();
}
