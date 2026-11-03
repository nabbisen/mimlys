# Handoff 001 — Task state machine and action lifecycle

**RFC.** [RFC 001](../../done/001-task-state-machine-and-action-lifecycle.md) (Accepted 2026-10-27)
**Specification.** Requirements §4 (REQ-ARCH-001, 002, 006), §5 (REQ-TASK-001 … 012), §6 (REQ-DEC-001, 010, 012); external design §5, §6.1, §6.5, §7.1, §7.3–7.5, §17.1; limits LIM-T1 … LIM-T6, LIM-U4 … LIM-U6.
**Crate.** `mimlys-core`. No I/O, no Tokio needed here.
**Milestone.** M1.

## 1. Scope

In: the domain model and the state machines of tasks, steps, actions and executions; counters and budgets; halt reasons, failure causes, termination and dispositions; the event vocabulary; identifier and clock sources; canonical hashing. Everything is pure: transitions are functions from a state and an input to a new state or a refusal.

Out: parsing decisions (handoff 002), effects and grants (003), persistence (004), the CLI (010). Design the types so those handoffs plug in without changing this one.

## 2. Module layout

```text
crates/mimlys-core/src/
  lib.rs
  ids.rs        ids/tests.rs       TaskId, StepId, ActionId, ExecutionId, ObservationId, ApprovalId, QuestionId, GrantId, EventId
                                   (UUID v7, lowercase hyphenated; `IdSource` trait with a `SystemIds` impl, LIM-U5)
  clock.rs      clock/tests.rs     `Clock` trait (now() -> Instant as RFC 3339 UTC ms, LIM-U4); `SystemClock`; `FakeClock` behind `cfg(test)` or a `test-support` feature
  hash.rs       hash/tests.rs      canonical JSON (sorted keys, no insignificant whitespace, NFC) → SHA-256 hex (LIM-U6)
  task.rs       task/tests.rs      TaskState, HaltReason, Completion, FailureReason, Task, `transition(&Task, TaskInput) -> Result<Task, StateError>`
  step.rs       step/tests.rs      StepPhase, StepOutcome, Step, Counters, Budgets, `Counters::apply(...)`
  action.rs     action/tests.rs    ActionStatus, Action, Termination, FailureCause, Disposition, Execution (record)
  event.rs      event/tests.rs     Event (the vocabulary of ED §17.1), Actor
  limits.rs                        every LIM-T and LIM-U constant with its id in the doc comment (the other handoffs add their groups)
```

## 3. Types and behaviours

### 3.1 Task (ED §5, REQ-TASK-002 … 012)

- `TaskState`: `Created, Running, WaitingUser{kind: WaitKind}, Paused, Halted{reason: HaltReason}, Recovering, Completed{completion: Completion}, Failed{reason: FailureReason}, Cancelled`. Serialize the state name in snake_case; keep the payloads as sibling fields in records (`state`, `halt_reason`, …) so `--json` output stays flat.
- `HaltReason`: exactly the fourteen values of ED §5.2 (REQ-TASK-007).
- `TaskInput`: `Resume{extend: Option<Extension>}`, `Pause`, `Cancel`, `NeedsUser(WaitKind)`, `UserDecided`, `UserHalted`, `Expired`, `Halt(HaltReason)`, `Complete(Completion)`, `Fail(FailureReason)`, `StartRecovery`, `Recovered{resume: bool}`.
- `transition` implements exactly the table of ED §5.1 and refuses everything else with `StateError { from, input }` (REQ-TASK-002, error kind `state` at the API). Terminal states refuse every input (REQ-TASK-003).
- `Extension` applies only from `Halted{BudgetSteps | BudgetDeadline}` and only raises `steps` and `deadline` within the ceiling passed in (REQ-TASK-005); the ceiling type comes from handoff 003 — take it as a parameter, do not define policy here.
- `pause_requested: bool` on `Task` is a flag, not a state; `Pause` while `Running` sets it; the loop (handoff 010) turns it into `Paused` at the step boundary.
- Deadline is an absolute instant set at creation from LIM-T5 (REQ-TASK-009); `Task::remaining(&self, now)`.

### 3.2 Steps, counters, budgets (ED §6.1, §6.5; REQ-DEC-001, REQ-DEC-010, REQ-TASK-008)

