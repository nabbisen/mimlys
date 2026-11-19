# Handoff 013d — Operation on one machine, part two (release 0.12, phase 2; M7b)

**RFC.** [RFC 013](../../accepted/013-operation-on-one-machine.md), decisions 5 to 8, read with Amendments 1 and 2. **Found by.** The scope of 0.12 (`ROADMAP.md`, "Release 0.12", phase 2), and 1.0's condition 5.

**Specification** (written before this handoff):
- external design §19.3 (new: the registry, admission, the engine gate, the fail-open, the CPU part, the machine view), §17.1 (`task.admission_changed`) and §19.2 (`runtime reap`);
- REQ-SCHED-006, REQ-SCHED-007, REQ-CONF-013 (amended) and REQ-OBS-009;
- LIM-X5, LIM-H7 and LIM-O12, fixed from the M7b report; LIM-O11;
- THR-37 and THR-38.

**Embargo:** none.

## Today

These are as found on `main`; check them on today's `main`.

- **Nothing of decisions 5 to 8 exists:** no registry, no `LIM_X5`, no `cpu.max`, no gate, no machine view.
- **No queued state exists** (Amendment 2, point 6). A runtime runs one task at a time.
- **The worker's cgroup** (Linux) is `mimlys-<pid>` with its `tool` child. The worker sits in `mimlys-<pid>` itself, so no controller can be enabled below it (Amendment 2, point 1).
- **A worker per task** on macOS and Windows (011o, 011p), whose Job Object (Windows) holds the tools.

## Items

One PR each, or stacked, as you prefer.

1. **The registry and admission (decision 5).**
   - **The registry:** the directory, the entries, the locking and the stale-entry rule, as §19.3 says. Every operation is bounded by LIM-O11. It is owner-only, it never follows a link, and it refuses an entry it cannot parse (reaped, logged).
   - **The slot:** taken before the action is recorded `running`, returned when the execution ends, however it ends (a crash's slot is stale and reaped). `[runtime] admission_slots`, within LIM-X5's range, is checked at load.
   - **The wait:** the task stays `running`; `task show` and `status` say `waiting: admission (…)`; `task.admission_changed` is recorded at each change; `admission_wait_ms` is measured.
     - **The first stop request ends the wait** with the action not begun and the task paused. Use the start gate's rule (010f, `ExecutionContext.start_gate`): the wait is one more wait before the send.
     - **The deadline keeps counting.**
   - **The fail-open:** past LIM-O11 the runtime proceeds, says so at `warn`, records `task.admission_changed{state: unadmitted, reason}`, and `status` shows `admission: unavailable (<reason>)`.
   - **`mimlys runtime reap`.**
2. **The engine gate (decision 7).**
   - **The gate:** one per backend URL per user, in the registry. It is taken before a model request is sent and returned when its answer is read.
   - **Its width:** `[backend] max_in_flight`, default LIM-O12.
   - **The wait:** `engine_wait_ms` and a log line, ended by the first stop request as an interrupted request is today (010e).
   - **The request's deadline (LIM-M1) starts when it is sent.**
   - **`status`'s probe** takes no gate.
3. **The CPU part of clause C (decision 6; REQ-CONF-013, Amendment 2).**
   - **Linux:** the worker moves into a leaf `worker` of its cgroup, beside `tool`. The parent keeps the memory and process limits, which cover both leaves; `cgroup.kill` of `tool` and the descendant walk are unchanged. Then `cpu.max` goes on `tool` from LIM-H7. Where the controller cannot be enabled, the part is `unavailable` with the reason, and the book's page says how to bound the whole runtime with `systemd-run --user --scope -p CPUQuota=…`.
   - **Windows:** the worker's Job Object rate, converted (Amendment 2, point 2).
   - **macOS:** `setpriority` (a lowered nice value), never the background policy, reported `best_effort (priority)`.
   - **The probe and `status`** list the part beside the clauses, and **the tier never changes because of it.**
   - **The budget:** `[policy.ceiling] cpu_share`, raised per task within the ceiling as the other budgets are.
4. **The machine view (decision 8).** `status`'s section and the two metrics. The words `running`, `waiting` and `slot` keep the one meaning §19.3 gives them.

## Tests (by state, §3c)

- **Two runtimes of one user with LIM-X5 = 1** (separate data directories, one registry):
  - the second's execution waits;
  - its `task show` and `status` say why, with the holder's pid;
  - it is admitted when the first's execution ends;
  - the events record both changes.
- **A first Ctrl-C during the admission wait:** the action is not begun, `stopped_before_start`, and the task is paused (on every platform's job).
- **A stale entry** (its holder killed) is reaped at the next open, and `runtime reap` lists it.
- **A registry that cannot be written** (permissions, a seam for the time) runs unadmitted, and says so in `status` and the events.
- **The engine gate:** two runtimes against one fake backend URL with `max_in_flight = 1`: the second request waits, the metric counts it, and a Ctrl-C ends the wait.
- **The CPU part on Linux:**
  - the worker's leaf and `tool`'s `cpu.max` where a delegated cgroup allows it (the CI's `systemd-run -p Delegate=yes` arrangement from the spike);
  - `unavailable` with the reason where it does not;
  - the memory and process limits still hold, and the tree kill is unchanged.
- **The CPU part on Windows:** the Job Object's rate, read back. **On macOS:** the nice value, read back, and the status word.
- **The machine view:** the section lists both runtimes, and its JSON shape.
- **The registry is outside every write set:** a host tool cannot create, change or remove an entry (Linux, macOS and Windows jobs).

## Mutations

Each of these must be killed:
- no slot taken;
- the slot taken after `running` is recorded;
- the wait not ended by the stop request;
- a stale entry never reaped;
- the fail-open silent;
- the gate not taken;
- `cpu.max` on the worker's own cgroup instead of `tool`;
- the macOS background policy instead of `setpriority`;
- the machine view's slots not counted.

## Book and CHANGELOG

- **The book:** the operation page (two runtimes; what `waiting` means; `runtime reap`; the CPU part per platform and the `systemd-run` way on Linux), `status` in `cli.md`, and the configuration keys.
- **CHANGELOG:** Added (admission, the gate, the machine view, `runtime reap`, the CPU part), Changed (the worker's cgroup leaf on Linux).

## Return

`.git-exclude/review-requests/013d-operation-on-one-machine/README.md`, with:
- the PRs and their CI runs;
- the mutations;
- the design questions.
