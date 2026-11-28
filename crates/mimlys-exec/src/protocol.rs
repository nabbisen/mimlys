//! The runtime ↔ worker protocol (external design §13.1): length-prefixed
//! JSON messages over the worker's standard input and output. M1 fixes
//! the message types; M2 implements the worker (RFC 005 fixes the exact
//! schema and the framing).

use mimlys_core::action::{EffectView, ExecutionMetrics, Termination};
use mimlys_core::ids::{ActionId, ExecutionId};
use mimlys_core::observation::Notice;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The protocol version carried by every message. Version 2 (handoff 004d-4): a captured
/// stream travels as base64 text in the record, at about a byte a byte, and the largest frame
/// follows the bounds in force ([`frame_limit`]); version 1 wrote the streams as JSON arrays
/// of numbers, three to four characters a byte.
pub const PROTOCOL_VERSION: u32 = 2;

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
        /// Standard output, as base64 text on the wire.
        #[serde(with = "stream_bytes")]
        stdout: Vec<u8>,
        /// Standard error, as base64 text on the wire.
        #[serde(with = "stream_bytes")]
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

/// The framing is the crate's one (`frame`), shared with the daemon's protocol: the worker's
/// version and limit are given to it here.
pub use crate::frame::{Frame, FrameError};

/// What a record holds besides its two streams (the payload, the notices, the metrics), and what
/// a request holds: room for a script's source and the arguments of a call.
pub const RECORD_MARGIN_BYTES: usize = 64 * 1024;

/// The length of `raw` bytes as padded base64 text.
pub const fn encoded_len(raw: usize) -> usize {
    raw.div_ceil(3) * 4
}

/// The largest frame either side accepts, from the bounds in force (ED §13.1): the larger of a
/// stream's two bounds (`output_bytes`, LIM-S4 as configured, and LIM-H4), encoded, twice (both
/// streams), plus the record's margin. Both sides call it with the same value, which the runtime
/// passes to the worker as `--output-bytes`; the value is held within LIM-S4's own range, so that
/// a wrong flag can neither make a frame too small for a bound in force nor ask for an
/// unbounded allocation.
pub fn frame_limit(output_bytes: usize) -> usize {
    let (low, high) = mimlys_core::limits::LIM_S4_OUTPUT_BYTES_RANGE;
    let stream = (output_bytes.clamp(low as usize, high as usize))
        .max(mimlys_core::limits::LIM_H4_HOST_OUTPUT_BYTES);
    2 * encoded_len(stream) + RECORD_MARGIN_BYTES
}

/// A captured stream on the wire: standard base64 text (RFC 4648, padded), strict on reading.
mod stream_bytes {
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;
    use serde::{Deserialize, Deserializer, Serializer, de::Error};

    pub fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&STANDARD.encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        let text = String::deserialize(deserializer)?;
        STANDARD
            .decode(text.as_bytes())
            .map_err(|e| D::Error::custom(format!("a stream is not base64: {e}")))
    }
}

/// Writes one frame of the worker's protocol ([`PROTOCOL_VERSION`]): a 4-byte big-endian length,
/// then the JSON body. A body above `limit` is not written.
pub fn write_frame<T: Serialize>(
    out: &mut impl std::io::Write,
    body: &T,
    limit: usize,
) -> Result<(), FrameError> {
    crate::frame::write_frame_versioned(out, PROTOCOL_VERSION, body, limit)
}

/// Reads one frame of at most `limit` bytes and checks that it is of the worker's protocol
/// ([`PROTOCOL_VERSION`]); another version is `Version` whatever the body holds.
pub fn read_frame<T: serde::de::DeserializeOwned>(
    input: &mut impl std::io::Read,
    limit: usize,
) -> Result<T, FrameError> {
    crate::frame::read_frame_versioned(input, PROTOCOL_VERSION, limit)
}

#[cfg(test)]
mod tests;
