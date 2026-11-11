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

// The proof of `decide` (spike S3, `verify/`). Everything below with
// `verus_keep_ghost` is read by the Verus checker only; the compiler of the
// product does not see it, and the product does not depend on Verus.
#[cfg(verus_keep_ghost)]
use vstd::pervasive::FnWithRequiresEnsures;
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

// The wait of the second attempt, as the proof states it.
#[cfg(verus_keep_ghost)]
verus_builtin_macros::verus! {
    pub open spec fn wait_ms_spec(limit_ms: u64) -> u32 {
        if limit_ms > 0xFFFF_FFFFu64 { 0xFFFF_FFFFu32 } else { limit_ms as u32 }
    }
}

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

/// The wait of the second attempt: `limit_ms`, saturating at `u32::MAX`.
#[cfg_attr(verus_keep_ghost, verus_builtin_macros::verus_spec(r =>
    ensures r == wait_ms_spec(limit_ms),
))]
fn wait_ms(limit_ms: u64) -> u32 {
    if limit_ms > u64::from(u32::MAX) {
        u32::MAX
    } else {
        limit_ms as u32
    }
}

/// Decides from the results of the attempts: `first` is the attempt without
/// waiting; `second` is the attempt after the wait, made when `first` was
/// `Held`. A `Held` first attempt with no second one refuses as `Held`.
#[cfg_attr(verus_keep_ghost, verus_builtin_macros::verus_spec(r =>
    ensures
        // P1: no write without an attempt that gave the lock.
        r == Decision::Write ==> (first == Attempt::Acquired
            || (first == Attempt::Held && second == Some(Attempt::Acquired))),
        r == Decision::WriteAfterAbandoned ==> (first == Attempt::Abandoned
            || (first == Attempt::Held && second == Some(Attempt::Abandoned))),
        // P2: held twice is a refusal as held.
        (first == Attempt::Held && second == Some(Attempt::Held))
            ==> r == Decision::Refuse(Cause::Held),
        // P3: a failed attempt that decides is a refusal as unavailable.
        (first == Attempt::Failed || (first == Attempt::Held && second == Some(Attempt::Failed)))
            ==> r == Decision::Refuse(Cause::LockUnavailable),
))]
fn decide_from(first: Attempt, second: Option<Attempt>) -> Decision {
    let result = match first {
        Attempt::Held => match second {
            Some(second) => second,
            None => Attempt::Held,
        },
        other => other,
    };
    match result {
        Attempt::Acquired => Decision::Write,
        Attempt::Abandoned => Decision::WriteAfterAbandoned,
        Attempt::Held => Decision::Refuse(Cause::Held),
        Attempt::Failed => Decision::Refuse(Cause::LockUnavailable),
    }
}

/// Decides from the attempts. `attempt(timeout_ms)` waits at most that long
/// for the lock; it is first called with 0, which tells a free lock from a
/// held one. `on_wait` is called once, when the lock is found held, before
/// the wait. `limit_ms` is LIM-O7 in production. A failed attempt, first or
/// after the wait, refuses: there is no lock to hold.
#[cfg_attr(verus_keep_ghost, verus_builtin_macros::verus_spec(r =>
    requires
        on_wait.requires(()),
        // P4: the attempt is called with 0, or with the wait, and with nothing else.
        attempt.requires((0u32,)),
        attempt.requires((wait_ms_spec(limit_ms),)),
        forall|t: u32| attempt.requires((t,)) ==> (t == 0u32 || t == wait_ms_spec(limit_ms)),
))]
pub fn decide(
    mut attempt: impl FnMut(u32) -> Attempt,
    on_wait: impl FnOnce(),
    limit_ms: u64,
) -> Decision {
    let first = attempt(0);
    let second = if let Attempt::Held = first {
        on_wait();
        #[cfg(verus_keep_ghost)]
        verus_builtin_macros::proof! { assert(first == Attempt::Held); }
        Some(attempt(wait_ms(limit_ms)))
    } else {
        None
    };
    decide_from(first, second)
}

/// [`decide`] with LIM-O7.
pub fn decide_with_limit(attempt: impl FnMut(u32) -> Attempt, on_wait: impl FnOnce()) -> Decision {
    decide(attempt, on_wait, LIM_O7_PERMISSION_WRITE_WAIT_MS)
}

#[cfg(test)]
mod tests;
