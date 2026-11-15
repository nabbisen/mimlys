# Handoff 003b — The approval card hides no argument; a stop signal at a prompt ends the process (release 0.10)

**RFC.** [RFC 003](../../done/003-effects-grants-and-approvals.md) (approvals) and [RFC 010](../../done/010-cli-and-operator-interface.md) (follow mode) · **Found by.** The independent audit before 0.10, which audited commit `f5a94fa`. The owner accepted the triage on 2026-11-14.

**Specification** (amended before this handoff):
- external design §10.1 ("No argument is ever hidden");
- REQ-APR-002;
- REQ-CLI-005 and §19.2 as they stand ("the process exits within LIM-O1").

**The defects, verified in the code at `f5a94fa`:**
- **A-02, high.** `approval.rs:110-118` cuts the whole Arguments line at its tail, with one `…`. `render_arguments` (`:177-207`) walks a `serde_json::Map` that is sorted by key.
  - A long early value (`body`) pushes later ones (`subject`, `to`) past LIM-I6, and no line names what was dropped.
  - For an MCP tool, the "Will" line names only the tool, so the recipient appears nowhere.
  - **The user has no way to see the full arguments:** the prompt (`prompt.rs:45-53`), `task show` and its JSON carry the cut text only.
  - Also: `display_text` appends the model's reason after the bound, so the whole text can exceed LIM-I6.
- **A-07, medium.** After `drive` installs the stop listeners (`follow.rs:131-143`), the `WaitingUser` branch blocks in `read_line` (`prompt.rs:121-129`) and never looks at the signals.
  - At an approval or question prompt, Ctrl-C and SIGTERM leave the process waiting, past LIM-O1.
  - Both lines it prints are false there ("the task pauses when the step in flight has finished…", "cancelling the task…").

## Items

1. **Every argument is named, and none is hidden** (A-02).
   - **Order:** destination arguments and path-like arguments first, then the rest in a stable order.
   - **Each value has its own share** of what LIM-I6 leaves after the fixed lines. A cut value says that it is cut and how many characters are left out, for example `… (5 200 more characters)`.
   - **The model's reason is inside the bound.**
   - **Everything stays sanitized and quoted,** as today.
2. **The whole arguments, before the decision.**
   - The prompt gains `[v]iew`, which prints every argument in full (sanitized) and asks again.
   - `mimlys task show <task> --arguments` prints the pending action's arguments in full.
   - The JSON of `task show` gains `pending_approval.arguments`, in full and sanitized (an additive field).
3. **An action whose arguments cannot all be named is not put to the user.**
   - Show from the limits that every key always fits: the parameter count and name length bounds, which handoff 009g tightens for MCP tools. Then the refusal path is unreachable, and you test the bound.
   - If the limits do not make every key fit, reject the action with a stated reason, and propose its word. I register it in §6.4 at review.
4. **A stop signal at a prompt** (A-07). The read races the stop listener. On a signal at a prompt:
   - the task is left as it is, `waiting_user` (it holds nothing: REQ-APR-006);
   - one true line is printed in the user's words, for example `stop signal: <task> is still waiting for your approval; decide later with mimlys approve <task> or mimlys reject <task>` (for a question, `mimlys answer`);
   - the process exits within LIM-O1, with code 0.

   Neither "pausing" nor "cancelling" is said, since neither happens.

## Tests (by state, §3c)

- **1:**
  - the auditor's `send_email{body, subject, to}` with a 5 800-character body: the card shows `to` and `subject` before `body`, and `body` says how much is cut;
  - every key appears for the tool with the most parameters the limits allow;
  - the card is at most LIM-I6, with the reason included.

  REQ-APR-002.
- **2:**
  - `[v]iew` prints the full body and the prompt asks again;
  - `task show --arguments` and the JSON field hold the full arguments.
- **4:** `mimlys run` at an approval prompt, on a pty (as the auditor did), and again at a question prompt:
  - SIGINT, and separately SIGTERM: the process has exited within LIM-O1;
  - the task is `waiting_user`;
  - stderr is the one line.

  REQ-CLI-005.

## Mutations

- the arguments rendered in key order;
- the whole line cut again;
- `[v]iew` printing the cut text;
- the prompt read not raced.

## Book and CHANGELOG

- The approval page shows the new card and `[v]iew`.
- `cli.md` covers `--arguments`, the JSON field, and the signal at a prompt.
- CHANGELOG: Fixed, citing A-02 and A-07.

Return: `.git-exclude/review-requests/003b-approval-audit/README.md`.
