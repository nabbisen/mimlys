//! Which tasks have a step in flight, for which a cancel is in progress, and which a request
//! holds (REQ-DEC-001, REQ-TASK-006; external design §4, "Steps of one task never overlap" and
//! "Every other state-changing request on a task whose step is in flight").
//!
//! The runtime's own, so that no caller can bypass it: neither the command line's loop today nor
//! a scheduler that drives many tasks. It is a guard per task, not a lock around the store: a
//! step of another task is never delayed by it, and a refused step never reaches the store.
//!
//! Three parties write a task's state: the step in flight (from the task it loaded when it
//! began), `cancel_task`, and every other request. **No write is made to a task while its step
//! holds it, except by the step itself**, and the slot makes the parties take turns without
//! either waiting for a lock it cannot see:
//!
//! - a step begins only when no step of the task is in flight, no cancel of it is in progress,
//!   no request holds it and none waits to (otherwise it is refused as `conflict`, and writes
//!   nothing: the driver asks again);
//! - a cancel marks the slot, so that no step begins meanwhile, asks the step in flight to stop,
//!   waits for it to end, and only then reads the task and writes;
//! - **a request that writes** holds the task while it reads and writes ([`Steps::begin_write`]).
//!   What it does when a step is in flight is the request's own rule: a pause or a stop is
//!   *noted* in the slot and taken at the step's end, in the step's own turn; a grant or a
//!   revocation *waits* for the step to end, and applies from a fresh read; every other request
//!   needs a state that has no step in flight (an approval, a rejection and an answer need
//!   `waiting_user`; a rollback, a promotion and a deletion, a task that is not `running`): it is
//!   *refused* as the state it would be refused in while the task is `running`, and waits for the
//!   step only while the step is returning from having written a state that is not.
//!
//! Whichever of the parties comes second finds the other's work done.

use crate::ids::{RequestId, TaskId};
use crate::runtime::{RuntimeError, Session};
use crate::task::StateError;
use std::collections::HashMap;
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

/// A request that waits for the step in flight to end, noted in the slot: who made it, and
/// under which id, so that what it does at the step's end is recorded as its own.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Note {
    pub(super) request_id: RequestId,
    pub(super) session: Session,
}

/// What the slot of a task says.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Slot {
    /// A step of the task is in flight.
    step: bool,
    /// A cancel of the task is in progress.
    cancel: bool,
    /// A request holds the task while it writes.
    writer: bool,
    /// Requests that wait for the step to end, and are applied before any step begins again.
    waiting: u32,
    /// A pause, to be taken when the step ends.
    pause: Option<Note>,
    /// A stop (the second signal), to be taken when the step ends: the task is cancelled if the
    /// step left it `running`.
    stop: Option<Note>,
}

/// What a request that writes does when a step of the task is in flight.
pub(super) enum OnStep<'a> {
    /// The request needs the task in a state that has no step in flight. While a step is in
    /// flight and the task is `running` (the closure says so, read while the slot is held), the
    /// request is refused, as `state`, naming the verb: it would reach something the user has
    /// not seen. While a step is in flight and the task is already in another state (the step
    /// wrote its end, and is only returning), the request waits for the step to end, and applies
    /// from a fresh read: that state is the step's last word.
    UnlessRunning(&'static str, &'a dyn Fn() -> Result<bool, RuntimeError>),
    /// The request waits for the step to end, saying so once through the callback.
    Wait(&'a dyn Fn()),
    /// The request is noted and taken at the step's end.
    Note(Noted),
}

/// What is noted for the step's end.
pub(super) enum Noted {
    /// A pause.
    Pause(Note),
    /// A stop.
    Stop(Note),
}

/// What [`Steps::begin_write`] gave.
#[must_use = "the guard is the request's claim on its task"]
pub(super) enum Held<'a> {
    /// The task is held for the request, which reads and writes now.
    Claimed(WriteGuard<'a>),
    /// A step is in flight, and the request is noted for its end.
    Noted,
}

/// The slots of the tasks that have a step in flight, a cancel in progress or a request holding
/// them; a task with none has no entry, so that a long-lived runtime keeps nothing for the
/// tasks that have ended.
#[derive(Default)]
pub(super) struct Steps {
    slots: Mutex<HashMap<TaskId, Slot>>,
    /// Signalled when a step ends, a cancel or a request releases its claim.
    changed: Condvar,
}

