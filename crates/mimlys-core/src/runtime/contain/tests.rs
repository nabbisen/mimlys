//! Containment of an adapter's future (REQ-SEC-011): a panic in the call, in
//! a poll, in the destructor of a finished future and in the destructor of a
//! future dropped while it was still pending.

use super::*;
use std::sync::{Arc, Mutex};
use std::task::Waker;

/// A future that panics when dropped, and optionally never completes.
struct PanicsOnDrop {
    message: &'static str,
    completes: bool,
}

impl Future for PanicsOnDrop {
    type Output = u32;
    fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<u32> {
        if self.completes {
            Poll::Ready(7)
        } else {
            Poll::Pending
        }
    }
}

impl Drop for PanicsOnDrop {
    fn drop(&mut self) {
        panic!("{}", self.message);
    }
}

fn reports() -> (Arc<Mutex<Vec<String>>>, impl Fn(&Panicked) + Unpin) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&seen);
    (seen, move |p: &Panicked| {
        sink.lock().unwrap().push(p.message.clone())
    })
}

/// REQ-SEC-011: a future dropped while it is still pending is dropped under
/// containment, and a panic in its destructor is reported, not raised.
#[test]
fn req_sec_011_a_pending_future_dropped_by_cancellation_panics_contained() {
    let (seen, report) = reports();
    let mut call = Box::pin(contained(
        || -> BoxFuture<'static, u32> {
            Box::pin(PanicsOnDrop {
                message: "pending drop",
                completes: false,
            })
        },
        report,
    ));
    let mut cx = Context::from_waker(Waker::noop());
    assert!(call.as_mut().poll(&mut cx).is_pending());
    let dropped = catch_unwind(AssertUnwindSafe(move || drop(call)));
    assert!(dropped.is_ok(), "the panic did not escape the drop");
    assert_eq!(*seen.lock().unwrap(), ["pending drop"]);
}

/// REQ-SEC-011: a future that finished and panics in its destructor is
/// contained: the value is returned, the panic is reported.
#[test]
fn req_sec_011_a_finished_future_that_panics_on_drop_is_contained() {
    let (seen, report) = reports();
    let mut call = Box::pin(contained(
        || -> BoxFuture<'static, u32> {
            Box::pin(PanicsOnDrop {
                message: "finished drop",
                completes: true,
            })
        },
        report,
    ));
    let mut cx = Context::from_waker(Waker::noop());
    let polled = catch_unwind(AssertUnwindSafe(|| call.as_mut().poll(&mut cx)));
    assert!(matches!(polled, Ok(Poll::Ready(Ok(7)))));
    assert_eq!(*seen.lock().unwrap(), ["finished drop"]);
}

/// REQ-SEC-011: a panic in the call or in a poll is returned to the caller,
/// who logs it; it is not reported twice.
#[test]
fn req_sec_011_a_panic_in_the_call_or_a_poll_is_returned_not_reported() {
    let (seen, report) = reports();
    let mut cx = Context::from_waker(Waker::noop());
    let mut in_call = Box::pin(contained(
        || -> BoxFuture<'static, u32> { panic!("in the call") },
        report,
    ));
    match in_call.as_mut().poll(&mut cx) {
        Poll::Ready(Err(p)) => assert_eq!(p.message, "in the call"),
        other => panic!("{other:?}"),
    }
    let (_, report) = reports();
    let mut in_poll = Box::pin(contained(
        || -> BoxFuture<'static, u32> { Box::pin(async { panic!("in a poll") }) },
        report,
    ));
    match in_poll.as_mut().poll(&mut cx) {
        Poll::Ready(Err(p)) => assert_eq!(p.message, "in a poll"),
        other => panic!("{other:?}"),
    }
    assert!(seen.lock().unwrap().is_empty());
}

/// REQ-SEC-011: while a contained region runs, the thread says so, so that
/// the process-wide hook stays silent for a panic the runtime logs itself.
#[test]
fn req_sec_011_a_contained_region_is_marked_for_the_panic_hook() {
    use crate::panic_text::inside_containment;
    assert!(!inside_containment());
    let seen = contained_call(inside_containment).unwrap();
    assert!(seen, "inside the call");
    assert!(!inside_containment(), "and no longer after it");
}
