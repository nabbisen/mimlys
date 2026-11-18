//! Shared by the suites: a child process run with a bound. Its streams go
//! to files, never to pipes, so a run that hangs is killed after the bound
//! and its output is read all the same, whatever grandchild may still hold
//! a handle; and the test fails with that output instead of hanging the
//! whole binary, which would hide every other test's message.

#![allow(dead_code)]

#[cfg(feature = "test-bins")]
pub mod printer;
#[cfg(unix)]
pub mod pty;

use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// The bound of one run.
pub const BOUND: Duration = Duration::from_secs(180);

/// What a bounded run produced.
pub struct Bounded {
    /// The exit status.
    pub status: ExitStatus,
    /// Standard output, read from its file.
    pub stdout: String,
    /// Standard error, read from its file.
    pub stderr: String,
}

impl Bounded {
    /// The exit code, or -1 without one.
    pub fn code(&self) -> i32 {
        self.status.code().unwrap_or(-1)
    }
}

static RUNS: AtomicU64 = AtomicU64::new(0);

/// Runs `cmd` to its end within [`BOUND`], its streams in files under `dir`.
/// Panics with both streams when the bound passes.
pub fn run_bounded(cmd: &mut Command, dir: &Path) -> Bounded {
    let n = RUNS.fetch_add(1, Ordering::Relaxed);
    // The stack-margin guard (REQ-CLI-001, handoff 009h item 8): every `mimlys`
    // a suite runs through here has the command thread's stack at a quarter of
    // its size (a debug build only; a release build ignores the variable; the
    // other programs of the suites are left as they are), so that a
    // growth of the heaviest path past a quarter fails here, with a margin of
    // four, and not on a platform with less stack. A suite that tests the
    // variable sets its own value first, and a value exported in the shell that
    // runs the tests wins (to measure the stack a path needs).
    let is_mimlys = Path::new(cmd.get_program())
        .file_stem()
        .is_some_and(|name| name == "mimlys");
    if is_mimlys
        && std::env::var_os("MIMLYS_DEBUG_STACK_DIVISOR").is_none()
        && cmd
            .get_envs()
            .all(|(key, _)| key != "MIMLYS_DEBUG_STACK_DIVISOR")
    {
        cmd.env("MIMLYS_DEBUG_STACK_DIVISOR", "4");
    }
    let out_path = dir.join(format!("run-{}-{n}.stdout", std::process::id()));
    let err_path = dir.join(format!("run-{}-{n}.stderr", std::process::id()));
    cmd.stdin(Stdio::null())
        .stdout(std::fs::File::create(&out_path).unwrap())
        .stderr(std::fs::File::create(&err_path).unwrap());
    let mut child = cmd.spawn().expect("spawn");
    let started = Instant::now();
    let status = loop {
        if let Some(s) = child.try_wait().unwrap() {
            break Some(s);
        }
        if started.elapsed() > BOUND {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let stdout = std::fs::read_to_string(&out_path).unwrap_or_default();
    let stderr = std::fs::read_to_string(&err_path).unwrap_or_default();
    let Some(status) = status else {
        panic!(
            "the process did not exit within {} s\nstdout: {stdout}\nstderr: {stderr}",
            BOUND.as_secs()
        );
    };
    Bounded {
        status,
        stdout,
        stderr,
    }
}

/// Records that a test did not run its subject, and why. libtest has no
/// skipped state and hides the output of a passing test, so the line goes to
/// standard error and is also appended to `skips.log` in the target's
/// temporary directory (`CARGO_TARGET_TMPDIR`), which survives; a review
/// request counts the lines of that file per platform and reports them apart
/// from the passes. Delete the file before a run to count that run alone.
pub fn skip(test: &str, reason: &str) {
    use std::io::Write;
    let line = format!("SKIPPED {test}: {reason}");
    eprintln!("{line}");
    let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join("skips.log");
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        // One write per line: appends from parallel tests do not interleave.
        let _ = file.write_all(format!("{line}\n").as_bytes());
    }
}
