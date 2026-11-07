//! The hook's line (REQ-SEC-011): bounded, redacted, one line. The hook as
//! installed in the binaries is tested through them
//! (`crates/mimlys/tests/panic_hook.rs`).

use super::*;
use mimlys_core::limits::LIM_I5_MESSAGE_CHARS;

fn secrets() -> Redactor {
    Redactor::new(&[("API_KEY".into(), "hunter2-the-secret-value".into())])
}

/// REQ-SEC-011: the configured secret is redacted, the text is one line, and
/// it is cut at LIM-I5 characters.
#[test]
fn req_sec_011_the_hook_line_is_redacted_and_bounded() {
    let message = format!(
        "boom hunter2-the-secret-value\nsecond line\u{202e} {}",
        "x".repeat(1000)
    );
    let line = line(
        "mimlys",
        Some("main"),
        &message,
        Some((
            "/home/u/work/mimlys/crates/mimlys-core/src/runtime/loop_.rs",
            412,
        )),
        Some(&secrets()),
    );
    assert_eq!(line.lines().count(), 1, "{line}");
    assert!(!line.contains("hunter2"), "{line}");
    let value: serde_json::Value = serde_json::from_str(&line).unwrap();
    let text = value["fields"]["panic_message"].as_str().unwrap();
    assert!(
        text.starts_with("boom [redacted:API_KEY] second line"),
        "{text}"
    );
    assert!(text.chars().count() <= LIM_I5_MESSAGE_CHARS);
    assert!(!text.contains('\u{202e}') && !text.contains('\n'));
    assert_eq!(value["fields"]["binary"], "mimlys");
    assert_eq!(value["fields"]["thread"], "main");
    assert!(!line.contains("backtrace"), "{line}");
    assert_eq!(value["fields"]["location"], "src/runtime/loop_.rs:412");
    assert!(!line.contains("/home/") && !line.contains("work"), "{line}");
}

/// REQ-SEC-011: before the configuration loaded the hook cannot redact, and
/// says nothing it could not stand behind: it still bounds and strips.
#[test]
fn req_sec_011_the_hook_line_before_the_secrets_are_known_still_bounds_and_strips() {
    let message = format!("early\u{7}\u{202e}\nline {}", "y".repeat(1000));
    let line = line("mimlys-worker", None, &message, None, None);
    let value: serde_json::Value = serde_json::from_str(&line).unwrap();
    let text = value["fields"]["panic_message"].as_str().unwrap();
    assert!(text.starts_with("early line "), "{text}");
    assert!(text.chars().count() <= LIM_I5_MESSAGE_CHARS);
    assert_eq!(value["fields"]["thread"], "unnamed");
    assert!(value["fields"]["location"].is_null(), "{line}");
}

/// REQ-SEC-011: the location is crate-relative: from the last `src`
/// component on, whatever the separator, and never an absolute path.
#[test]
fn req_sec_011_the_location_is_crate_relative() {
    for (file, expected) in [
        ("crates/mimlys/src/main.rs", "src/main.rs"),
        (
            "/home/u/work/mimlys/crates/mimlys-exec/src/worker_main.rs",
            "src/worker_main.rs",
        ),
        (
            "/home/u/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/tokio-1.0.0/src/runtime/task/harness.rs",
            "src/runtime/task/harness.rs",
        ),
        (
            r"D:\a\mimlys\mimlys\crates\mimlys\src\main.rs",
            "src/main.rs",
        ),
        (
            r"C:\Users\me\.cargo\registry\src\x\a-1.0\src\lib.rs",
            "src/lib.rs",
        ),
        ("/rustc/abc123/library/core/lib.rs", "lib.rs"),
        ("build.rs", "build.rs"),
        ("/home/u/src", "src"),
        ("", ""),
    ] {
        assert_eq!(crate_relative(file), expected, "{file}");
    }
}
