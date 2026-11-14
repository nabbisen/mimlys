//! The worker's phase trace: one line on standard error per phase of an
//! execution (received, set up, returned, record sent), written only when
//! the worker is started with `--trace`, which the runtime passes when its
//! own log admits `debug` for the worker. The runtime forwards the lines at
//! `debug`; they carry identifiers, phases and durations, never content.

use std::sync::atomic::{AtomicBool, Ordering};

static ENABLED: AtomicBool = AtomicBool::new(false);

/// The prefix of a trace line, by which the runtime tells it from the worker's other output.
pub const PREFIX: &str = "worker: trace: ";

/// The prefix of a warning line. The worker has no log of its own: a line
/// with this prefix on its standard error is logged by the runtime at `warn`
/// (target `mimlys_worker`), always, trace or not.
pub const WARN_PREFIX: &str = "worker: warn: ";

/// Writes one warning line for the runtime to log at `warn`. `text` is a
/// fixed word and numbers, never content.
pub fn warn(text: &str) {
    eprintln!("{WARN_PREFIX}{text}");
}

/// How the runtime logs one line of the worker's standard error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineKind<'a> {
    /// A phase line of the trace, at `debug`.
    Trace(&'a str),
    /// A warning, at `warn`.
    Warn(&'a str),
    /// Anything else, at `info`.
    Other(&'a str),
}

/// Classifies one line of the worker's standard error by its prefix.
pub fn classify(line: &str) -> LineKind<'_> {
    if let Some(t) = line.strip_prefix(PREFIX) {
        LineKind::Trace(t)
    } else if let Some(t) = line.strip_prefix(WARN_PREFIX) {
        LineKind::Warn(t)
    } else {
        LineKind::Other(line)
    }
}

/// Turns the trace on for this process.
pub fn enable() {
    ENABLED.store(true, Ordering::Relaxed);
}

/// Whether the trace is on.
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Writes one trace line when the trace is on; `text` is built only then.
pub fn line(text: impl FnOnce() -> String) {
    if enabled() {
        eprintln!("{PREFIX}{}", text());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// REQ-CONF-009: the runtime tells a warning line of the worker from a
    /// trace line and from anything else by its prefix.
    #[test]
    fn req_conf_009_a_warning_line_is_told_from_the_rest() {
        assert_eq!(
            classify("worker: warn: tree_kill_incomplete: 1 process(es) remained"),
            LineKind::Warn("tree_kill_incomplete: 1 process(es) remained")
        );
        assert_eq!(
            classify("worker: trace: execution x: record sent"),
            LineKind::Trace("execution x: record sent")
        );
        assert_eq!(
            classify("worker: protocol error"),
            LineKind::Other("worker: protocol error")
        );
    }
}
