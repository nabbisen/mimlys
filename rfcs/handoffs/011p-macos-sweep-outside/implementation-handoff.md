# Handoff 011p — macOS: the tree is swept from outside the sandbox (release 0.11, phase 3)

**RFC.** [RFC 011](../../done/011-confinement-backends.md) (Amendment 2, implementation decisions, point 6). **Found by.** The audit before 0.10 (A-21), and the scope of 0.11 (`ROADMAP.md`, "Release 0.11", phase 3).

**Specification** (amended before this handoff): external design §13.3, the rule "The tree ends when the worker is killed, on macOS too".

**Owner's decision (2026-11-16):** a spike first, on the macOS job. If the private `sandbox_check` membership test works reliably, it is used, with a CI test that fails if a macOS update breaks it. Otherwise the residual stays, stated plainly as accepted. If phase 3 runs long, it becomes 0.12. Your spike decides that, so say early if it will.

**Embargo:** none. The residual is public in the 0.10 release notes and the threat register.

## Today

These are as found on `main`; check them on today's `main`.

- **The sweeper is a member of the worker's sandbox** (`confine/sweeper.rs`, spawned by `macos.rs:742`). It sweeps when the worker's pipe reaches end of file. A tool may signal the members of its own sandbox, so it can kill the sweeper and then the worker. Its processes then outlive both, still confined, and still writing the workspace. That is A-21, and the tool can cause it deterministically.
- **The runtime is outside the sandbox** and cannot use the worker's way of finding members (`members_of`: the pids the profile lets a member inspect).
- **`worker.json`** (`worker.rs:44`, `reap_orphans` at `worker.rs:56`) records a worker for the next start, on Linux.

## Stage 1: spike M1, on a throwaway branch measured by the macOS jobs

Measure on `macos-latest` and on the Rosetta job (not locally). Resolve `sandbox_check` at run time (`dlsym` from `libsystem_sandbox.dylib`), never by linking, so that a missing symbol is a stated answer, not a crash.

1. **A rule unique to one worker.** Give two workers' profiles each a marker that nothing else names, for example `(allow file-read-metadata (literal "<data>/tmp/mimlys-marker-<nonce>"))` with a fresh nonce per worker. Then, from an unsandboxed process, ask `sandbox_check(pid, "file-read-metadata", SANDBOX_FILTER_PATH, marker)` of:
   - worker A and a tool of A: allowed for A's marker, denied for B's;
   - worker B: the mirror;
   - an unsandboxed process: tell it apart (expected: `sandbox_check(pid, NULL, SANDBOX_FILTER_NONE)` says it is not sandboxed);
   - another application's sandboxed process (for example one the system runs): denied for both markers.
2. **What membership survives.** A tool's child, a re-parented orphan (its parent killed), and a process that called `setsid` are still found, and a process of A that killed A's sweeper and A's worker is still found.
3. **The cost.** Time one round (list every pid, check each) on the runner.
4. **A reused pid.** Outside the sandbox the kernel no longer refuses a signal to an outsider, so a pid freed and reused between the check and the signal could hit another process. Measure how often a pid is reused within a round on the runner. Propose the guard: check, signal, then check again; or a start time read with the check (`proc_pidinfo`), compared before the signal. Name the one you measured.
5. **The verification at start.** A check of a known member (a child of the worker made for it) and a known outsider (the runtime itself) gives the expected answers in under LIM-O8.

**Stop and report after the spike, instead of going on to stage 2, if:**
- `sandbox_check` is missing or does not tell the two workers apart (item 1);
- a member of item 2 is not found;
- one round takes more than LIM-O2;
- or the reused-pid risk of item 4 has no guard you can measure.

In that case the residual is restated as accepted, and that is the decision, not a failure. Otherwise go on, and put the spike's numbers in the request.

## Stage 2: the items

1. **The marker.** Each worker's profile names a marker of its own, made by the runtime and recorded with the worker (in `worker.json` on macOS too).
2. **The sweep from outside.** Whenever a worker ends (exit, crash, kill, or a replacement for another task), the runtime sweeps its members: the kill loop of `sweep_with`, with the outside membership test and the guard of item 4, within LIM-O2. A member that survives is `tree_kill_incomplete`, logged at `warn`, as today.
3. **After a crash.** The next start sweeps the members of a worker that `worker.json` records, before anything else, as `reap_orphans` does on Linux.
4. **The verification.** Each start checks the test on a known member and a known outsider before relying on it.
   - **If it fails:** `status` says in one line that the sweep outside the sandbox is unavailable, with the reason, and the in-sandbox sweeper alone runs, as in 0.10.
   - **Neither case changes the tier.** K's state is `seatbelt_membership` either way; the line is in addition.
5. **The in-sandbox sweeper stays,** as the first layer for a runtime that dies before its worker.

## Tests (by state, §3c)

- **On the macOS job, ROADMAP's "done when":**
  - a host tool tries to signal the runtime, and is refused;
  - a host tool kills the sweeper and then the worker; its tree (a child and a re-parented orphan) is gone within LIM-O2 afterwards, swept by the runtime; the workspace receives no write after the sweep.
- **The breakage test:** a CI test that fails if `sandbox_check` stops telling a member from an outsider. This is the owner's condition.
- **After a crash:** a recorded worker whose runtime was killed has its members swept at the next start.
- **The verification failing** (a seam that makes the known member's answer wrong): `status` says so, the in-sandbox sweeper still sweeps, and nothing is killed by the outside loop.
- **No outsider is touched:** an unsandboxed process of the user, and a process of another worker's sandbox, survive a sweep.
- **On Linux and Windows:** nothing changes.

## Mutations

Each of these must be killed:
- the outside sweep not run when a worker ends;
- the marker shared by two workers;
- the reused-pid guard removed (a seam that reuses a pid between the check and the signal);
- the verification skipped;
- the sweep at the next start removed.

## Book and CHANGELOG

- **The book:** the macOS confinement page states what happens when a tool kills its worker.
- **CHANGELOG:** Security, A-21.
- The threat register's residual row is closed at review, not by you, or restated as accepted if the spike stops.

## Return

`.git-exclude/review-requests/011p-macos-sweep-outside/README.md`, with:
- the spike's numbers;
- the PRs and their CI runs;
- the mutations;
- the design questions.
