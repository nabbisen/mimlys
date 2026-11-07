//! The decision of a writer of a directory's permissions that finds the
//! directory's lock held (REQ-CONF-008, external design §13.5, THR-36): wait
//! up to LIM-O7, then refuse. The decision is pure (the lock and the log are
//! passed in), so that it is tested on every platform; the Windows module
//! `windows::acl` passes the named mutex.

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

/// What the writer does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    /// Write, holding the lock.
    Write,
    /// Write, holding the lock; the holder died, so the writer reads the
    /// base again under the lock (its write either landed or did not).
    WriteAfterAbandoned,
    /// The wait failed in the system: there is no lock to hold, and no
    /// holder that is known. The write goes ahead as it did before the
    /// lock existed (a mutex the system refuses cannot be waited on).
    WriteUnlocked,
    /// Refuse: another runtime's write did not end within LIM-O7.
    Refuse,
}

/// The reason of a refusal, in the words of external design §13.5.
pub fn refusal_reason(dir: &str) -> String {
    format!("acl_not_applied: the permissions of {dir} are being written by another runtime")
}

/// Decides from the attempts. `attempt(timeout_ms)` waits at most that long
/// for the lock; it is first called with 0, which tells a free lock from a
/// held one. `on_wait` is called once, when the lock is found held, before
/// the wait. `limit_ms` is LIM-O7 in production.
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
        Attempt::Held => Decision::Refuse,
        Attempt::Failed => Decision::WriteUnlocked,
    }
}

/// [`decide`] with LIM-O7.
pub fn decide_with_limit(attempt: impl FnMut(u32) -> Attempt, on_wait: impl FnOnce()) -> Decision {
    decide(attempt, on_wait, LIM_O7_PERMISSION_WRITE_WAIT_MS)
}

#[cfg(test)]
mod tests;
