//! The process-wide panic hook of `mimlys` and `mimlys-worker` (REQ-SEC-011).
//!
//! Rust's default hook prints the panic message and its location on standard
//! error, unredacted: a model's text or a configured secret that reached a
//! panic message would leave the process there. This hook replaces it. It
//! writes **one** line, JSON like the log's lines, whose message passes
//! through the same bounding and redaction as the runtime's own log line of a
//! contained panic (`mimlys_core::panic_text::bounded_message`): redacted,
//! invisible and control characters removed, one line, at most LIM-I5
//! characters; no stack trace. The location is the panic's file from its last
//! `src/` component on (the file name when there is none) and the line,
//! `src/runtime/loop_.rs:412`: never the absolute path, which carries the
//! build machine's home directory.
//!
//! Until the configuration has loaded the configured secrets are not known,
//! so before [`set_secrets`] the hook can only bound the message and strip
//! control characters; it cannot redact. A panic that early cannot hold a
//! configured secret the process has not read yet; the line says nothing of
//! one. `mimlys-worker` never learns the secrets (it has none) and stays in
//! that mode.
//!
//! A panic the runtime contains and logs itself (a model backend or a tool
//! provider) is not written here again: the hook is silent while
//! `mimlys_core::panic_text::inside_containment` says so.

use mimlys_core::context::Redactor;
use mimlys_core::panic_text::{bounded_message, inside_containment, payload_text};
use std::io::Write;
use std::sync::OnceLock;

#[cfg(test)]
mod tests;

static REDACTOR: OnceLock<Redactor> = OnceLock::new();

/// Installs the hook for `binary` (`mimlys` or `mimlys-worker`). Call it first
/// thing in `main`, after `harden_process`. A hook that itself failed would
/// abort the process, so it only formats and does one write whose error it
/// ignores.
pub fn install(binary: &'static str) {
    std::panic::set_hook(Box::new(move |info| {
        if inside_containment() {
            return;
        }
        let thread = std::thread::current();
        let text = payload_text(info.payload());
        let location = info.location().map(|l| (l.file(), l.line()));
        let line = line(binary, thread.name(), &text, location, REDACTOR.get());
        let _ = writeln!(std::io::stderr().lock(), "{line}");
    }));
}

/// From now on the hook redacts these configured secrets (`(name, value)`
/// pairs, as the log's redactor takes them). Once: a second call is ignored.
pub fn set_secrets(secrets: &[(String, String)]) {
    let _ = REDACTOR.set(Redactor::new(secrets));
}

/// A source file as the hook prints it: from the last `src` component on,
/// with `/` as the separator, or the file name when there is no `src`
/// component. Either separator splits (a Windows build's path has `\\`).
pub fn crate_relative(file: &str) -> String {
    let parts: Vec<&str> = file.split(['/', '\\']).filter(|p| !p.is_empty()).collect();
    match parts.iter().rposition(|p| *p == "src") {
        Some(i) if i + 1 < parts.len() => parts[i..].join("/"),
        _ => parts.last().copied().unwrap_or("").to_owned(),
    }
}

/// The hook's line for a panic with `message` in `thread` at `location`
/// (file and line): one JSON object, the shape of the log's lines.
pub fn line(
    binary: &str,
    thread: Option<&str>,
    message: &str,
    location: Option<(&str, u32)>,
    redactor: Option<&Redactor>,
) -> String {
    let location = location
        .map(|(file, line)| bounded_message(&format!("{}:{line}", crate_relative(file)), redactor));
    serde_json::json!({
        "level": "ERROR",
        "fields": {
            "message": "a panic ended its thread and was not contained",
            "binary": binary,
            "thread": bounded_message(thread.unwrap_or("unnamed"), redactor),
            "location": location,
            "panic_message": bounded_message(message, redactor),
        },
    })
    .to_string()
}
