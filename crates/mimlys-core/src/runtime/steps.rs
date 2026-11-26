//! Which tasks have a step in flight, and for which a cancel is in progress
//! (REQ-DEC-001, REQ-TASK-006; external design §4, "Steps of one task never
//! overlap").
//!
//! The runtime's own, so that no caller can bypass it: neither the command
//! line's loop today nor a scheduler that drives many tasks. It is a guard per
//! task, not a lock around the store: a step of another task is never delayed
//! by it, and a refused step never reaches the store.
//!
//! Two parties write a task's state while a step is in flight: the step
//! itself, from the task it loaded when it began, and `cancel_task`. Neither
//! may write over the other. The slot makes them take turns without either
//! waiting for a lock it cannot see:
//!
//! - a step begins only when no step of the task is in flight and no cancel of
//!   it is in progress (otherwise it is refused as `conflict`, and writes
//!   nothing);
//! - a cancel marks the slot, so that no step begins meanwhile, asks the step
//!   in flight to stop, waits for it to end, and only then reads the task and
//!   writes. Whichever of the two comes second finds the other's work done.

use crate::ids::TaskId;
use crate::runtime::RuntimeError;
use std::collections::HashMap;
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

/// What the slot of a task says.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Slot {
    /// A step of the task is in flight.
    step: bool,
    /// A cancel of the task is in progress.
    cancel: bool,
}

/// The slots of the tasks that have a step in flight or a cancel in progress;
/// a task with neither has no entry, so that a long-lived runtime keeps
/// nothing for the tasks that have ended.
#[derive(Default)]
pub(super) struct Steps {
    slots: Mutex<HashMap<TaskId, Slot>>,
    /// Signalled when a step ends.
    ended: Condvar,
}

/// A step in flight: held for the whole of `run_step`, released when it ends
/// in any way, a dropped future included.
#[must_use = "the guard is the step's claim on its task"]
pub(super) struct StepGuard<'a> {
    steps: &'a Steps,
    task_id: TaskId,
}

/// A cancel in progress: no step of the task begins while it is held.
#[must_use = "the guard is the cancel's claim on its task"]
pub(super) struct CancelGuard<'a> {
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
        if slot.cancel {
            return Err(RuntimeError::Conflict {
                detail: format!("task {task_id} is being cancelled: no step of it begins"),
            });
        }
        if slot.step {
            return Err(RuntimeError::Conflict {
                detail: format!(
                    "a step of task {task_id} is in flight: steps of one task never overlap"
                ),
            });
        }
        slot.step = true;
        Ok(StepGuard {
            steps: self,
            task_id,
        })
    }

    /// Claims the task for a cancel, or refuses it as `conflict` when another
    /// cancel of it is in progress. The flag says whether a step is in flight.
    pub(super) fn begin_cancel(
        &self,
        task_id: TaskId,
    ) -> Result<(CancelGuard<'_>, bool), RuntimeError> {
        let mut slots = self.lock();
        let slot = slots.entry(task_id).or_default();
        if slot.cancel {
            return Err(RuntimeError::Conflict {
                detail: format!("a cancel of task {task_id} is already in progress"),
            });
        }
        slot.cancel = true;
        Ok((
            CancelGuard {
                steps: self,
                task_id,
            },
            slot.step,
        ))
    }

    /// Waits until no step of the task is in flight, for at most `within`;
    /// whether none is.
    pub(super) fn wait_for_step_end(&self, task_id: TaskId, within: Duration) -> bool {
        let (_slots, timeout) = self
            .ended
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
        self.ended.notify_all();
    }
}

impl Drop for StepGuard<'_> {
    fn drop(&mut self) {
        self.steps.release(self.task_id, |slot| slot.step = false);
    }
}

impl Drop for CancelGuard<'_> {
    fn drop(&mut self) {
        self.steps.release(self.task_id, |slot| slot.cancel = false);
    }
}
