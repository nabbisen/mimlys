//! Panic containment at the runtime's call sites into adapters (REQ-SEC-011,
//! REQ-ENG-004; external design §21). Release builds unwind, so a panic in a
//! model backend or a tool provider can be stopped where the runtime calls
//! it, once, instead of in every adapter. Only `std` is used.
//!
//! A panic is caught in the call itself (before a future exists) and in every
//! poll of the returned future. The panicked future is never polled again and
//! is dropped under the same containment, because a destructor may panic too.

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
        let message = if let Some(text) = payload.downcast_ref::<&'static str>() {
            (*text).to_owned()
        } else if let Some(text) = payload.downcast_ref::<String>() {
            text.clone()
        } else {
            "a panic with a payload that is not text".to_owned()
        };
        // Dropping the payload is arbitrary code as well.
        let _ = catch_unwind(AssertUnwindSafe(move || drop(payload)));
        Self { message }
    }
}

/// Runs a synchronous call of an adapter under containment.
pub(super) fn contained_call<R>(call: impl FnOnce() -> R) -> Result<R, Panicked> {
    catch_unwind(AssertUnwindSafe(call)).map_err(Panicked::from_payload)
}

/// Makes an asynchronous call of an adapter and awaits it under containment:
/// a panic in `call` or in any poll of its future is `Err(Panicked)`.
pub(super) async fn contained<'a, T>(
    call: impl FnOnce() -> BoxFuture<'a, T>,
) -> Result<T, Panicked> {
    let future = contained_call(call)?;
    Contained {
        future: Some(future),
    }
    .await
}

struct Contained<'a, T> {
    future: Option<BoxFuture<'a, T>>,
}

impl<T> Future for Contained<'_, T> {
    type Output = Result<T, Panicked>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let Some(future) = self.future.as_mut() else {
            // Awaited once; a completed future is not polled again.
            return Poll::Pending;
        };
        let polled = catch_unwind(AssertUnwindSafe(|| future.as_mut().poll(cx)));
        match polled {
            Ok(Poll::Pending) => Poll::Pending,
            Ok(Poll::Ready(value)) => {
                self.finish();
                Poll::Ready(Ok(value))
            }
            Err(payload) => {
                let panicked = Panicked::from_payload(payload);
                self.finish();
                Poll::Ready(Err(panicked))
            }
        }
    }
}

impl<T> Contained<'_, T> {
    /// Drops the finished future under containment.
    fn finish(&mut self) {
        let future = self.future.take();
        let _ = catch_unwind(AssertUnwindSafe(move || drop(future)));
    }
}
