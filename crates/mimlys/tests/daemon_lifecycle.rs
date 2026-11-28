//! The daemon's lifecycle and its socket (RFC 014, Amendment 1; external design §19.5;
//! REQ-CLI-008, REQ-SEC-016, REQ-PERS-014): `mimlys daemon start | stop | status`, the socket's
//! modes, the store's lock held by the daemon, the idle exit, a dead daemon.
//!
//! Each test environment owns one registry (and so one socket, or one pipe's name), one data
//! directory and its own daemon, which the environment stops when it is dropped. The tests of a
//! socket's modes, its directory and its path are Unix's; those of a pipe's user, on Windows, are in
//! the same file beside them.

use serde_json::Value;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

mod common;
use common::run_bounded;

struct Env {
    registry: common::Registry,
    data: tempfile::TempDir,
    config: PathBuf,
}

impl Env {
    fn new() -> Self {
        let data = tempfile::tempdir().unwrap();
        let script = data.path().join("script.json");
        std::fs::write(&script, r#"[{"decision": "complete", "summary": "done"}]"#).unwrap();
        let config = data.path().join("config.toml");
        std::fs::write(
            &config,
            format!(
                "config_version = 1\n[runtime]\ndata_dir = {:?}\n[backend]\nkind = \"fake\"\nscript = {:?}\n",
                data.path().join("data").to_string_lossy(),
                script.to_string_lossy(),
            ),
        )
        .unwrap();
        Self {
            registry: common::Registry::new(),
            data,
            config,
        }
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut cmd = common::mimlys_in(&self.registry);
        cmd.arg("--config")
            .arg(&self.config)
            .args(args)
            .env("MIMLYS_CONFIG", "")
            .env("MIMLYS_WORKER_BIN", env!("CARGO_BIN_EXE_mimlys-worker"));
        cmd
    }

    fn run(&self, args: &[&str]) -> common::Bounded {
        run_bounded(&mut self.command(args), self.data.path())
    }

    fn run_with(&self, args: &[&str], envs: &[(&str, &str)]) -> common::Bounded {
        let mut cmd = self.command(args);
        for (k, v) in envs {
            cmd.env(k, v);
        }
        run_bounded(&mut cmd, self.data.path())
    }

    /// `daemon status --json`'s daemon object.
    fn status(&self) -> Value {
        let out = self.run(&["--json", "daemon", "status"]);
        assert_eq!(out.code(), 0, "{}\n{}", out.stdout, out.stderr);
        serde_json::from_str::<Value>(out.stdout.trim()).unwrap()["daemon"].clone()
    }

    fn socket_dir(&self) -> PathBuf {
        self.registry.path()
    }

    /// What the daemon leaves in the registry's directory while it serves: its socket (Unix), and,
    /// on Windows, where a pipe is no file, its record.
    fn sockets(&self) -> Vec<PathBuf> {
        let extension = if cfg!(unix) { "sock" } else { "daemon" };
        std::fs::read_dir(self.socket_dir())
            .map(|d| {
                d.flatten()
                    .map(|e| e.path())
                    .filter(|p| p.extension().is_some_and(|e| e == extension))
                    .collect()
            })
            .unwrap_or_default()
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        // A daemon a test left is stopped; one that does not stop is killed by its pid.
        let status = self.run(&["--json", "daemon", "status"]);
        let pid = serde_json::from_str::<Value>(status.stdout.trim())
            .ok()
            .and_then(|v| v["daemon"]["pid"].as_u64());
        let _ = self.run(&["daemon", "stop"]);
        if let Some(pid) = pid.and_then(|p| u32::try_from(p).ok()) {
            std::thread::sleep(Duration::from_millis(200));
            common::kill_pid(pid);
        }
    }
}

fn wait_for(limit: Duration, mut done: impl FnMut() -> bool) -> bool {
    let started = std::time::Instant::now();
    while started.elapsed() < limit {
        if done() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    done()
}

/// REQ-CLI-008: `daemon start` detaches and returns when the daemon answers; `status` says what
/// it is; `stop` ends it, with nothing to pause; then no daemon runs and its socket and record are
/// gone.
#[test]
fn req_cli_008_start_status_stop() {
    let env = Env::new();
    let none = env.status();
    assert_eq!(none["running"], false, "{none}");

    let started = env.run(&["--json", "daemon", "start"]);
    assert_eq!(started.code(), 0, "{}\n{}", started.stdout, started.stderr);
    let started: Value = serde_json::from_str(started.stdout.trim()).unwrap();
    assert_eq!(started["daemon"]["running"], true);
    assert_eq!(started["daemon"]["already_running"], false);
    let pid = started["daemon"]["pid"].as_u64().unwrap();

    let status = env.status();
    assert_eq!(status["running"], true);
    assert_eq!(status["pid"].as_u64().unwrap(), pid);
    assert_eq!(status["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(status["same_configuration"], true);
    assert_eq!(status["running_tasks"], serde_json::json!([]));
    if cfg!(unix) {
        assert!(status["socket"].as_str().unwrap().ends_with(".sock"));
    } else {
        // A pipe, named from the user's SID and the data directory (external design §19.5).
        assert!(
            status["socket"]
                .as_str()
                .unwrap()
                .starts_with(r"\\.\pipe\mimlys-d-"),
            "{status}"
        );
    }
    // Its log is a file under the data directory.
    let log = PathBuf::from(status["log"].as_str().unwrap());
    assert!(log.starts_with(env.data.path().join("data")), "{log:?}");
    assert!(log.is_file());

    // Started again, it is the same daemon.
    let again = env.run(&["--json", "daemon", "start"]);
    let again: Value = serde_json::from_str(again.stdout.trim()).unwrap();
    assert_eq!(again["daemon"]["already_running"], true);
    assert_eq!(again["daemon"]["pid"].as_u64().unwrap(), pid);

    let stopped = env.run(&["--json", "daemon", "stop"]);
    assert_eq!(stopped.code(), 0, "{}\n{}", stopped.stdout, stopped.stderr);
    let stopped: Value = serde_json::from_str(stopped.stdout.trim()).unwrap();
    assert_eq!(stopped["daemon"]["pid"].as_u64().unwrap(), pid);
    assert_eq!(stopped["daemon"]["paused"], serde_json::json!([]));
    assert!(env.sockets().is_empty(), "{:?}", env.sockets());
    assert_eq!(env.status()["running"], false);
    // The daemon's record in the registry went with it.
    let records: Vec<_> = std::fs::read_dir(env.socket_dir())
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("daemon-"))
        .collect();
    assert!(records.is_empty(), "{records:?}");
}

/// REQ-SEC-016: the socket is of mode 0600, in a directory of mode 0700; the daemon is recorded
/// in the registry by a file an older release does not read as an entry.
#[test]
fn req_sec_016_the_socket_is_0600_in_a_0700_directory_and_the_daemon_is_recorded() {
    let env = Env::new();
    assert_eq!(env.run(&["daemon", "start"]).code(), 0);
    let sockets = env.sockets();
    assert_eq!(sockets.len(), 1, "{sockets:?}");
    #[cfg(unix)]
    {
        let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&sockets[0]), 0o600);
        assert_eq!(mode(&env.socket_dir()), 0o700);
        let name = sockets[0]
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert!(name.starts_with("d-") && name.len() == 19, "{name}");
    }
    // The record: `daemon-<id>.daemon` with its lock file; no `.entry`, which an older release
    // would call unusable and remove.
    let mut names: Vec<String> = std::fs::read_dir(env.socket_dir())
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert!(
        names
            .iter()
            .any(|n| n.starts_with("daemon-") && n.ends_with(".daemon")),
        "{names:?}"
    );
    assert!(
        names
            .iter()
            .any(|n| n.starts_with("daemon-") && n.ends_with(".lock")),
        "{names:?}"
    );
    assert!(!names.iter().any(|n| n.ends_with(".entry")), "{names:?}");
}

/// REQ-PERS-014: the daemon holds the store's lock for its life; a command that writes and does
/// not go through it is refused, naming the daemon and its process.
#[test]
fn req_pers_014_a_command_that_writes_beside_the_daemon_is_refused_naming_it() {
    let env = Env::new();
    let started = env.run(&["--json", "daemon", "start"]);
    let pid = serde_json::from_str::<Value>(started.stdout.trim()).unwrap()["daemon"]["pid"]
        .as_u64()
        .unwrap();
    // `toolchain grant` writes the store's record of a permission, which only the process that holds
    // the store may do: it is not a request the daemon serves (`model probe` is, since 014c-5).
    let refused = env.run(&[
        "toolchain",
        "grant",
        "/toolchain-that-is-not-there",
        "--yes",
    ]);
    assert_eq!(refused.code(), 1, "{}\n{}", refused.stdout, refused.stderr);
    assert!(
        refused.stderr.contains("the daemon") && refused.stderr.contains(&format!("pid {pid}")),
        "{}",
        refused.stderr
    );
    // A second daemon of the same store says who holds it, and exits.
    let second = env.run(&["daemon", "start", "--foreground"]);
    assert_eq!(second.code(), 1, "{}\n{}", second.stdout, second.stderr);
    assert!(
        second.stderr.contains(&format!("pid {pid}")),
        "{}",
        second.stderr
    );
}

/// REQ-CLI-008, LIM-O13: a daemon with no client and no task running, queued or waiting for the
/// user exits after its idle time (the test shortens it with the debug seam), removing its socket.
#[test]
fn req_cli_008_an_idle_daemon_exits_by_itself_and_says_so_in_its_log() {
    let env = Env::new();
    let started = env.run_with(
        &["--json", "daemon", "start"],
        // Long enough for `start` to greet the daemon before it idles out: on a loaded runner
        // (macOS x86_64 under Rosetta, `main`'s run 46119145949) 400 ms passed before the first
        // greeting, and `start` found a daemon that had ended: "its process ended with code 0".
        &[("MIMLYS_DAEMON_IDLE_MS", "4000")],
    );
    assert_eq!(started.code(), 0, "{}\n{}", started.stdout, started.stderr);
    let log = PathBuf::from(env.status()["log"].as_str().unwrap_or_default());
    assert!(
        wait_for(Duration::from_secs(60), || env.sockets().is_empty()),
        "the daemon did not exit"
    );
    assert_eq!(env.status()["running"], false);
    let text = std::fs::read_to_string(&log).unwrap();
    assert!(text.contains("nothing to do for its idle time"), "{text}");
}

/// REQ-CLI-008, external design §20: a socket whose daemon is dead, with the store's lock free, is
/// removed and said once (`daemon not running`), and the command goes on.
#[cfg(unix)]
#[test]
fn req_cli_008_a_dead_daemons_socket_is_removed_and_said_once() {
    let env = Env::new();
    let started = env.run(&["--json", "daemon", "start"]);
    let pid = serde_json::from_str::<Value>(started.stdout.trim()).unwrap()["daemon"]["pid"]
        .as_i64()
        .unwrap();
    // Killed: nothing closed its socket.
    let pid = u32::try_from(pid).unwrap();
    common::kill_pid(pid);
    assert!(wait_for(Duration::from_secs(30), || !common::pid_alive(
        pid
    )));
    assert_eq!(env.sockets().len(), 1, "the socket is left");
    let first = env.run(&["daemon", "status"]);
    assert_eq!(first.code(), 0, "{}\n{}", first.stdout, first.stderr);
    assert_eq!(
        first.stderr.matches("daemon not running").count(),
        1,
        "{}",
        first.stderr
    );
    assert!(env.sockets().is_empty(), "the stale socket was removed");
    // Once only: the next command finds nothing to say.
    let second = env.run(&["daemon", "status"]);
    assert!(
        !second.stderr.contains("daemon not running"),
        "{}",
        second.stderr
    );
}

/// REQ-SEC-016, external design §20: a directory that is not the user's own, mode 0700, is
/// refused before anything is sent: a registry directory of mode 0755.
#[cfg(unix)]
#[test]
fn req_sec_016_a_client_refuses_a_directory_that_is_not_0700() {
    let env = Env::new();
    assert_eq!(env.run(&["daemon", "start"]).code(), 0);
    let dir = env.socket_dir();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    let refused = env.run(&["daemon", "status"]);
    assert_eq!(refused.code(), 1, "{}\n{}", refused.stdout, refused.stderr);
    assert!(
        refused.stderr.contains("mode 0755") && refused.stderr.contains("nothing was sent"),
        "{}",
        refused.stderr
    );
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
}

/// The uid the test makes "another user" of (the debug seam of `MIMLYS_PRETEND_UID`: CI has no
/// second account).
#[cfg(unix)]
fn other_uid() -> String {
    (rustix::process::geteuid().as_raw() + 1).to_string()
}

/// The user a peer is made "another user" of on Windows: a SID that is not the user's own (the seam
/// of `MIMLYS_PRETEND_SID`; CI has no second account).
#[cfg(windows)]
fn other_user() -> (&'static str, String) {
    ("MIMLYS_PRETEND_SID", "S-1-5-21-1-2-3-4".to_owned())
}

#[cfg(unix)]
fn other_user() -> (&'static str, String) {
    ("MIMLYS_PRETEND_UID", other_uid())
}

/// REQ-SEC-016, THR-39, external design §20: the check is mutual. **The daemon refuses a peer of
/// another user** before anything is sent or read: a daemon that takes its own user for another
/// than the client's closes the connection, says so in its log, and answers nothing.
#[test]
fn req_sec_016_the_daemon_refuses_a_peer_of_another_user() {
    let env = Env::new();
    let mut daemon = env
        .command(&["daemon", "start", "--foreground"])
        .env(other_user().0, other_user().1)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    assert!(wait_for(Duration::from_secs(120), || !env
        .sockets()
        .is_empty()));
    let refused = env.run(&["daemon", "status"]);
    let log =
        std::fs::read_to_string(env.data.path().join("data/logs/daemon.log")).unwrap_or_default();
    let _ = daemon.kill();
    let _ = daemon.wait();
    assert_eq!(refused.code(), 1, "{}\n{}", refused.stdout, refused.stderr);
    assert!(
        refused.stderr.contains("did not greet"),
        "nothing was said to it: {}",
        refused.stderr
    );
    assert!(log.contains("belongs to another user"), "{log}");
}

/// REQ-SEC-016, THR-39: **the client sends nothing to a daemon of another user**: it reads the
/// daemon's credentials first, and a process that is not the user's own gets no hello.
#[test]
fn req_sec_016_the_client_refuses_a_daemon_of_another_user() {
    let env = Env::new();
    assert_eq!(env.run(&["daemon", "start"]).code(), 0);
    let refused = env.run_with(&["daemon", "status"], &[(other_user().0, &other_user().1)]);
    assert_eq!(refused.code(), 1, "{}\n{}", refused.stdout, refused.stderr);
    assert!(
        refused.stderr.contains("another user") && refused.stderr.contains("nothing was sent"),
        "{}",
        refused.stderr
    );
    // The daemon saw no hello from it: its log has no accepted client from that attempt.
    assert_eq!(env.status()["running"], true);
}

/// REQ-SEC-016: a socket path past the platform's limit is refused, saying why, and not truncated.
#[cfg(unix)]
#[test]
fn req_sec_016_a_socket_path_past_the_limit_is_refused() {
    let env = Env::new();
    let long = env.data.path().join("r".repeat(120));
    let mut cmd = env.command(&["daemon", "start"]);
    cmd.env("MIMLYS_RUN_DIR", &long);
    let refused = run_bounded(&mut cmd, env.data.path());
    assert_eq!(refused.code(), 1, "{}\n{}", refused.stdout, refused.stderr);
    assert!(
        refused.stderr.contains("past the") && refused.stderr.contains("never shortened"),
        "{}",
        refused.stderr
    );
}

/// ED §19.5 (014d): **a daemon that was killed leaves no pipe**, so there is nothing stale to remove
/// and nothing to say: the next command finds no daemon, goes on (the store's lock went with the
/// process), and `daemon status` says that none runs. (On Unix the same case leaves a socket,
/// which is removed and said once: the test above.)
#[cfg(windows)]
#[test]
fn req_cli_008_a_killed_daemon_leaves_no_pipe_and_the_next_command_goes_on() {
    let env = Env::new();
    let started = env.run(&["--json", "daemon", "start"]);
    let pid = serde_json::from_str::<Value>(started.stdout.trim()).unwrap()["daemon"]["pid"]
        .as_u64()
        .unwrap();
    let pid = u32::try_from(pid).unwrap();
    common::kill_pid(pid);
    assert!(wait_for(Duration::from_secs(30), || !common::pid_alive(
        pid
    )));
    let first = env.run(&["daemon", "status"]);
    assert_eq!(first.code(), 0, "{}\n{}", first.stdout, first.stderr);
    assert!(
        !first.stderr.contains("daemon not running"),
        "there is no stale pipe to say anything of: {}",
        first.stderr
    );
    assert_eq!(env.status()["running"], false);
    // Embedded again: the store is free, and a command that writes goes on.
    let scope = env.data.path().join("scope");
    std::fs::create_dir_all(&scope).unwrap();
    let ran = env.run(&["run", "goal", "--scope", scope.to_str().unwrap()]);
    assert_eq!(ran.code(), 0, "{}\n{}", ran.stdout, ran.stderr);
}
