# Handoff 004 — Persistence, write ordering and recovery (M1 part)

**RFC.** [RFC 004](../../accepted/004-persistence-checkpoints-and-recovery.md) (Accepted 2026-10-27)
**Specification.** Requirements REQ-PERS-001 … 008, 012; REQ-WS-001, 008; REQ-OBS-001, 002, 003, 008; REQ-SEC-008; REQ-MAIN-002; external design §14.1, §16, §17, §21.3; limits LIM-P1, LIM-P5, LIM-O3, LIM-S4, LIM-C2, LIM-W1 … W3.
**Crates.** `mimlys-persist` (store, blobs, migrations); `mimlys-core` (`recovery.rs`: the pure disposition logic; `Persistence` trait already defined in handoff 002).
**Milestone.** M1. Snapshots, promotion and effect-based rollback are M3 (handoff 006); this handoff stores snapshot ids as opaque strings and implements recovery without rollback.

## 1. Scope

In: the SQLite store with WAL and `synchronous = FULL`; the schema with versions and forward-only migrations; the lock file; owner-only permissions; the write ordering per action; blobs; the event log and queries; the startup integrity check with CONS-01 … CONS-08; the recovery procedure's M1 form; crash-injection test support; metrics derived from events.

Out: the snapshot store (M3), retention and garbage collection (M3), the data-directory creation for workspaces beyond what the store needs (handoff 010 creates the tree).

## 2. Module layout

```text
crates/mimlys-persist/src/
  lib.rs
  sqlite.rs        sqlite/tests.rs       connection setup (pragmas), schema, migrations table, `open(data_dir) -> Store`
  lock.rs          lock/tests.rs         `mimlys.lock` with LIM-O3 wait; refuses a second runtime
  store.rs         store/tests.rs        `impl Persistence for Store`: load/persist of every entity, transactions per transition
  blob.rs          blob/tests.rs         `blobs/<id>` write-then-fsync-then-record; bounded by LIM-S4
  events.rs        events/tests.rs       append, `since(position)`, counters for metrics (REQ-OBS-003, 008)
  integrity.rs     integrity/tests.rs    `PRAGMA integrity_check`, CONS-01 … CONS-08 queries
  crash.rs                               `#[cfg(feature = "crash-injection")]` hooks: a `CrashPoint` enum (the eight lines of ED §16.2 plus blob-before-record) and a global "crash at" switch used by tests to abort the process at a point
crates/mimlys-core/src/
  recovery.rs      recovery/tests.rs     `plan_recovery(&Task, Option<&Action>, &Policy) -> RecoveryPlan` (pure)
