//! On-demand start (handoff 014d, item 3; RFC 014, Amendment 1; external design §19.5, §20;
//! REQ-CLI-008): with `[runtime] serve = "on_demand"` the first command that finds no daemon starts
//! one and goes through it; two at once leave one, the store's lock deciding; a start that fails is
//! said and runs nothing embedded; `manual` (the default) never starts one. On every platform.

use serde_json::Value;
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
    /// A store with a fake backend that completes whatever it is asked, and `serve` as given.
    fn new(serve: Option<&str>) -> Self {
        let data = tempfile::tempdir().unwrap();
        let script = data.path().join("script.json");
        std::fs::write(
            &script,
            r#"[{"decision": "complete", "summary": "done"}, {"decision": "complete", "summary": "done"}, {"decision": "complete", "summary": "done"}, {"decision": "complete", "summary": "done"}]"#,
        )
        .unwrap();
        let serve = serve.map_or_else(String::new, |s| format!("serve = {s:?}\n"));
        let config = data.path().join("config.toml");
        std::fs::write(
            &config,
            format!(
                "config_version = 1\n[runtime]\ndata_dir = {:?}\n{serve}[backend]\nkind = \"fake\"\nscript = {:?}\n",
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

    fn command(&self, args: &[&str], envs: &[(&str, &str)]) -> Command {
        let mut cmd = common::mimlys_in(&self.registry);
        cmd.arg("--config")
            .arg(&self.config)
            .args(args)
            .env("MIMLYS_CONFIG", "")
            .env("MIMLYS_WORKER_BIN", env!("CARGO_BIN_EXE_mimlys-worker"))
            // A daemon a test leaves ends by itself (the debug seam of LIM-O13).
            .env("MIMLYS_DAEMON_IDLE_MS", "60000");
        for (k, v) in envs {
            cmd.env(k, v);
        }
        cmd
    }

    fn run(&self, args: &[&str]) -> common::Bounded {
        self.run_with(args, &[])
    }

    fn run_with(&self, args: &[&str], envs: &[(&str, &str)]) -> common::Bounded {
        run_bounded(&mut self.command(args, envs), self.data.path())
    }

    fn scope(&self, name: &str) -> String {
        let dir = self.data.path().join(name);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/lib.rs"), "fn a() {}\n").unwrap();
        dir.to_string_lossy().into_owned()
    }

    /// `daemon status --json`'s daemon object.
    fn status(&self) -> Value {
        let out = self.run(&["--json", "daemon", "status"]);
        assert_eq!(out.code(), 0, "{}\n{}", out.stdout, out.stderr);
        serde_json::from_str::<Value>(out.stdout.trim()).unwrap()["daemon"].clone()
    }

    /// The ids of the tasks, as `task list` says them (it reads the store, and starts nothing).
    fn tasks(&self) -> Vec<String> {
        let out = self.run(&["--json", "task", "list"]);
        assert_eq!(out.code(), 0, "{}\n{}", out.stdout, out.stderr);
        serde_json::from_str::<Value>(out.stdout.trim())
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["task_id"].as_str().unwrap().to_owned())
            .collect()
    }

    /// The daemons recorded in the registry's directory, by pid.
    fn recorded_daemons(&self) -> Vec<u64> {
        std::fs::read_dir(self.registry.path())
            .map(|d| {
                d.flatten()
                    .filter(|e| e.path().extension().is_some_and(|x| x == "daemon"))
                    .filter_map(|e| std::fs::read_to_string(e.path()).ok())
                    .filter_map(|t| serde_json::from_str::<Value>(&t).ok())
                    .filter_map(|v| v["pid"].as_u64())
                    .collect()
            })
            .unwrap_or_default()
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let out = self.run(&["--json", "daemon", "status"]);
        let pid = serde_json::from_str::<Value>(out.stdout.trim())
            .ok()
            .and_then(|v| v["daemon"]["pid"].as_u64())
            .and_then(|p| u32::try_from(p).ok());
        let _ = self.run(&["daemon", "stop"]);
        if let Some(pid) = pid {
            std::thread::sleep(Duration::from_millis(300));
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
        std::thread::sleep(Duration::from_millis(100));
    }
    done()
}

/// The pid a command says it started a daemon with: the word after `(pid ` in its standard error.
fn started_pid(stderr: &str) -> Option<u64> {
    let line = stderr
        .lines()
        .find(|l| l.contains("started a daemon for this data directory (pid "))?;
    let after = line.split("(pid ").nth(1)?;
    after
        .split(|c: char| !c.is_ascii_digit())
        .next()?
        .parse()
        .ok()
}

/// REQ-CLI-008, external design §19.5: **the first command that finds no daemon starts one**, says so
/// once with its pid, and sends its request through it; **the second uses that daemon** and says
/// nothing of starting one; the daemon outlives the command that started it. A command that only
/// reads starts nothing.
#[test]
fn req_cli_008_with_on_demand_the_first_client_starts_a_daemon_and_the_second_uses_it() {
    let env = Env::new(Some("on_demand"));
    // Readers start nothing, `daemon status` neither.
    assert_eq!(env.tasks().len(), 0);
    assert_eq!(env.run(&["status"]).code(), 0);
    assert_eq!(env.status()["running"], false);
    assert!(env.recorded_daemons().is_empty());

    let first_scope = env.scope("first");
    let first = env.run(&["--json", "run", "goal", "--detach", "--scope", &first_scope]);
    assert_eq!(first.code(), 0, "{}\n{}", first.stdout, first.stderr);
    let pid = started_pid(&first.stderr).unwrap_or_else(|| panic!("{}", first.stderr));
    assert_eq!(
        first.stderr.matches("started a daemon").count(),
        1,
        "{}",
        first.stderr
    );
    // The daemon outlives the command: it runs, is the one the command named, and has the task.
    let status = env.status();
    assert_eq!(status["running"], true, "{status}");
    assert_eq!(status["pid"].as_u64(), Some(pid), "{status}");

    let second_scope = env.scope("second");
    let second = env.run(&[
        "--json",
        "run",
        "goal",
        "--detach",
        "--scope",
        &second_scope,
    ]);
    assert_eq!(second.code(), 0, "{}\n{}", second.stdout, second.stderr);
    assert!(
        !second.stderr.contains("started a daemon"),
        "the second command used the daemon: {}",
        second.stderr
    );
    assert_eq!(env.status()["pid"].as_u64(), Some(pid));
    assert_eq!(env.recorded_daemons(), vec![pid]);
    assert!(
        wait_for(Duration::from_secs(60), || env.tasks().len() == 2),
        "both tasks were made through the daemon"
    );
}

/// REQ-CLI-008, external design §19.5: **two commands at once leave one daemon**: each starts one
/// (the test makes them start at the same instant), the store's lock decides, the second daemon
/// ends naming the first, and its command goes through the first. Exactly one says that it started
/// a daemon, and both of their tasks exist. (`mutation: the store's lock not taken`)
#[test]
fn req_cli_008_two_clients_at_once_leave_one_daemon_and_both_go_through_it() {
    let env = Env::new(Some("on_demand"));
    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis()
        + 4000;
    let at = at.to_string();
    let scopes = [env.scope("one"), env.scope("two")];
    let results: Vec<common::Bounded> = std::thread::scope(|s| {
        let handles: Vec<_> = scopes
            .iter()
            .map(|scope| {
                let (env, at) = (&env, &at);
                s.spawn(move || {
                    env.run_with(
                        &["--json", "run", "goal", "--detach", "--scope", scope],
                        &[("MIMLYS_ON_DEMAND_SPAWN_AT_MS", at.as_str())],
                    )
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    for r in &results {
        assert_eq!(r.code(), 0, "{}\n{}", r.stdout, r.stderr);
    }
    let said: Vec<_> = results
        .iter()
        .filter_map(|r| started_pid(&r.stderr))
        .collect();
    assert_eq!(
        said.len(),
        1,
        "exactly one command started the daemon: {:?}",
        results
            .iter()
            .map(|r| r.stderr.as_str())
            .collect::<Vec<_>>()
    );
    let daemons = env.recorded_daemons();
    assert_eq!(daemons, vec![said[0]], "one daemon, the one that was said");
    assert_eq!(env.status()["pid"].as_u64(), Some(said[0]));
    assert!(
        wait_for(Duration::from_secs(60), || env.tasks().len() == 2),
        "both commands' tasks exist"
    );
}

/// REQ-CLI-008, external design §19.5: **the store's lock decides between two daemons**, and the
/// second ends: a client that began before the first daemon was there (it saw the store free) starts
/// its own daemon while the first runs. That daemon finds the store held and ends; its client goes
/// through the first and says nothing of having started one; one daemon is left, beyond the second
/// one's own end. (`mutation: the store's lock not taken`)
#[test]
fn req_cli_008_a_second_daemon_started_beside_the_first_ends_and_its_client_uses_the_first() {
    let env = Env::new(Some("on_demand"));
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis();
    let (first_at, second_at) = ((now + 4000).to_string(), (now + 9000).to_string());
    let scopes = [env.scope("one"), env.scope("two")];
    let results: Vec<common::Bounded> = std::thread::scope(|s| {
        let handles: Vec<_> = scopes
            .iter()
            .zip([&first_at, &second_at])
            .map(|(scope, at)| {
                let env = &env;
                s.spawn(move || {
                    env.run_with(
                        &["--json", "run", "goal", "--detach", "--scope", scope],
                        &[("MIMLYS_ON_DEMAND_SPAWN_AT_MS", at.as_str())],
                    )
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    for r in &results {
        assert_eq!(r.code(), 0, "{}\n{}", r.stdout, r.stderr);
    }
    let first = started_pid(&results[0].stderr).expect("the first command started the daemon");
    assert!(
        started_pid(&results[1].stderr).is_none(),
        "the second used the first's daemon: {}",
        results[1].stderr
    );
    // The second daemon (it was started; the lock refused it) is given the time it needs to end or
    // to show itself: a negative has no state to wait for, so the wait is bounded and generous.
    std::thread::sleep(Duration::from_secs(5));
    assert_eq!(env.recorded_daemons(), vec![first], "one daemon is left");
    assert!(wait_for(Duration::from_secs(60), || env.tasks().len() == 2));
}

/// REQ-CLI-008, external design §19.5, §20: **a start that fails is said, and the command does not
/// run embedded**: it exits 1 with the daemon's own reason, no task was made (an embedded run
/// would have made one), and no daemon runs. (`mutation: on-demand falling back to embedded after
/// a failed start`)
#[test]
fn req_cli_008_a_start_that_fails_is_said_and_runs_nothing_embedded() {
    let env = Env::new(Some("on_demand"));
    let scope = env.scope("work");
    let failed = env.run_with(
        &["run", "goal", "--scope", &scope],
        &[(
            "MIMLYS_DAEMON_START_FAILS",
            "there is no room for a daemon here",
        )],
    );
    assert_eq!(failed.code(), 1, "{}\n{}", failed.stdout, failed.stderr);
    assert!(
        failed.stderr.contains("the daemon did not start")
            && failed.stderr.contains("there is no room for a daemon here"),
        "{}",
        failed.stderr
    );
    assert!(
        env.tasks().is_empty(),
        "nothing ran embedded: {:?}",
        env.tasks()
    );
    assert_eq!(env.status()["running"], false);
}

/// REQ-CLI-008: **`manual` (the default) never starts a daemon**: without one, `--detach` is refused
/// (it needs a daemon), and a command that runs tasks runs embedded; none is started by either.
#[test]
fn req_cli_008_manual_never_starts_a_daemon() {
    let env = Env::new(None);
    let scope = env.scope("work");
    let refused = env.run(&["run", "goal", "--detach", "--scope", &scope]);
    assert_eq!(refused.code(), 1, "{}\n{}", refused.stdout, refused.stderr);
    assert!(refused.stderr.contains("daemon"), "{}", refused.stderr);
    let embedded = env.run(&["--json", "run", "goal", "--scope", &scope]);
    assert_eq!(
        embedded.code(),
        0,
        "{}\n{}",
        embedded.stdout,
        embedded.stderr
    );
    assert!(!embedded.stderr.contains("started a daemon"));
    assert_eq!(env.status()["running"], false);
    assert!(env.recorded_daemons().is_empty());
    // The word `manual` written out is the same.
    let written = Env::new(Some("manual"));
    let scope = written.scope("work");
    assert_eq!(
        written
            .run(&["--json", "run", "goal", "--scope", &scope])
            .code(),
        0
    );
    assert_eq!(written.status()["running"], false);
}

/// An unknown word of `[runtime] serve` is an error of the configuration, naming the key.
#[test]
fn ed_19_1_an_unknown_serve_word_is_a_configuration_error() {
    let env = Env::new(Some("sometimes"));
    let out = env.run(&["status"]);
    assert_ne!(out.code(), 0, "{}\n{}", out.stdout, out.stderr);
    assert!(
        out.stderr.contains("sometimes") && out.stderr.contains("on_demand"),
        "the word and the words it may be are named: {}",
        out.stderr
    );
}
