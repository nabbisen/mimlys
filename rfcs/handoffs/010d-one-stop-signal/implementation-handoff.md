# Handoff 010d — One stop signal must end `mimlys run` (release 0.10)

**RFC.** [RFC 010](../../done/010-cli-and-operator-interface.md) (follow mode and its signals) · **Specification.** REQ-CLI-005 (a stop signal pauses running tasks at their next boundary, a second one cancels them; the process exits within LIM-O1 with the worker terminated), LIM-O1; `repository-guards.md` §3c · **Found.** `main`'s CI run `43338280616` at `04dc422`, attempt 1, job `check (ubuntu-24.04-arm)`.

**What was seen.**
- `req_cli_005_one_stop_signal_pauses_at_the_next_boundary` (`crates/mimlys/tests/m6a_host_tools.rs`) failed with "`mimlys run` did not exit after the stop signal":
  - the run was still alive 120 s after one SIGINT, which was sent while the host tool's 2.7 s step ran;
  - the re-run of the job passed, and the same commit passed on the same runner type in the integration run `43335880013`.
- So the run intermittently does not end after one signal. For a user that is a Ctrl-C that pauses nothing and leaves the terminal hanging. Until shown otherwise, treat it as a product defect, not a test flake.
- **What is ruled out by reading:**
  - The log level is the default `info`, so the test's unread pipes cannot have filled.
  - `pause_task` only records the request; the pause is taken at the boundary inside `run_step`.
- **The evidence was lost:** on its timeout the test kills the run and panics without printing the run's standard output and error.

1. **Keep the evidence first.**
   - The test writes the run's two streams to files, as handoff 002d item 7 did for `m6a_cancel_kills_the_whole_tree`, and prints them when it gives up. Before it kills the run, it also prints the process tree and the task's events.
   - The bound stays far longer than the step (§3c).
2. **Find the cause.** Read `crates/mimlys/src/follow.rs` with a stop signal at every point of a step:
   - before the step's action starts, during it, and at its end;
   - against LIM-O1's watchdog (`grace` = LIM-O1 less one second, which cancels the execution if the pause has not been applied);
   - against `drive`'s loop. Two things to check there:
     - After `Cancelled` with the task still `running`, the loop pauses the task and then calls `run_step` once more. Say what that call is for and whether it can wait.
     - The two `tokio::spawn`ed signal tasks.

   Use the loaded aarch64 runner, or a stress loop locally (the test 200 times, the machine loaded), to make the failure show.
3. **Fix it** so that one signal always ends the run within LIM-O1 after the step's boundary (the pause), or cancels it at LIM-O1. A test that decides by state shows each of those signal points: the task's recorded state, the process gone, nothing left of the worker's tree.
4. **If the cause is the test and not the product,** show it with the captured streams and fix the test. Say why a user's Ctrl-C cannot meet the same case.

**Mutations:**
- the pause never applied (the watchdog's cancellation must still end the run);
- the watchdog removed (the test of a step longer than the grace fails).

Return: `.git-exclude/review-requests/010d-one-stop-signal/README.md`, with the captured streams of a failing run if you could produce one.
