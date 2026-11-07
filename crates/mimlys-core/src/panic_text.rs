//! The text of a panic, made fit for one log line (REQ-SEC-011). The
//! runtime's containment and the panic hook of the two binaries use the same
//! functions, so a panic's message is bounded and redacted in one way. Pure
//! text: nothing is written here (`mimlys-core` has no I/O).

use crate::context::Redactor;
use crate::limits::LIM_I5_MESSAGE_CHARS;
use std::any::Any;
use std::cell::Cell;

/// The message of a panic payload; a payload that is not text says so.
pub fn payload_text(payload: &(dyn Any + Send)) -> String {
    if let Some(text) = payload.downcast_ref::<&'static str>() {
        (*text).to_owned()
    } else if let Some(text) = payload.downcast_ref::<String>() {
        text.clone()
    } else {
        "a panic with a payload that is not text".to_owned()
    }
}

/// A panic message for a log line: configured secrets redacted (when a
/// redactor is known), invisible and control characters removed, whitespace
/// collapsed to single spaces, and the length cut at LIM-I5. It is one line
/// and never a stack trace.
pub fn bounded_message(message: &str, redactor: Option<&Redactor>) -> String {
    let redacted = match redactor {
        Some(redactor) => redactor.redact(message).0,
        None => message.to_owned(),
    };
    let (clean, _) = crate::observation::sanitize(&redacted);
    let clean = clean.split_whitespace().collect::<Vec<_>>().join(" ");
    if clean.chars().count() > LIM_I5_MESSAGE_CHARS {
        clean
            .chars()
            .take(LIM_I5_MESSAGE_CHARS - 1)
            .collect::<String>()
            + "…"
    } else {
        clean
    }
}

thread_local! {
    static INSIDE: Cell<u32> = const { Cell::new(0) };
}

/// Marks the current thread as inside a contained region while it lives. A
/// panic there is caught and logged by the containment, once, with the
/// adapter's name; the process-wide hook stays silent for it.
pub(crate) struct InsideContainment(());

impl InsideContainment {
    pub(crate) fn enter() -> Self {
        let _ = INSIDE.try_with(|depth| depth.set(depth.get() + 1));
        Self(())
    }
}

impl Drop for InsideContainment {
    fn drop(&mut self) {
        let _ = INSIDE.try_with(|depth| depth.set(depth.get().saturating_sub(1)));
    }
}

/// Is the current thread inside a region where the runtime contains and logs
/// panics itself? The panic hook of a binary asks this and says nothing then.
pub fn inside_containment() -> bool {
    INSIDE.try_with(|depth| depth.get() > 0).unwrap_or(false)
}
