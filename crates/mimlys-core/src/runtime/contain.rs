//! Panic containment at the runtime's call sites into adapters (REQ-SEC-011,
//! REQ-ENG-004; external design §21). Release builds unwind, so a panic in a
//! model backend or a tool provider can be stopped where the runtime calls
//! it, once, instead of in every adapter. Only `std` is used.
//!
//! A panic is caught in the call itself (before a future exists) and in every
//! poll of the returned future. The panicked future is never polled again and
//! is dropped under the same containment, because a destructor may panic too.
//! So is a future that is still pending when the call is given up (the task
//! that awaits it is cancelled): its `Drop` drops it under containment.
//!
//! A panic that is caught here is returned (call, poll) or reported (drop) to
//! the runtime, which logs it bounded and redacted; the process-wide hook of
//! the binaries is silent for it (`panic_text::inside_containment`).

use crate::panic_text::{InsideContainment, payload_text};
use crate::spi::BoxFuture;
use std::any::Any;
use std::future::Future;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::Pin;
use std::task::{Context, Poll};

/// A panic that was stopped. The message is the adapter's own text, neither
/// bounded nor redacted yet: the runtime does both before it logs it.
#[derive(Debug)]
pub(super) struct Panicked {
    pub(super) message: String,
}

impl Panicked {
    fn from_payload(payload: Box<dyn Any + Send>) -> Self {
        let message = payload_text(payload.as_ref());
        // Dropping the payload is arbitrary code as well.
        let _ = catch_unwind(AssertUnwindSafe(move || drop(payload)));
        Self { message }
    }
}

/// Runs `f`, catching a panic, with the thread marked as inside containment.
fn catch<R>(f: impl FnOnce() -> R) -> Result<R, Panicked> {
    let inside = InsideContainment::enter();
    let caught = catch_unwind(AssertUnwindSafe(f));
    drop(inside);
    caught.map_err(Panicked::from_payload)
}

/// Runs a synchronous call of an adapter under containment.
pub(super) fn contained_call<R>(call: impl FnOnce() -> R) -> Result<R, Panicked> {
    catch(call)
}

/// Makes an asynchronous call of an adapter and awaits it under containment:
/// a panic in `call` or in any poll of its future is `Err(Panicked)`. A panic
/// while the future is dropped (after it finished, or when this future is
/// dropped before it did) cannot be returned: it goes to `report`.
pub(super) async fn contained<'a, T>(
    call: impl FnOnce() -> BoxFuture<'a, T>,
    report: impl Fn(&Panicked) + Unpin,
) -> Result<T, Panicked> {
    let future = contained_call(call)?;
    Contained {
        future: Some(future),
        report,
    }
    .await
}

struct Contained<'a, T, R: Fn(&Panicked) + Unpin> {
    future: Option<BoxFuture<'a, T>>,
    report: R,
}

impl<T, R: Fn(&Panicked) + Unpin> Future for Contained<'_, T, R> {
    type Output = Result<T, Panicked>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // `Contained` is `Unpin`: its future is boxed and `R` is `Unpin`.
        let this = self.get_mut();
        let Some(future) = this.future.as_mut() else {
            // Awaited once; a completed future is not polled again.
            return Poll::Pending;
        };
        match catch(|| future.as_mut().poll(cx)) {
            Ok(Poll::Pending) => Poll::Pending,
            Ok(Poll::Ready(value)) => {
                this.finish();
                Poll::Ready(Ok(value))
            }
            Err(panicked) => {
                this.finish();
                Poll::Ready(Err(panicked))
            }
        }
    }
}

impl<T, R: Fn(&Panicked) + Unpin> Contained<'_, T, R> {
    /// Drops the future, if one is left, under containment; a panic in its
    /// destructor is reported.
    fn finish(&mut self) {
        if let Some(future) = self.future.take()
            && let Err(panicked) = catch(move || drop(future))
        {
            // A panic in the report itself must not leave this frame.
            let _ = catch(|| (self.report)(&panicked));
        }
    }
}

impl<T, R: Fn(&Panicked) + Unpin> Drop for Contained<'_, T, R> {
    /// A call given up while still pending: the future is dropped here, under
    /// containment, not by the field's own drop glue.
    fn drop(&mut self) {
        self.finish();
    }
}

#[cfg(test)]
mod tests;
