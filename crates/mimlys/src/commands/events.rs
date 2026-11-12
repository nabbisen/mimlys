//! `mimlys events [<task>] [--since N] [--follow]` (REQ-OBS-002).

use crate::app::{AppError, ReadApp};
use crate::follow::event_text;
use crate::messages;
use crate::output::Output;
use mimlys_core::limits::LIM_O17_FOLLOW_HOLDER_WAIT_MS;
use mimlys_core::task::TaskState;
use std::time::{Duration, Instant};

/// How often `events --follow` reads for new events, in milliseconds. A
/// reader's poll; it takes no lock and writes nothing. (Not in the limits
/// register: the value was a literal before this name; see the review request.)
pub const EVENTS_POLL_MS: u64 = 500;

/// Prints events after `since`; with `follow`, keeps polling until the
/// task ends (or forever without a task).
pub async fn run(
    app: &ReadApp,
    task: Option<&str>,
    since: u64,
    follow: bool,
    out: &Output,
) -> Result<(), AppError> {
    let task_id = match task {
        Some(t) => Some(app.resolve_task(t)?),
        None => None,
    };
    let mut position = since;
    let mut all = Vec::new();
    let mut unheld = UnheldWatch::default();
    loop {
        let batch = app.reader.get_events(task_id, position, 1000)?;
        for e in &batch {
            out.line(event_text(e));
            position = e.position;
        }
        all.extend(batch.iter().cloned());
        if !batch.is_empty() {
            continue;
        }
        if !follow {
            break;
        }
        if let Some(id) = task_id {
            let state = app.reader.get_task(id)?.task.state;
            if !matches!(
                state,
                TaskState::Running
                    | TaskState::WaitingUser { .. }
                    | TaskState::Created
                    | TaskState::Paused
            ) {
                break;
            }
            // A `running` task that no runtime holds: either a runtime is about to
            // take the store (it has not yet, or it recovers a previous runtime's
            // task) or the runtime that ran it is gone and nobody has recovered
            // the store. The follower waits LIM-O17 for a runtime, and past it the
            // task will not move on its own: say so and stop (REQ-PERS-013).
            if matches!(state, TaskState::Running) {
                let free = app.store.probe_lock() == mimlys_persist::LockState::Free;
                match unheld.observe(free, Instant::now()) {
                    Unheld::Held => {}
                    Unheld::Waiting { first } => {
                        if first {
                            out.line(messages::FOLLOW_WAITING_FOR_RUNTIME);
                        }
                    }
                    Unheld::Gone => {
                        out.line(messages::FOLLOW_RUNTIME_GONE);
                        break;
                    }
                }
            } else {
                unheld.observe(false, Instant::now());
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(EVENTS_POLL_MS)).await;
    }
    if all.is_empty() {
        out.line(messages::EVENTS_EMPTY);
    }
    out.value(&all);
    Ok(())
}

/// What the follower finds of the store's holder at one poll of a `running` task.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Unheld {
    /// A runtime holds the store (or the lock cannot be tested): nothing to wait for.
    Held,
    /// Nobody holds it, for less than LIM-O17 so far; `first` at the first poll of the episode.
    Waiting { first: bool },
    /// Nobody has held it for LIM-O17 running: the runtime is gone.
    Gone,
}

/// How long the store has been seen unheld, without a break, by a follower of a
/// `running` task (LIM-O17). Any poll that finds it held starts the count over.
#[derive(Default)]
struct UnheldWatch {
    since: Option<Instant>,
}

impl UnheldWatch {
    /// One poll at `now`: `free` when the lock was found free.
    fn observe(&mut self, free: bool, now: Instant) -> Unheld {
        if !free {
            self.since = None;
            return Unheld::Held;
        }
        let first = self.since.is_none();
        let since = *self.since.get_or_insert(now);
        if now.saturating_duration_since(since)
            >= Duration::from_millis(LIM_O17_FOLLOW_HOLDER_WAIT_MS)
        {
            Unheld::Gone
        } else {
            Unheld::Waiting { first }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// REQ-PERS-013: the count of an unheld store starts at the first poll that finds
    /// it free, ends at LIM-O17 and starts over after any poll that finds it held. No
    /// clock is read: the instants are made up.
    #[test]
    fn req_pers_013_the_follower_waits_lim_o17_for_a_runtime_and_a_holder_resets_it() {
        let wait = Duration::from_millis(LIM_O17_FOLLOW_HOLDER_WAIT_MS);
        let t0 = Instant::now();
        let mut watch = UnheldWatch::default();
        assert_eq!(watch.observe(true, t0), Unheld::Waiting { first: true });
        assert_eq!(
            watch.observe(true, t0 + wait / 2),
            Unheld::Waiting { first: false }
        );
        assert_eq!(
            watch.observe(true, t0 + wait - Duration::from_millis(1)),
            Unheld::Waiting { first: false },
            "not before the wait"
        );
        // A runtime took the store at the last moment: the count is over.
        assert_eq!(watch.observe(false, t0 + wait), Unheld::Held);
        // And it is not held again later: a new episode starts from its own first poll.
        let t1 = t0 + wait + Duration::from_millis(500);
        assert_eq!(watch.observe(true, t1), Unheld::Waiting { first: true });
        assert_eq!(
            watch.observe(true, t1 + wait - Duration::from_millis(1)),
            Unheld::Waiting { first: false }
        );
        assert_eq!(watch.observe(true, t1 + wait), Unheld::Gone, "at the wait");
        assert_eq!(watch.observe(true, t1 + wait * 3), Unheld::Gone);
    }
}
