//! The fake backend (REQ-MDL-002, REQ-TEST-002): scripted responses from a
//! file or from memory, with every request recorded for assertions.
//!
//! Script format: a JSON array of entries. Each entry is either a decision
//! object (returned as its compact JSON), or an object with `raw` (bytes to
//! return verbatim, to test malformed output), optionally `delay_ms`, or
//! `fail` from `unavailable`, `timeout`.

use mimlys_core::spi::{
    BackendDescriptor, BackendError, BoxFuture, Cancellation, GenerateRequest, Locality,
    ModelBackend,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::VecDeque;
use std::path::Path;
use std::sync::Mutex;

/// A control entry: return `raw` verbatim after `delay_ms`, or fail.
/// Unknown fields are refused so that a decision object never matches it.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControlEntry {
    /// Bytes to return verbatim.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw: Option<String>,
    /// Delay before answering, in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delay_ms: Option<u64>,
    /// A failure to simulate: `unavailable` or `timeout`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fail: Option<String>,
}

/// One scripted response.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ScriptEntry {
    /// A control entry.
    Control(ControlEntry),
    /// A decision object returned as compact JSON.
    Decision(Value),
}

/// A backend that answers from a script and records requests.
#[derive(Debug)]
pub struct FakeBackend {
    script: Mutex<VecDeque<ScriptEntry>>,
    requests: Mutex<Vec<GenerateRequest>>,
}

impl FakeBackend {
    /// A backend with the given entries.
    pub fn new(entries: Vec<ScriptEntry>) -> Self {
        Self {
            script: Mutex::new(entries.into()),
            requests: Mutex::new(Vec::new()),
        }
    }

    /// A backend that returns these decisions in order.
    pub fn with_decisions(decisions: Vec<Value>) -> Self {
        Self::new(decisions.into_iter().map(ScriptEntry::Decision).collect())
    }

    /// Reads a script file.
    pub fn from_script(path: &Path) -> std::io::Result<Self> {
        let text = std::fs::read_to_string(path)?;
        let entries: Vec<ScriptEntry> =
            serde_json::from_str(&text).map_err(std::io::Error::other)?;
        Ok(Self::new(entries))
    }

    /// Every request received so far.
    pub fn requests(&self) -> Vec<GenerateRequest> {
        self.requests.lock().expect("fake lock").clone()
    }

    /// Entries still queued.
    pub fn remaining(&self) -> usize {
        self.script.lock().expect("fake lock").len()
    }
}

impl ModelBackend for FakeBackend {
    fn generate<'a>(
        &'a self,
        request: GenerateRequest,
        cancel: Cancellation,
    ) -> BoxFuture<'a, Result<Vec<u8>, BackendError>> {
        Box::pin(async move {
            let max_bytes = request.max_bytes;
            self.requests.lock().expect("fake lock").push(request);
            let entry = self
                .script
                .lock()
                .expect("fake lock")
                .pop_front()
                .ok_or(BackendError::ScriptExhausted)?;
            let (bytes, delay, fail) = match entry {
                ScriptEntry::Decision(value) => (
                    serde_json::to_vec(&value).expect("value serializes"),
                    None,
                    None,
                ),
                ScriptEntry::Control(ControlEntry {
                    raw,
                    delay_ms,
                    fail,
                }) => (raw.unwrap_or_default().into_bytes(), delay_ms, fail),
            };
            if let Some(ms) = delay {
                tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
            }
            if cancel.is_cancelled() {
                return Err(BackendError::Cancelled);
            }
            match fail.as_deref() {
                Some("unavailable") => {
                    return Err(BackendError::Unavailable {
                        detail: "scripted".into(),
                    });
                }
                Some("timeout") => return Err(BackendError::Timeout),
                _ => {}
            }
            // REQ-MDL-007: output is bounded while being read; the parser rejects the cut.
            let mut bytes = bytes;
            if bytes.len() > max_bytes.saturating_add(1) {
                bytes.truncate(max_bytes + 1);
            }
            Ok(bytes)
        })
    }

    fn describe(&self) -> BackendDescriptor {
        BackendDescriptor {
            kind: "fake".into(),
            model: None,
            locality: Locality::Local,
        }
    }
}

#[cfg(test)]
mod tests;
