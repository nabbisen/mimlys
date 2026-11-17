# Handoff 010f — The start gate: a stop signal and an action's start are one step (release 0.11, phase 2)

**RFC.** [RFC 010](../../done/010-cli-and-operator-interface.md) (follow mode and its signals). **Found by.** The audit before 0.10 (A-33), the review of 009g, the review of 009h (a CI failure), and the re-check of 2026-11-16 (`ROADMAP.md`, "Release 0.11", phase 2).

**Specification** (amended before this handoff): external design §19.2, "The check and the start are one step"; REQ-CLI-005 as it stands ("the step in flight starts no new action").

**Embargo:** none.

## Items

The lines below are as found on `main` at `70ab081`; check them on today's `main`.

1. **A-33: the stop check is atomic with the start.**
   - **Today:** the check of the first request is at `loop_.rs:1259`, `Start` at `:1289`, the durable `record_running` at `:1291`, and the call at `:1307`. A first signal that falls in between lets an action that cannot be undone begin after the stop. On the other side, `follow.rs:254-264` sets the request and then looks at the persisted running action.
   - **The rule (§19.2):** either the action is not begun, and then never begins, or it is begun and recorded `running` before the signal's handler looks.
   - **The design (the re-check's option B):** a per-task start lock, held across the check and `record_running`, and taken by `start_no_new_action` as well. The signal's handler then waits for at most one durable write.
   - If you find a cleaner shape, say so before you build it.
2. **The pin check observes the first stop request.**
   - **Today:** the MCP pin check inside `execute` (`crates/mimlys-mcp/src/tools.rs:621-622`) observes `context.cancel`, which is `execution_interruption`: the undoable or the anyway token, never the weakest one. The boundary's check does use the weakest.
   - **The change:** give `ExecutionContext` the start gate's token, for waits made before anything is sent. A provider that stops at it reports **not started**, and the loop records that like `stopped_before_start`.
   - **The trap to avoid** (the re-check found it): routing it through today's `list:cancelled`, then `tool_unavailable`, would **halt a non-idempotent task `tool_unavailable`** on a first stop signal (`action.rs:559-567`). A first Ctrl-C must end the action as not begun and pause the task, never halt it.
3. **Find why `req_cli_005_one_stop_signal_pauses_at_the_next_boundary` failed once on the arm runner** (#118, run `43784914996`, attempt 1). The step ran 4 526 ms against the grace of 4 000 ms, and the action was `succeeded` where the test expects `cancelled`. This test led to 010d.
   - Decide whether the test's timing allows an action to finish just as the grace passes (then fix the test so that it decides by state, §3c), or whether a step past the grace can escape its interruption (then it is a defect: fix it).
   - Item 1 may change the answer, so do it after item 1, and state what you found and how.

## Tests (by state, §3c)

- **1:**
  - a persistence fake that raises the first signal inside `record_running`: the action is either not begun, or begun and waited for or interrupted by its kind, never begun after the handler found nothing;
  - a first signal just before the check: no call reaches the tool;
  - `m5_signals` reruns green.
- **2:**
  - an MCP pin check that hangs (`HANG_LIST_AFTER`) and a first signal: the action ends not begun (`stopped_before_start`) within LIM-O1, and the task pauses, **not** halts;
  - the same for an external write, an external read that is not idempotent, and an idempotent read.
- **3:** your finding's own test.

## Mutations

- the lock not taken by `start_no_new_action`;
- the check outside the lock;
- the pin check observing the strong token again;
- "not started" routed to `tool_unavailable`.

## Book and CHANGELOG

- `cli.md` (signals).
- CHANGELOG: Fixed.

Return: `.git-exclude/review-requests/010f-start-gate/README.md`.

## After review (2026-11-17): 010f-4

010f-1 to 010f-3 are landed. Two changes follow.

1. **Check, record running, check again, instead of the lock** (your question 1: accepted). Replace the start gate with a second look at the weakest request after `record_running` and before the call. If it is set, close the action as not begun by the path item 2 made (`close_not_started` from `running`).
   - **The proof:** the handler makes its request and then looks. If its look found nothing, the request came before `record_running`, so the second check sees it. Otherwise the look found the action `running`.
   - **What goes:** `at_the_start_gate`, `request_at_the_gate`, the gate field, the poison handling, and the library note that a stop request "may block".
   - **What stays:** the tests, asserting the same states. Re-run your mutations against the new shape, the second check removed among them.
2. **`Server::call`'s restart wait observes the first stop request too** (your question 3). Today `ensure_started` waits for a server that died between the pin check and the call with the call's strong token. Give it the start gate's token, so that a first signal ends that wait with the action not begun.

**Your other questions:**
- **2, the waiting line before a pin check that ends not begun:** accepted. It is true when said.
- **4, a seam for `drive`:** not wanted. Reading, plus your local reproduction, is accepted.
- **5:** §8's clause is written.

Return: `.git-exclude/review-requests/010f-4-start-gate/README.md`.
