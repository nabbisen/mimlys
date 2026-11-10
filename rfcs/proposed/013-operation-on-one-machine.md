# RFC 013 — Operation on one machine

**Status.** Proposed (2026-11-10). Drafted by the owner with the startup architect; reviewed by the architect, whose changes are marked *[review]* and argued in `.git-exclude/reviewed/rfc-013-015-review/README.md`. For the owner's sign-off.
**Tracks.** The owner's review of the replanning report (2026-11-08) and its assessment `assessment-operation-on-one-machine.md`; the replanning report §3 C (hygiene) and §3 D; the roadmap's "After 0.10"; DR-002 in part (the parts that need no daemon).
**Touches.** `mimlys-persist` (the store's lock and a read-only open), `mimlys-core` (scheduling: admission as a queued reason; events; metrics), `mimlys-exec` (the `tool` cgroup, the Job Object and the macOS watchdog: a CPU bound), `mimlys` (the registry under the user's runtime directory, `status`, `task prune`, the storage view, the book); requirements §12, §15, §16, §21, §22; external design §13.3, §13.5, §14.6, §17, §19, §20; the limits register (new LIM-X5, LIM-H7, LIM-O11, LIM-O12); the threat register (new THR-37, THR-38).
**Release.** The read-only open, the storage view, `task prune` and the book's pages in 0.10 (decisions 1 to 4; design-light, shipped as hygiene under this RFC); admission, the CPU bound, the engine gate and the machine view in the release after 0.10 (decisions 5 to 8). Partial shipping as RFC 000 allows; the Status line records what shipped where.

## Summary

mimlys bounds every execution, and every bound it enforces is per process: one runtime, one worker, one tool tree. A user with several projects runs several `mimlys` processes, each honouring its bounds while the machine is exhausted; nothing bounds CPU at all; a second terminal cannot even read the store while a task runs, because every command takes the store's exclusive lock; and what mimlys stores on disk has no view and no tidy-up. This RFC makes the bounds hold for the machine, as far as a process-per-command design can, and gives the user one place to see what is running and what is stored. It adds no trust boundary: everything is per operating-system user, nothing listens on a network, and no level of anything grants a capability.

## Motivation

Verified at `0.9.0` (the assessment, 2026-11-08):

- `mimlys-persist/src/sqlite.rs` takes an exclusive lock on `<data_dir>/mimlys.lock` in every open and gives up after LIM-O3; `status`, `events`, `task show` and `follow` fail as locked while another process runs a task. A reader is told the store is busy when it could read (SQLite in WAL mode serves readers beside one writer).
- The `tool` cgroup sets `memory.max` and `pids.max` (`confine/linux.rs`); the Job Object sets memory and the process count; macOS has the two watchdogs. No platform bounds CPU. A `cargo test` takes every core for up to LIM-H1.
- LIM-X1 and LIM-X2 are per runtime and fixed at 1. N runtimes are N workers, N tool trees of up to LIM-H2 each, N working trees, and N concurrent requests to one engine, which then swaps models or queues as it sees fit. The runtime knows nothing of its siblings.
- The data directory holds one working tree per task until the user deletes each task by id; there is no store-wide footprint and no prune; the book has no page on storage.

"Bounded execution" is a core promise (requirements §10, §11). A promise that evaporates when the user opens a second terminal is not finally safe, and a reader refused as "locked" is a confusing interface. Both contradict the owner's philosophy.

## Decision

### Part one: 0.10, design-light