```

## 3. Behaviours

### 3.1 Store (ED §14.1, §16.1; REQ-PERS-001, 002; REQ-SEC-008)

- `rusqlite` with the bundled SQLite; pragmas `journal_mode = WAL`, `synchronous = FULL` (LIM-P1), `foreign_keys = ON`, `busy_timeout` = LIM-O3. File mode 0600 on POSIX (`OpenOptions::mode`), owner-only DACL on Windows (create the file with an explicit security descriptor or set the ACL immediately after creation; document which). The data directory itself is created 0700 / owner-only (REQ-WS-001).
- `schema_version` table; migrations are numbered SQL files embedded with `include_str!`, applied forward only inside one transaction each; a database with a newer version is refused with both versions in the error (REQ-PERS-002). The schema has one table per entity of ED §16.1 plus `events` and `blobs`; every row carries `schema_version` of its entity type (REQ-MAIN-002) and the task rows carry `policy_hash`, `decision_schema_version`, `tool_definition_hashes`, `runtime_version` (REQ-PERS-012).
- Lock: `mimlys.lock` opened with an exclusive advisory lock (`fs2`/`fd-lock`); a second runtime waits LIM-O3 then fails with the error kind `conflict` (REQ-PERS-001).

### 3.2 Durable transitions and write ordering (ED §16.2; REQ-PERS-003, 004)

- `Persistence` methods are the eight lines of ED §16.2: `record_proposed`, `record_validation`, `record_approval_request` / `record_approval_decision`, `record_authorized{snapshot_before}`, `record_running_intent`, `record_execution_and_observation`, `record_action_terminal`, `record_step_outcome{counters, task_state}`; each is one `BEGIN IMMEDIATE … COMMIT` and returns only after commit. Expose nothing that batches two lines into one transaction.
- Blobs: write the file, `fsync` it and its directory, then insert the row that references it (REQ-PERS-005; the same rule M3 applies to snapshots, REQ-PERS-009).
- Grants, approvals, questions, plan versions and task transitions outside a step (pause, cancel, grant) are each one transaction too.

### 3.3 Events and metrics (ED §17; REQ-OBS-001, 002, 003, 008)

- `events` table with a monotonically increasing `position`; `append(EventRecord)` inside the same transaction as the transition it describes (pass the transaction handle); `since(position, task_id?)` streams in order.
- Metrics (REQ-OBS-003): SQL views or queries over `events` for the counters of ED §17.2 that exist in M1 (steps, rejections by code, actions by effect, failures by cause, approvals by decision, halts by reason, completion kind); latencies come from `step.started/finished` and `action.started/finished` timestamps. Refusals counted per REQ-OBS-008.

### 3.4 Integrity at startup (ED §16.3, §21.3; REQ-PERS-006)

`integrity::check(&Store) -> Result<(), Violation>`: `PRAGMA integrity_check` (LIM-P5), then one query per CONS-01 … CONS-08; the first violation names its rule; the runtime (010) refuses to run tasks on a violation.

### 3.5 Recovery, M1 form (ED §16.3; REQ-PERS-007, 008)

- `plan_recovery` (pure, in core): for a task in `Running`: if an action is `Running` → mark it `Unknown`; disposition from handoff 001's `after_unknown` (via the effect) → in M1, every reversible case halts with `Recovered` *without* restoring a snapshot (M3 adds the restore) and every non-reversible case halts with `UnknownOutcome` listing the action; no action in flight → `Recovered`; `auto_resume` only when no action was in flight (M1) or after a restore (M3). `WaitingUser`, `Paused`, `Halted`, `Created` keep their state; expired approvals and questions are processed through handoff 003's `expire` (REQ-APR-006).
- The store applies the plan as transitions (one transaction each) and emits `task.recovered{disposition}`.
- INV-07 / REQ-PERS-008: there is no code path that moves an `Unknown` action back to `Running`; assert it with a test.

### 3.6 Crash injection (REQ-TEST-007, REQ-TEST-002)

Behind the feature `crash-injection`, a `CrashPoint` set by an environment variable read once at startup; each persistence method calls `crash::maybe(CrashPoint::X)` just before and just after its commit; `maybe` calls `std::process::abort()` when the point matches. Tests spawn the `mimlys` test binary (handoff 010 provides a `mimlys-testloop` example or an integration test harness) with the variable set, then reopen the store and assert the recovery plan.

## 4. Tests (REQ-TEST-007, 003, 016)

- Open, pragmas, permissions (POSIX mode 0600 asserted; Windows ACL asserted via `icacls` output in CI or the `windows-acl` crate), lock refusal within LIM-O3 (`req_pers_001_*`, `req_sec_008_*`).
- Migrations forward only; newer version refused with both numbers (`req_pers_002_*`).
- Each of the eight methods is one transaction (count `BEGIN` statements with a trace callback) and durable (reopen after each) (`req_pers_003_*`, `req_pers_004_*`).
- Blob written and fsynced before its row; a crash between file write and row leaves an orphan that integrity reports and GC (M3) removes (`req_pers_005_*`).
- Integrity: a fixture database per CONS rule violation; each is detected and named (`req_pers_006_*`).
- Recovery plan table: every task state × action state × effect class (`req_pers_007_*`); no path from `Unknown` to `Running` (`req_pers_008_*`, `inv_07_*`).
- Events ordered and complete; metrics match hand-counted fixtures (`req_obs_001_*`, `req_obs_003_*`).
- Crash injection at every `CrashPoint` (before and after commit) followed by recovery: the task is `Halted{Recovered|UnknownOutcome}` as the plan says, and no transition is lost or duplicated (`req_test_007_*`).

## 5. Recommended crates

`rusqlite` (`bundled`), `fd-lock` or `fs2`, `serde_json`, `tokio` (`rt`; run the store on a blocking thread through `spawn_blocking`), plus handoffs 001–003's. No `sqlx`: the store is synchronous by design and small.

## 6. Return

Review-request package under `.git-exclude/reviewed/m1-004/`, including the schema SQL, the migration policy text for the maintainers' docs, and the list of CONS queries.
