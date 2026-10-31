# Handoff 005 — Execution worker, sandbox profile, confinement probe (M2)

**RFC.** [RFC 005](../../accepted/005-execution-worker-and-sandbox-profile.md) (Accepted 2026-10-27)
**Specification.** Requirements REQ-SBX-001 … 012; REQ-ARCH-007, 008; REQ-CONF-001, 002, 003, 009, 010; REQ-SEC-006; REQ-SCHED-003; REQ-TOOL-006, 007, 008; REQ-TEST-008, 013; REQ-PLAT-003; external design §7.4, §11 (`run_script`, `run_wasm`), §12, §13.1, §13.2, §13.4, §23 (M2); limits LIM-S1 … LIM-S8, LIM-X2, LIM-X3, LIM-O2, LIM-O1.
**Crates.** `mimlys-exec` (worker binary, sandbox, protocol framing, confinement interface and probe, the two tools); `mimlys` (`[interpreters]` configuration, `interpreters` commands, `status` clause table, the worker as the task's tool provider for `run_*`); `mimlys-core` (small: tool definitions of `run_script` and `run_wasm` live in `mimlys-exec`; no change to the validator).
**Milestone.** M2. Depends on handoffs 001–004 and 010.

## 1. Scope

In: the worker process and its framing; the Wasmtime sandbox with the three guards, the WASI configuration, output capture, clocks, replay mode, module validation, the compiled-artifact cache; `run_script` (JavaScript) and `run_wasm`; interpreter configuration, verification and installation; the confinement interface, the portable clauses (E, K; P on Linux), the probe, the tier in `status`; the security suite on three platforms; the overhead and memory report.

Out: the per-platform confinement backends F, N, M, C (M6a–M6c); host tools (M6); rollback of a failed write (M3); Python until a build is pinned.

## 2. Module layout

```text
crates/mimlys-exec/src/
  protocol.rs   protocol/tests.rs      message types (M1) + framing: `read_frame`, `write_frame`, bounded size
  worker.rs     worker/tests.rs        the runtime side: spawn through `confine`, request/response loop, restarts (LIM-X3), cancel within LIM-O2, death → host_fault{worker_died}
  sandbox.rs    sandbox/               engine (pinned config), `Guards` (fuel, epoch ticker, limiter), `wasi.rs` (preopens, pipes, clocks, random), `run.rs` (instantiate, run, map the termination), `cache.rs` (serialize/deserialize keyed by hash + engine version), tests.rs
  interpreters.rs  interpreters/tests.rs   the pinned table (language → artifact url, sha256), verification, `<data_dir>/interpreters/<sha256>.wasm`
  tools/run_script.rs, tools/run_wasm.rs   definitions and argument handling; the executor is the worker
  confine.rs    confine/               `Confinement` trait, `Clause`, `ClauseState`, `probe`, `tree.rs` (process group / Job Object), `linux.rs` (no_new_privs; the rest M6a), `windows.rs` (Job Object; the rest M6b), `macos.rs` (process group; the rest M6c), tests.rs
  bin/mimlys-worker.rs                 the worker's main: frames in, executions out, epoch ticker, panic containment per execution
crates/mimlys/src/
  config.rs (+ `[interpreters.<language>]`), commands/interpreters.rs, commands/status.rs (clause table), workspace.rs (tools: built-in + worker-backed run_*)
tools/interpreters/quickjs/            reproducible build recipe (pinned quickjs-ng commit, pinned wasi-sdk), `expected.sha256`; CI job rebuilds and compares
```

## 3. Behaviours

### 3.1 Framing and the worker loop (ED §13.1; REQ-ARCH-008)

Frame = `u32` big-endian length + JSON object; a frame above twice LIM-S4 plus 64 KiB is a protocol error that ends the worker. The runtime sends `probe` once after spawn and keeps the result; `execute` carries `execution_id`, `action_id`, `tool`, `arguments` (canonical, from the `AuthorizedAction`), `effect`, `deadline_ms`, `workspace_root`, `read_only`, and `replay{seed, clock_ms}?`. The worker answers each `execute` with exactly one `execution_record`; `cancel{execution_id}` makes the worker interrupt the epoch and answer the pending `execute` with `cancelled`. `shutdown` ends the worker after the in-flight execution ends or is cancelled. A worker that does not answer `cancel` within LIM-O2 is killed through the tree primitive and the execution is `host_fault{worker_died}`. The runtime restarts a dead worker lazily at the next execution, at most LIM-X3 times per task; beyond that the execution is `host_fault{unavailable}` and the disposition halts the task `tool_unavailable`.

### 3.2 Engine and guards (ED §12; REQ-SBX-002, 003, 008)

`Config`: `consume_fuel(true)`, `epoch_interruption(true)`, `wasm_threads(false)`, `wasm_reference_types` default, `cranelift_nan_canonicalization(true)`, `relaxed_simd_deterministic(true)`, no cache, `max_wasm_stack` default. Per execution: a fresh `Store` with `set_fuel(LIM_S1)`, `set_epoch_deadline(LIM_S2 / LIM_S7)`, a limiter built from `StoreLimitsBuilder::memory_size(LIM_S3).trap_on_grow_failure(true)` wrapped in a type that records `memory_refused = true` on refusal. A ticker thread increments the engine epoch every LIM-S7. The interrupt reason is a per-execution cell set *before* the worker bumps the deadline: `Deadline`, `Cancelled`, `OutputLimit`. Mapping: `Trap::OutOfFuel` → `fuel_exhausted`; `Trap::Interrupt` → the recorded reason; a trap with `memory_refused` → `memory_limit`; other traps → `trapped{trap: <wasmtime trap name>}`; `proc_exit` → `exited{exit_code}`; a non-trap host error → `host_fault{unavailable}`.

### 3.3 WASI (REQ-SBX-004, 005; REQ-SEC-006)

`WasiCtxBuilder` (preview1 via `wasmtime-wasi`'s p1 adapter): `preopened_dir(tree, "/workspace", perms by mode)`, `preopened_dir(tmp, "/tmp", ALL)`, `args`, `stdin` from the declared bytes, stdout and stderr to memory pipes of capacity LIM-S4 whose overflow sets the interrupt reason `OutputLimit` and bumps the deadline, empty env, no `inherit_network`, no `socket_addr_check` allowance, `wall_clock` and `monotonic_clock` implementations rounding to LIM-S6 and returning fixed values in replay, `secure_random`/`insecure_random` from a seeded ChaCha in replay (the system source otherwise). The tmp directory is `<data_dir>/workspaces/<task_id>/tmp/<execution_id>/`, created before and removed after, also on failure.

### 3.4 Tools (ED §11)

`run_script`: `language` (enum of configured interpreters), `source` (string, LIM-W3 bytes), `stdin?`, `args?` (strings), `mode` (`read_only` | `read_write`, default `read_write`). The worker writes `source` to `/tmp/main.js` (extension per language), args become `["<interpreter>", "/tmp/main.js", args…]`. `run_wasm`: `module` (workspace path, LIM-S5), `args?`, `stdin?`, `mode`. Both: effect by mode, `reversible: true`, `idempotent: false`, `profile: sandbox`, deadline LIM-S2. The output schema of both is the execution-record fields of ED §7.4; `payload` carries `termination`, `exit_code?`. The definitions live in `mimlys-exec::tools` and are offered by the task's tool provider next to the built-in file tools; the provider routes `run_*` to the worker and everything else in process.

### 3.5 Interpreters (REQ-SBX-001, 007; REQ-ARCH-007)

`PINNED: [(language, artifact_url, sha256)]` in `interpreters.rs`; configuration `[interpreters.javascript] module = PATH, sha256 = HEX` overrides the path (the hash must still be given and must match the file). `mimlys interpreters list` shows language, configured hash, installed or not; `mimlys interpreters install javascript [--from FILE]` reads the file or downloads the artifact (the CLI process, never the worker), verifies the hash, writes `<data_dir>/interpreters/<sha256>.wasm` (0600), and reports. At worker start the runtime hashes each configured module file and refuses a mismatch; the worker compiles once, serializes to `<sha256>.<engine_version>.cwasm`, and deserializes on later starts. `tools/interpreters/quickjs/` holds the Dockerfile and script pinning quickjs-ng and wasi-sdk; CI rebuilds and asserts `expected.sha256`.

### 3.6 Confinement interface and probe (ED §13.2, §13.4; REQ-CONF-002, 003, 009)

```rust
pub trait Confinement: Send + Sync {
    fn probe(&self) -> Probe;                       // per clause: Enforced | BestEffort | Unavailable{reason}
    fn spawn(&self, command: WorkerCommand) -> Result<ConfinedChild, ConfineError>;  // env explicit, tree primitive attached
}
pub struct ConfinedChild { … fn kill_tree(&mut self) -> …; fn wait_within(&mut self, Duration) -> … }
```

M2 implementations: `posix` (process group; `no_new_privs` on Linux through `rustix`; orphan sweep by a marker file with the previous worker's pid and start time) and `windows` (Job Object with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` and breakaway forbidden, through the `windows-sys` crate). Clauses E and K are `enforced` on every platform; P is `enforced` on Linux and `unavailable{reason}` elsewhere until the backends land; F, N, M, C are `unavailable{reason: "backend arrives in M6a|M6b|M6c"}`. The probe result is persisted as an event `runtime.started{tier}` detail and shown by `status`. Honesty test: an environment variable `MIMLYS_PROBE_FAIL=<clause>` (test builds only) makes the probe report `unavailable` for that clause, and the status must show it.

### 3.7 Panic containment and fail-closed (REQ-ENG-004, REQ-SBX-012)

Each execution in the worker runs under `catch_unwind`; a panic is `host_fault{unavailable}` with the reason in the worker log, and the worker continues. A missing or mismatched interpreter, a preopen that cannot be opened, or an engine configuration error is `host_fault{unavailable}` before anything runs.

## 4. Tests (REQ-TEST-008, 013; CI on three platforms)

- Each guard alone: an infinite loop with a large fuel budget ends `deadline`; the same loop with a small fuel budget ends `fuel_exhausted`; an allocation bomb ends `memory_limit`; a print flood ends `output_limit`; a `proc_exit(3)` ends `exited{3}`; an `unreachable` ends `trapped`.
- Filesystem: a write in `read_only` fails inside the guest with an error, the tree is unchanged; `../` and absolute paths fail with `ENOTCAPABLE`-class errors; `/tmp` is gone after the execution.
- Network: a module importing `sock_*` fails to link or to call; the worker has no network on the platforms whose backend exists (M6).
- Clocks: two reads within LIM-S6 differ by a multiple of LIM-S6; replay with a fixed clock and seed produces identical output twice (REQ-SBX-008, REQ-TEST-016).
- Malformed module → `host_fault{invalid_module}`; an interpreter file with a wrong hash → `host_fault{unavailable}` and the runtime's status names it.
- Worker killed during an execution → `host_fault{worker_died}`, the runtime survives, the next execution restarts the worker; after LIM-X3 deaths the task halts `tool_unavailable`.
- Cancel during an execution → `cancelled` within LIM-O2; runtime exit kills the tree (a guest that spawned nothing in M2; the primitive is tested with a plain child on each platform).
- Probe honesty (`MIMLYS_PROBE_FAIL`); `status --json` clause table; tier `core` on every platform.
- Overhead: a benchmark test measures instantiate + WASI setup for QuickJS with and without the cache and prints the numbers; the M2 report fixes LIM-S8 and states LIM-X2 × LIM-S3 + base.
- The M2 demonstration (ED §23) through `mimlys-testloop` with scripts under `tests/m2/`.

## 5. Recommended crates

`wasmtime = "=50.0.0"`, `wasmtime-wasi = "=50.0.0"` (features: `preview1`), `rustix` (Linux `no_new_privs`, process groups), `windows-sys` (Job Objects), `rand_chacha` (replay random), `sha2` (exists), `ureq` or `reqwest` (blocking, rustls) for `interpreters install` in the `mimlys` crate only.

## 6. Return

Review-request package under `.git-exclude/reviewed/m2-005/` with the security-suite results on the three platforms, the overhead and memory report (LIM-S8, REQ-SCHED-003), the `status` clause table per platform, and the interpreter artifact's hash and recipe.

## 7. Amendments from the implementation (2026-10-27)

1. **No on-disk compilation cache.** Wasmtime 50.0.0's `cache` feature cannot be built from crates.io (its internal cache crate is unpublished at that version), and an own `Module::deserialize` path would need `unsafe` outside `confine/`. Interpreters are compiled once per worker process and shared read-only by its executions; the worker's start, including that compilation, is bounded by the new LIM-X4 (60 s; measured at 1.4 s in debug and 0.13 s in release for QuickJS-NG on the reference machine). REQ-SBX-007's "cached by content hash under the data directory" holds for the module file; the compiled form lives in the worker's memory.
2. **The `mimlys-worker` binary is built by the `mimlys` crate** (`crates/mimlys/src/bin/mimlys-worker.rs` calling `mimlys_exec::worker_main::main`), so `cargo install mimlys` ships both binaries and the tests of the `mimlys` crate find the worker through `CARGO_BIN_EXE_mimlys-worker`. `MIMLYS_WORKER_BIN` overrides the location.
3. **Clause P is set by the worker on itself** (`no_new_privs` at its start, Linux) rather than by the spawner, which keeps `unsafe` out of the spawn path; the worker's probe reports it and the spawn side merges nothing it cannot see.
4. **The memory limiter traps at `memory.grow`** (an error from `ResourceLimiter::memory_growing`) instead of returning -1: with -1 an allocator turns the refusal into an ordinary out-of-memory exit and the termination would read `exited{1}`; now it reads `memory_limit` (REQ-SBX-003).
5. **The interpreter's `std`/`os` modules are absent from the pinned QuickJS-NG wasip1 build.** JavaScript reads stdin and args and prints; files are the file tools' job and `run_wasm` covers the read-write demonstration. A later pinned build may enable them.
6. **The sandbox limits travel to the worker at spawn** (`--fuel`, `--memory-bytes`, `--output-bytes` from `[sandbox]`); the deadline travels per execution. The `run_*` definitions carry `[sandbox] deadline_ms` as their deadline.
7. **The interpreter artifact recipe** (`tools/interpreters/quickjs/`) mirrors upstream's release build and is exercised by the on-demand `interpreters` workflow; it was not executed on the reference machine (no wasi-sdk), so the pin is the upstream release asset's hash, verified on download.
