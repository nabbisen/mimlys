# Handoff 010 — PR plan (M1 part)

| PR | Content | Depends on | Size |
|---|---|---|---|
| 010-1 | `config`: schema, secret references, defaults, ranges, locations; `mimlys check`; tests | 003-3 | M |
| 010-2 | `runtime/loop_.rs` and `control.rs`: the step loop over the SPI with the fakes; control functions; scheduler; tests with fake backend, tool, approval, clock | 002-6, 003-4, 004-3 | L |
| 010-3 | `mimlys-testloop` harness and the M1 demonstration scripts; crash-injection wiring with handoff 004 | 010-2 | M |
| 010-4 | `workspace`: materialization, exclusions, symlink rule, size refusal, case probe, quota accounting; tests on three platforms | 002-7 | M |
| 010-5 | CLI commands, `--json`, exit codes, signals; `prompt`; `follow`; `messages`; `logging` | 010-1, 010-2 | L |
| 010-6 | `mimlys-exec::protocol` message types (round-trip tests only) | 001-5 | S |
| 010-7 | User and integrator chapters of the book for M1 (`getting-started`, `decisions-and-tools`, `configuration`, `cli`) and the changelog entry for 0.1 | 010-5 | M |
