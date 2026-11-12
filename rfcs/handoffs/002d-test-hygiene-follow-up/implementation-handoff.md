# Handoff 002d — Test hygiene, follow-up (release 0.10)

**RFC.** [RFC 002](../../done/002-decision-schema-and-validation-order.md) (as handoff 002b) · **Specification.** REQ-TEST-001, REQ-SEC-004, REQ-SEC-013, REQ-SBX-011, REQ-PERS-013, REQ-PLAT-003, REQ-PLAT-004, REQ-SCHED-002, REQ-SCHED-005; `repository-guards.md` §3, §3a, §3c · **From.** Handoff 002b's five requests and their review (2026-11-12).

Two product questions first, then the test-side items.

1. **The fuel of a run ended by its deadline is recorded as 0 (REQ-SBX-011).** Slice 2 found it, with a probe:
   - `(loop br 0)` under a 100 ms deadline gives `fuel: Some(0)`;
   - a counting loop that then traps gives the true count.

   Find the cause: how wasmtime accounts fuel when an epoch interruption ends the run. Check `cancelled` and `output_limit` too, which also interrupt.
   - Then record the fuel consumed.
   - If the engine cannot give it after an interruption, record it as unknown (`null`), never as 0, and say so where the execution's fields are documented.
   - Extend slice 2's test to the interrupted runs.

   Mutation: the figure forced to 0.
2. **`events --follow` can end before the task's runtime holds the store (REQ-PERS-013).** `req_pers_013_follow_reads_the_events_of_a_task_another_process_runs` failed twice under Rosetta, in 010b's and in another PR's run: "the follower ended early" after the first events.
   - Find out whether a user who starts `events --follow` just after `run` in another terminal can see it end at once.
   - If so, it is a product defect. A follower of a task that is not terminal and is not held waits for a runtime, within a bound that is a LIM id (propose the value with its reason; the architect registers it), and says why when it ends.
   - Fix it, and make the test decide by state.
3. **Skips that CI cannot count (REQ-PLAT-003).**
   - The Linux tests that need the `full` tier and return after an `eprintln!` use `common::skip` instead, as `m6a_host_tools.rs` already does in one place.
   - Then CI's `skipped tests on <os>: 0` covers them, and the Linux full tier is asserted.
4. **The invariants line fails the `rules` job** (REQ-SEC-004) when an invariant is named by no test. It is 0 of 14 today. The requirements list stays report-only (§3).
5. **The corpus's symbolic-link scenario** (handoff 002b item 2, question 5): its step fields are required, with explicit empty lists, like every other field of the corpus, so that deleting one fails.
6. **The book's tested pages** (handoff 010b, question 2): `book_examples.rs` reads the `<!-- tested -->` blocks of every page in `SUMMARY.md`, instead of a list of names, and fails if a page that has such blocks is not read.
7. **A bounded wait:** `m6a_cancel_kills_the_whole_tree` waits for the runtime with a bound, as `run_bounded` does (handoff 002b item 3, question 4).
8. **A dead constant:** `LIM_P3_RETENTION_DAYS` and its range are used nowhere and read like a timer. Remove them; LIM-P3's row already says there is no timer.
9. **The untested requirements left** (§3):
   - **REQ-SCHED-002** (one step and one execution at a time per task) and **REQ-SCHED-005** (a terminal task holds no worker, backend, lock or scheduler resource), as slice 4;
   - **REQ-PLAT-004** on Windows:
     - the workspace's symbolic links recreated on `windows-latest`. If the runner cannot create them, skip with `common::skip`, so the skip is counted;
     - the case-sensitivity probe of the real filesystem.

     REQ-PLAT-004 stays listed until each of its clauses is tested on each platform.

The rule of §3c applies to every test written here.

Return: one request per item group, `.git-exclude/review-requests/002d-<item>/README.md` (1, 2, and 3 to 9 together, or in slices).

## After review (2026-11-12): 002d-3

002d-1 and 002d-2 are landed. Four items follow.

10. **A terminal task leaves nothing behind in the runtime** (REQ-SCHED-005; 002d-2 question 4). This is in scope, because a daemon (RFC 014) keeps the runtime alive.
    - At a task's terminal transition, the three per-task maps of `Runtime` drop its entry: `cancellations`, `last_requests` and `unpinned`.
    - Extend the SCHED-005 test with the three, through the public API or a test-support accessor. Do not make the maps public.
    - Mutation: one of the three not cleared.
11. **The Linux full tier is asserted** (002d-2 question 3).
    - The Linux CI jobs fail when `skips.log` holds a skip whose reason is the tier. The demonstration's rustup skip and the opt-in tests' (`MIMLYS_RYPSOK_BIN`, `MIMLYS_M4_*`) are allowed by name.
    - The step's rule is written in `repository-guards.md` §1.
    - The opt-in tests stay printed skips (question 1).
12. **The Landlock unit test is counted** (question 2): a skip helper that `mimlys-exec`'s unit tests can reach, writing to the same `skips.log`.
13. **A script in `read_only` mode reads** (002c question 7). RFC 005 §5 says `mode = read_only` derives `workspace_read`.
    - Today the policy derives every `run_script` and `run_wasm` call from the definition's `workspace_write`, so a read-only script:
      - takes the write snapshots;
      - is judged for approval as a write;
      - is never a read for the profile.
    - The sandbox already opens the workspace read-only.
    - Derive the effect from `mode` at validation, as RFC 005 says. Correct the comment in `tools.rs`. Say in the request what changes for snapshots, approval and the profiles.
    - Test through the loop: a read-only script takes no snapshot, and a read-write one does.
    - Mutation: the mode ignored.

The second finding of 002d-2 (two overlapping `run_step` calls for one task in one process) is a note for RFC 014's handoff: the daemon's design says who serialises a task's steps. Return: `.git-exclude/review-requests/002d-3/README.md`.
