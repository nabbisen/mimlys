# Handoff 010e — A stop signal interrupts only what can be undone; the interruption API says what it does (release 0.10)

**RFC.** [RFC 010](../../done/010-cli-and-operator-interface.md) (follow mode and its signals) · **Specification.**
- REQ-CLI-005 and REQ-TASK-006, amended 2026-11-14 by the owner's decision;
- external design §19.2 and §7.2 ("After `unknown`");
- LIM-O1 (amended), LIM-O2, LIM-O5.

**Found.** The owner's check of handoff 010d's two decisions against the design philosophy (clean, safe and secure, robust; no user confusion).

**What is wrong after 010d.**
- **(a) A single Ctrl-C can interrupt an action that cannot be undone.** LIM-O1's watchdog interrupts any action in flight. For an external write, a host write without a backup, or an external read that is not idempotent, the effect may have happened; the action is still recorded `cancelled` and the task pauses (`loop_.rs`: a cancelled execution only restores the workspace snapshot). The design's own rule for a crash (§7.2, §16.3) says such an outcome is unknown and the task halts `unknown_outcome`.
- **(b) A request outlives a transition.** `Runtime::request_cancel` interrupts the step in flight and does not cancel the task, yet its name says "cancel". A request made while no step runs is kept until the next step, which may begin an hour later after a resume, and that step is then interrupted with no visible reason.
- **(c) The messages use the word "boundary",** which is not the user's word.

1. **The first signal interrupts only what can be undone.**
   - The watchdog interrupts the step's model request, or an action whose "After `unknown`" (§7.2) is not halt `unknown_outcome`: reads, workspace writes (rolled back), host writes with a backup.
   - Use the one existing rule, `action::after_unknown`, so that recovery and the signal can never disagree.
   - For any other action in flight, the process prints once: `waiting for <tool> to finish: it changes something outside this machine and cannot be undone; press Ctrl-C again to interrupt it anyway`. It waits for the action's end, which its own deadline bounds (LIM-O5 for an MCP call; the declaration's `timeout_ms` for a host tool), and then takes the pause at the step's end.
2. **A second signal interrupts it anyway, honestly.**
   - The action's outcome is recorded as unknown: the action is `cancelled` with an `outcome_unknown` notice, or a status if a notice cannot carry it. Say which you chose and why; an event field is additive, under §22.
   - The task halts `unknown_outcome` with the action listed, never `cancelled` as if nothing had happened.
   - A second signal with nothing undoable in flight cancels as today.