/// A step in flight: held for the whole of `run_step`, released when it ends
/// in any way, a dropped future included.
#[must_use = "the guard is the step's claim on its task"]
pub(super) struct StepGuard<'a> {
    steps: &'a Steps,
    task_id: TaskId,
    /// The claim was released by [`StepGuard::finish`].
    released: bool,
}

/// A cancel in progress: no step of the task begins while it is held.
#[must_use = "the guard is the cancel's claim on its task"]
pub(super) struct CancelGuard<'a> {
    steps: &'a Steps,
    task_id: TaskId,
}

/// A request that holds its task while it writes.
#[must_use = "the guard is the request's claim on its task"]
pub(super) struct WriteGuard<'a> {
    steps: &'a Steps,
    task_id: TaskId,
}

impl Steps {
    fn lock(&self) -> MutexGuard<'_, HashMap<TaskId, Slot>> {
        // A panic elsewhere must not stop a guard from being released.
        self.slots.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Claims the task for a step, or refuses it as `conflict`, naming the task.
    pub(super) fn begin_step(&self, task_id: TaskId) -> Result<StepGuard<'_>, RuntimeError> {
        let mut slots = self.lock();
        let slot = slots.entry(task_id).or_default();
        let refusal = if slot.cancel {
            Some(format!(
                "task {task_id} is being cancelled: no step of it begins"
            ))
        } else if slot.step {
            Some(format!(
                "a step of task {task_id} is in flight: steps of one task never overlap"
            ))
        } else if slot.writer || slot.waiting > 0 {
            Some(format!(
                "a request is being applied to task {task_id}: no step of it begins before it is done"
            ))
        } else {
            None
        };
        if let Some(detail) = refusal {
            if *slot == Slot::default() {
                slots.remove(&task_id);
            }
            return Err(RuntimeError::Conflict { detail });
        }
        slot.step = true;
        Ok(StepGuard {
            steps: self,
            task_id,
            released: false,
        })
    }

    /// Claims the task for a cancel, or refuses it as `conflict` when another cancel of it is in
    /// progress. A request that holds the task is waited for. The flag says whether a step is in
    /// flight. A pause or a stop noted for the step's end is dropped: the cancel is the end.
    pub(super) fn begin_cancel(
        &self,
        task_id: TaskId,
    ) -> Result<(CancelGuard<'_>, bool), RuntimeError> {
        let mut slots = self.lock();
        loop {
            let slot = slots.entry(task_id).or_default();
            if slot.cancel {
                return Err(RuntimeError::Conflict {
                    detail: format!("a cancel of task {task_id} is already in progress"),
                });
            }
            if !slot.writer {
                slot.cancel = true;
                slot.pause = None;
                slot.stop = None;
                return Ok((
                    CancelGuard {
                        steps: self,
                        task_id,
                    },
                    slot.step,
                ));
            }
            slots = self
                .changed
                .wait(slots)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }

    /// Claims the task for a request that writes, as `on_step` says when a step is in flight.
    /// A cancel in progress refuses the request as `conflict`; another request that holds the
    /// task is waited for (it holds it while it writes, which is short).
    pub(super) fn begin_write(
        &self,
        task_id: TaskId,
        on_step: OnStep<'_>,
    ) -> Result<Held<'_>, RuntimeError> {
        let mut slots = self.lock();
        let mut waiting = false;
        let result = loop {
            let slot = slots.entry(task_id).or_default();
            if slot.cancel {
                break Err(RuntimeError::Conflict {
                    detail: format!(
                        "task {task_id} is being cancelled: nothing else is applied to it meanwhile"
                    ),
                });
            }
            if slot.writer {
                slots = self
                    .changed
                    .wait(slots)
                    .unwrap_or_else(PoisonError::into_inner);
                continue;
            }
            if slot.step {
                match &on_step {
                    OnStep::UnlessRunning(verb, is_running) => match is_running() {
                        Ok(true) => {
                            break Err(RuntimeError::State(StateError::Illegal {
                                from: "running".into(),
                                input: (*verb).into(),
                            }));
                        }
                        Err(error) => break Err(error),
                        Ok(false) => {
                            if !waiting {
                                waiting = true;
                                slot.waiting += 1;
                            }
                            slots = self
                                .changed
                                .wait(slots)
                                .unwrap_or_else(PoisonError::into_inner);
                            continue;
                        }
                    },
                    OnStep::Note(Noted::Pause(note)) => {
                        slot.pause = Some(note.clone());
                        break Ok(Held::Noted);
                    }
                    OnStep::Note(Noted::Stop(note)) => {
                        slot.stop = Some(note.clone());
                        break Ok(Held::Noted);
                    }
                    OnStep::Wait(say) => {
                        if !waiting {
                            waiting = true;
                            slot.waiting += 1;
                            say();
                        }
                        slots = self
                            .changed
                            .wait(slots)
                            .unwrap_or_else(PoisonError::into_inner);
                        continue;
                    }
                }
            }
            slot.writer = true;
            break Ok(Held::Claimed(WriteGuard {
                steps: self,
                task_id,
            }));
        };
        if waiting && let Some(slot) = slots.get_mut(&task_id) {
            slot.waiting -= 1;
        }
        if let Some(slot) = slots.get(&task_id)
            && *slot == Slot::default()
        {
            slots.remove(&task_id);
        }
        result
    }

    /// Waits until no step of the task is in flight, for at most `within`;
    /// whether none is.
    pub(super) fn wait_for_step_end(&self, task_id: TaskId, within: Duration) -> bool {
        let (_slots, timeout) = self
            .changed
            .wait_timeout_while(self.lock(), within, |slots| {
                slots.get(&task_id).is_some_and(|slot| slot.step)
            })
            .unwrap_or_else(PoisonError::into_inner);
        !timeout.timed_out()
    }

    /// Whether a cancel of the task is in progress.
    #[cfg(test)]
    pub(super) fn cancel_in_progress(&self, task_id: TaskId) -> bool {
        self.lock().get(&task_id).is_some_and(|slot| slot.cancel)
    }

    /// Whether a step of the task is in flight.
    pub(super) fn step_in_flight(&self, task_id: TaskId) -> bool {
        self.lock().get(&task_id).is_some_and(|slot| slot.step)
    }

    /// How many requests wait for the task's step to end.
    #[cfg(test)]
    pub(super) fn waiting(&self, task_id: TaskId) -> u32 {
        self.lock().get(&task_id).map_or(0, |slot| slot.waiting)
    }

    /// How many tasks hold a slot.
    #[cfg(test)]
    pub(super) fn held(&self) -> usize {
        self.lock().len()
    }

    fn release(&self, task_id: TaskId, edit: impl FnOnce(&mut Slot)) {
        let mut slots = self.lock();
        if let Some(slot) = slots.get_mut(&task_id) {
            edit(slot);
            if *slot == Slot::default() {
                slots.remove(&task_id);
            }
        }
        drop(slots);
        self.changed.notify_all();
    }
}

