//! One execution: a fresh store, the guards, the WASI boundary, the call,
//! and the exact termination mapping (REQ-SBX-003, REQ-SBX-007, REQ-SBX-011).

use super::guards::{Interrupt, InterruptReason, MemoryGuard, Ticker};
use super::wasi::{BoundedOutput, CoarseMonotonicClock, CoarseWallClock};
use crate::protocol::Replay;
use mimlys_core::action::{ExecutionMetrics, HostFaultDetail, Termination};
use mimlys_core::limits::{
    LIM_S1_FUEL, LIM_S2_DEADLINE_MS, LIM_S3_MEMORY_MIB, LIM_S7_EPOCH_TICK_MS,
};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};
use wasmtime::{Config, Engine, Linker, Module, OptLevel, Store, Trap};
use wasmtime_wasi::p1::WasiP1Ctx;
use wasmtime_wasi::{FsPerms, I32Exit, WasiCtxBuilder};

/// The guards' values for one execution (defaults from the limits register).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// LIM-S1.
    pub fuel: u64,
    /// LIM-S2, or the smaller remaining deadline.
    pub deadline_ms: u64,
    /// LIM-S3 in bytes.
    pub memory_bytes: usize,
    /// LIM-S4.
    pub output_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            fuel: LIM_S1_FUEL,
            deadline_ms: LIM_S2_DEADLINE_MS as u64,
            memory_bytes: LIM_S3_MEMORY_MIB as usize * 1024 * 1024,
            output_bytes: super::LIM_S4_BYTES,
        }
    }
}

/// What to run.
#[derive(Clone, Debug)]
pub struct Request {
    /// `argv`, the program name first.
    pub args: Vec<String>,
    /// Standard input.
    pub stdin: Vec<u8>,
    /// The workspace tree, preopened as `/workspace`.
    pub workspace: PathBuf,
    /// Read-only workspace (`mode = read_only`).
    pub read_only: bool,
    /// The ephemeral directory preopened as `/tmp` (created by the caller, removed by the caller).
    pub tmp: PathBuf,
    /// Replay mode.
    pub replay: Option<Replay>,
    /// Guards.
    pub limits: Limits,
}

/// What an execution produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    /// How it ended (REQ-SBX-003).
    pub termination: Termination,
    /// Captured standard output, at most LIM-S4.
    pub stdout: Vec<u8>,
    /// Captured standard error, at most LIM-S4.
    pub stderr: Vec<u8>,
    /// Accounting (REQ-SBX-011).
    pub metrics: ExecutionMetrics,
}

/// The pinned engine with its epoch ticker.
pub struct Sandbox {
    engine: Engine,
    _ticker: Ticker,
}

impl Sandbox {
    /// Builds the engine with the pinned configuration (RFC 005 §1).
    pub fn new() -> wasmtime::Result<Self> {
        let mut config = Config::new();
        config
            .consume_fuel(true)
            .epoch_interruption(true)
            .wasm_multi_memory(false)
            .cranelift_nan_canonicalization(true)
            .relaxed_simd_deterministic(true)
            .cranelift_opt_level(OptLevel::Speed);
        let engine = Engine::new(&config)?;
        let ticker = Ticker::start(engine.clone(), Duration::from_millis(LIM_S7_EPOCH_TICK_MS));
        Ok(Self {
            engine,
            _ticker: ticker,
        })
    }

    /// The engine (to compile modules once per process).
    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    /// Validates and compiles a module (REQ-SBX-006).
    pub fn compile(&self, bytes: &[u8]) -> Result<Module, String> {
        if bytes.starts_with(b"\0asm") {
            Module::validate(&self.engine, bytes).map_err(|e| e.to_string())?;
        }
        // Text-format modules are accepted here for the test suite only; every
        // production module is binary and validated above.
        Module::new(&self.engine, bytes).map_err(|e| e.to_string())
    }

    /// An interrupt handle for an execution with this deadline.
    pub fn interrupt(&self, deadline_ms: u64) -> Interrupt {
        Interrupt::new(self.engine.clone(), ticks(deadline_ms))
    }

    /// Runs `module` under the request's guards; `interrupt` is the handle
    /// the cancel path holds (REQ-SBX-002, REQ-SBX-004).
    pub fn run(&self, module: &Module, request: &Request, interrupt: &Interrupt) -> Outcome {
        let started = Instant::now();
        let stdout = BoundedOutput::new(request.limits.output_bytes, interrupt.clone());
        let stderr = BoundedOutput::new(request.limits.output_bytes, interrupt.clone());
        let setup = match self.setup(module, request, interrupt, &stdout, &stderr) {
            Ok(s) => s,
            Err(detail) => {
                return host_fault(detail, started, &stdout, &stderr);
            }
        };
        let Setup {
            mut store,
            instance_start,
            refused,
            peak,
            setup_ms,
        } = setup;
        let result = instance_start.call(&mut store, ());
        let wall_ms = started.elapsed().as_millis() as i64;
        let fuel_used = request
            .limits
            .fuel
            .saturating_sub(store.get_fuel().unwrap_or(0));
        let termination = match result {
            Ok(()) => Termination::Exited { exit_code: 0 },
            Err(err) => classify(&err, interrupt, refused.load(Ordering::SeqCst)),
        };
        let out = stdout.contents();
        let errs = stderr.contents();
        Outcome {
            termination,
            metrics: ExecutionMetrics {
                wall_ms,
                fuel: Some(fuel_used),
                memory_peak_bytes: Some(peak.load(Ordering::SeqCst)),
                stdout_bytes: out.len() as u64,
                stderr_bytes: errs.len() as u64,
                setup_ms: Some(setup_ms),
            },
            stdout: out,
            stderr: errs,
        }
    }