3. **`mimlys task cancel`** (REQ-TASK-006) ends the task `cancelled` as asked. An interrupted action that cannot be undone is recorded the same way, and the command's text and JSON name it.
4. **The interruption API** (library users; RFC 014's clients).
   - Rename `Runtime::request_cancel` to say what it does: it interrupts the current step (for example `interrupt_step`). Rename `cancellation` to match.
   - A request is spent by the step it ends (010d), and is also **cleared at every task transition**: pause, resume, cancel, halt. So it never reaches a step that began after it.
   - CHANGELOG under Changed, for library users. The CLI's behaviour does not change by this item.
5. **The words.** `SIGNAL_INTERRUPTED` and its neighbours say what happened in the user's words: "the step was still running N seconds after the stop signal, so it was interrupted (its workspace changes are rolled back); the task is paused". The book's page on signals, and `cli.md`, follow.

**Tests,** by state (§3c), naming REQ-CLI-005 and REQ-TASK-006:
- a first signal during an external write (an MCP tool declared `external_write`, held longer than the grace): the waiting line, the action's own end, the task `paused`, nothing interrupted;
- the same with a second signal: the outcome unknown, the task `halted` with `unknown_outcome`, the action listed;
- a first signal during a workspace write longer than the grace: interrupted and rolled back, as 010d;
- `task cancel` during an external write: `cancelled`, the action named with its outcome unknown;
- a request made while no step runs, then a pause and a resume: the next step is not interrupted.

**Mutations:**
- the rule ignored (every action interrupted at the grace);
- the second signal recorded as a plain `cancelled`;
- the request not cleared at a transition.

Return: `.git-exclude/review-requests/010e-interrupt/README.md`.

## After review (2026-11-14): 010e-3

010e-1 and 010e-2 are landed. The review answered the six design questions. The specification now says the following:
- external design §7.1, §7.2 (`host_write`), §7.5 (new paragraph), §16.3 and §19.2;
- REQ-CLI-005 and REQ-PERS-008;
- RFC 014's decision 6 note.

Eight changes follow. One PR is fine; two if 6 is large.

1. **A host write is never interrupted by the first signal** (question 2). The runtime undoes no host write: `plan_recovery` already halts `UndoThenHalt` `unknown_outcome`, and no code undoes "through the tool's backup". So `can_be_undone` is true only for `AfterUnknown::RedoPermitted` and `AfterUnknown::Rollback`. The signal and recovery then agree on what is done as well as on which actions. Keep the variant (it is recorded); fix the doc comments that list "a host write with a backup".
   - Test: for every effect kind × flags, `can_be_undone` is true exactly when `plan_recovery` of a running action with that effect does not give `unknown_outcome`. This replaces the test against `after_unknown`; it checks the one rule by its result.
2. **The first signal starts nothing new** (found in review). Today a first Ctrl-C during the model request does not stop the step from then starting its action. An external read that is not idempotent needs no approval, so a stop can be followed by the start of a call that cannot be taken back.
   - The first signal sets a third, weakest request, as at once as it sets the stage. Name it for what it does, for example `Runtime::start_no_new_action`; the second request and the third imply it.
   - `execute` reads it **before** `ActionInput::Start`. When it is set, the action is closed `cancelled` without starting, and its observation says the stop signal came before it began. The step ends `cancelled` and the task pauses as after the watchdog.
   - The same check covers a request of either other kind that is already set when an execution would begin. Then nothing is recorded `unknown` for a call that was never sent.
   - Cleared as the other two are.
   - In the signal handler, set the request **before** reading `irreversible_in_flight`. Then an action that slipped past the check was started before the read, and the check at the grace sees it. So the 250 ms re-check loop is no longer needed: remove it, unless you show a case it still covers.
3. **The waiting line names the effect** (question 3). `IrreversibleAction` carries the effect. The texts are in §19.2:
   - external write: `it changes something outside this machine and cannot be undone`;
   - host write: `it changes files outside the task's workspace and cannot be undone`;
   - external read that is not idempotent: `it has sent a request outside this machine that cannot be taken back`.

   Each ends `; press Ctrl-C again to interrupt it anyway`. When the store cannot say (`SIGNAL_UNKNOWN_ACTION`): `waiting for the action in flight to finish: mimlys cannot tell whether it can be undone; press Ctrl-C again to interrupt it anyway`.
4. **Say what happened after it happened** (your 6.7). The watchdog no longer prints the interruption line at the grace. `drive` prints it when a step has ended `cancelled` and the pause is taken. Then the race you describe (the step between its model request and an action that cannot be undone) can no longer print "interrupted" before a wait.
   - Wording: `the step was still running 4 seconds after the stop signal, so it was interrupted; the task is paused`.
   - Insert `, and its workspace changes were rolled back` before `; the task is paused` only when the step wrote `workspace.rollback`.
   - The second-signal line predicts nothing: `stop signal: interrupting <tool> although it cannot be undone`. The halt text and the unknown-action line then say what came of it.
5. **An action that cannot be undone, ended by the runtime, is `unknown`** (question 4 b). External design §7.5 and REQ-PERS-008 state the rule.
   - It applies when the execution started and the runtime, not the tool, ended it:
     - at `deadline`, `memory_limit`, `output_limit`, `process_limit`, `cancelled` or `host_fault{worker_died}`;
     - or by an MCP connection lost after the call was sent.
   - Then the action is recorded `unknown` (`MarkUnknown`), the observation's status is `unknown`, nothing is restored, and the task halts `unknown_outcome`. There is no retry and no feedback.
   - Map the MCP case to the termination it has in code, and say which.
   - The interrupted-anyway path of 010e-1 is one case of this rule; make it the same code.
   - Your test `..._ends_at_its_own_deadline` then expects the action `unknown` and the task `halted`/`unknown_outcome`.
6. **Every halt is in the events** (question 5). Give the step's end the task as it was before `execute`, so that each halt decided inside `execute` writes `task.state_changed`:
   - a disposition `Halt{..}` (`tool_unavailable`);
   - a failed restore;
   - item 5.

   Remove the special event of `ff9bd1b`. An event that was missing is additive (§22): no schema version moves. CHANGELOG, Fixed.
   - Test: for each of these halts, the last `task.state_changed` is the halt with its reason, and `metrics`' `halts_by_reason` counts it.
7. **The words for an unknown outcome cover every cause.** These are no longer only a crash and an interruption.
   - The halt text: `an action that cannot be undone did not finish by itself (the runtime stopped, it was interrupted, or it reached a limit), so what it did is unknown; check before resuming`.
   - The line: `outcome unknown: action <id> (<tool>, <effect>) did not finish by itself and cannot be undone; check what it did before relying on it`.
8. **The book and the CHANGELOG follow.** `cli.md` and `getting-started.md` drop "a host write that made a backup" and "a host write without a backup" (it is "a host write"). They state rule 2 ("starts nothing new"), the three waiting lines and rule 5. They also say that `unknown_actions` lists such actions for the task's whole life, a resume included (question 6: kept as you did it).

**Answers that need no code:**
- question 1: status `unknown`, confirmed, and §7.1 now says what it means;
- question 4 a: RFC 014's note, for the daemon's handoff;
- question 6: kept.

**Tests,** by state (§3c), naming REQ-CLI-005, REQ-PERS-008 and REQ-OBS-001:
- 1: the test against recovery.
- 2:
  - a first signal during the model request of a step whose action is an external read that is not idempotent: the action `cancelled` and never started (no `action.started`; the test server received no call), the task `paused`;
  - the same for a workspace write: never started, nothing rolled back.
- 3: each effect's line, by a unit test of the text chosen.
- 4: in `follow.rs`, the fake step reaches an action that cannot be undone after the grace. No "interrupted" line is printed, the waiting line is, and the rolled-back clause appears only with a rollback.
- 5: the deadline test and a `host_fault{worker_died}` case: action `unknown`, task `halted`/`unknown_outcome`, no `workspace.rollback`. A workspace write that times out stays `failed` and rolled back.
- 6: as said in item 6.

With 4 and 5 the 7 s and 1 s tests no longer depend on the step ending within 3 s of its hold. Say whether their holds can then stay as they are.

**Mutations:**
- `can_be_undone` true for `UndoThenHalt`;
- the check before `Start` removed;
- the interruption line printed at the grace again;
- a deadline on an external write recorded `failed`;
- the halt event omitted for `tool_unavailable`.

Return: `.git-exclude/review-requests/010e-3-interrupt/README.md`.

## After review (2026-11-14): 010e-4

010e-3 is landed. The review answered your six questions:
- 1, 2 and 5: accepted as you did them;
- 3, 4 and 6: three small changes, below.

The specification now says them: external design §7.5, §8 (the notice) and §19.2, and REQ-CLI-005.

1. **No new step begins after a stop signal** (question 4). `run_step`'s boundary also reads the weakest request. A step that would begin after the first signal then ends `cancelled` at once, without a model request, and `drive` pauses as after any cancelled step. It is the same rule as "starts no new action", applied one phase earlier. The request is spent by that step, as the others are.
   - Test, in `interrupt.rs`: `start_no_new_action` on a running task with no step in flight. The next `run_step` makes no model request (the backend counts the calls) and ends `cancelled`, `pause_task` then gives `Paused`, and the step after that begins normally.
2. **A call that never left is `tool_unavailable`, not `worker_died`** (question 3). In `crates/mimlys-mcp/src/tools.rs`, two outcomes become what `ServerError::Unavailable` already gives: termination `host_fault{unavailable}`, cause `tool_unavailable`.
   - `send:closed`: the request was not written.
   - `ServerError::Died`: the restart limit LIM-X3 was reached in `ensure_started`, so no call was made.

   `call:closed` stays `worker_died`, since the request was written. Then §7.5's "nothing was started" holds for them, and the table's rule for a server not reachable applies (REQ-TOOL-009, then halt `tool_unavailable`). An honest record beats a safe-looking one: the user checks for nothing when nothing was sent.
   - Tests:
     - a unit test of `map_outcome` for `send:closed` and `call:closed`;
     - a case where the restart limit is reached: the test server's `die` repeated past LIM-X3 (or a lowered limit in the test), the last attempt `tool_unavailable` and nothing `unknown`.
3. **`fuel_exhausted` is an end by the runtime** (question 6). LIM-S1 is a resource limit, as `memory_limit` is, and `trapped` stays the guest's own. No action that cannot be undone runs as WASM today, so this changes no run. It keeps `ended_by_runtime` the list §7.5 states.
   - Test: add it to the table of item 5's test.

**Mutations:**
- the boundary not reading the weakest request;
- `send:closed` back to `worker_died`.

CHANGELOG: Fixed (items 1 and 2). Book: `cli.md` Signals, one clause ("no new step begins").

Return: `.git-exclude/review-requests/010e-4-interrupt/README.md`.
