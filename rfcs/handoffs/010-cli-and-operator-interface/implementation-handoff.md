# Handoff 010 — CLI, configuration, the step loop and the M1 demonstration

**RFC.** [RFC 010](../../done/010-cli-and-operator-interface.md) (Accepted 2026-10-27 with the M1 review)
**Specification.** Requirements REQ-CFG-001 … 006; REQ-CLI-001 … 005; REQ-APR-009; REQ-TASK-001, 005, 006, 012; REQ-DEC-001, 009; REQ-SCHED-001, 002, 004, 005; REQ-OBS-004, 006; REQ-PRIV-003, 004; REQ-WS-002, 003, 008; REQ-ENG-004; REQ-ARCH-004 (loop API); external design §4, §5.1, §6.1, §10.1, §15.2, §19; limits LIM-O1, LIM-X1, LIM-X2, LIM-W1 … W5.
**Crates.** `mimlys` (binary: commands, configuration, prompt, follow mode, logging); `mimlys-core` (`runtime/`: the step loop and the control API implementation, pure orchestration over the SPI).
**Milestone.** M1. Depends on handoffs 001–004. Worker IPC skeleton: `mimlys-exec::protocol` message types only (M2 implements the worker).

## 1. Scope

In: the step loop (the only place that calls `ModelBackend`, the validator, the executor, the persistence methods in the order of ED §16.2); the control API as functions; configuration loading and validation; the CLI commands of M1; the approval prompt; follow mode; logging; workspace materialization by copy with exclusions and the size refusal; the M1 end-to-end demonstration with the fakes.

Out: `rollback`, `promote`, `gc` (M3), `status` tier details and `run_script` (M2), servers (M5). Register the commands with a clear "available from milestone Mx" error so the command set is stable (CLR-1).

## 2. Module layout

```text
crates/mimlys-core/src/
  runtime.rs    runtime/            loop_.rs (run_step), control.rs (control API fns), context_m1.rs (minimal prompt: segments 1–4, 7–9 of ED §15.1 using handoff 002's segment writer), tests.rs
crates/mimlys/src/
  main.rs                            clap entry; exit codes 0/1/2
  config.rs     config/tests.rs      TOML schema of ED §19.1 (M1 keys), `deny_unknown_fields`, secrets by reference, defaults from limits
  commands/                          run.rs, task.rs, approve.rs, reject.rs, answer.rs, grant.rs, revoke.rs, events.rs, metrics.rs, status.rs, check.rs
  prompt.rs     prompt/tests.rs      the fixed approval template over handoff 003's `Rendered`
  follow.rs                          event streaming to the terminal; prompts on `waiting_user`
  workspace.rs  workspace/tests.rs   materialization by copy with exclusions, case-mode probe, size refusal
  messages.rs                        the message catalog (REQ-CFG-006): every user-facing string as a named constant
  logging.rs                         tracing subscriber, stderr, JSON lines
crates/mimlys-exec/src/
  protocol.rs   protocol/tests.rs    IPC message types of ED §13.1 (serde round-trip only in M1)
```

## 3. Behaviours

### 3.1 The step loop (ED §6.1, §16.2; REQ-DEC-001, 009; REQ-SCHED-002)

`run_step(task) -> StepOutcome`, in order: check `pause_requested`, deadline and counters (halt or pause at the boundary) → assemble the M1 context → `backend.generate` with a timeout of `min(LIM-M1, remaining)` → `validate` → for a control decision: `plan` stores a version, `ask_user` → `waiting_user`, `complete` → completion rule, `give_up` → `failed` → for an action: persistence line 1–2 → approval required? → request, `waiting_user`, return; else line 4 (snapshot id `None` in M1) → line 5 → executor (built-in tool) under its deadline → line 6 → disposition (feedback / retry / halt) → line 7 → counters, line 8. Rejections become the next step's `feedback` segment. The loop never calls the executor without an `AuthorizedAction` (the type makes it impossible) and never calls the backend for anything but the decision (REQ-MDL-008). Reconcile grants against the ceiling at the boundary (REQ-CAP-010).

### 3.2 Control API (ED §4; REQ-TASK-001, 005, 006, 012; REQ-SCHED-001)

Functions over `Persistence` plus the loop: `create_task` (validate the goal and scope, refuse an overlapping scope with `conflict`, materialize the workspace, create in `created`), `resume_task`, `pause_task`, `cancel_task` (abort the backend request through `cancel`, cancel the execution token, persist, `cancelled`), `approve_action`, `reject_action`, `answer_question`, `grant_capability`, `revoke_capability`, `get_task`, `list_tasks`, `get_events`, `get_metrics`, `get_runtime_status`. Each state-changing call takes a `request_id` and is idempotent for a repeated id. A `Scheduler` runs at most LIM-X1 tasks and LIM-X2 executions; others wait in `created` with `queued = true`.

### 3.3 Configuration (ED §19.1; REQ-CFG-001 … 005)

`Config` with `#[serde(deny_unknown_fields)]` on every section; `config_version` must be 1; secret fields are a `SecretRef` enum parsed from `env:NAME` / `file:PATH`, an inline value is a hard error naming the key; every default is a `limits` constant; ranges enforced with errors that state the range (CLR-3). Location: `--config`, then `MIMLYS_CONFIG`, then the platform path (`directories` crate). `mimlys check` prints the resolved configuration with secrets masked and exits 0/1. No reload.

### 3.4 CLI (ED §19.2; REQ-CLI-001 … 005)

