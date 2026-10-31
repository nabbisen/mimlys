# Handoff 005 — PR plan (M2)

| PR | Content | Depends on | Size |
|---|---|---|---|
| 005-1 | `protocol` framing (`read_frame`, `write_frame`, bounds) and `worker.rs` runtime side with a stub worker binary that echoes `probe` and `execution_record`; restarts, cancel timeout, death handling; tests | 010-6 | M |
| 005-2 | `sandbox`: pinned engine config, guards with the interrupt-reason cell, termination mapping, WASI builder (preopens, pipes, clocks, random), artifact cache; unit tests with hand-written `.wat` modules | — | L |
| 005-3 | `confine`: trait, clauses, probe, `tree.rs` on POSIX and Windows, Linux `no_new_privs`, orphan sweep; honesty injection; tests per platform | — | M |
| 005-4 | `interpreters`: pinned table, verification, data-directory storage, compiled cache; `tools/interpreters/quickjs/` recipe and CI hash check; `mimlys interpreters list|install`; `[interpreters]` configuration | 005-2 | M |
| 005-5 | `run_script` and `run_wasm` definitions, the worker-backed tool provider, routing in `mimlys::workspace`; `mimlys-worker` main with panic containment | 005-1, 005-2, 005-4 | M |
| 005-6 | `status` clause table and tier; the shipped profiles' `approval_tools` for `run_*` (until M3); book chapter updates (sandbox, interpreters, status) | 005-3, 005-5 | S |
| 005-7 | Security suite on three platforms, the M2 demonstration scripts, the overhead and memory report; CHANGELOG 0.2 entry | 005-5, 005-6 | L |
