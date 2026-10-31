//! The three guards and the interrupt reason (REQ-SBX-002, REQ-SBX-003):
//! fuel ends with `OutOfFuel`, the memory limiter records its refusal
//! before the trap, and the epoch interrupt carries the reason the worker
//! recorded before it bumped the epoch. Nothing is inferred from timing.

use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use wasmtime::{Engine, ResourceLimiter};

/// Why the epoch was advanced past the deadline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum InterruptReason {
    /// The deadline passed (the ticker alone).
    Deadline = 0,
    /// The runtime asked to cancel.
    Cancelled = 1,
    /// A captured stream reached LIM-S4.
    OutputLimit = 2,
}

/// The interrupt channel of one execution: a reason recorded before the
/// epoch is advanced, shared with the cancel path and the output streams.
#[derive(Clone)]
pub struct Interrupt {
    reason: Arc<AtomicU8>,
    engine: Engine,
    ticks: u64,
    fired: Arc<AtomicBool>,
}

impl Interrupt {
    pub(crate) fn new(engine: Engine, ticks: u64) -> Self {
        Self {
            reason: Arc::new(AtomicU8::new(InterruptReason::Deadline as u8)),
            engine,
            ticks,
            fired: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Records the reason, then advances the epoch past the deadline (REQ-SBX-003).
    pub fn fire(&self, reason: InterruptReason) {
        if self.fired.swap(true, Ordering::SeqCst) {
            return;
        }
        self.reason.store(reason as u8, Ordering::SeqCst);
        for _ in 0..=self.ticks {
            self.engine.increment_epoch();
        }
    }

    /// Whether `fire` was called.
    pub fn fired(&self) -> bool {
        self.fired.load(Ordering::SeqCst)
    }

    /// The reason in force when the interrupt trap is observed.
    pub fn reason(&self) -> InterruptReason {
        match self.reason.load(Ordering::SeqCst) {
            1 => InterruptReason::Cancelled,
            2 => InterruptReason::OutputLimit,
            _ => InterruptReason::Deadline,
        }
    }
}

/// The epoch ticker: one increment every LIM-S7 while an execution runs.
pub struct Ticker {
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Ticker {
    pub(crate) fn start(engine: Engine, tick: Duration) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let handle = std::thread::Builder::new()
            .name("mimlys-epoch".into())
            .spawn(move || {
                while !flag.load(Ordering::Relaxed) {
                    std::thread::sleep(tick);
                    engine.increment_epoch();
                }
            })
            .expect("epoch ticker thread");
        Self {
            stop,
            handle: Some(handle),
        }
    }
}

impl Drop for Ticker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

/// The memory limiter: refuses growth above the ceiling and records the
/// refusal, so the trap that follows is named `memory_limit` and not by its
/// trap code (REQ-SBX-003).
pub struct MemoryGuard {
    limit: usize,
    refused: Arc<AtomicBool>,
    peak: Arc<AtomicU64>,
    inner: wasmtime::StoreLimits,
}

impl MemoryGuard {
    pub(crate) fn new(limit: usize) -> Self {
        Self {
            limit,
            refused: Arc::new(AtomicBool::new(false)),
            peak: Arc::new(AtomicU64::new(0)),
            inner: wasmtime::StoreLimitsBuilder::new()
                .memory_size(limit)
                .trap_on_grow_failure(true)
                .build(),
        }
    }

    pub(crate) fn refused_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.refused)
    }

    pub(crate) fn peak(&self) -> Arc<AtomicU64> {
        Arc::clone(&self.peak)
    }
}

impl ResourceLimiter for MemoryGuard {
    fn memory_growing(
        &mut self,
        current: usize,
        desired: usize,
        maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        if desired > self.limit {
            // An error here traps the guest at the `memory.grow` (REQ-SBX-002); `Ok(false)` would
            // hand it a -1 that an allocator turns into an ordinary out-of-memory exit.
            self.refused.store(true, Ordering::SeqCst);
            return Err(wasmtime::format_err!("memory_limit"));
        }
        self.peak.fetch_max(desired as u64, Ordering::SeqCst);
        self.inner.memory_growing(current, desired, maximum)
    }

    fn table_growing(
        &mut self,
        current: usize,
        desired: usize,
        maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        self.inner.table_growing(current, desired, maximum)
    }

    fn memory_grow_failed(&mut self, error: wasmtime::Error) -> wasmtime::Result<()> {
        // Growth past the ceiling traps the guest (REQ-SBX-002) instead of returning -1,
        // which an allocator would turn into an ordinary out-of-memory exit.
        self.refused.store(true, Ordering::SeqCst);
        Err(error)
    }

    fn instances(&self) -> usize {
        self.inner.instances()
    }

    fn tables(&self) -> usize {
        self.inner.tables()
    }

    fn memories(&self) -> usize {
        self.inner.memories()
    }
}

/// Shared bookkeeping of one execution's streams.
pub(crate) struct Captured {
    pub(crate) bytes: Mutex<Vec<u8>>,
}
