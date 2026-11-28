//! The daemon's mark on each tool server's tree (Windows; external design §19.5, REQ-SEC-016).
//!
//! On Unix a tool server leads a session of its own, and a peer in that session is the daemon's
//! own tree. On Windows the equivalent is membership of a Job Object. The worker's job is the
//! daemon's own; a tool server's is made by the library that spawns it and cannot be reached, so
//! **the daemon puts each server's process in a second job of its own** (an empty one that sets no
//! limit and ends nothing when it closes; it nests inside the first) the moment it is spawned,
//! before the server runs anything it was asked: what the server starts afterwards is in it too.
//!
//! Marks exist only in a process that [`enable`]d them (the daemon): an embedded runtime has no
//! clients to tell its tree from.
//!
//! What it does not catch (THR-39's residual, as `setsid` on Unix): a process the server started
//! in the instant between its creation and the mark. Nothing here is a secret of the server's.

use mimlys_exec::daemon_pipe::{JobMark, JobRef};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError};

static ENABLED: AtomicBool = AtomicBool::new(false);
static MARKS: Mutex<Option<HashMap<u32, JobMark>>> = Mutex::new(None);

/// Marks the servers that start from now on (called once, by the daemon, before it opens its app).
pub fn enable() {
    ENABLED.store(true, Ordering::SeqCst);
}

/// Puts the process `pid` of a tool server in a job of its own, when marks are enabled. A server
/// that cannot be marked runs, and is said so in the log: its tree is then not told from a client.
pub fn mark(pid: u32) {
    if !ENABLED.load(Ordering::SeqCst) {
        return;
    }
    match JobMark::put(pid) {
        Ok(job) => {
            let mut marks = MARKS.lock().unwrap_or_else(PoisonError::into_inner);
            let marks = marks.get_or_insert_with(HashMap::new);
            // A mark whose processes have all gone is dropped, here and not before, so that the
            // list stays as long as the servers that were started.
            marks.retain(|_, mark| !mark.is_empty());
            marks.insert(pid, job);
        }
        Err(e) => tracing::warn!(
            pid,
            "a tool server could not be marked as the daemon's own tree: {e}"
        ),
    }
}

/// The jobs of the servers that were marked and still hold a process.
pub fn jobs() -> Vec<JobRef> {
    let mut marks = MARKS.lock().unwrap_or_else(PoisonError::into_inner);
    let Some(marks) = marks.as_mut() else {
        return Vec::new();
    };
    marks.retain(|_, mark| !mark.is_empty());
    marks.values().map(JobMark::job).collect()
}
