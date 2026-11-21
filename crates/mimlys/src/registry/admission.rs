//! The wait for a slot or a gate over the registry (RFC 013 decisions 5 and 7; external design
//! §19.3): the implementation of the core's `Admission`.
//!
//! One look is one bounded take (LIM-O11). A runtime that finds every slot held leaves a waiting
//! entry (so that `status` and `task show` can say who waits, for what and behind whom), looks
//! again every [`super::Bounds::poll`], and ends the wait at the first stop request, at the
//! task's deadline, or when a slot is free. A registry that cannot be used at any look ends the
//! wait with the execution going on unadmitted, and says why (the fail-open): the registry is a
//! liveness aid, and a defect of it must never stop the user's work.

use super::{Class, Handle, Kind, Owner, Registry, Take, endpoint_of};
use mimlys_core::spi::{
    Admission, AdmissionOutcome, AdmissionRequest, BoxFuture, Cancellation, Held, PersistenceError,
    WaitChange, WaitObserver,
};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::Instant;

/// The wait over one class of the registry.
pub struct Wait {
    registry: Arc<Registry>,
    class: Class,
    owner: Owner,
}

/// The execution slots of the user: at most `limit` at once across runtimes.
pub fn slots(registry: Arc<Registry>, limit: usize, owner: Owner) -> Arc<dyn Admission> {
    Arc::new(Wait {
        registry,
        class: Class {
            kind: Kind::Slot,
            key: None,
            url: None,
            limit,
        },
        owner,
    })
}

/// As [`slots`] over the registry in `dir`, which the platform may not have: no directory is an
/// admission that reports why at every execution and lets it go on (the fail-open).
pub fn slots_in(
    dir: Result<std::path::PathBuf, String>,
    limit: usize,
    owner: Owner,
) -> Arc<dyn Admission> {
    match dir {
        Ok(dir) => slots(
            Registry::new(
                dir,
                super::Bounds::default(),
                Arc::new(mimlys_core::clock::SystemClock),
            ),
            limit,
            owner,
        ),
        Err(reason) => Arc::new(Unavailable(reason)),
    }
}

/// As [`gate`] over the registry in `dir`, which the platform may not have (see [`slots_in`]).
pub fn gate_in(
    dir: Result<std::path::PathBuf, String>,
    url: &str,
    max_in_flight: usize,
    owner: Owner,
) -> Option<Arc<dyn Admission>> {
    match dir {
        Ok(dir) => gate(
            Registry::new(
                dir,
                super::Bounds::default(),
                Arc::new(mimlys_core::clock::SystemClock),
            ),
            url,
            max_in_flight,
            owner,
        ),
        Err(reason) => {
            endpoint_of(url).map(|_| Arc::new(Unavailable(reason)) as Arc<dyn Admission>)
        }
    }
}

/// An admission with no registry to use: the reason, at every look.
struct Unavailable(String);

impl Admission for Unavailable {
    fn admit<'a>(
        &'a self,
        _request: AdmissionRequest,
        _stop: Cancellation,
        changed: WaitObserver<'a>,
    ) -> BoxFuture<'a, Result<AdmissionOutcome, PersistenceError>> {
        Box::pin(async move {
            changed(WaitChange::Unadmitted {
                reason: self.0.clone(),
            })?;
            Ok(AdmissionOutcome::Admitted {
                held: None,
                waited_ms: 0,
            })
        })
    }
}

/// The gate of one backend's endpoint: at most `max_in_flight` model requests of the user at
/// once across runtimes. `None` when `url` names no endpoint (nothing to share a gate over).
pub fn gate(
    registry: Arc<Registry>,
    url: &str,
    max_in_flight: usize,
    owner: Owner,
) -> Option<Arc<dyn Admission>> {
    let (key, shown) = endpoint_of(url)?;
    Some(Arc::new(Wait {
        registry,
        class: Class {
            kind: Kind::Gate,
            key: Some(key),
            url: Some(shown),
            limit: max_in_flight,
        },
        owner,
    }))
}

/// What a slot or a gate that is held is: the entry, removed when the guard is dropped.
struct HeldEntry(#[allow(dead_code)] Handle);

/// How often a wait looks at the stop request between two looks at the registry.
const STOP_LOOK: Duration = Duration::from_millis(20);

impl Admission for Wait {
    fn admit<'a>(
        &'a self,
        request: AdmissionRequest,
        stop: Cancellation,
        changed: WaitObserver<'a>,
    ) -> BoxFuture<'a, Result<AdmissionOutcome, PersistenceError>> {
        Box::pin(async move {
            // The check before the slot: a request already made takes no slot.
            if stop.is_cancelled() {
                return Ok(AdmissionOutcome::Stopped { waited_ms: 0 });
            }
            let task = Some(request.task_id.to_string());
            let deadline =
                Instant::now() + Duration::from_millis(request.remaining_ms.max(0) as u64);
            let mut waiting_since: Option<Instant> = None;
            let mut waiter: Option<Handle> = None;
            loop {
                let look = self
                    .registry
                    .take(&self.class, task.clone(), &self.owner, waiter.take())
                    .await;
                let waited =
                    |since: Option<Instant>| since.map_or(0, |s| s.elapsed().as_millis() as u64);
                match look {
                    Ok(Take::Granted { handle, taken }) => {
                        // A request that came while the slot was being taken: it goes back.
                        if stop.is_cancelled() {
                            drop(handle);
                            return Ok(AdmissionOutcome::Stopped {
                                waited_ms: waited(waiting_since),
                            });
                        }
                        let waited_ms = waited(waiting_since);
                        if waiting_since.is_some() {
                            changed(WaitChange::Admitted {
                                slots_taken: taken,
                                waited_ms,
                            })?;
                        }
                        return Ok(AdmissionOutcome::Admitted {
                            held: Some(Held::new(HeldEntry(handle))),
                            waited_ms,
                        });
                    }
                    Ok(Take::Busy {
                        taken,
                        holders,
                        waiter: entry,
                    }) => {
                        waiter = Some(entry);
                        if waiting_since.is_none() {
                            waiting_since = Some(Instant::now());
                            changed(WaitChange::Waiting {
                                slots_taken: taken,
                                holders,
                            })?;
                        }
                    }
                    // The fail-open: the execution goes on as if alone, and says why.
                    Err(reason) => {
                        changed(WaitChange::Unadmitted { reason })?;
                        return Ok(AdmissionOutcome::Admitted {
                            held: None,
                            waited_ms: waited(waiting_since),
                        });
                    }
                }
                // Between two looks: the stop request and the deadline are watched often, the
                // registry only every poll.
                let next = Instant::now() + self.registry.bounds().poll;
                while Instant::now() < next {
                    if stop.is_cancelled() {
                        return Ok(AdmissionOutcome::Stopped {
                            waited_ms: waited(waiting_since),
                        });
                    }
                    if Instant::now() >= deadline {
                        return Ok(AdmissionOutcome::DeadlinePassed {
                            waited_ms: waited(waiting_since),
                        });
                    }
                    tokio::time::sleep(STOP_LOOK.min(next - Instant::now())).await;
                }
            }
        })
    }
}
