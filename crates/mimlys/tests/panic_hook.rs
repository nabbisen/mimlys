//! The panic hook of the binaries (REQ-SEC-011): a panic that ends the
//! process writes one line to standard error, bounded and with the configured
//! secrets redacted, never the default hook's text, location or note.
//!
//! The panics are injected by debug builds only (`cargo test` builds them): the
//! variable `MIMLYS_DEBUG_PANIC_AFTER_CONFIG` for `mimlys`, after the
//! configuration loaded, and the argument `--debug-panic` for the worker.

#![cfg(debug_assertions)]

use mimlys_core::limits::LIM_I5_MESSAGE_CHARS;
use std::process::Command;

mod common;
use common::run_bounded;

const SECRET: &str = "hunter2-the-secret-value";

fn assert_one_hook_line(stderr: &str) -> serde_json::Value {
    let lines: Vec<&str> = stderr.lines().collect();
    assert_eq!(lines.len(), 1, "exactly one line:\n{stderr}");
    assert!(
        !stderr.contains("panicked at") && !stderr.contains("RUST_BACKTRACE"),
        "not the default hook's text:\n{stderr}"
    );
    serde_json::from_str(lines[0]).unwrap_or_else(|e| panic!("one JSON line ({e}):\n{stderr}"))
}

/// The line's `location` is `src/…:<line>`: no home directory, no drive
/// letter, no backslash.
fn assert_crate_relative_location(line: &serde_json::Value, file: &str) {
    let location = line["fields"]["location"].as_str().expect("a location");
    let (path, number) = location.rsplit_once(':').expect("file:line");
    assert_eq!(path, file, "{location}");
    assert!(number.parse::<u32>().is_ok_and(|n| n > 0), "{location}");
    assert!(
        !location.contains("/home/") && !location.contains('\\'),
        "{location}"
    );
    let bytes = location.as_bytes();
    assert!(
        !(bytes.len() > 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'),
        "no drive letter: {location}"
    );
}

/// REQ-SEC-011: a panic after the configuration loaded: the secret is
/// redacted, the control and bidirectional characters are gone, the text is
/// one line of at most LIM-I5 characters, and the process ends (not 0).
#[test]
fn req_sec_011_the_hook_redacts_and_bounds_a_panic_after_the_configuration_loaded() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config.toml");
    std::fs::write(
        &config,
        format!(
            "config_version = 1\n[runtime]\ndata_dir = {:?}\n[backend]\nkind = \"openai_compatible\"\nurl = \"http://127.0.0.1:9/v1\"\nmodel = \"m\"\napi_key = \"env:MIMLYS_TEST_KEY\"\n",
            dir.path().join("data").to_string_lossy()
        ),
    )
    .unwrap();
    let message = format!("boom {SECRET}\nsecond line\u{202e} {}", "x".repeat(1000));
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_mimlys"));
    cmd.arg("--config")
        .arg(&config)
        .arg("check")
        .env("MIMLYS_CONFIG", "")
        .env("MIMLYS_TEST_KEY", SECRET)
        .env("MIMLYS_DEBUG_PANIC_AFTER_CONFIG", &message);
    let out = run_bounded(&mut cmd, dir.path());
    assert_ne!(out.code(), 0, "{}", out.stderr);
    assert!(!out.stderr.contains(SECRET), "{}", out.stderr);
    let line = assert_one_hook_line(&out.stderr);
    let text = line["fields"]["panic_message"].as_str().unwrap();
    assert!(
        text.starts_with("boom [redacted:backend.api_key] second line"),
        "{text}"
    );
    assert!(text.chars().count() <= LIM_I5_MESSAGE_CHARS, "{text}");
    assert!(!text.contains('\n') && !text.contains('\u{202e}'), "{text}");
    assert_eq!(line["fields"]["binary"], "mimlys");
    assert_crate_relative_location(&line, "src/main.rs");
}

/// REQ-SEC-011: the worker's hook: it has no secrets to redact, and still
/// bounds the message and strips control characters, in one line.
#[test]
fn req_sec_011_the_worker_hook_bounds_and_strips_a_panic() {
    let dir = tempfile::tempdir().unwrap();
    let message = format!("worker boom\u{7}\nsecond {}", "y".repeat(1000));
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_mimlys-worker"));
    cmd.arg("--debug-panic").arg(&message).env_clear();
    let out = run_bounded(&mut cmd, dir.path());
    assert_ne!(out.code(), 0, "{}", out.stderr);
    let line = assert_one_hook_line(&out.stderr);
    let text = line["fields"]["panic_message"].as_str().unwrap();
    assert!(text.starts_with("worker boom second "), "{text}");
    assert!(text.chars().count() <= LIM_I5_MESSAGE_CHARS, "{text}");
    assert_eq!(line["fields"]["binary"], "mimlys-worker");
    assert_crate_relative_location(&line, "src/worker_main.rs");
}