- `StepPhase`: `Assemble, Decide, Validate, Wait, Execute, Observe, Evaluate`; `StepOutcome`: the eight values of ED §6.1.
- `Counters { steps, consecutive_rejections, consecutive_failures, identical_decisions }` and `Budgets { steps, consecutive_rejections, consecutive_failures, identical_decisions, executions_per_action, deadline }` from LIM-T1 … LIM-T6.
- `Counters::apply(&mut self, StepOutcome, decision_hash, snapshot_id) -> Option<HaltReason>`: increments per ED §6.5, resets per the table, and returns the halt reason when a limit is reached. "Identical" means the same decision hash as the previous step with the same snapshot id (snapshot ids are opaque strings here; M3 fills them).
- Counters are plain data so handoff 004 persists them durably; rolled-back executions still count (REQ-TASK-008).

### 3.3 Actions and executions (ED §7.1, §7.3–7.5; REQ-TOOL-009, REQ-PERS-008)

- `ActionStatus`: `Proposed, WaitingApproval, Authorized, Running, Succeeded, Failed{cause}, Cancelled, Unknown, Rejected{code}`; `transition` per ED §7.1; content fields (`tool`, `arguments`, `action_hash`, `grant_set_hash`, `effect`, …) are set once at `Proposed` and never mutated (make them non-`pub mut`; expose builders).
- `Termination` with the eight values and `exit_code: Option<i32>` present only for `Exited` (REQ-SBX-003; enforce in the constructor).
- `FailureCause`: `ToolError{detail}, Timeout, ResourceExhausted{limit}, SandboxFault{detail}, ToolUnavailable, OutputInvalid`; `FailureCause::from_termination`.
- `Disposition`: `Feedback, Retry, Rollback, Halt(HaltReason)`; `disposition(cause, effect_is_reversible, idempotent, executions, budgets)` per ED §7.5 and REQ-TOOL-009. Effect data comes from handoff 003; take a small `EffectView { reversible, idempotent, kind }` parameter defined here so 003 can convert into it.
- `Execution { execution_id, action_id, attempt, started_at, ended_at, termination, snapshot_before, snapshot_after, stdout_blob, stderr_blob, metrics }` (ED §7.4).

### 3.4 Events (ED §17.1; REQ-OBS-001)

- `Event` enum with exactly the names of ED §17.1 (serialize as `task.state_changed` etc.), `Actor` (`User{session, os_user}`, `Runtime`, `Model`, `Tool{name}`), and the envelope `EventRecord { event_id, at, actor, task_id, step_id, action_id, policy_hash, event }`.
- Events are values; appending them is handoff 004's job.

### 3.5 Determinism (REQ-ARCH-006)

Everything that needs time or ids takes `&dyn Clock` and `&dyn IdSource`. No `SystemTime::now()` or `Uuid::new_v7()` outside those two impls (add a unit test that greps the crate for them, or a clippy `disallowed-methods` entry).

## 4. Tests (REQ-TEST-003, REQ-TEST-011, REQ-TEST-016)

- Every legal transition of ED §5.1 and §7.1, and a table-driven test that every other `(state, input)` pair is refused (`req_task_002_*`, `req_task_003_*`).
- Each halt reason reachable through `Counters::apply` (`req_dec_010_*`, `req_task_008_*`): LIM-T1, T2, T4, T6; and that an accepted step resets consecutive counters.
- `Termination::exited` requires a code and every other variant refuses one (`req_sbx_003_*`).
- Disposition table of ED §7.5 (`req_tool_009_*`): retry only for `Timeout`/`ToolUnavailable`, idempotent, executions below LIM-T3.
- Canonical hash: key order and whitespace do not change the hash; NFC normalization does not; a changed value does (`lim_u6_*`).
- Two runs with the same `FakeClock` and a scripted `IdSource` produce identical records (`req_arch_006_*`).

## 5. Recommended crates

`serde`, `serde_json` (canonical serialization implemented here, not via a feature), `sha2`, `uuid` (`v7`, `serde`), `unicode-normalization`, `thiserror`, `time` or `jiff` for RFC 3339 (pick one and state why). No others in this handoff.

## 6. Return

A review-request package under `.git-exclude/reviewed/m1-001/` with: the PR list, the test names per requirement, `cargo test` and `scripts/check-core-deps.sh` output, and any question where the specification seemed ambiguous. Do not resolve ambiguities locally.
