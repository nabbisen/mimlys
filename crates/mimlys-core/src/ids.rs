//! Identifiers (LIM-U5): UUID version 7, lowercase hyphenated, obtained only
//! through an [`IdSource`] so that tests and replay are deterministic
//! (REQ-ARCH-006).

use serde::{Deserialize, Serialize};
use std::fmt;
use uuid::Uuid;

/// A source of identifiers. The runtime uses [`SystemIds`]; tests use
/// `ScriptedIds` (feature `test-support`).
pub trait IdSource: Send + Sync {
    /// Returns the next identifier, a UUID version 7.
    fn next(&self) -> Uuid;
}

/// Time-ordered UUID version 7 identifiers from the system clock and random
/// bits. The only place in this crate that generates identifiers.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemIds;

impl IdSource for SystemIds {
    fn next(&self) -> Uuid {
        Uuid::now_v7()
    }
}

/// Deterministic identifiers for tests: valid version 7 UUIDs whose
/// timestamp and "random" bits come from a counter.
#[cfg(any(test, feature = "test-support"))]
#[derive(Debug)]
pub struct ScriptedIds {
    counter: std::sync::atomic::AtomicU64,
}

#[cfg(any(test, feature = "test-support"))]
impl ScriptedIds {
    /// A source whose first identifier uses `start` as its counter value.
    pub fn starting_at(start: u64) -> Self {
        Self {
            counter: std::sync::atomic::AtomicU64::new(start),
        }
    }
}

#[cfg(any(test, feature = "test-support"))]
impl Default for ScriptedIds {
    fn default() -> Self {
        Self::starting_at(1)
    }
}

#[cfg(any(test, feature = "test-support"))]
impl IdSource for ScriptedIds {
    fn next(&self) -> Uuid {
        let n = self
            .counter
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let mut random = [0u8; 10];
        random[2..].copy_from_slice(&n.to_be_bytes());
        uuid::Builder::from_unix_timestamp_millis(n, &random).into_uuid()
    }
}

macro_rules! id_type {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(Uuid);

        impl $name {
            /// Draws a fresh identifier from the source.
            pub fn new(source: &dyn IdSource) -> Self {
                Self(source.next())
            }

            /// Wraps an existing UUID (for records loaded from the store).
            pub fn from_uuid(uuid: Uuid) -> Self {
                Self(uuid)
            }

            /// The underlying UUID.
            pub fn as_uuid(&self) -> Uuid {
                self.0
            }

            /// Whether the identifier is a UUID version 7 (LIM-U5).
            pub fn is_v7(&self) -> bool {
                self.0.get_version_num() == 7
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0.hyphenated())
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}({})", stringify!($name), self.0.hyphenated())
            }
        }
    };
}

id_type!(TaskId, "Identifier of a task.");
id_type!(StepId, "Identifier of a step.");
id_type!(ActionId, "Identifier of an action.");
id_type!(
    ExecutionId,
    "Identifier of an execution (one attempt of an action)."
);
id_type!(ObservationId, "Identifier of an observation.");
id_type!(ApprovalId, "Identifier of an approval request.");
id_type!(QuestionId, "Identifier of a question to the user.");
id_type!(GrantId, "Identifier of a capability grant.");
id_type!(EventId, "Identifier of an event record.");
id_type!(SnapshotId, "Identifier of a workspace snapshot.");
id_type!(PromotionId, "Identifier of a promotion.");
id_type!(BlobId, "Identifier of a stored blob.");

#[cfg(test)]
mod tests;
