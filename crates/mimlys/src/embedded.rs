//! Embedded mode's transport and the host of its driver (RFC 014, Amendment 1, item 1; external
//! design §4, "The driver and the follower"; REQ-ARCH-009).
//!
//! The runtime is in the command's process, and so is the driver that runs the steps of its
//! tasks: a task that a request made `running` is handed to the driver, which a task of the
//! process's executor steps one at a time (LIM-X1) until the task waits or ends. **The driver
//! lives as long as the command**: it ends with the process's executor, and a command that has no
//! terminal ends at a wait, as before. A follower never runs a step: it waits here for the driver
//! to say that something changed.

use crate::client::{ConnectionBroken, Transport};
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
    /// One creation or resumption at a time. A daemon takes requests on a thread of each
    /// connection: two creations at once would both find the line free and both start (LIM-X1 then
    /// refuses the second with `conflict`, though it should wait in line), and would write the
    /// same snapshot objects at the same moment. Held only for the request itself, which does not
    /// wait for a step, so that no stop request is held behind it (handoff 014d).
    starting: Mutex<()>,
    me: Weak<InProcess>,
}

impl InProcess {
    /// The transport to `runtime`, whose driver has not started: it starts at the first task it
    /// is handed, inside the executor of the process.
    #[must_use]
    pub fn new(runtime: Arc<Runtime>) -> Arc<Self> {
        Self::over(Driver::new(runtime))
    }

    /// The transport of a daemon's host: the same, over a driver that keeps a line (external
    /// design §19.5): a resume that LIM-X1 does not allow yet waits and starts in its turn.
    #[must_use]
    pub fn with_line(runtime: Arc<Runtime>) -> Arc<Self> {
        Self::over(Driver::with_line(runtime))
    }

    fn over(driver: Driver) -> Arc<Self> {
        Arc::new_cyclic(|me| Self {
            driver,
            wake: Arc::new(Notify::new()),
            changed: Arc::new(Notify::new()),
            task: Mutex::new(None),
            starting: Mutex::new(()),
            me: me.clone(),
        })
    }

    /// The driver, for the tests that look at what it owns.
    #[must_use]
    pub fn driver(&self) -> &Driver {
        &self.driver
    }

    /// Starts the driver's loop now, on the executor of the caller (the daemon starts it at its
    /// own start: its requests arrive on threads that have no executor).
    pub fn start_driving(&self) {
        self.ensure_driving();
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
    fn send(
        &self,
        envelope: Envelope,
        notice: &dyn Fn(Notice),
    ) -> Result<Response, ConnectionBroken> {
        let _one_at_a_time = matches!(envelope.request.name(), "create_task" | "resume_task")
            .then(|| self.starting.lock().unwrap_or_else(PoisonError::into_inner));
        let response = self.driver.handle_noticing(envelope, notice);
        // The request may have made a task `running`, or ended the wait of a task.
        self.ensure_driving();
        self.notify();
        // An in-process connection does not break.
        Ok(response)
    }

    fn queued(&self, task_id: TaskId) -> Option<mimlys_core::api::Line> {
        self.driver.queued(task_id)
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
