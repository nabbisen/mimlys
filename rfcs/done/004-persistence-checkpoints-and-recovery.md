# RFC 004 — Persistence, checkpoints and recovery

**Status.** Implemented (0.7.0; accepted 2026-10-27)
**Tracks.** Decision D-15 of the startup review; findings M-09, R-11, R-12, A-05, M-20; threat THR-08, THR-29, THR-31.
**Touches.** `mimlys-persist` (SQLite store, snapshot store), `mimlys-core` (recovery logic), the data-directory layout, external design §14.1, §16.

## Summary

Make every transition durable in its own SQLite transaction with a fixed write ordering per action, define a checkpoint as nothing more than a step boundary plus a snapshot reference, and specify the recovery procedure at startup so that an interrupted action becomes `unknown` and is disposed of by its effect, never replayed when irreversible.

## Motivation

- The drafts promised crash recovery but defined neither the durability boundary nor the integrity checks at startup (M-09).
- The database and the filesystem are two durability domains that cannot commit together; nothing stated which may reference which (R-12).
- "Reduce the probability of replaying an irreversible action" is not a safety property (R-11).
- "Checkpoint" read like a separate feature from persisted state (A-05).

## Decision

The normative text is external design §14.1 (layout), §16 (entities, write ordering, recovery, retention), §21.3 (consistency rules) and requirements REQ-PERS-001 to REQ-PERS-012, REQ-WS-004, REQ-WS-009, REQ-WS-012, REQ-TASK-008, REQ-TASK-009. In brief:

1. **Store:** one SQLite database per data directory in WAL mode with `synchronous = FULL`; owner-only permissions; a lock file; a single writer; a schema version with forward-only migrations.
2. **Durability boundary:** a transition is durable when its transaction returns; every task, step, action, approval, question and grant transition and every event is its own transaction.
3. **Write ordering per action:** proposed → validation → approval → authorized with the pre-execution snapshot id → running intent → execution record and observation → terminal status → step outcome and counters. Execution starts only after the running intent is durable.
4. **Checkpoint:** a step boundary at which the persisted state and the current snapshot allow resumption; nothing else is stored. Model context is rebuilt from persisted history.
5. **Two domains, one rule:** the database references only snapshots whose objects and manifest were fully written and fsynced before the referencing row; unreferenced objects are collected; recovery verifies a manifest before trusting it.
6. **Recovery at startup:** integrity check; consistency rules CONS-01 to CONS-08; every `running` task becomes `recovering`; an in-flight action becomes `unknown`; reversible effects are rolled back and the task halts as `recovered`; non-reversible effects halt the task as `unknown_outcome` with the action listed; `recovery.auto_resume` may return a cleanly restored task to `running`; expired approvals are processed; orphan processes are terminated where identifiable. **No non-reversible effect is ever re-executed automatically** (REQ-PERS-008, an absolute rule).
7. **Budgets and deadlines:** counters are durable and survive restarts; the deadline is absolute and downtime does not stop it; only the user extends.
8. **Retention:** LIM-P2 for raw outputs, LIM-P3 for snapshots, LIM-P4 for audit; never the only recovery point of a non-terminal task.

## Alternatives considered

- `synchronous = NORMAL` for speed: a power loss could lose the last transactions and reorder the action record relative to its effect. Rejected; local tasks are not write-bound.
- Storing full model context in checkpoints: duplicates persisted history and grows without bound. Rejected.
- Auto-resuming after recovery by default: autonomous work continuing after a crash without the user knowing contradicts P1. Rejected; opt-in by policy.
- A separate "unknown outcome" table instead of an action state: splits one fact across two places. Rejected.

## Consequences

- Crash-injection tests kill the runtime at each of the eight persistence lines and verify resumption (REQ-TEST-007).
- The data directory is a complete backup when the runtime is stopped (REQ-PERS-011).
- M1 implements lines 1–3 and 5–8 with the running intent; M3 adds snapshot references and the effect-based disposition with real rollback.

## Open questions

Whether to add a periodic `PRAGMA wal_checkpoint` schedule for very long tasks; decided from M3 measurements.

## References

Requirements §12, §14; external design §14, §16, §21.3; startup review D-15, R-11, R-12.
