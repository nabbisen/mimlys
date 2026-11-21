//! A scripted admission for tests (RFC 013, REQ-TEST-002): it answers from a script, counts the
//! slots held, and calls a hook inside the wait, where a test makes the stop request or looks at
//! the store. It reads no clock and never pends.

use crate::spi::{
    Admission, AdmissionOutcome, AdmissionRequest, BoxFuture, Cancellation, Held, PersistenceError,
    WaitChange, WaitObserver,
};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// What one call of [`ScriptedAdmission::admit`] does.
#[derive(Clone, Debug)]
pub enum Script {
    /// A free slot at once.
    Admit,
    /// No free slot: reports `Waiting`, runs the hook (the other runtime's execution ends
    /// there), then reports `Admitted`.
    WaitThenAdmit {
        /// The processes that hold the slots.
        holders: Vec<u32>,
        /// How long the wait lasted.
        waited_ms: u64,
    },
    /// No free slot: reports `Waiting`, runs the hook (which makes the stop request), and ends
    /// the wait only if the token it was given is set: a wait given a token that the request
    /// does not reach fails the test.
    WaitUntilStopped,
    /// No free slot, and the task's deadline passes during the wait.
    WaitUntilDeadline,
    /// The registry cannot be used: reports `Unadmitted` and goes on with no slot.
    Unadmitted(String),
}

/// The scripted admission.
#[derive(Default)]
pub struct ScriptedAdmission {
    script: Mutex<VecDeque<Script>>,
    held: Arc<AtomicUsize>,
    taken: AtomicUsize,
    requests: Mutex<Vec<AdmissionRequest>>,
    hook: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

/// One slot taken: the count goes down when it is dropped.
struct Slot(Arc<AtomicUsize>);

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl ScriptedAdmission {
    /// An admission that answers `script` in order, then `Admit`.
    pub fn new(script: impl IntoIterator<Item = Script>) -> Arc<Self> {
        Arc::new(Self {
            script: Mutex::new(script.into_iter().collect()),
            ..Self::default()
        })
    }

    /// Runs `hook` inside every call (inside the wait, when it waits, and before the answer).
    pub fn on_admit(&self, hook: impl Fn() + Send + Sync + 'static) {
        *self.hook.lock().unwrap() = Some(Arc::new(hook));
    }

    /// Slots held right now.
    pub fn held(&self) -> usize {
        self.held.load(Ordering::SeqCst)
    }

    /// Slots taken in all.
    pub fn taken(&self) -> usize {
        self.taken.load(Ordering::SeqCst)
    }

    /// The requests it was given.
    pub fn requests(&self) -> Vec<AdmissionRequest> {
        self.requests.lock().unwrap().clone()
    }

    fn take(&self) -> Held {
        self.held.fetch_add(1, Ordering::SeqCst);
        self.taken.fetch_add(1, Ordering::SeqCst);
        Held::new(Slot(Arc::clone(&self.held)))
    }
}

impl Admission for ScriptedAdmission {
    fn admit<'a>(
        &'a self,
        request: AdmissionRequest,
        stop: Cancellation,
        changed: WaitObserver<'a>,
    ) -> BoxFuture<'a, Result<AdmissionOutcome, PersistenceError>> {
        Box::pin(async move {
            self.requests.lock().unwrap().push(request);
            let hook = self.hook.lock().unwrap().clone();
            let call_hook = || {
                if let Some(hook) = &hook {
                    hook();
                }
            };
            let step = self
                .script
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Script::Admit);
            match step {
                Script::Admit => {
                    call_hook();
                    Ok(AdmissionOutcome::Admitted {
                        held: Some(self.take()),
                        waited_ms: 0,
                    })
                }
                Script::WaitThenAdmit { holders, waited_ms } => {
                    changed(WaitChange::Waiting {
                        slots_taken: holders.len() as u32,
                        holders,
                    })?;
                    call_hook();
                    changed(WaitChange::Admitted {
                        slots_taken: 1,
                        waited_ms,
                    })?;
                    Ok(AdmissionOutcome::Admitted {
                        held: Some(self.take()),
                        waited_ms,
                    })
                }
                Script::WaitUntilStopped => {
                    changed(WaitChange::Waiting {
                        slots_taken: 1,
                        holders: vec![4242],
                    })?;
                    call_hook();
                    assert!(
                        stop.is_cancelled(),
                        "the wait was given a token that the first stop request does not reach"
                    );
                    Ok(AdmissionOutcome::Stopped { waited_ms: 7 })
                }
                Script::WaitUntilDeadline => {
                    changed(WaitChange::Waiting {
                        slots_taken: 1,
                        holders: vec![4242],
                    })?;
                    call_hook();
                    Ok(AdmissionOutcome::DeadlinePassed { waited_ms: 7 })
                }
                Script::Unadmitted(reason) => {
                    changed(WaitChange::Unadmitted { reason })?;
                    call_hook();
                    Ok(AdmissionOutcome::Admitted {
                        held: None,
                        waited_ms: 0,
                    })
                }
            }
        })
    }
}
