//! The request that a change is made under (external design §4, "The session and the request
//! id"; RFC 014, Amendment 1, item 2; handoff 014b, item 6).
//!
//! **Recorded.** Every event that a state-changing request's change writes carries the request's
//! id in its envelope (`request_id`, external design §17.1). The events are made in the verbs and
//! in the helpers the verbs share with the step (a snapshot, a closed action), so the id is not
//! passed down some sixty signatures: the handler enters a [`Scope`] for the request, which is
//! the thread's for as long as the handler runs, and `Runtime::event` reads it. A handler is
//! synchronous, never yields, and starts no step, so the scope is exactly the request's change;
//! the step's own events, made on the step's thread, carry none.
//!
//! **Repeated.** A request whose id is already recorded is answered with the recorded outcome and
//! applies nothing again. Two requests with one id that are in flight together (a client that
//! sent it again while the first was still handled) are made to take turns ([`InFlight`]), so
//! that the second finds what the first recorded.

use crate::ids::RequestId;
use std::cell::RefCell;
use std::collections::HashSet;
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};

thread_local! {
    static CURRENT: RefCell<Option<RequestId>> = const { RefCell::new(None) };
}

/// The request whose change this thread is writing, until dropped.
#[must_use = "the scope ends when it is dropped"]
pub(super) struct Scope {
    previous: Option<RequestId>,
}

impl Scope {
    /// Makes `id` the request of this thread.
    pub(super) fn enter(id: RequestId) -> Self {
        Self {
            previous: CURRENT.with(|current| current.borrow_mut().replace(id)),
        }
    }
}

impl Drop for Scope {
    fn drop(&mut self) {
        let previous = self.previous.take();
        CURRENT.with(|current| *current.borrow_mut() = previous);
    }
}

/// The request this thread is writing the change of, if any: what `Runtime::event` records.
pub(super) fn current() -> Option<RequestId> {
    CURRENT.with(|current| *current.borrow())
}

/// The ids of the requests being handled now.
#[derive(Default)]
pub(super) struct InFlight {
    ids: Mutex<HashSet<RequestId>>,
    done: Condvar,
}

/// A request being handled: its id is free again when this is dropped.
#[must_use = "the id is held while this is"]
pub(super) struct Turn<'a> {
    in_flight: &'a InFlight,
    id: RequestId,
}

impl InFlight {
    fn lock(&self) -> MutexGuard<'_, HashSet<RequestId>> {
        self.ids.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Waits until no other request of this id is being handled, and takes the id.
    pub(super) fn enter(&self, id: RequestId) -> Turn<'_> {
        let mut ids = self.lock();
        while ids.contains(&id) {
            ids = self.done.wait(ids).unwrap_or_else(PoisonError::into_inner);
        }
        ids.insert(id);
        Turn {
            in_flight: self,
            id,
        }
    }

    /// How many ids are held (tests).
    #[cfg(test)]
    pub(super) fn held(&self) -> usize {
        self.lock().len()
    }
}

impl Drop for Turn<'_> {
    fn drop(&mut self) {
        self.in_flight.lock().remove(&self.id);
        self.in_flight.done.notify_all();
    }
}
