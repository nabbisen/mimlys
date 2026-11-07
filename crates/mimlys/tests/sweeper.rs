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
#![cfg(any(target_os = "linux", target_os = "macos"))]

use std::io::{BufRead, BufReader, Read};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

fn demo(marker: &Path, hold: u64, slow_ms: u64, child: bool) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_mimlys-probe-tool"));
    cmd.args(["sweeper-demo", marker.to_str().unwrap()])
        .arg(hold.to_string())
        .arg(slow_ms.to_string())
        .arg(if child { "1" } else { "0" })
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
/// it is alive and the marker is absent) and acts when the owner ends.
#[test]
fn req_conf_009_sweeper_acts_at_end_of_file_and_only_then() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("eof");
    let mut owner = demo(&marker, 2, 0, false)
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
    owner.wait().unwrap();
    assert!(
        wait_for(Duration::from_secs(3), || marker.exists()),
        "the owner ended: end of file reached the sweeper"
    );
}

/// REQ-CONF-009: the write end is close-on-exec. The owner starts a child
/// that outlives it (4 s) and exits at once; if the child inherited the pipe
/// the sweeper would wait for it and the marker would come after the child's
/// end, not within 2.5 s of the owner's.
#[test]
fn req_conf_009_sweeper_pipe_is_not_inherited_by_a_child() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("eof");
    let mut owner = demo(&marker, 0, 0, true)
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let (_sweeper, _out) = sweeper_pid(&mut owner);
    owner.wait().unwrap();
    let ended = Instant::now();
    assert!(
        wait_for(Duration::from_millis(2500), || marker.exists()),
        "the sweeper saw end of file while the owner's child still lived (after {:?})",
        ended.elapsed()
    );
}

/// REQ-CONF-009: the sweeper has a process group of its own, so that a group
/// kill of its owner (what the runtime does to a worker) does not reach it,
/// and it holds none of its owner's streams, so that whoever reads the
/// owner's output sees end of file when the owner is gone, not when the
/// sweeper is. The sweeper is slow on purpose (3 s) so that both can be seen.
#[test]
fn req_conf_009_sweeper_has_its_own_group_and_holds_no_stream() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("eof");
    let mut owner = demo(&marker, 30, 3000, false)
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
        .args(["-c", &format!("kill -KILL -- -{group}")])
        .status()
        .unwrap();
    assert!(killed.success());
    let started = Instant::now();
    let reader = std::thread::spawn(move || {
        let mut rest = String::new();
        let _ = out.read_to_string(&mut rest);
    });
    while !reader.is_finished() && started.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(25));
    }
    assert!(
        reader.is_finished() && started.elapsed() < Duration::from_millis(2000),
        "end of file on the owner's output came with the owner, not with the sweeper ({:?})",
        started.elapsed()
    );
    owner.wait().unwrap();
    assert!(
        wait_for(Duration::from_secs(8), || marker.exists()),
        "the sweeper survived the group kill and acted"
    );
}
