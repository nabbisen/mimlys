//! The span a reader that logs on its own thread or task writes its lines
//! in (REQ-OBS-004). A thread or a spawned task does not inherit the span of
//! the step that started it, and a reader of a process's standard error
//! outlives any one step (a worker serves many steps; an MCP server lives
//! across calls). So the reader cannot capture the span where it is spawned:
//! the code that runs a call or an execution registers its own span here for
//! the duration of the call, and the reader asks, for each line it reads,
//! which span to write it in.
//!
//! What this can tell and what it cannot: a line of standard error is
//! asynchronous, written by a process that does not say which request it
//! concerns, so a line is attributed by the moment the reader *reads* it. It
//! is written in the span of the call in flight at that moment when exactly
//! one call is in flight, and in no span otherwise (none in flight, or
//! several, where a line cannot be told apart). A line that the process wrote
//! during a call but that the reader reads after the call ended has no ids.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tracing::Span;

/// The calls in flight on one process and their spans. Cloning shares them.
#[derive(Clone, Debug, Default)]
pub struct ReaderSpan {
    calls: Arc<Mutex<Vec<(u64, Span)>>>,
    next: Arc<AtomicU64>,
}

impl ReaderSpan {
    /// A registry with no call in flight.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers the calling context's current span (the step's, when a step
    /// is running this call) as a call in flight until the guard drops.
    pub fn begin(&self) -> ReaderSpanGuard {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        self.lock().push((id, Span::current()));
        ReaderSpanGuard {
            id,
            owner: self.clone(),
        }
    }

    /// The span to write a line in now: that of the one call in flight, else none.
    pub fn current(&self) -> Span {
        match self.lock().as_slice() {
            [(_, span)] => span.clone(),
            _ => Span::none(),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<(u64, Span)>> {
        self.calls.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// A call in flight; ends when dropped.
#[derive(Debug)]
pub struct ReaderSpanGuard {
    id: u64,
    owner: ReaderSpan,
}

impl Drop for ReaderSpanGuard {
    fn drop(&mut self) {
        self.owner.lock().retain(|(id, _)| *id != self.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The span to write in is the one call's when exactly one is in flight,
    /// and none with none or with several.
    #[test]
    fn one_call_in_flight_gives_its_span_and_otherwise_none() {
        tracing::subscriber::with_default(tracing_subscriber::registry(), || {
            let reader = ReaderSpan::new();
            assert!(reader.current().id().is_none(), "no call in flight");
            let first = tracing::error_span!("first");
            let second = tracing::error_span!("second");
            assert!(first.id().is_some() && second.id().is_some());
            let guard_first = first.in_scope(|| reader.begin());
            assert_eq!(reader.current().id(), first.id());
            let guard_second = second.in_scope(|| reader.begin());
            assert!(
                reader.current().id().is_none(),
                "two calls in flight: a line cannot be told apart"
            );
            drop(guard_first);
            assert_eq!(reader.current().id(), second.id());
            drop(guard_second);
            assert!(reader.current().id().is_none(), "the call ended");
        });
    }
}
