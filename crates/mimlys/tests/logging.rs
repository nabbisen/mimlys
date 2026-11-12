//! The log on standard error (external design §17.3). What is verified here:
//! the lines are JSON objects with a timestamp and a level; at the default
//! level no prompt content and no file content reaches standard error; the
//! warning of `logging.debug_prompts` is the first thing written at start, and
//! only when the switch is on. What is NOT verified, because the code does not
//! do it: that a line carries a task id, an action id and an event name (no
//! `tracing` call site in the workspace carries them; see the review request
//! 002b-untested-1, finding 1). For that reason these tests do not name the
//! logging requirement (the "Logs" requirement of the OBS family, requirement
//! 004): it stays listed by `scripts/check-req-tests.sh` until the architect
//! settles the disagreement.

use serde_json::Value;
use std::process::Command;

mod common;
use common::run_bounded;

const GOAL: &str = "GOALMARKER-7731 rename the function";
const FILE: &str = "FILEMARKER-5519";
const WARNING: &str = "logging.debug_prompts is on";

/// A fake-backend run that reads one file whose content is `FILE`, under the
/// given `[logging]` section; returns standard error. A configured MCP server
/// whose command does not exist makes the runtime log one warning at the start
/// of the run (`server ghost unavailable`), so that there is always a line of
/// the runtime's own to look at.
fn run_with_logging(logging: &str) -> String {
    let dir = tempfile::tempdir().unwrap();
    let scope = dir.path().join("scope");
    std::fs::create_dir_all(&scope).unwrap();
    std::fs::write(scope.join("a.txt"), FILE).unwrap();
    let script = dir.path().join("script.json");
    std::fs::write(
        &script,
        r#"[{"decision":"act","tool":"read_file","arguments":{"path":"a.txt"}},{"decision":"complete","summary":"done"}]"#,
    )
    .unwrap();
    let config = dir.path().join("config.toml");
    std::fs::write(
        &config,
        format!(
            "config_version = 1\n[runtime]\ndata_dir = {:?}\n[backend]\nkind = \"fake\"\nscript = {:?}\n{logging}\n[servers.ghost]\ncommand = {:?}\n",
            dir.path().join("data").to_string_lossy(),
            script.to_string_lossy(),
            dir.path().join("no-such-server").to_string_lossy()
        ),
    )
    .unwrap();
    let out = run_bounded(
        Command::new(env!("CARGO_BIN_EXE_mimlys"))
            .args(["--config"])
            .arg(&config)
            .args(["--json", "run", GOAL, "--scope"])
            .arg(&scope)
            .env("MIMLYS_CONFIG", "")
            .env("MIMLYS_WORKER_BIN", env!("CARGO_BIN_EXE_mimlys-worker")),
        dir.path(),
    );
    assert_eq!(out.code(), 0, "{}\n{}", out.stdout, out.stderr);
    assert!(out.stdout.contains("\"completed\""), "{}", out.stdout);
    out.stderr
}

/// At the default level the warning is a JSON line with a timestamp and a level
/// (`WARN`), nothing of the goal or of the file read reaches standard error,
/// and the debug-prompts warning is not written.
#[test]
fn the_default_log_is_json_and_holds_no_prompt_and_no_file_content() {
    let stderr = run_with_logging("");
    let line = stderr.lines().next().expect("the warning is logged");
    let value: Value = serde_json::from_str(line).unwrap_or_else(|e| panic!("{e}: {line}"));
    assert_eq!(value["level"], "WARN", "{line}");
    assert!(value["timestamp"].is_string(), "{line}");
    assert!(line.contains("ghost"), "{line}");
    assert!(!stderr.contains("GOALMARKER"), "{stderr}");
    assert!(!stderr.contains("FILEMARKER"), "{stderr}");
    assert!(!stderr.contains(WARNING), "{stderr}");
}

/// `debug_prompts = true` writes the warning first, as a plain line; every
/// line after it is a JSON object with a timestamp and a level.
#[test]
fn the_debug_prompts_warning_comes_first_and_the_lines_are_json() {
    let stderr = run_with_logging("[logging]\nlevel = \"debug\"\ndebug_prompts = true\n");
    let mut lines = stderr.lines();
    let first = lines.next().expect("standard error is not empty");
    assert!(first.contains(WARNING), "{first}");
    let mut logged = 0;
    for line in lines.filter(|l| !l.trim().is_empty()) {
        let value: Value =
            serde_json::from_str(line).unwrap_or_else(|e| panic!("not JSON ({e}): {line}"));
        assert!(value["timestamp"].is_string(), "{line}");
        assert!(value["level"].is_string(), "{line}");
        logged += 1;
    }
    assert!(logged > 0, "the debug level logged nothing:\n{stderr}");
}