impl StepGuard<'_> {
    /// Ends the step. What was noted for its end (a pause, a stop) is handed to `apply`, which
    /// writes it while the step still holds the task, and the claim is released only when
    /// nothing was noted meanwhile: a request that finds the step in flight is never lost
    /// between the step's last look and its release.
    pub(super) fn finish(mut self, mut apply: impl FnMut(Option<Note>, Option<Note>)) {
        loop {
            let (pause, stop) = {
                let mut slots = self.steps.lock();
                let Some(slot) = slots.get_mut(&self.task_id) else {
                    self.released = true;
                    return;
                };
                let notes = (slot.pause.take(), slot.stop.take());
                if notes.0.is_none() && notes.1.is_none() {
                    slot.step = false;
                    if *slot == Slot::default() {
                        slots.remove(&self.task_id);
                    }
                    drop(slots);
                    self.steps.changed.notify_all();
                    self.released = true;
                    return;
                }
                notes
            };
            apply(pause, stop);
        }
    }
}

impl Drop for StepGuard<'_> {
    fn drop(&mut self) {
        if !self.released {
            // The step ended without finishing (a dropped future, a panic): what was noted for
            // its end goes with it.
            self.steps.release(self.task_id, |slot| {
                slot.step = false;
                slot.pause = None;
                slot.stop = None;
            });
        }
    }
}

impl Drop for CancelGuard<'_> {
    fn drop(&mut self) {
        self.steps.release(self.task_id, |slot| slot.cancel = false);
    }
}

impl Drop for WriteGuard<'_> {
    fn drop(&mut self) {
        self.steps.release(self.task_id, |slot| slot.writer = false);
    }
}
