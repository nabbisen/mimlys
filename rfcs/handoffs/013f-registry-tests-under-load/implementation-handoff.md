# Handoff 013f — The registry's tests under load (release 0.13, before the candidate)

**Status (2026-11-27): done.** Landed at `3659f66` (`.git-exclude/reviewed/handoff-013f/README.md`). Both failures were faults of the tests; the registry's message now says only what the code knows.

**RFC.** [RFC 013](../../done/013-operation-on-one-machine.md), part two (the admission registry; external design §19.3; LIM-O11). **Found by.** The reviews of 015c-7, the dev team's and mine (`.git-exclude/reviewed/handoff-015c-7/README.md`).

**Purpose.** Two tests of `crates/mimlys/src/registry/tests.rs` fail on a loaded machine. A test that fails under load either hides a defect of the product or is itself one, and the release candidate must not carry either.

**Embargo:** none.

## What was seen

1. **`req_sched_006_a_wait_ends_when_the_holder_returns_its_slot`** failed twice in the dev team's library runs of 015c-7, with the machine at 83 to 95 C under other projects' load. It passed alone, 3 of 3, and in CI. No output was kept.
2. **`req_sched_006_a_registry_that_cannot_be_used_fails_with_a_reason`** failed once in 20 of my full runs of the `mimlys` library tests, four running at once on `748f373`. The output is `.git-exclude/reviewed/handoff-015c-7/registry-flake-full-lib-run.txt`:

   > panicked at `crates/mimlys/src/registry/tests.rs:87:10` (`granted`): called `Result::unwrap()` on an `Err` value: "another runtime holds the registry's lock"

   The same test alone, 8 at once, passed 96 of 96. So it takes the whole suite's load.

## What I know of the cause, and what I do not

- **The message comes only from `lock_mutex`,** on `WouldBlock` past the lock deadline (`registry.rs:558`). So `registry.lock` was really held by an open file description at that moment. It was not a timer racing a free lock.
- **`Drop` for `Handle` is synchronous** and does not touch `registry.lock` (`registry.rs:279`), so a returned slot is not the holder.
- **`bounded_from` answers its caller on `op`'s timeout while its thread goes on** (`registry.rs:627`). An operation's thread can therefore act after its caller was told it failed. Whether that is what holds the lock here, I do not know.
- **The test uses a 20 ms lock wait** for two `granted` calls (lines 302 and 325), and that test's own lock and reap come between them.

## Items

1. **Find what holds the lock** in failure 2, and say whether failure 1 has the same cause.
   - **Reproduce first,** under the same load: the whole `mimlys` library tests, several at once, at the owner's limit for the machine (nice 19, the cores you are given, the thermal guard). Keep each failure's output.
   - **Then:**
     - **If the product holds it** (an operation's thread acting after its caller had an answer, or a second open of `registry.lock` within one operation): that is a defect. An operation reported failed must not then succeed, nor hold the lock against the next. Fix the product, and test it by state.
     - **If only the test holds it:** make the test decide by state (wait for what it depends on to be over), not by the 20 ms.
2. **The message tells the truth.** "Another runtime holds the registry's lock" is untrue when the holder is this runtime's own operation. Make the words say what is known, for example "the registry's lock is held by another operation (of this runtime or another)". Make it only as precise as the code can know.
3. **The other registry tests:** look for the same shape (a short lock wait, or an answer taken before a thread is done), and fix each one you find the same way.

## Tests and evidence

- **Before and after:** the failing count of the reproduction, before and after the fix (as 015b-2's lock fix was measured: 64 of 500 before, 0 of 500 after).
- **A test by state** for the cause you find.

## Housekeeping (the owner's rule of 2026-11-26)

**A PR is closed, and its branch deleted, as soon as its role is finished,** by the architect or by you. Never leave them for the owner. On GitHub, every finished PR is now closed and every finished branch deleted: only `main` remains. The heads are archived in `.git-exclude/archive/branches-before-cleanup-2026-11-26.bundle`.

**Your local copies are yours to clear:** 72 worktrees under `.claude/worktrees/` (1.6 TB with their targets) and 285 local branches.
- **Keep** a worktree you are working in, and one with changes that are not committed. Commit those first, or say why they stay.
- **Remove** every other worktree (`git worktree remove`), and delete the local branches whose work is on `main` or whose role is over.
- **From now on,** remove a worktree and its branch when its PR is closed. Close your own throwaway PRs and delete their branches when their review is over.
- **Report** what you removed and what you kept, with the reason, in the request.

## Return

- **A PR on `main`,** with its CI.
- **The request:** `.git-exclude/review-requests/013f-registry-tests-under-load/README.md`, with:
  - the cause, with its evidence;
  - the reproduction's counts before and after;
  - anything you could not settle;
  - the housekeeping: what was removed, and what was kept and why.
