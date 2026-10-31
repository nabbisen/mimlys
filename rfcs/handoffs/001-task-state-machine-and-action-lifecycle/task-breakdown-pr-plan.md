# Handoff 001 — PR plan

| PR | Content | Depends on | Size |
|---|---|---|---|
| 001-1 | `ids`, `clock`, `hash`, `limits` (LIM-T, LIM-U) with tests; the determinism guard | — | S |
| 001-2 | `task`: states, halt reasons, terminal fields, transition table, extension rule; tests for every legal and illegal transition | 001-1 | M |
| 001-3 | `step`: phases, outcomes, counters, budgets, `apply`; tests for every halt reason | 001-1 | M |
| 001-4 | `action`: statuses, termination, failure causes, dispositions, execution record; tests | 001-1 | M |
| 001-5 | `event`: vocabulary and envelope; serde round-trip tests for every variant; `lib.rs` exports and crate docs | 001-2 … 001-4 | S |

Each PR: `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test`, `scripts/check-core-deps.sh` green; test names carry the requirement ids.
