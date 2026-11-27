//! Embedded mode's transport and the host of its driver (RFC 014, Amendment 1, item 1; external
//! design §4, "The driver and the follower"; REQ-ARCH-009).
//!
//! The runtime is in the command's process, and so is the driver that runs the steps of its
//! tasks: a task that a request made `running` is handed to the driver, which a task of the
//! process's executor steps one at a time (LIM-X1) until the task waits or ends. **The driver
//! lives as long as the command**: it ends with the process's executor, and a command that has no
//! terminal ends at a wait, as before. A follower never runs a step: it waits here for the driver
//! to say that something changed.

use crate::client::Transport;
use mimlys_core::api::{Envelope, Notice, Response};
use mimlys_core::driver::{Driver, Turn};
use mimlys_core::ids::TaskId;
use mimlys_core::runtime::{Runtime, RuntimeError};
use std::sync::{Arc, Mutex, PoisonError, Weak};
use tokio::sync::Notify;
use tokio::task::JoinHandle;

/// The in-process transport: requests go to the driver's handler, and the driver's steps run on
/// the executor of the process.
pub struct InProcess {
    driver: Driver,
    /// The driver's loop is told that there is something to do.
    wake: Arc<Notify>,
    /// A follower is told that something changed.
    changed: Arc<Notify>,
    /// The driver's loop, once started (at the first request that gives it work).
    task: Mutex<Option<JoinHandle<()>>>,
    me: Weak<InProcess>,
}

impl InProcess {
    /// The transport to `runtime`, whose driver has not started: it starts at the first task it
    /// is handed, inside the executor of the process.
    #[must_use]
    pub fn new(runtime: Arc<Runtime>) -> Arc<Self> {
        Arc::new_cyclic(|me| Self {
            driver: Driver::new(runtime),
            wake: Arc::new(Notify::new()),
            changed: Arc::new(Notify::new()),
            task: Mutex::new(None),
            me: me.clone(),
        })
    }

    /// The driver, for the tests that look at what it owns.
    #[must_use]
    pub fn driver(&self) -> &Driver {
        &self.driver
    }

    /// Starts the driver's loop on the executor of the process, once. Outside an executor there
    /// is no loop to start: nothing runs steps, which is what a command without one means.
    fn ensure_driving(&self) {
        let mut task = self.task.lock().unwrap_or_else(PoisonError::into_inner);
        if task.is_some() {
            return;
        }
        let Ok(executor) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let (me, wake) = (self.me.clone(), Arc::clone(&self.wake));
        *task = Some(executor.spawn(drive(me, wake)));
    }

    fn notify(&self) {
        self.wake.notify_one();
        self.changed.notify_one();
    }
}

/// The driver's loop. It holds the transport only while it steps, so that the loop ends with it.
async fn drive(me: Weak<InProcess>, wake: Arc<Notify>) {
    loop {
        let Some(host) = me.upgrade() else { return };
        match host.driver.step_once().await {
            Turn::Ran { .. } | Turn::Released(_) | Turn::Failed(_) => {
                host.changed.notify_one();
                drop(host);
                // A step blocks its thread in the store's writes, and what it started meanwhile
                // (a server's notice that its tools changed, a signal) waits on that thread's
                // queue: the next step's boundary comes after those have had their turn.
                tokio::task::yield_now().await;
            }
            Turn::Idle => {
                host.changed.notify_one();
                drop(host);
                wake.notified().await;
            }
            // A request holds the task: it asks again when the request is done, or soon.
            Turn::Deferred(_) => {
                drop(host);
                let _ = tokio::time::timeout(
                    std::time::Duration::from_millis(crate::commands::events::EVENTS_POLL_MS),
                    wake.notified(),
                )
                .await;
            }
        }
    }
}

impl Transport for InProcess {
    fn send(&self, envelope: Envelope, notice: &dyn Fn(Notice)) -> Response {
        let response = self.driver.handle_noticing(envelope, notice);
        // The request may have made a task `running`, or ended the wait of a task.
        self.ensure_driving();
        self.notify();
        response
    }

    fn attach(&self, task_id: TaskId) {
        self.driver.hand_over(task_id);
        self.ensure_driving();
        self.notify();
    }

    fn changes(&self) -> Arc<Notify> {
        Arc::clone(&self.changed)
    }

    fn ended_driver(&self) -> Option<JoinHandle<()>> {
        let mut task = self.task.lock().unwrap_or_else(PoisonError::into_inner);
        task.take_if(|t| t.is_finished())
    }

    fn failure(&self, task_id: TaskId) -> Option<RuntimeError> {
        self.driver.take_failure(task_id)
    }
}

impl Drop for InProcess {
    fn drop(&mut self) {
        if let Some(task) = self
            .task
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        {
            task.abort();
        }
    }
}

#[cfg(test)]
mod tests;
