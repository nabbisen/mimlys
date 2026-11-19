# Handoff 013d-0 — The operation theme's measurements (release 0.12, phase 1)

**RFC.** [RFC 013](../../accepted/013-operation-on-one-machine.md), part two, with Amendment 1. **Found by.** The scope of 0.12 (`ROADMAP.md`, "Release 0.12", phase 1): RFC 013 says its defaults are measured first, "a report in the book, like the M2 and M4 reports".

**Purpose.** This handoff measures and writes the report; it builds nothing for the product. Handoff 013d (decisions 5 to 8) is written from the report, and the defaults are registered at its review. Run it after 004d-5, or beside it if your time allows: they touch different code.

**Embargo:** none.

## What to measure

Measure on throwaway branches with a workflow of their own, on the CI runners (`ubuntu-latest`, `windows-latest`, `macos-latest`), at most eight runs. Measure on this machine too where you can, and say which numbers come from where. Each number is a median of at least three runs, with its spread.

1. **LIM-X5: the user's executions at once (candidate 2).**
   - Run the M6a demonstration (`cargo test --offline` of the native project, as a host tool) in 1, 2, 3 and 4 runtimes of one user at once, each with its own data directory.
   - Record, for each count:
     - each run's wall time;
     - the machine's peak memory;
     - the CPU time;
     - whether any run failed (out of memory, a deadline).
   - Do the same with a script workload (`run_wasm` of a loop) for the sandbox side.
   - Propose the default and the rule that gives it, for example "the largest count whose median run is within 1.5 times the single run and whose peak memory is within half the runner's".
2. **LIM-H7: the CPU share of a host tool's tree (candidate half the cores, at least one).**
   - **Linux:** run the M6a demonstration under the `tool` cgroup's `cpu.max` at one core, half the cores and all of them, and record its duration. First say whether the runner delegates a cgroup with `cpu` to the user at all. If it does not, the CPU part is `unavailable` there, which is itself the finding.
   - **Windows:** the same through `JOB_OBJECT_CPU_RATE_CONTROL` on the worker's Job Object. It is the worker's tree, since 011o made the worker per task.
   - **macOS:** a lowered priority only. Measure that it is not a bound (an idle machine still gives the tree every core), so the report can say `best_effort (priority)` with numbers.
   - Propose the default.
3. **The cost of a task switch with a worker per task (macOS, Windows).** Measure, from the request to the result:
   - an execution of task B right after one of task A (a worker replaced);
   - against a second execution of the same task (a worker kept);
   - for a script and a host tool.

   State it against LIM-X4. This cost is paid by every interleaving of tasks, and admission's design should know it.
4. **The engine gate (LIM-O12).** No measurement is needed: the default 1 stands for one consumer GPU. Say only whether anything you saw argues otherwise.

## Rules

- **Nothing here goes into product code.** The throwaway branches carry the measuring programs and workflows; their PRs, if any, say "not for merge". You may add a small measuring example under `examples/` on the throwaway branch only.
- **Use the real binaries** (the release build), the real confinement and the real host-tool path. Say what the runner had: cores, memory, the kernel or OS build, the cgroup delegation.

## Return

- **The report, as one PR** on `main`: `docs/src/maintainers/m7b-report.md`, in the M2 and M4 reports' form, with:
  - what was measured;
  - the numbers, each with its source;
  - the proposed defaults and their rules;
  - what could not be measured, and why.

  It changes no limit; the limits are mine to register.
- **The request:** `.git-exclude/review-requests/013d-0-operation-measurements/README.md`, with:
  - the PR and its CI run;
  - the runs used;
  - anything in the measurements that contradicts RFC 013 or its Amendment 1.
