//! The runtime ↔ worker protocol (external design §13.1): length-prefixed
//! JSON messages over the worker's standard input and output. M1 fixes
//! the message types; M2 implements the worker (RFC 005 fixes the exact
//! schema and the framing).

use mimlys_core::action::{EffectView, ExecutionMetrics, Termination};
use mimlys_core::ids::{ActionId, ExecutionId};
use mimlys_core::observation::Notice;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The protocol version carried by every message.
pub const PROTOCOL_VERSION: u32 = 1;

/// A request from the runtime to the worker.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "request", rename_all = "snake_case")]
pub enum Request {
    /// Which confinement clauses the worker can enforce here.
    Probe,
    /// Run an execution.
    Execute {
        /// The execution.
        execution_id: ExecutionId,
        /// The action.
        action_id: ActionId,
        /// The tool.
        tool: String,
        /// Canonical arguments.
        arguments: Value,
        /// The effect in force.
        effect: EffectView,
        /// Deadline in milliseconds.
        deadline_ms: i64,
        /// Absolute path of the workspace tree.
        workspace_root: String,
        /// Whether the workspace is preopened read-only.
        read_only: bool,
        /// Replay mode: a fixed clock and random source (REQ-SBX-008).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        replay: Option<Replay>,
    },
    /// Cancel an execution.
    Cancel {
        /// The execution.
        execution_id: ExecutionId,
    },
    /// Shut the worker down.
    Shutdown,
}

/// How a confinement clause stands on this machine (external design §13.4).
/// Replay settings (REQ-SBX-008, REQ-TEST-016).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Replay {
    /// Bytes cycled by the random source.
    pub seed: Vec<u8>,
    /// The wall clock's fixed value, milliseconds since the epoch.
    pub clock_ms: u64,
}

/// What the probe found for one clause: the core's vocabulary, so that the
/// worker reports in the words the status prints (RFC 011 §1).
pub use mimlys_core::confine::ClauseState;

/// A response from the worker.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "response", rename_all = "snake_case")]
pub enum Response {
    /// The probe result: clauses F, N, E, K, P, M, C by letter.
    ProbeResult {
        /// Clause states by letter (`F`, `N`, `E`, `K`, `P`, `M`, `C`), as the worker observes them.
        clauses: std::collections::BTreeMap<String, ClauseState>,
        /// The pinned engine's version.
        engine_version: String,
        /// Interpreters the worker loaded, by language, with the module's hash.
        interpreters: std::collections::BTreeMap<String, String>,
        /// Interpreters it could not load, by language, with the reason (REQ-SBX-012).
        #[serde(default)]
        interpreter_errors: std::collections::BTreeMap<String, String>,
        /// The worker's own cgroup, for the record and the cleanup (RFC 011 §9).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cgroup: Option<std::path::PathBuf>,
    },
    /// An execution finished.
    ExecutionRecord {
        /// The execution.
        execution_id: ExecutionId,
        /// How it ended.
        termination: Termination,
        /// The structured result.
        payload: Value,
        /// Standard output.
        stdout: Vec<u8>,
        /// Standard error.
        stderr: Vec<u8>,
        /// Notices.
        notices: Vec<Notice>,
        /// Measurements.
        metrics: ExecutionMetrics,
    },
    /// The request could not be served.
    Error {
        /// A fixed-vocabulary detail.
        detail: String,
    },
}

/// One framed message: the protocol version and the body.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Frame<T> {
    /// Protocol version.
    pub version: u32,
    /// The body.
    #[serde(flatten)]
    pub body: T,
}

/// The largest frame either side accepts: both captured streams plus the record (ED §13.1).
pub const MAX_FRAME_BYTES: usize = 2 * crate::sandbox::LIM_S4_BYTES + 64 * 1024;

/// A framing failure.
#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    /// The peer closed the stream.
    #[error("end of stream")]
    Eof,
    /// A frame above `MAX_FRAME_BYTES`.
    #[error("frame of {len} bytes exceeds the limit")]
    TooLarge {
        /// Its declared length.
        len: usize,
    },
    /// The body is not the expected JSON.
    #[error("malformed frame: {0}")]
    Malformed(String),
    /// The peer speaks another protocol version.
    #[error("protocol version {0} is not {PROTOCOL_VERSION}")]
    Version(u32),
    /// The stream failed.
    #[error("stream: {0}")]
    Io(#[from] std::io::Error),
}

/// Writes one frame: a 4-byte big-endian length, then the JSON body.
pub fn write_frame<T: Serialize>(
    out: &mut impl std::io::Write,
    body: &T,
) -> Result<(), FrameError> {
    let frame = Frame {
        version: PROTOCOL_VERSION,
        body,
    };
    let bytes = serde_json::to_vec(&frame).map_err(|e| FrameError::Malformed(e.to_string()))?;
    if bytes.len() > MAX_FRAME_BYTES {
        return Err(FrameError::TooLarge { len: bytes.len() });
    }
    out.write_all(&(bytes.len() as u32).to_be_bytes())?;
    out.write_all(&bytes)?;
    out.flush()?;
    Ok(())
}

/// Reads one frame and checks its version.
pub fn read_frame<T: serde::de::DeserializeOwned>(
    input: &mut impl std::io::Read,
) -> Result<T, FrameError> {
    let mut len = [0u8; 4];
    match input.read_exact(&mut len) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Err(FrameError::Eof),
        Err(e) => return Err(e.into()),
    }
    let len = u32::from_be_bytes(len) as usize;
    if len > MAX_FRAME_BYTES {
        return Err(FrameError::TooLarge { len });
    }
    let mut bytes = vec![0u8; len];
    input.read_exact(&mut bytes)?;
    let frame: Frame<T> =
        serde_json::from_slice(&bytes).map_err(|e| FrameError::Malformed(e.to_string()))?;
    if frame.version != PROTOCOL_VERSION {
        return Err(FrameError::Version(frame.version));
    }
    Ok(frame.body)
}

#[cfg(test)]
mod tests;
