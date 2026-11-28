//! Length-prefixed JSON frames (external design §13.1, §19.5; RFC 005, RFC 014): a 4-byte
//! big-endian length, then a JSON object that carries a `version` and the body's own fields.
//!
//! Two protocols speak this framing, and each names the version it carries and the largest frame
//! it accepts: the runtime and its worker (`protocol`, whose version and limit follow the build),
//! and a client and the daemon (`mimlys::daemon`, whose frames carry a version that never
//! changes, so that any release can read the hello and send `stop`). The framing is one piece of
//! code: this module, which neither protocol copies.

use serde::{Deserialize, Serialize};

/// One framed message: the version of its protocol and the body, whose fields sit beside it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Frame<T> {
    /// The version of the protocol that carries the frame.
    pub version: u32,
    /// The body.
    #[serde(flatten)]
    pub body: T,
}

/// A framing failure.
#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    /// The peer closed the stream.
    #[error("end of stream")]
    Eof,
    /// A frame above the limit in force.
    #[error("frame of {len} bytes exceeds the limit of {limit}")]
    TooLarge {
        /// Its length.
        len: usize,
        /// The limit it exceeds.
        limit: usize,
    },
    /// The body is not the expected JSON.
    #[error("malformed frame: {0}")]
    Malformed(String),
    /// The peer speaks another version of the protocol.
    #[error("frame version {0} is not the version in force")]
    Version(u32),
    /// The stream failed.
    #[error("stream: {0}")]
    Io(#[from] std::io::Error),
}

/// Writes one frame of protocol `version`: a 4-byte big-endian length, then the JSON body. A body
/// above `limit` is not written.
pub fn write_frame_versioned<T: Serialize>(
    out: &mut impl std::io::Write,
    version: u32,
    body: &T,
    limit: usize,
) -> Result<(), FrameError> {
    let frame = Frame { version, body };
    let bytes = serde_json::to_vec(&frame).map_err(|e| FrameError::Malformed(e.to_string()))?;
    if bytes.len() > limit {
        return Err(FrameError::TooLarge {
            len: bytes.len(),
            limit,
        });
    }
    out.write_all(&(bytes.len() as u32).to_be_bytes())?;
    out.write_all(&bytes)?;
    out.flush()?;
    Ok(())
}

/// Reads one frame of at most `limit` bytes and checks that it is of protocol `version`. A frame
/// of another version is `Version` whatever its body holds: the version is looked at before the
/// body's shape is blamed, so that a peer of another build is named as that.
pub fn read_frame_versioned<T: serde::de::DeserializeOwned>(
    input: &mut impl std::io::Read,
    version: u32,
    limit: usize,
) -> Result<T, FrameError> {
    let mut len = [0u8; 4];
    match input.read_exact(&mut len) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Err(FrameError::Eof),
        Err(e) => return Err(e.into()),
    }
    let len = u32::from_be_bytes(len) as usize;
    if len > limit {
        return Err(FrameError::TooLarge { len, limit });
    }
    // Read what arrives, up to the length declared: the buffer grows with the bytes, so a
    // peer that declares a long frame and sends none holds no memory.
    use std::io::Read as _;
    let mut bytes = Vec::new();
    input.take(len as u64).read_to_end(&mut bytes)?;
    if bytes.len() < len {
        return Err(FrameError::Io(std::io::ErrorKind::UnexpectedEof.into()));
    }
    match serde_json::from_slice::<Frame<T>>(&bytes) {
        Ok(frame) if frame.version == version => Ok(frame.body),
        Ok(frame) => Err(FrameError::Version(frame.version)),
        Err(e) => {
            #[derive(Deserialize)]
            struct Versioned {
                version: u32,
            }
            match serde_json::from_slice::<Versioned>(&bytes) {
                Ok(Versioned { version: theirs }) if theirs != version => {
                    Err(FrameError::Version(theirs))
                }
                _ => Err(FrameError::Malformed(e.to_string())),
            }
        }
    }
}

#[cfg(test)]
mod tests;