1. **A read-only open of the store (REQ-PERS-013, new).** Commands that only read (`status`, `events`, `task list`, `task show`, `metrics`, `follow` of a task another process runs, `toolchain list`, `check`) open the database as WAL readers without the exclusive lock. The lock keeps protecting every writer, the worker record and the snapshot journal. A read command that would write is a defect, refused at build time by the type of its persistence handle. *[review]* The read-only open runs no recovery, no integrity check, no reaping and no migration: those write. If the store needs recovery (the lock is free and the last owner ended uncleanly), a read command still reads what is committed and says `recovery pending: the next command that writes recovers it`. While the lock is held, `status` says `store: in use by runtime <pid> since <time>` and reads on. (ED §16, §19.)
2. **A storage view (REQ-WS-013, new).** `mimlys status --storage`: the store's path, its filesystem and whether it reflinks, the bytes by kind (working trees, snapshot objects exclusive and shared, blobs, interpreters, database and WAL, temporary), and the tasks that hold the most with their state and age. Read-only; the numbers come from the manifests and reference counts the store already keeps, not from walking the tree, except for `tmp/` and the database files. (ED §14.6.)
3. **Prune (REQ-WS-014, new).** `mimlys task prune [--state <list>] [--older-than <duration>] [--yes]`: lists the terminal tasks that match with their footprints, asks once, then runs the existing deletion per task; audits stay unless `--audit`. It never touches a task that is not terminal, nor a promotion's backup snapshot still needed for reversal (a task whose promotion is reversible is listed as `kept: promotion reversible` and skipped). The plan is shown before anything goes, as `task delete` shows it. (ED §14.6.)
4. **The book.** A page "Storage" (what lives where in the user's words, reflinks per filesystem, what grows, how to reclaim) and a section "Several projects, several jobs" in the getting-started page: one store per user, how two runtimes behave today and after this RFC. (REQ-CLI-007, new: the two pages exist and their examples are tested.)

### Part two: the release after 0.10

5. **Admission per user (REQ-SCHED-006, new; LIM-X5, new).** A registry under the user's runtime directory (`$XDG_RUNTIME_DIR/mimlys/` with `$XDG_DATA_HOME/mimlys/run/` as the fallback; `%LOCALAPPDATA%\mimlys\run\`; `~/Library/Application Support/mimlys/run/`), owner-only, holding one entry per **execution slot**: a runtime takes a slot when an execution starts and returns it when the execution ends, so an idle runtime, a `follow` or a waiting approval costs nothing. At most **LIM-X5** slots are taken at once per user (default to be measured; the candidate is 2). A runtime that finds no free slot waits with its task queued; the task's queued flag carries `reason: admission`, `status` says `waiting: LIM-X5 = 2 slots taken by runtimes <pids>`, and the event `task.admission_changed{waiting | admitted, reason, slots_taken}` is recorded (ED §17). Each entry is a file the holder keeps locked plus its pid, data directory and start time; an entry whose lock is free and whose pid is gone is stale and is reaped at every open, logged at `info`. A wait is bounded by nothing but the task's own deadline (LIM-T5), which keeps counting; the message says so.
6. **A CPU bound per host-tool tree (REQ-CONF-013, new; LIM-H7, new).** *[review]* Clause **C** (count and wall clock today) gains a CPU part; the contract has no clause R. The CPU part is **best effort allowed on every platform**, so it never lowers the tier: `cpu.max` on the `tool` cgroup on Linux (quota and period) where a cgroup is delegated, else `unavailable` with the reason, as the cgroup rung is today; `JOB_OBJECT_CPU_RATE_CONTROL` on the tool's Job Object on Windows; on macOS a lowered priority, which is not a bound, reported `best_effort (priority)` so that nobody reads it as one. (K is `enforced` on macOS since 0.8, so the draft's comparison with K no longer holds.) LIM-H7 is a share of the machine's cores (default to be measured against the M6a demonstration's duration; the candidate is half the cores, at least one). The bound is a budget a user may raise per task within the ceiling, like the others. The probe reports the CPU part of each platform's primitive; `status` lists it with the clauses.
7. **One request in flight per engine endpoint per user (REQ-SCHED-007, new; LIM-O12, new).** The same registry holds one gate per backend URL. A second runtime's model request waits for the gate; the wait is a metric (`engine_wait_ms`) and a log line, and it is bounded by the request's own deadline (LIM-M1), which starts when the request is sent, not when it waits. The probe in `status` does not take the gate. The registry's own operations (take, return, reap) are bounded by **LIM-O11** (candidate 2 000 ms) and fail safe: a registry that cannot be read or written within LIM-O11 lets the runtime proceed as if alone and says so at `warn`, because a liveness defect in the registry must never stop the user's work; the residual risk is THR-37's. *[review]* This fail-open is deliberate and must be visible, not only logged: while it lasts, `status` shows `admission: unavailable (<reason>)`, and the task's events record that it ran unadmitted. The gate's width is configurable per backend (`max_in_flight`, default 1, LIM-O12's range), because a llama-server with several slots serves requests in parallel by design.
8. **The machine view (REQ-OBS-009, new).** `status` gains a section: runtimes of this user that are executing or waiting, each with pid, data directory, task id and since when; the engine gates held; the slots taken against LIM-X5. The words `running`, `waiting` and `slot` are used nowhere else with another meaning. `metrics` gains `admission_wait_ms` and `engine_wait_ms` per task.

