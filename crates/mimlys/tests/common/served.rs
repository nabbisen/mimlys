//! The suite, served (handoff 014c, "Tests": the whole CLI suite also runs against a daemon).
//!
//! With `MIMLYS_TEST_SERVED=1` (every platform: a socket on Linux and macOS, a pipe on Windows), every `mimlys` command that `run_bounded`
//! runs for a test environment is served: the first one of an environment (its registry, so its
//! socket, and its configuration) starts a daemon for it, with its own data directory, and the
//! commands that follow go through it. The daemon is stopped when the environment ends (its
//! [`super::Registry`] is dropped; a solo command's own registry, when the command ends), and
//! ends by itself after a minute with nothing to do if a test left it. **The outputs a test
//! compares are the embedded ones, byte for byte**: nothing here changes what a command says.
//!
//! A test that cannot be served says why, by its name, as the skip tier does
//! ([`super::skip`]): the test then runs embedded, as it always did. Three kinds:
//! the **command** needs the store itself (`toolchain grant`), the **subject** is
//! embedded mode itself (the store's lock, the registry's slots between runtimes), or the test
//! runs its commands **outside** `run_bounded` (a pseudo-terminal, a process it signals), which
//! this hook never sees.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;

/// The variable that serves the suite.
pub const SERVED_ENV: &str = "MIMLYS_TEST_SERVED";

/// A daemon this process started for a test environment.
struct Started {
    run_dir: PathBuf,
    config: PathBuf,
    envs: Vec<(std::ffi::OsString, Option<std::ffi::OsString>)>,
}

static STARTED: Mutex<Vec<Started>> = Mutex::new(Vec::new());
static SKIPPED: Mutex<Option<HashSet<String>>> = Mutex::new(None);

/// Whether the suite is served.
pub fn enabled() -> bool {
    std::env::var(SERVED_ENV).is_ok_and(|v| v == "1")
}

fn current_test() -> String {
    std::thread::current()
        .name()
        .unwrap_or("unnamed")
        .to_owned()
}

/// Says once per test, by its name, that it is not served, and why.
fn not_served(why: &str) {
    let name = current_test();
    let mut seen = SKIPPED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if seen.get_or_insert_with(HashSet::new).insert(name.clone()) {
        super::skip(&name, &format!("not served: {why}"));
    }
}

/// The tests that are not served, by name, with why: their subject is embedded mode, or they run
/// commands by means this hook cannot see. Each is a test the suite already has; none is
/// skipped from the embedded run.
const NOT_SERVED: &[(&str, &str)] = &[
    // The subject is what the process that runs a task writes: under a daemon, that process is
    // the daemon, and its lines are its log's (`logs/daemon.log`), not the command's standard error.
    (
        "req_obs_004_a_line_during_a_step_has_the_ids_and_one_outside_has_none",
        "the lines of a step are written by the process that runs it: under a daemon, its log's",
    ),
    (
        "req_obs_004_a_host_tools_worker_line_carries_the_ids",
        "the lines of a step are written by the process that runs it: under a daemon, its log's",
    ),
    (
        "req_obs_004_a_servers_standard_error_line_during_a_call_carries_the_ids",
        "the lines of a step are written by the process that runs it: under a daemon, its log's",
    ),
    (
        "req_priv_004_with_the_prompt_option_on_prompts_and_replies_are_logged_redacted",
        "the lines of a step are written by the process that runs it: under a daemon, its log's",
    ),
    (
        "req_priv_004_with_the_prompt_option_on_the_level_does_not_matter",
        "the lines of a step are written by the process that runs it: under a daemon, its log's",
    ),
    // The subject is the end of an embedded process, or its start.
    (
        "req_sched_005_a_finished_run_leaves_no_worker_and_no_lock",
        "its subject is the end of the embedded process: a daemon keeps its worker and the store's lock",
    ),
    (
        "req_main_006_the_start_recovers_the_others_and_leaves_a_newer_task_as_it_is",
        "its subject is the recovery that an embedded runtime makes at its start",
    ),
    (
        "req_ws_007_a_restore_that_failed_at_start_can_be_restored_by_rollback",
        "its subject is the recovery that an embedded runtime makes at its start",
    ),
    (
        "req_sec_003_a_worker_with_a_short_report_is_refused_before_any_work",
        "the lines of a step are written by the process that runs it: under a daemon, its log's",
    ),
    (
        "req_sec_003_the_dependency_steps_worker_is_checked_too",
        "the lines of a step are written by the process that runs it: under a daemon, its log's",
    ),
    (
        "req_conf_009_a_survivor_of_the_termination_is_reported",
        "the lines of a step are written by the process that runs it: under a daemon, its log's",
    ),
    (
        "req_tool_006_a_slow_tree_kill_after_the_deadline_still_records_deadline",
        "the lines of a step are written by the process that runs it: under a daemon, its log's",
    ),
    (
        "req_tool_006_a_worker_ended_after_the_deadline_is_recorded_deadline",
        "the lines of a step are written by the process that runs it: under a daemon, its log's",
    ),
    // The subject is what the runtime's own process asks of the engine.
    (
        "req_mdl_003_run_reads_ollama_again_after_the_first_reply",
        "it counts the requests the runtime's own process makes to the engine: under a daemon the daemon makes them",
    ),
];

