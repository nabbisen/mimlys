//! Shared by the suites: a child process run with a bound. Its streams go
//! to files, never to pipes, so a run that hangs is killed after the bound
//! and its output is read all the same, whatever grandchild may still hold
//! a handle; and the test fails with that output instead of hanging the
//! whole binary, which would hide every other test's message.

#![allow(dead_code)]

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
