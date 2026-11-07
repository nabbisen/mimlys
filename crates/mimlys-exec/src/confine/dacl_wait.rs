//! The decision of a writer of a directory's permissions about the
//! directory's lock (REQ-CONF-008, external design §13.5, THR-36): take it,
//! wait up to LIM-O7 when another writer holds it, and refuse when the wait
//! runs out or the system will not give the lock at all: never a write
//! without the lock. The decision is pure (the lock and the log are passed
//! in), so that it is tested on every platform; the Windows module
//! `windows::acl` passes the named mutex. The refusal is a typed error
//! ([`Refusal`]) that the commands turn into the error kind `unavailable`
//! (external design §18); nothing recognizes it by its text.

// Only the Windows module calls this outside the tests.
#![cfg_attr(not(windows), allow(dead_code))]

use mimlys_core::limits::LIM_O7_PERMISSION_WRITE_WAIT_MS;

/// What one attempt on the lock came to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Attempt {
    /// The lock is now this writer's.
    Acquired,
    /// The lock is now this writer's; its holder ended without releasing it.
    Abandoned,
    /// The lock is still held by another writer after the time given.
    Held,
    /// The system could not wait (a failed call, not a held lock).
    Failed,
}

/// Why a write is refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cause {
    /// Another runtime's write did not end within LIM-O7.
    Held,
    /// The system will not give the lock (it cannot be created or waited on).
    LockUnavailable,
}

/// What the writer does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    /// Write, holding the lock.
    Write,
    /// Write, holding the lock; the holder died, so the writer reads the
    /// base again under the lock (its write either landed or did not).
    WriteAfterAbandoned,
    /// Refuse the write.
    Refuse(Cause),
}

/// The refusal of a write of a directory's permissions: the lock that
/// orders the writers was held past LIM-O7, or could not be taken. Its text
/// is `acl_not_applied: <reason>` (external design §13.5).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// Another runtime is writing the directory's permissions.
    Held {
        /// The directory.
        dir: String,
    },
    /// The system will not give the directory's lock.
    LockUnavailable {
        /// The directory.
        dir: String,
        /// The system's error, as text.
        system_error: String,
    },
}

impl Refusal {
    /// The refusal for a cause. `system_error` is used for
    /// [`Cause::LockUnavailable`] only.
    pub fn new(cause: Cause, dir: &str, system_error: &str) -> Self {
        match cause {
            Cause::Held => Refusal::Held {
                dir: dir.to_owned(),
            },
            Cause::LockUnavailable => Refusal::LockUnavailable {
                dir: dir.to_owned(),
                system_error: system_error.to_owned(),
            },
        }
    }
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refusal::Held { dir } => write!(
                f,
                "acl_not_applied: the permissions of {dir} are being written by another runtime"
            ),
            Refusal::LockUnavailable { dir, system_error } => write!(
                f,
                "acl_not_applied: the permissions lock of {dir} cannot be taken: {system_error}"
            ),
        }
    }
}

impl std::error::Error for Refusal {}

/// Decides from the attempts. `attempt(timeout_ms)` waits at most that long
/// for the lock; it is first called with 0, which tells a free lock from a
/// held one. `on_wait` is called once, when the lock is found held, before
/// the wait. `limit_ms` is LIM-O7 in production. A failed attempt, first or
/// after the wait, refuses: there is no lock to hold.
pub fn decide(
    mut attempt: impl FnMut(u32) -> Attempt,
    on_wait: impl FnOnce(),
    limit_ms: u64,
) -> Decision {
    let mut result = attempt(0);
    if result == Attempt::Held {
        on_wait();
        result = attempt(u32::try_from(limit_ms).unwrap_or(u32::MAX));
    }
    match result {
        Attempt::Acquired => Decision::Write,
        Attempt::Abandoned => Decision::WriteAfterAbandoned,
        Attempt::Held => Decision::Refuse(Cause::Held),
        Attempt::Failed => Decision::Refuse(Cause::LockUnavailable),
    }
}

/// [`decide`] with LIM-O7.
pub fn decide_with_limit(attempt: impl FnMut(u32) -> Attempt, on_wait: impl FnOnce()) -> Decision {
    decide(attempt, on_wait, LIM_O7_PERMISSION_WRITE_WAIT_MS)
}

#[cfg(test)]
mod tests;