    fn setup(
        &self,
        module: &Module,
        request: &Request,
        interrupt: &Interrupt,
        stdout: &BoundedOutput,
        stderr: &BoundedOutput,
    ) -> Result<Setup, HostFaultDetail> {
        let setup_started = Instant::now();
        let mut builder = WasiCtxBuilder::new();
        builder
            .args(&request.args)
            .stdin(wasmtime_wasi::p2::pipe::MemoryInputPipe::new(
                request.stdin.clone(),
            ))
            .stdout(stdout.clone())
            .stderr(stderr.clone())
            .allow_blocking_current_thread(true)
            .allow_ip_name_lookup(false)
            .allow_tcp(false)
            .allow_udp(false)
            .wall_clock(CoarseWallClock::new(
                request.replay.as_ref().map(|r| r.clock_ms),
            ))
            .monotonic_clock(CoarseMonotonicClock::new(request.replay.is_some()));
        if let Some(replay) = &request.replay {
            let seed = if replay.seed.is_empty() {
                vec![0]
            } else {
                replay.seed.clone()
            };
            builder
                .secure_random(wasmtime_wasi::Deterministic::new(seed.clone()))
                .insecure_random(wasmtime_wasi::Deterministic::new(seed))
                .insecure_random_seed(0);
        }
        let perms = if request.read_only {
            FsPerms::ReadOnly
        } else {
            FsPerms::ReadWrite
        };
        builder
            .preopened_dir(&request.workspace, "/workspace", perms)
            .map_err(|_| HostFaultDetail::Unavailable)?;
        builder
            .preopened_dir(&request.tmp, "/tmp", FsPerms::ReadWrite)
            .map_err(|_| HostFaultDetail::Unavailable)?;
        let wasi = builder.build_p1();
        let guard = MemoryGuard::new(request.limits.memory_bytes);
        let refused = guard.refused_flag();
        let peak = guard.peak();
        let mut store = Store::new(&self.engine, State { wasi, guard });
        store.limiter(|s| &mut s.guard);
        store
            .set_fuel(request.limits.fuel)
            .map_err(|_| HostFaultDetail::Unavailable)?;
        store.set_epoch_deadline(ticks(request.limits.deadline_ms));
        store.epoch_deadline_trap();
        let mut linker: Linker<State> = Linker::new(&self.engine);
        wasmtime_wasi::p1::add_to_linker_sync(&mut linker, |s: &mut State| &mut s.wasi)
            .map_err(|_| HostFaultDetail::Unavailable)?;
        let instance = linker
            .instantiate(&mut store, module)
            .map_err(|_| HostFaultDetail::InvalidModule)?;
        let instance_start = instance
            .get_typed_func::<(), ()>(&mut store, "_start")
            .map_err(|_| HostFaultDetail::InvalidModule)?;
        let _ = interrupt;
        Ok(Setup {
            store,
            instance_start,
            refused,
            peak,
            setup_ms: setup_started.elapsed().as_millis() as i64,
        })
    }
}

struct State {
    wasi: WasiP1Ctx,
    guard: MemoryGuard,
}

struct Setup {
    store: Store<State>,
    instance_start: wasmtime::TypedFunc<(), ()>,
    refused: std::sync::Arc<std::sync::atomic::AtomicBool>,
    peak: std::sync::Arc<std::sync::atomic::AtomicU64>,
    setup_ms: i64,
}

fn ticks(deadline_ms: u64) -> u64 {
    deadline_ms.div_ceil(LIM_S7_EPOCH_TICK_MS).max(1)
}

/// The exact mapping of REQ-SBX-003.
fn classify(err: &wasmtime::Error, interrupt: &Interrupt, memory_refused: bool) -> Termination {
    if let Some(exit) = err.downcast_ref::<I32Exit>() {
        return Termination::Exited { exit_code: exit.0 };
    }
    if memory_refused {
        return Termination::MemoryLimit;
    }
    // The worker records why it interrupted before it does; a stream that hit its bound traps
    // the guest from inside the host call, so the recorded reason, not the error, names the end.
    if interrupt.fired() {
        return match interrupt.reason() {
            InterruptReason::Deadline => Termination::Deadline,
            InterruptReason::Cancelled => Termination::Cancelled,
            InterruptReason::OutputLimit => Termination::OutputLimit,
        };
    }
    match err.downcast_ref::<Trap>() {
        Some(Trap::OutOfFuel) => Termination::FuelExhausted,
        Some(Trap::Interrupt) => match interrupt.reason() {
            InterruptReason::Deadline => Termination::Deadline,
            InterruptReason::Cancelled => Termination::Cancelled,
            InterruptReason::OutputLimit => Termination::OutputLimit,
        },
        Some(trap) => Termination::Trapped {
            trap: trap_name(*trap),
        },
        None => Termination::HostFault {
            detail: HostFaultDetail::Unavailable,
        },
    }
}

fn trap_name(trap: Trap) -> String {
    format!("{trap:?}").chars().fold(String::new(), |mut s, c| {
        if c.is_ascii_uppercase() {
            if !s.is_empty() {
                s.push('_');
            }
            s.push(c.to_ascii_lowercase());
        } else {
            s.push(c);
        }
        s
    })
}

fn host_fault(
    detail: HostFaultDetail,
    started: Instant,
    stdout: &BoundedOutput,
    stderr: &BoundedOutput,
) -> Outcome {
    Outcome {
        termination: Termination::HostFault { detail },
        stdout: stdout.contents(),
        stderr: stderr.contents(),
        metrics: ExecutionMetrics {
            wall_ms: started.elapsed().as_millis() as i64,
            fuel: None,
            memory_peak_bytes: None,
            stdout_bytes: 0,
            stderr_bytes: 0,
            setup_ms: None,
        },
    }
}
