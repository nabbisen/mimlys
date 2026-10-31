# Handoff 004 — PR plan

| PR | Content | Depends on | Size |
|---|---|---|---|
| 004-1 | `sqlite` (open, pragmas, permissions), `lock`; tests on all three platforms | 001-5 | M |
| 004-2 | Schema v1 and migrations; entity tables; version columns; tests | 004-1 | M |
| 004-3 | `store`: the eight ordered methods and the out-of-step transitions; transaction-count and durability tests | 004-2, 003-6 | L |
| 004-4 | `blob` and `events` with `since` and metrics queries; tests | 004-2 | M |
| 004-5 | `integrity`: CONS-01 … CONS-08; fixture databases; tests | 004-3 | M |
| 004-6 | `recovery.rs` in core (pure plan) and its application in the store; tests | 004-3, 003-1 | M |
| 004-7 | `crash-injection` feature and the crash-and-recover test harness (with handoff 010's test binary) | 004-3 … 004-6, 010-3 | M |
