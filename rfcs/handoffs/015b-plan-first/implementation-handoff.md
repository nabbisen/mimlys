# Handoff 015b — Plan first: a plan shown and approved before anything acts (release 0.13, phase 1)

**RFC.** [RFC 015](../../accepted/015-adaptive-effort.md), decision 6, read with its **Amendment 1**, item 8. **From.** The scope of 0.13 (`ROADMAP.md`, "Release 0.13", phase 1).

**Specification** (amended before this handoff):
- REQ-TASK-015;
- external design §5 (the diagram, §5.1's rows, and the paragraph "Plan first"), §6.4 (`state{plan_required}`; `approval_rejected` for a plan), §17.1 (`plan.approval_requested`, `plan.approval_decided`) and §19.2 (`--plan-first` on `run` and `task new`; `approve [--note]`; the plan in `task show`).

**Owner's decision (2026-11-21):** the scope's §2.4 as recommended. The task waits for the plan as for an approval, the same verbs keep one meaning, `ask_user` and `give_up` are allowed before the plan, and only the first plan is gated.

**Embargo:** none.

## Today

These are as found on `main` at `daede80`; check them on today's `main`.

- **A `plan` decision** records a plan version and the step continues (`loop_.rs:1204`). Nothing waits on it.
- **`WaitKind` is `approval` or `question`** (`task.rs:14`), and `PendingItem` has the same two (`task.rs:203`). `StateReason` has no `plan_required` (`rejection.rs:203`).
- **A rejection's note** reaches the model as a `user` segment of kind `rejection_note` (`runtime/context.rs:490`, `:552`). An approval carries no note: `approve` has only the task (`main.rs:98`).
- **No command shows a plan.** Plan versions are recorded (REQ-DEC-011), but `task show` prints none.
- **The expiry of a pending item** (LIM-A1) is in `recovery.rs:468` (`expire_waiting`), and in the runtime's own expiry.
- **Readers:** 0.13's reader tolerates the new words (004e). 0.12.0's `task show` fails on a task that holds `wait_kind: plan` or `plan_required` (the notes say so).

## Items

1. **`--plan-first` on `run` and `task new`.**
   - The task records `plan_first` (an optional field of its record, absent when false), and `task.created` carries it.
   - A fork is plan-first only when it is created with the flag.
2. **The gate (external design §5, "Plan first").**
   - **Before the approved plan:** `act` and `complete` are rejected `state{reason: plan_required}`, with the hint "state your plan first (decide `plan`)", counted toward LIM-T2. `ask_user` and `give_up` go through.
   - **The first `plan`:**
     - the version is recorded;
     - the task goes to `waiting_user{wait_kind: plan}` with `pending: plan{version}`;
     - `plan.approval_requested{version}` is recorded;
     - the step ends in the wait phase.
   - **What the gate holds:** which version is approved, for example `plan_approved: version`, optional. After the gate, a later plan is recorded and the task goes on.
   - **The runtime knows the gate from the task's record,** never from the configuration, so a restart, a recovery or a resume keeps it.
3. **`approve` and `reject` on a plan.**
   - **`approve <task> [--note TEXT]`:**
     - ends the gate and records `plan.approval_decided{version, decision: approved}`;
     - the task runs again;
     - the note, bounded by LIM-D4 as a rejection's note is, reaches the model as `<<user kind="approval_note">>` beside the plan.
   - **`approve --note` on an action's approval** carries the note the same way. Say what the model's instructions need to say of it.
   - **`reject <task> --note TEXT`:**
     - records `plan.approval_decided{version, decision: rejected}`;
     - the plan's step ends `approval_rejected`, with the note as `rejection_note`, as for an action;
     - the task runs and stays gated: the next `plan` waits again.
   - **`reject --halt`** halts `approval_rejected`. **`task cancel`** and **the expiry** (LIM-A1, halting `approval_expired`) act as for an approval.
   - **A writer that meets the wait as something other than these** (for example `answer`) is refused with the reason. Say which commands you found.
4. **Showing the plan.**
   - **`task show`** gives the plan in force of every task (its version, its steps, when it was made), and for a plan-first task whether it is approved and which version.
   - **`--json`:** `plan` and `plan_first` / `plan_approved`. Propose the shape.
   - **`run`:** prints the plan when the task waits for it, as an approval card with every step shown and bounded as the card is (LIM-D3), and prompts approve, reject with a note, or cancel. On a non-interactive terminal it says how to decide and exits as for a pending approval.
5. **Metrics:** `rejections_by_code` counts `plan_required` under `state` as it counts the other reasons. The waits for a plan are counted where the waits for an approval are. Propose.
6. **The book:**
   - `how-a-task-runs.md`: a plan-first task, step by step;
   - `cli.md`: the flags, `approve --note`, the plan in `task show`;
   - a `<!-- tested -->` example of `run --plan-first` with the fake backend (1.0's condition 4).

7. **Also, found on `main`'s CI (a defect of 0.12's registry, not of this handoff).**
   - **What failed:** `registry::tests::req_sched_006_a_registry_that_cannot_be_used_fails_with_a_reason` failed once on `macos-latest` (run `45091069608`, `daede80`). It said `the registry did not answer within 100 ms` where `another runtime holds the registry's lock` was expected.
   - **The cause:**
     - `lock_mutex` waits for the lock until a deadline of the operation's bound;
     - `bounded` gives the whole operation up at that same bound;
     - so the two race. On a slow machine the outer one wins, and a user whose registry is held is told the vaguer reason.
   - **The fix:** make the lock's own deadline end before the operation's bound (a margin you justify, within LIM-O11), so that a held lock is always reported as such, and `did not answer` means only a filesystem that did not answer.
   - **The test** decides by state, not by a race: the lock held for longer than the bound, and the reason asserted. Show it under a slowed scheduler (for example a busy loop on every core), before and after.

## Tests (by state, §3c)

- **The gate:**
  - an `act` first is rejected `plan_required`, three times halts `budget_rejections`;
  - `complete` first is rejected;
  - `ask_user` first is asked and answered;
  - `give_up` first fails the task.
- **The first plan:**
  - the task waits `plan`, and nothing acted (no action record, the workspace unchanged);
  - `approve`, then an `act` runs;
  - `approve --note` puts the note in the next request as `approval_note`;
  - a second plan after approval does not wait.
- **`reject --note`:**
  - the model's next request holds the note as `rejection_note` and `approval_rejected`;
  - an `act` is still `plan_required`;
  - the next plan waits again.
- **`reject --halt`, `task cancel`, and the expiry,** each from the plan wait.
- **A restart while waiting for the plan,** and a recovery: the gate holds, and the approved version holds after a restart.
- **A task without `--plan-first`:** behaviour unchanged; a mutation-free check that no `plan_first` key appears in its record.
- **`task show`** of a plan-first task before and after approval, in text and JSON. `task show` of an ordinary task shows its plan.
- **`run --plan-first`:** the prompt, approved and rejected, with the mock or fake backend.

## Mutations

Each of these must be killed:
- `act` allowed before the plan's approval;
- `ask_user` refused before the plan;
- the first plan not waiting;
- a later plan waiting again;
- `reject --note` ending the gate;
- the note dropped from the next request;
- the gate lost at a restart;
- the registry's lock deadline equal to the operation's bound again (item 7).

## Book and CHANGELOG

- **The book:** item 6.
- **CHANGELOG:**
  - **Added:** `--plan-first`; `approve --note`; the plan in `task show`.
  - **Changed:** new words for readers: the wait kind `plan`, the reason `plan_required`, the two events. Say that 0.12.0's `task show` fails on such a task.

## Return

`.git-exclude/review-requests/015b-plan-first/README.md`, with:
- the PRs and their CI runs;
- the mutations;
- the proposed JSON shape and metric;
- the commands that meet a plan wait;
- the design questions.