### Cross-cutting

- **Vocabulary.** A task may be queued for two reasons that are kept apart by one field: `concurrency` (LIM-X1 within the runtime, as today) and `admission` (LIM-X5 across the user's runtimes). The book explains both in one table.
- **Threats.** THR-37 (new): several runtimes of one user exhaust the machine's memory, CPU or the engine; controls: decisions 5 to 7; residual: the registry's fail-safe path. THR-38 (new): a stale or crafted registry entry (same user) blocks or bypasses admission; controls: the lock-plus-pid liveness, owner-only files, reaping at every open, `mimlys runtime reap` as the manual path; residual: a same-user process can always consume the user's own resources, which this RFC does not try to prevent.
- **Relationship to RFC 014 (the daemon).** If the daemon is accepted, its scheduler takes over decisions 5 and 7: the registry then records "a daemon serves this user and this data directory", the words, the events and the LIM ids stay, and clients never take slots themselves. Decision 6 is independent of the daemon.

## Alternatives considered

- **Raise LIM-X1 and LIM-X2 inside one runtime.** It answers "several jobs" only for one process and one store, needs the worker to run executions concurrently (a different worker), and does nothing for several runtimes. Rejected for this RFC; the daemon's scheduler may revisit it.
- **A system-wide registry (all users).** It crosses the user boundary that every other control respects and would need a privileged helper. Rejected; per user only.
- **Bound CPU with `nice` and `SCHED_IDLE` only.** Cheap but not a bound: an idle machine still gives the tree every core, and the user's own foreground work is what the bound protects. A cgroup quota is a bound; a priority is kept as the macOS fallback where no quota exists.
- **Let the engine arbitrate requests.** Ollama and llama-server queue or swap by their own rules and tell the runtime nothing; the user sees two runtimes that are each "running" and both slow. The gate makes the wait visible and attributable.
- **Expire tasks after N days instead of `prune`.** Retention by time was removed in 0.3 for a reason: a user's work must never vanish without being shown. `prune` shows and asks.

## Consequences

- A second terminal can watch a task; a runtime that would exhaust the machine waits and says why; a build cannot take every core; the user can see and reclaim what mimlys holds.
- New configuration under `[runtime]` (`admission_slots`, bounded by LIM-X5's range) and `[policy.ceiling]` (`cpu_share`); new `status` sections; a new event; new metrics; a new command. Under REQ-MAIN-001 these are additions. The queued flag gains a `reason` field, an addition to the JSON output.
- The registry is one more place the runtime writes outside the data directory; it is owner-only, bounded in size (one small file per slot and per gate), and reaped.
- The measurements for LIM-X5 and LIM-H7 are a report in the book, like the M2 and M4 reports.

## Open questions

1. Should `admission_slots` count executions, as decided, or runtimes? Executions were chosen so that waiting and idle runtimes are free; the RFC's review may find a case where a runtime's worker alone (resident, compiled) is a cost worth counting.
2. The macOS CPU share: a priority is all the platform offers without a privileged helper; is "best effort, reported" acceptable for a clause part, as it is for K? The owner's answer on K suggests yes.
3. Whether `task prune` should also offer the promotion backups of tasks whose promotion is no longer reversible (because the project changed since), with a separate confirmation.
4. Whether the read-only open should also serve `promote --patch` and `rollback --dry-run`, which read only.

## References

The owner's review package `.git-exclude/reviewed/owner-review-replanning-after-0.8/` (README §2 to §3, the assessment, the theme proposal); the replanning report §3 C, §3 D; requirements REQ-SCHED-001 … 005, REQ-CONF-001 … 012, REQ-WS-008, REQ-WS-009, REQ-PERS-001 … 012, REQ-OBS-001 … 008; external design §13.3, §13.5, §14.6, §16, §17, §19, §20; limits LIM-X1 … X4, LIM-H1 … H6, LIM-O3, LIM-M1, LIM-T5; threats THR-23, THR-32.