`clap` derive; subcommands exactly as ED §19.2 (M3/M2/M5 ones present but returning the "available from" error); `--json` on every command prints one JSON object or an array with stable snake_case fields; exit codes 0/1/2; usage errors never print a stack trace. `mimlys run` = `task new` + follow mode; a stop signal (SIGINT/SIGTERM, Ctrl-C on Windows via `tokio::signal`) sets `pause_requested` on running tasks and prints "pausing at the next step"; a second signal cancels; the process exits within LIM-O1 after persisting.

### 3.5 Approval prompt (ED §10.1; REQ-CLI-002; REQ-APR-009)

Prints the `Rendered` lines through the fixed template, never any other string from the task; reads one of `a`, `r`, `h`, `g`; `r` asks for an optional note (≤ LIM-D2); `g` opens a guided grant (kind, scope) checked against the ceiling. The session is the process; `decided_by` records the process id and the OS user name (REQ-APR-009). In `--json` or non-interactive mode a pending item is reported and the command exits 0; approvals then come from `mimlys approve`.

### 3.6 Workspace materialization, M1 (ED §14.2; REQ-WS-002, 003, 008)

Copy the scope into `<data_dir>/workspaces/<task_id>/tree` using `cap-std` directory handles on both sides; honour LIM-W4 and LIM-W5 exclusions with `globset`; recreate a symbolic link only when its target resolves inside the scope (lexically, against the scope root), else skip with `symlink_omitted`; count bytes and files during the copy and refuse above LIM-W1 / LIM-W2 with the measured numbers before writing anything (walk first, copy second); probe the case mode by creating and looking up a temporary file with a different case; reflink is an optimization for M3. Per-step quota accounting: sum bytes written by tools in the step and refuse above LIM-W1 with the halt reason `workspace_quota`.

### 3.7 Logging and privacy (REQ-OBS-004, REQ-PRIV-003, 004)

`tracing` JSON lines on stderr with timestamp, level, event, ids, durations; never the prompt, file content or a secret at `info`; a `debug_prompts = true` key logs prompts and prints a warning at start. Child processes (none in M1 except the test harness) receive an explicit environment built from `env_allow`.

### 3.8 Test harness (REQ-TEST-002, 007, 016)

`mimlys-testloop`, an integration-test binary or example in the `mimlys` crate that runs one task with the fake backend and fake tools from a script file, honours the crash-injection variable of handoff 004, and prints the final task record as JSON. Handoff 004's crash tests and the M1 demonstration use it.

## 4. The M1 demonstration (requirements §26, external design §23)

A script `tests/m1/demo.json` with decisions: `plan` → `act read_file` → `act write_file` (profile lists `write_file` in `approval_tools` for the demo so the prompt appears) → approve → `act read_file` → `complete`. Assertions: the state sequence, the eight persistence lines per action, the rendered approval text, `completion = asserted`; then the same script with `completion_check` configured → `verified`. A second script with `read_file{path: "../../etc/shadow"}` and `read_file{path: "../../does-not-exist"}` asserts byte-identical `denied`. Crash injection at every point resumes correctly (handoff 004). All of it runs in CI on the three platforms.

## 5. Recommended crates

`clap` (derive), `toml`, `directories`, `tracing`, `tracing-subscriber` (json), `tokio` (`rt-multi-thread`, `signal`, `time`), `cap-std`, `globset`, `whoami` or `std::env` for the OS user, `serde_json`.

## 6. Return

Review-request package under `.git-exclude/reviewed/m1-010/` with the demonstration output on the three platforms, the `--json` field list per command (contract), and the message catalog.

## 7. Amendments from the implementation (2026-10-27)

Recorded here so that the handoff stays the contract the code follows; the review package under `.git-exclude/reviewed/m1-010/` explains each.

1. **Events.** Persistence line 6 carries `execution.finished{attempt, termination}` (tool actor); `action.finished{status, cause?}` is emitted once, at line 7. External design §17.1 lists the new name.
2. **Approvals and grant changes.** A grant issued, or a grant the action does not use revoked, while an approval is pending withdraws the request (`approval.invalidated`) and asks again under the new grant set (`approval.requested`); the task keeps waiting. Revoking a grant the action uses rejects the action `denied` and the task returns to `running`. The prompt's `g` option relies on this.
3. **Cancellation.** The runtime keeps one cancellation token per task; `request_cancel` reaches the model request and the execution in flight, whose step then ends `cancelled`; the follow loop applies `cancel_task` or `pause_task` afterwards. A first stop signal pauses at the boundary, cancelling the execution only when the boundary is not reached within LIM-O1.
4. **Scheduler.** `resume_task` (and so `create_task` without `hold`) refuses to put a second task into `running` with `conflict` (LIM-X1 = 1); a held task waits in `created`. No separate scheduler type exists in deployment v1, where a single process drives a single task.
5. **Request ids.** The idempotency key of ED §4 is not implemented in the in-process tier: every CLI invocation is one request and no retry path exists. It returns with daemon mode (DR-002).
6. **Resume.** A resume that would halt again at once (deadline passed, step budget used up) is refused with a message naming the extension to give.
7. **Fake backend.** The script position is per process; `mimlys approve` in a second process restarts the script. The demonstration and the tests use `mimlys-testloop`, which drives one task in one process. A real backend (M4) has no such limitation.
8. **Follow mode after every verb.** Any command that leaves a task `running` follows it, because steps run only inside the driving process; `task new --hold` is the way to create without running.
9. **Windows ACLs.** As noted in handoff 004: the data directory is 0700/0600 on POSIX; on Windows the profile's ACL applies until M6b. `status` lists the confinement tier as `core` with the M2 note.