/// Whether two sets of environment changes are the same, in whatever order they were made.
fn same_env(
    a: &[(std::ffi::OsString, Option<std::ffi::OsString>)],
    b: &[(std::ffi::OsString, Option<std::ffi::OsString>)],
) -> bool {
    let sorted = |v: &[(std::ffi::OsString, Option<std::ffi::OsString>)]| {
        let mut v = v.to_vec();
        v.sort();
        v
    };
    sorted(a) == sorted(b)
}

fn arg(cmd: &Command, flag: &str) -> Option<PathBuf> {
    let args: Vec<_> = cmd.get_args().collect();
    args.windows(2)
        .find(|w| w[0] == flag)
        .map(|w| PathBuf::from(w[1]))
}

fn env_of(cmd: &Command, key: &str) -> Option<std::ffi::OsString> {
    cmd.get_envs()
        .find(|(k, _)| *k == key)
        .and_then(|(_, v)| v.map(ToOwned::to_owned))
}

/// The first word of the command line that is not an option: the subcommand.
fn subcommand(cmd: &Command) -> (Option<String>, Option<String>) {
    let mut words = Vec::new();
    let mut skip_next = false;
    for a in cmd.get_args() {
        let a = a.to_string_lossy().into_owned();
        if skip_next {
            skip_next = false;
            continue;
        }
        if a == "--config" {
            skip_next = true;
            continue;
        }
        if a.starts_with('-') {
            continue;
        }
        words.push(a);
    }
    (words.first().cloned(), words.get(1).cloned())
}

/// Whether the command only reads (the commands `main` routes to the reader: no daemon serves them).
fn is_reader(cmd: &Command, word: &str, second: Option<&str>) -> bool {
    match word {
        "status" | "events" | "metrics" => true,
        "task" => matches!(second, Some("show" | "list" | "output")),
        "toolchain" => second == Some("list"),
        "promote" => cmd.get_args().any(|a| a == "--patch"),
        _ => false,
    }
}

fn mimlys(
    args: &[&str],
    config: &Path,
    run_dir: &Path,
    envs: &[(std::ffi::OsString, Option<std::ffi::OsString>)],
) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_mimlys"));
    cmd.arg("--config").arg(config).args(args);
    // What the test's own command names wins over these.
    cmd.env("MIMLYS_WORKER_BIN", env!("CARGO_BIN_EXE_mimlys-worker"));
    for (k, v) in envs {
        match v {
            Some(v) => cmd.env(k, v),
            None => cmd.env_remove(k),
        };
    }
    cmd.env("MIMLYS_RUN_DIR", run_dir)
        .env("MIMLYS_CONFIG", "")
        .stdin(Stdio::null());
    cmd
}

