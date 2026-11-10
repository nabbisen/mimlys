//! The sweeper's pipe (REQ-CONF-009; RFC 011 Amendment 2, implementation
//! decisions, point 6), through the probe tool's `sweeper-demo`, on Linux and
//! macOS: the process that holds the read end of a pipe whose write end only
//! the worker holds acts at end of file and at nothing else. The kill loop
//! it runs on macOS is tested in `m6c_host_tools.rs`; what is tested here is
//! what makes it reliable and is the same code on both systems.
//!
//! Each test has its control (the sweeper is alive and idle while its owner
//! lives) and was shown to fail without the property it guards (the mutation
//! outputs are in the review request).
//!
//! **No test decides by a short elapsed time** (handoff 011i). A runner can
//! delay a thread or a process start by seconds (one test measured 2.004 s
//! against a bound of 2 s under Rosetta, while the property held). Each
//! property is read from the state of the processes at the moment that
//! matters: whether the sweeper, the owner's child or the owner is still alive,
//! and whether the marker exists. The held durations (a sweeper that waits
//! 15 s, a child that sleeps 30 s, an owner that holds 20 s) are far longer
//! than any delay the tests wait for, so a slow runner cannot cross them,
//! while a defect still shows (the mutated run waits for them and fails).
#![cfg(any(target_os = "linux", target_os = "macos"))]

use std::io::{BufRead, BufReader, Read};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

fn demo(marker: &Path, hold: u64, slow_ms: u64, child_secs: u64) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_mimlys-probe-tool"));
    cmd.args(["sweeper-demo", marker.to_str().unwrap()])
        .arg(hold.to_string())
        .arg(slow_ms.to_string())
        .arg(child_secs.to_string())
        .env_clear()
        .stdin(Stdio::null());
    cmd
}

/// The pid the demo prints.
fn sweeper_pid(child: &mut Child) -> (i32, BufReader<std::process::ChildStdout>) {
    let mut out = BufReader::new(child.stdout.take().expect("piped"));
    let mut line = String::new();
    out.read_line(&mut line)
        .expect("the demo prints the sweeper");
    let pid = line
        .trim()
        .strip_prefix("sweeper ")
        .unwrap_or_else(|| panic!("not a sweeper line: {line:?}"))
        .parse()
        .unwrap();
    (pid, out)
}

fn ps(field: &str, pid: i32) -> Option<String> {
    let out = Command::new("ps")
        .args(["-o", &format!("{field}="), "-p", &pid.to_string()])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    (!text.is_empty()).then_some(text)
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

/// REQ-CONF-009: the sweeper does nothing while its owner lives (the control:
/// it is alive and the marker is absent) and acts when the owner ends. The
/// owner holds for 20 s and is ended by the test (a kill, as the runtime ends a
/// worker), so the control is read while the owner is certainly alive.
#[test]
fn req_conf_009_sweeper_acts_at_end_of_file_and_only_then() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("eof");
    let mut owner = demo(&marker, 20, 0, 0)
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let (sweeper, _out) = sweeper_pid(&mut owner);
    std::thread::sleep(Duration::from_millis(700));
    assert!(
        ps("pid", sweeper).is_some(),
        "the sweeper is alive while the owner lives"
    );
    assert!(!marker.exists(), "and has done nothing");
    owner.kill().unwrap();
    owner.wait().unwrap();
    assert!(
        wait_for(Duration::from_secs(20), || marker.exists()),
        "the owner ended: end of file reached the sweeper"
    );
}

/// REQ-CONF-009: the write end is close-on-exec. The owner starts a child
/// that outlives it (30 s) and exits at once. If the child inherited the pipe
/// the sweeper would wait for the child's end, and the marker would come 30 s
/// later. So the test waits up to 20 s for the marker and then reads the state:
/// the child must still be alive when the marker is there, which proves that
/// end of file did not wait for it.
#[test]
fn req_conf_009_sweeper_pipe_is_not_inherited_by_a_child() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("eof");
    let mut owner = demo(&marker, 0, 0, 30)
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let (_sweeper, mut out) = sweeper_pid(&mut owner);
    let mut line = String::new();
    out.read_line(&mut line).expect("the demo prints its child");
    let child: i32 = line
        .trim()
        .strip_prefix("child ")
        .unwrap_or_else(|| panic!("not a child line: {line:?}"))
        .parse()
        .unwrap();
    owner.wait().unwrap();
    let acted = wait_for(Duration::from_secs(20), || marker.exists());
    let child_alive = ps("pid", child).is_some();
    // Do not leave a 30 s sleeper behind, whatever the result.
    let _ = Command::new("kill")
        .args(["-s", "KILL", &child.to_string()])
        .status();
    assert!(
        acted,
        "the sweeper did not see end of file while the owner's child lived (20 s)"
    );
    assert!(
        child_alive,
        "the sweeper saw end of file only after the owner's child ended"
    );
}

/// REQ-CONF-009: the sweeper has a process group of its own, so that a group
/// kill of its owner (what the runtime does to a worker) does not reach it,
/// and it holds none of its owner's streams, so that whoever reads the
/// owner's output sees end of file when the owner is gone, not when the
/// sweeper is. The sweeper is slow on purpose (15 s) so that both can be seen:
/// when the reader reaches end of file the sweeper must still be alive and its
/// marker absent. Had the sweeper held the owner's output, end of file would
/// have come with the sweeper's end, after the marker.
#[test]
fn req_conf_009_sweeper_has_its_own_group_and_holds_no_stream() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("eof");
    let mut owner = demo(&marker, 30, 15_000, 0)
        .process_group(0)
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let (sweeper, mut out) = sweeper_pid(&mut owner);
    let group = owner.id() as i32;
    assert_eq!(
        ps("pgid", group).as_deref(),
        Some(group.to_string().as_str())
    );
    let sweeper_group = ps("pgid", sweeper).expect("the sweeper lives");
    assert_ne!(
        sweeper_group,
        group.to_string(),
        "a group of its own, not the owner's"
    );
    // The runtime's kill of a worker: its whole process group.
    let killed = Command::new("sh")
        .args(["-c", &format!("kill -s KILL -- -{group}")])
        .status()
        .unwrap();
    assert!(killed.success());
    let started = Instant::now();
    let reader = std::thread::spawn(move || {
        let mut rest = String::new();
        let _ = out.read_to_string(&mut rest);
    });
    while !reader.is_finished() && started.elapsed() < Duration::from_secs(60) {
        std::thread::sleep(Duration::from_millis(25));
    }
    // The state at end of file, not the time it took.
    let finished = reader.is_finished();
    let acted = marker.exists();
    let sweeper_alive = ps("pid", sweeper).is_some();
    assert!(finished, "no end of file on the owner's output in 60 s");
    assert!(
        !acted && sweeper_alive,
        "end of file on the owner's output came with the sweeper's end, not with the owner's (marker {acted}, sweeper alive {sweeper_alive}, after {:?})",
        started.elapsed()
    );
    owner.wait().unwrap();
    assert!(
        wait_for(Duration::from_secs(40), || marker.exists()),
        "the sweeper survived the group kill and acted"
    );
}
