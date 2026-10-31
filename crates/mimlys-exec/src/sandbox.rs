//! The Wasmtime sandbox (external design §12; REQ-SBX-001 … 012). The
//! pinned engine, the three independent guards, the WASI boundary and the
//! exact termination mapping. Lives in the worker process; the runtime
//! never links it into its own path (REQ-ARCH-008).

mod guards;
mod run;
mod wasi;

pub use guards::{Interrupt, InterruptReason};
pub use run::{Limits, Outcome, Request, Sandbox};

use mimlys_core::limits::LIM_S4_OUTPUT_BYTES;

/// LIM-S4 as a `usize`.
pub const LIM_S4_BYTES: usize = LIM_S4_OUTPUT_BYTES as usize;

/// The engine's pinned version (REQ-ARCH-007).
pub const ENGINE_VERSION: &str = "50.0.1";

#[cfg(test)]
mod tests;
