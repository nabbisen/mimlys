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
