//! What a client and the daemon say to each other (external design §19.5; REQ-SEC-016).
//!
//! A connection opens with **two hellos and an answer**, then carries requests. Every frame is a
//! length-prefixed JSON object (`mimlys_exec::frame`, the worker's framing) whose `version` is
//! [`FRAME_VERSION`], and **that number never changes**, so that a client of any release can read
//! the daemon's hello, send its own and `stop`. The shapes of the three hello frames and of
//! `stop` and its answer are **fixed for every version** (the tests of this module fail when a
//! field is renamed or removed); the requests after them are the runtime's own, and a client and
//! a daemon of different versions never exchange them (the hello refuses first).

use mimlys_core::api::{Envelope, Notice, Response};
use mimlys_core::ids::TaskId;
use mimlys_core::limits::LIM_O22_DAEMON_FRAME_BYTES;
use mimlys_core::runtime::RuntimeError;
use mimlys_exec::frame::{FrameError, read_frame_versioned, write_frame_versioned};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

/// The protocol's name, the first thing a daemon says.
pub const PROTOCOL: &str = "mimlys-daemon";
/// The hello's version: the shape of the first three frames, never changed.
pub const HELLO_VERSION: u32 = 1;
/// The `version` every frame of the daemon's protocol carries, never changed.
pub const FRAME_VERSION: u32 = 1;
/// The largest frame either side accepts (LIM-O22).
pub const FRAME_LIMIT: usize = LIM_O22_DAEMON_FRAME_BYTES;

/// Writes one frame of the daemon's protocol.
pub fn write<T: Serialize>(out: &mut impl std::io::Write, body: &T) -> Result<(), FrameError> {
    write_frame_versioned(out, FRAME_VERSION, body, FRAME_LIMIT)
}

/// Reads one frame of the daemon's protocol.
pub fn read<T: serde::de::DeserializeOwned>(
    input: &mut impl std::io::Read,
) -> Result<T, FrameError> {
    read_frame_versioned(input, FRAME_VERSION, FRAME_LIMIT)
}

/// The first frame, from the daemon: who it is, what it runs and what it read, and a salt.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonHello {
    /// [`PROTOCOL`].
    pub protocol: String,
    /// [`HELLO_VERSION`].
    pub hello_version: u32,
    /// The runtime's version (`CARGO_PKG_VERSION`).
    pub runtime_version: String,
    /// The hash of the configuration file the daemon read at its start ([`config_hash`]).
    pub config_hash: String,
    /// A random salt, hex, new for every connection: the key of the secrets' hashes.
    pub salt: String,
}

/// The second frame, from the client, sent after it has checked the daemon.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientHello {
    /// The runtime's version of the client.
    pub runtime_version: String,
    /// The hash of the client's own configuration file.
    pub config_hash: String,
    /// For each `env:` reference of the client's configuration (its name, as the configuration
    /// names it), a keyed hash of its own value under the daemon's salt ([`keyed_hash`]). A value
    /// is never sent.
    pub secrets: BTreeMap<String, String>,
}

/// Why a daemon refuses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefusalKind {
    /// Another runtime version.
    Version,
    /// A configuration file that hashes differently (state-changing requests only).
    Configuration,
    /// A secret whose value differs (state-changing requests only).
    Secret,
    /// A peer of the daemon's own tree.
    OwnTree,
    /// The daemon is stopping.
    Stopping,
}

/// A refusal, as the daemon says it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Refusal {
    /// Why.
    pub kind: RefusalKind,
    /// The words, as external design §20 gives them.
    pub message: String,
    /// The reference a secret refusal names; never its value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
}

/// The third frame, from the daemon: whether the connection goes on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelloAnswer {
    /// The connection carries requests. When it does not, the only request the daemon reads is
    /// [`Request::Stop`], whatever the versions.
    pub accepted: bool,
    /// Why not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refusal: Option<Refusal>,
    /// Why state-changing requests of this connection are refused, when they are. Readers are not
    /// affected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub writes_refused: Option<Refusal>,
}

/// What a client asks after the hellos.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "request", rename_all = "snake_case")]
pub enum Request {
    /// **Fixed for every version.** Stop: running tasks pause at their boundary, the daemon lists
    /// them and exits. The one request a connection that was not accepted may make.
    Stop,
    /// A request of the control API (external design §4).
    Control {
        /// The envelope; the daemon replaces its session with the one it verified.
        envelope: Box<Envelope>,
    },
    /// A follower of the task begins: the driver takes it if it is running.
    Attach {
        /// The task.
        task_id: TaskId,
    },
    /// The error of the step that failed for this task, once.
    Failure {
        /// The task.
        task_id: TaskId,
    },
    /// What the task waits in line for, if it does.
    Queue {
        /// The task.
        task_id: TaskId,
    },
    /// This connection is told, by a frame, whenever something changed that a follower should
    /// look at. It reads nothing more.
    Subscribe,
    /// The daemon's status.
    Status,
}

