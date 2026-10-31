//! The worker's phase trace: one line on standard error per phase of an
//! execution (received, set up, returned, record sent), written only when
//! the worker is started with `--trace`, which the runtime passes when its
//! own log admits `debug` for the worker. The runtime forwards the lines at
//! `debug`; they carry identifiers, phases and durations, never content.

use std::sync::atomic::{AtomicBool, Ordering};

static ENABLED: AtomicBool = AtomicBool::new(false);

/// The prefix of a trace line, by which the runtime tells it from the worker's other output.
pub const PREFIX: &str = "worker: trace: ";

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
