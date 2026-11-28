//! The name of the daemon's pipe on Windows (external design §19.5): `\\.\pipe\mimlys-d-<hash>`,
//! from a short hash of the user's SID **and** the data directory's canonical path, because a
//! pipe's name is global to the machine: two users with the same data directory path must not
//! share one, and a user with two stores has two daemons. Pure, so that every platform tests it.

use sha2::{Digest, Sha256};
use std::path::Path;

/// The pipes' namespace and the name's prefix.
pub const PREFIX: &str = r"\\.\pipe\mimlys-d-";

/// The pipe's name for the user whose SID is `sid` and the data directory at `canonical` (its
/// canonical path): the prefix and twelve hexadecimal digits of the SHA-256 of the SID, a zero
/// byte and the path.
pub fn pipe_name(sid: &str, canonical: &Path) -> String {
    let mut hash = Sha256::new();
    hash.update(sid.as_bytes());
    hash.update([0u8]);
    hash.update(canonical.as_os_str().as_encoded_bytes());
    let digest = hash.finalize();
    let short: String = digest.iter().take(6).map(|b| format!("{b:02x}")).collect();
    format!("{PREFIX}{short}")
}

#[cfg(test)]
mod tests;
