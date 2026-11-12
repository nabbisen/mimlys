//! Structured logs on standard error (REQ-OBS-004, REQ-PRIV-004): JSON
//! lines with timestamp, level and event. A line written while a task's step
//! runs also carries the step span's fields, `task_id`, `step_id` and, once
//! there is an action, `action_id`, as `span` (the current span only: the
//! step is the one span there is). Prompt content, file content and secrets
//! never reach the log at the default level; `debug_prompts` is the only way
//! to log prompts and replies (at `debug`, on the prompt target, which is
//! switched off here unless the option is on) and it warns at start. Every
//! line passes through the redactor built from the configured secrets
//! (REQ-CTX-007).

use crate::config::LoggingSection;
use mimlys_core::context::Redactor;
use mimlys_core::runtime::PROMPT_LOG_TARGET;
use std::io::Write;
use std::sync::Arc;
use tracing_subscriber::EnvFilter;

/// Installs the subscriber once. Returns the warning to print when prompts are logged.
pub fn init(section: &LoggingSection, secrets: &[(String, String)]) -> Option<&'static str> {
    // The lines that carry a prompt or a reply have a target of their own,
    // off unless `debug_prompts` is on: nothing else decides it.
    let filter = if section.debug_prompts {
        EnvFilter::new(section.level.as_str())
    } else {
        EnvFilter::new(format!("{},{PROMPT_LOG_TARGET}=off", section.level))
    };
    let redactor = Arc::new(Redactor::new(secrets));
    let _ = tracing_subscriber::fmt()
        .json()
        .with_env_filter(filter)
        .with_writer(move || RedactingStderr {
            redactor: Arc::clone(&redactor),
            buffer: Vec::new(),
        })
        .with_target(false)
        .with_span_list(false)
        .try_init();
    section
        .debug_prompts
        .then_some(crate::messages::DEBUG_PROMPTS_WARNING)
}

/// Standard error with every written line redacted.
struct RedactingStderr {
    redactor: Arc<Redactor>,
    buffer: Vec<u8>,
}

impl Write for RedactingStderr {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.buffer.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if self.buffer.is_empty() {
            return Ok(());
        }
        let text = String::from_utf8_lossy(&self.buffer).into_owned();
        self.buffer.clear();
        let (clean, _) = self.redactor.redact(&text);
        let mut err = std::io::stderr().lock();
        err.write_all(clean.as_bytes())?;
        err.flush()
    }
}

impl Drop for RedactingStderr {
    fn drop(&mut self) {
        let _ = self.flush();
    }
}
