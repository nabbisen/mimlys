//! Canonical hashing (LIM-U6, REQ-DEC-012): SHA-256 over canonical JSON
//! (object keys sorted by code point, no insignificant whitespace, strings
//! and keys in Unicode NFC), rendered as lowercase hexadecimal. Decisions,
//! actions and grant sets are hashed this way; the same value always yields
//! the same hash whatever its textual form was.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fmt;
use unicode_normalization::UnicodeNormalization;

/// A SHA-256 digest as 64 lowercase hexadecimal characters.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Hash(String);

impl Hash {
    /// The hexadecimal text.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Wraps text that is already a 64-character lowercase hexadecimal digest.
    pub fn from_hex(text: &str) -> Option<Self> {
        let valid =
            text.len() == 64 && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
        valid.then(|| Self(text.to_owned()))
    }
}

impl fmt::Display for Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Hash({}…)", &self.0[..12])
    }
}

/// SHA-256 of raw bytes.
pub fn hash_bytes(bytes: &[u8]) -> Hash {
    let digest = Sha256::digest(bytes);
    let mut hex = String::with_capacity(64);
    for byte in digest {
        hex.push_str(&format!("{byte:02x}"));
    }
    Hash(hex)
}

/// SHA-256 of the canonical JSON form of `value`.
pub fn hash_canonical(value: &Value) -> Hash {
    hash_bytes(canonical_json(value).as_bytes())
}

/// The canonical JSON text of `value`: keys sorted by code point, no
/// whitespace, NFC strings. Numbers keep `serde_json`'s shortest form.
pub fn canonical_json(value: &Value) -> String {
    let mut out = String::new();
    write_canonical(value, &mut out);
    out
}

fn write_canonical(value: &Value, out: &mut String) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&n.to_string()),
        Value::String(s) => write_string(s, out),
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_canonical(item, out);
            }
            out.push(']');
        }
        Value::Object(map) => {
            let mut entries: Vec<(String, &Value)> = map
                .iter()
                .map(|(k, v)| (k.nfc().collect::<String>(), v))
                .collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            out.push('{');
            for (i, (key, item)) in entries.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_string(key, out);
                out.push(':');
                write_canonical(item, out);
            }
            out.push('}');
        }
    }
}

fn write_string(s: &str, out: &mut String) {
    let normalized: String = s.nfc().collect();
    out.push_str(&serde_json::to_string(&normalized).expect("a string always serializes"));
}

#[cfg(test)]
mod tests;
