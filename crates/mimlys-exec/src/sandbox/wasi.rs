//! The WASI boundary (REQ-SBX-004, REQ-SBX-005, REQ-SEC-006): exactly the
//! workspace and an ephemeral tmp, declared args and stdin, an empty
//! environment, bounded output streams that interrupt the execution when
//! full, coarse clocks and a replayable random source, no sockets.

use super::guards::{Captured, Interrupt, InterruptReason};
use bytes::Bytes;
use mimlys_core::limits::LIM_S6_CLOCK_GRANULARITY_MS;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio::io::AsyncWrite;
use wasmtime_wasi::cli::{IsTerminal, StdoutStream};
use wasmtime_wasi::p2::{OutputStream, Pollable, StreamError};
use wasmtime_wasi::{HostMonotonicClock, HostWallClock};

/// A bounded capture: keeps at most `capacity` bytes and, on overflow,
/// records `output_limit` and interrupts the execution.
#[derive(Clone)]
pub struct BoundedOutput {
    captured: Arc<Captured>,
    capacity: usize,
    interrupt: Interrupt,
}

impl BoundedOutput {
    pub(crate) fn new(capacity: usize, interrupt: Interrupt) -> Self {
        Self {
            captured: Arc::new(Captured {
                bytes: std::sync::Mutex::new(Vec::new()),
            }),
            capacity,
            interrupt,
        }
    }

    /// The bytes captured so far (at most the capacity).
    pub fn contents(&self) -> Vec<u8> {
        self.captured.bytes.lock().expect("capture lock").clone()
    }

    fn push(&self, bytes: &[u8]) -> Result<(), ()> {
        let mut buf = self.captured.bytes.lock().expect("capture lock");
        let room = self.capacity.saturating_sub(buf.len());
        if bytes.len() > room {
            buf.extend_from_slice(&bytes[..room]);
            drop(buf);
            self.interrupt.fire(InterruptReason::OutputLimit);
            return Err(());
        }
        buf.extend_from_slice(bytes);
        Ok(())
    }
}

impl IsTerminal for BoundedOutput {
    fn is_terminal(&self) -> bool {
        false
    }
}

impl StdoutStream for BoundedOutput {
    fn p2_stream(&self) -> Box<dyn OutputStream> {
        Box::new(self.clone())
    }

    fn async_stream(&self) -> Box<dyn AsyncWrite + Send + Sync> {
        Box::new(self.clone())
    }
}

#[wasmtime_wasi::async_trait]
impl Pollable for BoundedOutput {
    async fn ready(&mut self) {}
}

#[wasmtime_wasi::async_trait]
impl OutputStream for BoundedOutput {
    fn write(&mut self, bytes: Bytes) -> Result<(), StreamError> {
        self.push(&bytes)
            .map_err(|()| StreamError::trap("output_limit"))
    }

    fn flush(&mut self) -> Result<(), StreamError> {
        Ok(())
    }

    fn check_write(&mut self) -> Result<usize, StreamError> {
        let used = self.captured.bytes.lock().expect("capture lock").len();
        // Never report "closed": a write past the limit must trap, not fail quietly.
        Ok(self.capacity.saturating_sub(used).max(1))
    }
}

impl AsyncWrite for BoundedOutput {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        match self.push(buf) {
            Ok(()) => Poll::Ready(Ok(buf.len())),
            Err(()) => Poll::Ready(Err(std::io::Error::other("output_limit"))),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

/// Rounds milliseconds down to LIM-S6.
fn coarse(ms: u64) -> u64 {
    ms.div_euclid(LIM_S6_CLOCK_GRANULARITY_MS) * LIM_S6_CLOCK_GRANULARITY_MS
}

/// Wall clock at LIM-S6 granularity, or fixed in replay.
pub struct CoarseWallClock {
    fixed_ms: Option<u64>,
}

impl CoarseWallClock {
    pub(crate) fn new(fixed_ms: Option<u64>) -> Self {
        Self { fixed_ms }
    }
}

impl HostWallClock for CoarseWallClock {
    fn resolution(&self) -> std::time::Duration {
        std::time::Duration::from_millis(LIM_S6_CLOCK_GRANULARITY_MS)
    }

    fn now(&self) -> std::time::Duration {
        let ms = match self.fixed_ms {
            Some(ms) => ms,
            None => std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0),
        };
        std::time::Duration::from_millis(coarse(ms))
    }
}

/// Monotonic clock at LIM-S6 granularity, or frozen in replay.
pub struct CoarseMonotonicClock {
    start: std::time::Instant,
    frozen: bool,
}

impl CoarseMonotonicClock {
    pub(crate) fn new(frozen: bool) -> Self {
        Self {
            start: std::time::Instant::now(),
            frozen,
        }
    }
}

impl HostMonotonicClock for CoarseMonotonicClock {
    fn resolution(&self) -> u64 {
        LIM_S6_CLOCK_GRANULARITY_MS * 1_000_000
    }

    fn now(&self) -> u64 {
        if self.frozen {
            return 0;
        }
        let ms = self.start.elapsed().as_millis() as u64;
        coarse(ms) * 1_000_000
    }
}
