//! Shared by the suites: a child process run with a bound. Its streams go
//! to files, never to pipes, so a run that hangs is killed after the bound
//! and its output is read all the same, whatever grandchild may still hold
//! a handle; and the test fails with that output instead of hanging the
//! whole binary, which would hide every other test's message.

#![allow(dead_code)]

pub mod newer_store;
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

/// A registry directory (RFC 013) holding an entry held by this process, for the suites that
/// run a host tool under confinement: the registry is under the user's runtime directory, outside
/// every write set (Amendment 1; THR-38), so a tool can neither forge, change nor remove an entry.
/// `dir` is where the suite keeps its directories that no read set reaches.
pub struct HeldRegistry {
    /// The registry's directory.
    pub dir: std::path::PathBuf,
    /// The held entry's file stem.
    pub stem: String,
    text: Vec<u8>,
    _held: mimlys::registry::Handle,
}

impl HeldRegistry {
    pub fn new(dir: std::path::PathBuf) -> Self {
        use mimlys::registry::{Bounds, Class, Kind, Owner, Registry, Take};
        let _ = std::fs::remove_dir_all(&dir);
        let registry = Registry::new(
            dir.clone(),
            Bounds::default(),
            std::sync::Arc::new(mimlys_core::clock::SystemClock),
        );
        let class = Class {
            kind: Kind::Slot,
            key: None,
            url: None,
            limit: 8,
        };
        let owner = Owner {
            pid: std::process::id(),
            data_dir: "/data/held".into(),
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let held = match runtime
            .block_on(registry.take(&class, Some("held".into()), &owner, None))
            .unwrap()
        {
            Take::Granted { handle, .. } => handle,
            Take::Busy { .. } => panic!("an empty registry has a free slot"),
        };
        let stem = held.stem().to_owned();
        let text = std::fs::read(dir.join(format!("{stem}.entry"))).unwrap();
        Self {
            dir,
            stem,
            text,
            _held: held,
        }
    }

    /// `(a file that does not exist, the entry's text, its lock file, the writers' lock)`.
    pub fn paths(&self) -> [std::path::PathBuf; 4] {
        [
            self.dir.join("held-slot-forged.entry"),
            self.dir.join(format!("{}.entry", self.stem)),
            self.dir.join(format!("{}.lock", self.stem)),
            self.dir.join("registry.lock"),
        ]
    }

    /// What a tool that tried to create, change and remove all of them left: nothing forged, the
    /// entry's text as it was, its lock and the writers' lock there, and the entry still live.
    pub fn assert_untouched(&self) {
        let [forged, entry, lock, mutex] = self.paths();
        assert!(!forged.exists(), "an entry was forged");
        assert_eq!(
            std::fs::read(&entry).unwrap(),
            self.text,
            "an entry was changed"
        );
        assert!(lock.is_file() && mutex.is_file(), "a lock file was removed");
        let live = mimlys::registry::Registry::new(
            self.dir.clone(),
            mimlys::registry::Bounds::default(),
            std::sync::Arc::new(mimlys_core::clock::SystemClock),
        )
        .list()
        .unwrap()
        .live;
        assert!(
            live.iter().any(|s| s.stem == self.stem),
            "the entry is not live any more: {live:?}"
        );
    }
}

static REGISTRIES: AtomicU64 = AtomicU64::new(0);

/// A registry directory of its own, for one command (RFC 013): the user's runtimes share
/// `admission_slots`, so tests that run in parallel on one machine would wait for each other's
/// executions (and write to the user's real registry) if they used the platform's. A suite that
/// is about the registry names its own with `MIMLYS_RUN_DIR`.
pub fn run_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "registry-{}-{}",
        std::process::id(),
        REGISTRIES.fetch_add(1, Ordering::Relaxed)
    ))
}

/// A `mimlys` command alone in the world: its registry is its own ([`run_dir`]).
pub fn mimlys() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_mimlys"));
    cmd.env("MIMLYS_RUN_DIR", run_dir());
    cmd
}

/// The test harness's runtime (`mimlys-testloop`), with a registry of its own.
#[cfg(feature = "test-bins")]
pub fn mimlys_testloop() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_mimlys-testloop"));
    cmd.env("MIMLYS_RUN_DIR", run_dir());
    cmd
}

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
    // The user's registry (RFC 013) is shared by every runtime of the user, so that a run that
    // reaches it waits for the slots of the runs beside it (limit 2): a test that runs a runtime
    // names a registry of its own. `Command::env_clear` drops the one that `mimlys()` and
    // `mimlys_testloop()` set, and a clean environment must give it back (handoff 015b-3, item 3).
    let is_runtime = Path::new(cmd.get_program())
        .file_stem()
        .is_some_and(|name| name == "mimlys" || name == "mimlys-testloop");
    assert!(
        !is_runtime
            || cmd
                .get_envs()
                .any(|(key, value)| key == "MIMLYS_RUN_DIR" && value.is_some()),
        "this run of {:?} names no MIMLYS_RUN_DIR: it would use the user's own registry, shared with every other run (give it `common::run_dir()` after any `env_clear`)",
        cmd.get_program()
    );
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