/// What the daemon answers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "response", rename_all = "snake_case")]
pub enum Reply {
    /// **Fixed for every version.** The daemon stops: the tasks it paused, and the tasks that
    /// were queued and did not start, by id. It resumes none.
    Stopped {
        /// The daemon's process.
        pid: u32,
        /// The tasks paused at their boundary by this stop.
        paused: Vec<String>,
        /// The tasks that waited in line and did not start.
        queued: Vec<String>,
    },
    /// What a request waiting for the step in flight tells the user, before its answer.
    Notice {
        /// What.
        notice: Notice,
    },
    /// The answer to a control request.
    Control {
        /// The runtime's answer.
        answer: Box<Response>,
    },
    /// The answer to `attach`.
    Attached,
    /// The answer to `failure`.
    Failure {
        /// The step's error, once.
        error: Option<RuntimeError>,
    },
    /// The answer to `queue`.
    Queue {
        /// The line the task waits in.
        line: Option<Line>,
    },
    /// The answer to `subscribe`; frames of `changed` follow.
    Subscribed,
    /// Something changed that a follower should look at.
    Changed,
    /// The answer to `status`.
    Status {
        /// The daemon.
        status: Box<DaemonStatus>,
    },
    /// A request this connection may not make.
    Refused {
        /// Why.
        refusal: Refusal,
    },
}

/// What a queued task waits behind (external design §19.5): `queued behind task <id>, running
/// since <time>`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Line {
    /// The task that runs.
    pub behind: TaskId,
    /// Since when it runs.
    pub running_since: String,
}

/// What `mimlys daemon status` says first.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonStatus {
    /// The daemon's process.
    pub pid: u32,
    /// The runtime's version.
    pub runtime_version: String,
    /// When it started (RFC 3339, UTC).
    pub started_at: String,
    /// Seconds since.
    pub uptime_secs: u64,
    /// The socket.
    pub socket: String,
    /// The hash of the configuration it read.
    pub config_hash: String,
    /// Clients connected now (a follower holds its connection; a request holds its own).
    pub clients: u32,
    /// Tasks `running` now.
    pub running: Vec<TaskId>,
    /// Tasks waiting in line, each with what it waits behind.
    pub queued: Vec<(TaskId, Line)>,
    /// Tasks waiting for the user.
    pub waiting_user: Vec<TaskId>,
    /// The log file.
    pub log: String,
    /// The idle time after which it exits, in seconds; none for `never`.
    pub idle_secs: Option<u64>,
}

/// HMAC-SHA-256 (RFC 2104) of `message` under `key`, hex.
fn hmac_sha256_hex(key: &[u8], message: &[u8]) -> String {
    const BLOCK: usize = 64;
    let mut block = [0u8; BLOCK];
    if key.len() > BLOCK {
        block[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let mut inner = Sha256::new();
    inner.update(block.map(|b| b ^ 0x36));
    inner.update(message);
    let mut outer = Sha256::new();
    outer.update(block.map(|b| b ^ 0x5c));
    outer.update(inner.finalize());
    hex(&outer.finalize())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The keyed hash of a secret: HMAC-SHA-256 under the connection's salt, of the reference's name
/// and its value, so that two values are compared and neither is sent (REQ-SEC-016).
pub fn keyed_hash(salt: &str, reference: &str, value: &str) -> String {
    let mut message = Vec::new();
    message.extend_from_slice(reference.as_bytes());
    message.push(0);
    message.extend_from_slice(value.as_bytes());
    hmac_sha256_hex(salt.as_bytes(), &message)
}

/// The hash of the configuration file at `path`, as bytes; a configuration with no file (the
/// built-in defaults) hashes as an empty file under its own word. A file that cannot be read
/// hashes as that fact, so that it differs from every file that can.
pub fn config_hash(path: Option<&std::path::Path>) -> String {
    let text = match path {
        None => b"mimlys-config:defaults".to_vec(),
        Some(p) => match std::fs::read(p) {
            Ok(bytes) => bytes,
            Err(e) => format!("mimlys-config:unreadable:{:?}", e.kind()).into_bytes(),
        },
    };
    hex(&Sha256::digest(&text))
}

/// The salt of one connection: 32 random bytes, hex.
#[cfg(unix)]
pub fn new_salt() -> Result<String, std::io::Error> {
    let mut bytes = [0u8; 32];
    rustix::rand::getrandom(&mut bytes, rustix::rand::GetRandomFlags::empty())
        .map_err(std::io::Error::from)?;
    Ok(hex(&bytes))
}

#[cfg(test)]
mod tests;
