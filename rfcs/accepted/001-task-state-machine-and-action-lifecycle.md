# RFC 001 — Task state machine and action lifecycle

**Status.** Accepted (owner sign-off 2026-10-27)
**Tracks.** Decisions D-02 and D-03 of the startup review; findings C-01, C-02, C-03, C-05, A-10, A-11, R-11.
**Touches.** `mimlys-core` (task, step, action modules), the persistence schema, the CLI's state display, external design §5, §6.1, §7.

## Summary

Freeze the task state machine, the step structure and the action lifecycle, including the `unknown` outcome that makes crash recovery honest, and the fixed vocabularies of halt reasons and failure causes. These are the semantics every schema, API verb, test and audit event depends on; the startup review found them in three, two and four inconsistent versions respectively.

## Motivation

- The drafts listed seven, nine and ten task states, with `Halted` both terminal and resumable (C-01, C-02).
- The action lifecycle differed between two sections of one document and had no state for "the runtime restarted while this was running" (C-03), although the drafts required distinguishing that case to avoid replaying irreversible actions (R-11).
- Four failure taxonomies coexisted (C-05).
- One word, `Halted`, covered four different causes, so a user could not tell "I paused it" from "safety stopped it" (A-10).

## Decision

The normative text is external design §5 (task states and transitions), §6.1 (step phases and outcomes), §7.1 (action lifecycle), §7.4 (termination), §7.5 (failure causes and dispositions) and requirements REQ-TASK-001 to REQ-TASK-012, REQ-DEC-001. In brief:

1. **Task states:** `created`, `running`, `waiting_user`, `paused`, `halted`, `recovering`, `completed`, `failed`, `cancelled`. The last three are terminal and immutable; continuing from a terminal task is a fork (REQ-TASK-004). `paused` is the user's stop; `halted` is the runtime's stop with a reason from a fixed vocabulary (REQ-TASK-007). `recovering` exists only at startup and resolves to `halted` or, by policy, `running`.
2. **Planning and evaluating are phases of a step**, not states of a task. A step holds one decision and at most one action; steps are sequential.
3. **Action lifecycle:** `proposed → rejected | waiting_approval | authorized`; `authorized → running → succeeded | failed{cause} | cancelled | unknown`. Content is immutable from `proposed`. `unknown` is terminal for the action; its task-level disposition follows the effect (RFC 003) and never re-executes a non-reversible effect automatically (REQ-PERS-008).
4. **Failure causes** (one axis): `tool_error`, `timeout`, `resource_exhausted{limit}`, `sandbox_fault{detail}`, `tool_unavailable`, `output_invalid`. **Dispositions** (the policy's output, a second axis): `feedback`, `retry`, `rollback`, `halt`. Timeout and exhaustion are causes, not states.
5. **Termination** of an execution is one of `exited`, `trapped`, `fuel_exhausted`, `deadline`, `memory_limit`, `output_limit`, `cancelled`, `host_fault`; `exit_code` exists only for `exited`. Fuel exhaustion and deadline are never conflated.
6. **`waiting_user`** covers both approvals and questions (`wait_kind`), with the same expiry rule; this generalizes the `waiting_approval` state of D-02 so that a model's question to the user does not need a second waiting state.

## Alternatives considered

- Keep `Halted` terminal and add `paused` (D-02 option B): every runtime stop becomes final; recovery and budget extension would need a new task each time. Rejected.
- Keep the draft's external design as written (option C): internally contradictory. Rejected.
- Model timeout and resource exhaustion as action states (D-03 option C): they are causes of `failed`, and making them states duplicates the taxonomy. Rejected.
- Two waiting states (`waiting_approval`, `waiting_question`): one more state for no behavioural difference. Rejected in favour of `waiting_user` with `wait_kind`.

## Consequences

- Every transition is a durable transaction (RFC 004); illegal transitions are refused with the error kind `state`.
- The CLI shows `halted` with its reason and `completed` with `verified` or `asserted` (RFC 003 defines the completion check's effect).
- Tests enumerate every legal and illegal transition (REQ-TEST-003).

## Open questions

None at proposal. The `waiting_user` generalization deviates from the letter of D-02 and is flagged for the owner's confirmation in the M0 review package.

## References

Requirements §5, §6; external design §5, §6.1, §7; startup review decisions D-02, D-03.