/// Called before `run_bounded` runs `cmd`: starts the daemon of its environment when the command
/// is served, and stops it when the command cannot be.
pub fn before(cmd: &Command) {
    // The suites whose subject is the daemon start their own, and need no other beside it.
    let own_subject = std::env::current_exe().is_ok_and(|exe| {
        exe.file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with("daemon_"))
    });
    if own_subject
        || !enabled()
        || Path::new(cmd.get_program())
            .file_stem()
            .is_none_or(|s| s != "mimlys")
    {
        return;
    }
    let Some(run_dir) = env_of(cmd, "MIMLYS_RUN_DIR").map(PathBuf::from) else {
        return;
    };
    let config = arg(cmd, "--config").or_else(|| {
        env_of(cmd, "MIMLYS_CONFIG")
            .filter(|c| !c.is_empty())
            .map(PathBuf::from)
    });
    let Some(config) = config else { return };
    let (word, second) = subcommand(cmd);
    let word = word.unwrap_or_default();
    if matches!(word.as_str(), "daemon" | "check" | "runtime" | "") {
        return;
    }
    // A command that only reads opens the store as a reader, daemon or none (REQ-PERS-013), and
    // makes no daemon for its environment: a test that looks at what a reader creates, or at a
    // store no one has opened, would otherwise find the daemon's.
    if is_reader(cmd, &word, second.as_deref()) {
        return;
    }
    let test = current_test();
    if let Some((_, why)) = NOT_SERVED.iter().find(|(name, _)| *name == test) {
        not_served(why);
        stop(&run_dir, &config);
        return;
    }
    // `model probe` is a request the daemon runs (014c-5): it is served like the rest.
    if word == "toolchain" && matches!(second.as_deref(), Some("grant" | "revoke")) {
        not_served(&format!(
            "`{word}` needs the store itself, which the daemon holds"
        ));
        stop(&run_dir, &config);
        return;
    }
    let envs: Vec<_> = cmd
        .get_envs()
        .map(|(k, v)| (k.to_owned(), v.map(ToOwned::to_owned)))
        .collect();
    // The daemon runs with the environment of the command it serves: a test that changes it for
    // a later command (a knob that the runtime reads) has a daemon that was started with it.
    let already = STARTED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .any(|s| s.run_dir == run_dir && s.config == config && same_env(&s.envs, &envs));
    if already {
        record(&test, &word);
        return;
    }
    stop(&run_dir, &config);
    let mut start = mimlys(&["daemon", "start"], &config, &run_dir, &envs);
    // A daemon a test leaves ends by itself.
    start.env("MIMLYS_DAEMON_IDLE_MS", "60000");
    match start.output() {
        Ok(out) if out.status.success() => {
            record(&test, &word);
            STARTED
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(Started {
                    run_dir,
                    config,
                    envs,
                });
        }
        Ok(out) => not_served(&format!(
            "no daemon could be started here: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )),
        Err(e) => not_served(&format!("no daemon could be started here: {e}")),
    }
}

/// Appends one line to `served.log` in the target's temporary directory: this test sent this
/// command through a daemon. A review request counts the distinct tests of the file, and the
/// commands, to say how much of the suite ran served.
fn record(test: &str, word: &str) {
    use std::io::Write;
    let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join("served.log");
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = file.write_all(format!("{test} {word}\n").as_bytes());
    }
}

/// Stops the daemon this process started for the environment, if it did.
pub fn stop(run_dir: &Path, config: &Path) {
    let mut started = STARTED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(at) = started
        .iter()
        .position(|s| s.run_dir == run_dir && s.config == config)
    else {
        return;
    };
    let s = started.remove(at);
    drop(started);
    let _ = mimlys(&["daemon", "stop"], &s.config, &s.run_dir, &s.envs).output();
}

/// Stops every daemon this process started for the registry at `run_dir` (its environment ended).
pub fn stop_all_for(run_dir: &Path) {
    let configs: Vec<PathBuf> = STARTED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .filter(|s| s.run_dir == run_dir)
        .map(|s| s.config.clone())
        .collect();
    for config in configs {
        stop(run_dir, &config);
    }
}
