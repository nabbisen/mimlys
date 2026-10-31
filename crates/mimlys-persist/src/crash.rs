//! Crash injection for recovery tests (REQ-TEST-007). With the feature
//! `crash-injection`, the environment variable `MIMLYS_CRASH_AT` names a
//! point (`<line>.before` or `<line>.after` a commit) at which the process
//! aborts. Without the feature every call is a no-op.

/// The persistence points at which a crash can be injected, in the order
/// of external design §16.2 (plus task creation and blobs).
pub const POINTS: [&str; 16] = [
    "create_task",
    "step_started",
    "proposed",
    "validation",
    "approval_request",
    "approval_decision",
    "authorized",
    "running",
    "execution",
    "action_terminal",
    "step_outcome",
    "task_transition",
    "blob",
    "snapshot",
    "promotion",
    "delete",
];

/// Aborts the process when `MIMLYS_CRASH_AT` equals `"{point}.{phase}"`.
#[cfg(feature = "crash-injection")]
pub fn maybe(point: &str, phase: &str) {
    use std::sync::OnceLock;
    static TARGET: OnceLock<Option<String>> = OnceLock::new();
    let target = TARGET.get_or_init(|| std::env::var("MIMLYS_CRASH_AT").ok());
    if let Some(t) = target
        && t == &format!("{point}.{phase}")
    {
        std::process::abort();
    }
}

/// No-op without the feature.
#[cfg(not(feature = "crash-injection"))]
pub fn maybe(_point: &str, _phase: &str) {}
