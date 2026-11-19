# M7b report: the operation theme's measurements

Milestone M7b (RFC 013, part two, with its Amendment 1; handoff 013d-0) names
three numbers that its defaults wait for: how many executions of one user may
run at once (LIM-X5), how much of the machine one host tool's tree may take
(LIM-H7), and what a task switch costs now that a worker serves one task on
macOS and Windows (Amendment 1, point 2). This report records those
measurements, made on 2026-11-19, and the defaults and rules they point to.
It builds nothing for the product and registers no limit: the register is
the architect's, and handoff 013d is written from this page.

The measuring program, its harness and its workflow lived on throwaway
branches (`013d0-measure-1`) and were never part of `main`; the raw lines of
every run are in the review request of handoff 013d-0. Nothing in the
product was changed to measure, with one exception that is stated where it
applies (§4.2: a hard CPU rate set on the Windows worker's Job Object by a
patch that exists on the throwaway branch only).

## 1. What was measured

| Item | Measure | Where |
|---|---|---|
| LIM-X5 (§3) | 1, 2, 3 and 4 runtimes of one user at once, each with its own data directory, each running the M6a demonstration, a heavier build, two CPU-bound host tools, a `run_wasm` loop and a JavaScript loop; wall time of each run, the machine's peak memory, CPU time, failures | the three CI runners; this Linux machine |
| LIM-H7 (§4) | the same tool tree under a CPU bound of one core, half the cores and all of them (Linux: a cgroup `cpu.max`; Windows: `JOB_OBJECT_CPU_RATE_CONTROL`), and under a lowered priority (macOS) | the three CI runners; this Linux machine |
| The same, with the bound and with admission together (§3.3) | 1 to 4 runtimes at once, each tree bounded to half the cores | Linux and Windows runners; this Linux machine |
| The task switch (§5) | an execution of task B right after one of task A (worker replaced) against a second execution of task A (worker kept), for a script and a host tool, with and without the JavaScript interpreter | macOS and Windows runners (per-task workers); Linux with per-task workers forced, and as it is |
| LIM-O12 (§6) | nothing is measured | |

**How.** Everything goes through the release binaries (`mimlys` and
`mimlys-worker`, `cargo build --release`) and the real confinement. Each
runtime is one `mimlys run` with the fake backend replaying a script of
decisions, so no model is involved; the host tools are declared in the
configuration and run through `run_host_tool`, with the clauses of the
platform in force (the tier is `full` on every runner). A group of *n*
runtimes starts *n* such processes in the same instant, each with its own
data directory, project, script and configuration; a run is the wall time of
its process; the machine's peak memory is the largest of `total - available`
sampled every 50 ms, minus the same value just before the group started; the
CPU time is the children's `RUSAGE_CHILDREN` (Linux, macOS) or the machine's
busy time (Windows). A failure is a non-zero exit or a task that did not
complete. Each number below is the median of the runs named in its table,
with the smallest and largest in brackets; a ratio ("x1.59") is the median
against the median of the single runtime in the same table.

**The workloads.**

| Name | What it is | Where |
|---|---|---|
| `demo` | the M6a demonstration exactly: `cargo test --offline` of a workspace with a path dependency as a host tool; the model runs it (fails), fixes the line, runs it (passes), completes, and the completion check runs it again | Linux, macOS |
| `wide` | the same script over a project of eight more leaf crates (about 250 generic functions each), so that cargo uses every core for several seconds: a *parallel build* | Linux, macOS |
| `burn_2`, `burn_n` | a host tool (a small program of the measuring branch) that does a fixed amount of integer work on 2 threads (`burn_2`, like a test run) or on as many threads as the machine has cores (`burn_n`, like a parallel build) | every platform |
| `wasm` | `run_wasm` of a module that loops 1.5 thousand million times | every platform |
| `js` | `run_script` of a JavaScript loop of 20 million iterations in the pinned QuickJS build | every platform |

`cargo` does not run in the Windows container (M6b report §4: an AppContainer
cannot open the `NUL` device). It still did not, on `windows-latest` at
Windows Server 2025 build 26100.33674: the demonstration halted at its first
action with `failed to open NUL device for child stdin: Access is denied`. So
**the M6a demonstration could not be measured on Windows**; `burn_2` and
`burn_n` stand in for it there, through the same host-tool path.

## 2. Where the numbers come from

| Label | Source | Machine |
|---|---|---|
| **L-ci** | CI run `44521312268`, job `measure (ubuntu-latest)` | Ubuntu 26.04.1, kernel 7.0.0-1020-azure; 4 vCPU (AMD EPYC 9V74, 2 cores with two threads); 15 983 MiB; 3 GiB swap; the job runs in `/system.slice/hosted-compute-agent.service` as a non-root user |
| **W-ci** | the same run, job `measure (windows-latest)` | Windows Server 2025 (10.0.26100.33674); 4 vCPU (AMD, family 25 model 1); 16 379 MiB |
| **M-ci** | the same run, job `measure (macos-latest)` | macOS 26.7.2 (25G332), Darwin 25.6.0 (xnu-12377.161), a virtual Apple M1 with 3 CPUs; 7 168 MiB; no swap |
| **L-4** | this Linux machine with the processes pinned to four physical cores (`taskset -c 0-3`) | kernel 7.3.5, AMD Ryzen 9 9950X; the memory shown is the whole machine's (61 349 MiB) |
| **L-32** | the same machine, all 32 logical cores | |

L-ci, W-ci and M-ci are one CI run, so one runner instance of each image; their
rounds (5 on Linux and Windows, 3 on macOS for §3; 3 for §4; 3 invocations of
4 rounds for §5) are repeated on that instance. A pilot of the same harness
(run `44517226777`, one round, other instances: the Linux one had an AMD EPYC
7763) gave the same shapes; §7 gives the differences. L-4 and L-32 are for
shape: the memory they show is another process's as well as ours.

## 3. LIM-X5: executions of one user at once

### 3.1 What happened

No run failed: no out-of-memory kill (Linux: the kernel log was read before
and after and holds none), no deadline, no tool error, in the 490 runs of the
whole measurement on L-ci (of which 300 are in the tables of this section),
290 on W-ci (200) and 262 on M-ci (180), and none on L-4 and L-32.

Wall time of one runtime when 1, 2, 3 and 4 run at once (median, smallest and
largest in brackets, ratio to the single runtime). Rounds: 5 (L-ci, W-ci), 3
(M-ci); each round of *n* runtimes gives *n* runs.

**L-ci** (4 vCPU)

| Workload | 1 | 2 | 3 | 4 |
|---|---|---|---|---|
| `demo` | 1.23 s (1.21-1.28) | 1.96 s (1.94-2.08) x1.59 | 2.77 s (2.59-2.87) x2.25 | 3.60 s (3.43-3.80) x2.92 |
| `wide` | 10.23 s (9.96-11.27) | 19.28 s (18.81-20.57) x1.88 | 29.65 s (28.09-31.29) x2.90 | 39.68 s (38.16-40.90) x3.88 |
| `burn_2` | 3.98 s (3.97-4.00) | 4.62 s (4.55-4.71) x1.16 | 6.92 s (6.57-7.11) x1.74 | 9.19 s (8.62-9.46) x2.31 |
| `burn_n` | 4.02 s (3.99-4.08) | 8.04 s (7.91-8.16) x2.00 | 11.88 s (11.83-12.19) x2.95 | 15.78 s (15.54-15.95) x3.92 |
| `wasm` | 1.92 s (1.91-1.96) | 2.60 s (2.52-2.65) x1.36 | 3.87 s (3.27-3.93) x2.02 | 5.18 s (5.04-5.32) x2.70 |
| `js` | 1.83 s (1.79-1.86) | 2.51 s (2.45-2.61) x1.37 | 3.73 s (3.15-3.96) x2.04 | 4.97 s (4.80-5.15) x2.72 |

**W-ci** (4 vCPU; no `demo` or `wide`: §1)

| Workload | 1 | 2 | 3 | 4 |
|---|---|---|---|---|
| `burn_2` | 4.98 s (4.96-5.14) | 6.26 s (6.17-6.33) x1.26 | 9.20 s (9.02-9.30) x1.85 | 12.29 s (12.14-12.37) x2.47 |
| `burn_n` | 5.13 s (5.10-5.14) | 10.15 s (9.90-10.19) x1.98 | 14.92 s (14.73-15.04) x2.91 | 19.90 s (19.62-22.54) x3.88 |
| `wasm` | 2.68 s (2.66-2.71) | 3.97 s (3.89-4.15) x1.48 | 5.70 s (5.35-5.97) x2.13 | 7.95 s (7.71-8.09) x2.97 |
| `js` | 2.92 s (2.87-3.03) | 4.25 s (4.16-4.35) x1.46 | 6.08 s (5.74-6.21) x2.08 | 8.09 s (7.84-8.75) x2.77 |

**M-ci** (3 vCPU)

| Workload | 1 | 2 | 3 | 4 |
|---|---|---|---|---|
| `demo` | 1.93 s (1.67-2.23) | 2.37 s (2.21-2.60) x1.23 | 3.28 s (3.01-3.65) x1.70 | 4.16 s (3.63-4.46) x2.16 |
| `wide` | 10.48 s (10.19-10.55) | 17.81 s (16.75-19.25) x1.70 | 30.53 s (25.00-32.47) x2.91 | 37.34 s (36.89-38.99) x3.56 |
| `burn_2` | 5.21 s (5.02-5.26) | 7.04 s (6.96-7.10) x1.35 | 10.18 s (10.01-10.57) x1.95 | 13.29 s (13.07-13.57) x2.55 |
| `burn_n` | 5.01 s (4.91-5.44) | 9.75 s (9.47-10.24) x1.95 | 14.35 s (14.16-15.14) x2.86 | 19.71 s (18.62-19.94) x3.94 |
| `wasm` | 2.21 s (2.12-2.25) | 2.78 s (2.58-2.80) x1.26 | 3.31 s (3.12-3.58) x1.50 | 4.32 s (4.09-4.38) x1.95 |
| `js` | 1.74 s (1.69-1.90) | 2.21 s (2.13-2.28) x1.27 | 2.75 s (2.63-2.92) x1.59 | 3.72 s (3.24-3.83) x2.15 |

**L-4** (four physical cores of a fast machine) gave the same shape (`demo`
x1.38, x1.93, x2.40; `wide` x1.81, x2.63, x3.77; `burn_2` x1.09, x1.63, x2.09;
`burn_n` x1.98, x2.93, x3.91; `wasm` and `js` x1.29 to x1.31, x1.58 to
x1.64, x1.93 to x1.97). **L-32** (32 cores) shows what cores buy: `burn_2`
x1.02, x1.06, x1.09; `wasm` and `js` x1.06 to x1.08, x1.13 to x1.17, x1.24
to x1.27; `demo` x1.18, x1.33, x1.52; `wide` x1.24, x1.62, x2.05; and
`burn_n`, which wants every core, x2.03, x2.91, x3.83 as everywhere.

**Memory and CPU time.** The machine's peak memory above its baseline, the
largest value seen in any group of 1 to 4 runtimes, and CPU time:

| Source | Largest peak above baseline | As a share of memory | Workload | Group CPU time, 1 runtime to 4 (the build) |
|---|---|---|---|---|
| L-ci | 1 976 MiB | 12 % | `wide`, 4 runtimes | 30.4 s to 136.8 s |
| W-ci | 275 MiB | 2 % | `wasm`, 4 runtimes | (`burn_n`) 19.4 s to 77.8 s |
| M-ci | 1 076 MiB | 15 % | `wide`, 4 runtimes | 22.0 s to 99.0 s |
| L-4 | 3 497 MiB | 6 % | `wide`, 4 runtimes | 12.2 s to 58.0 s |
| L-32 | 5 397 MiB | 9 % | `wide`, 4 runtimes | 16.7 s to 105.4 s |

One runtime of the demonstration adds 30 to 59 MiB (median) to the machine;
one `wasm` or `js` execution 0 to 83 MiB (below the sampling's noise on
Linux); a parallel build 370 to 400 MiB.
Memory was never what limited anything; CPU was. The group's CPU time grows a
little faster than the count (a build: 30.4 s for one, 34.2 s per runtime
for four on L-ci), the cost of the same work sharing the cores.

### 3.2 Reading

- **A tool that wants every core is a zero-sum game**: two `burn_n` take x2.0
  each on every platform, three x2.9, four x3.9. So do the parallel builds
  (x1.7 to x1.9 for two). Nothing here is lost by admission; a second runtime
  only slows the first.
- **The demonstration and a script are partly parallel**: two of them cost
  x1.2 to x1.6 each on 3 to 4 cores (the demonstration x1.59 on L-ci, x1.23 on
  M-ci; a script x1.26 to x1.48); three cost x1.5 to x2.3; four x1.9 to x3.0.
- **The handoff's example rule** (the largest count whose median run is within
  1.5 times the single run, and whose peak memory is within half the
  machine's) gives, on the three runners: **1** for a build and for `burn_n`,
  **2** for the demonstration on M-ci (1 on L-ci, by 0.09), for `burn_2`,
  `wasm` and `js`, and 3 for `wasm` on M-ci (x1.50, at the line). Memory
  never decides. On L-32 the same rule gives 3 for the demonstration, 4 for
  `burn_2`, `wasm` and `js`, 2 for the build and 1 for `burn_n`.

### 3.3 With the CPU bound on: slots times share

If every tree is bounded to half the cores (§4: Linux by a unit's `CPUQuota`,
Windows by a Job Object's CPU rate), 1 to 4 runtimes at once, each bounded
(3 rounds, so 3 to 12 runs per cell):

**L-ci**

| Workload | 1 | 2 | 3 | 4 |
|---|---|---|---|---|
| `demo` | 1.85 s (1.82-2.38) | 2.11 s (2.03-2.53) x1.14 | 2.91 s (2.72-2.95) x1.57 | 3.81 s (3.67-3.96) x2.05 |
| `wide` | 17.38 s (17.18-17.70) | 19.95 s (19.65-20.56) x1.15 | 30.24 s (29.46-30.71) x1.74 | 41.57 s (40.12-44.05) x2.39 |
| `burn_2` | 4.56 s (4.54-4.57) | 4.68 s (4.63-4.72) x1.03 | 7.00 s (6.82-7.16) x1.54 | 9.33 s (9.12-9.46) x2.05 |
| `burn_n` | 7.83 s (7.82-7.86) | 8.02 s (7.97-8.20) x1.02 | 11.96 s (11.81-12.06) x1.53 | 15.99 s (15.80-16.16) x2.04 |

**W-ci**

| Workload | 1 | 2 | 3 | 4 |
|---|---|---|---|---|
| `burn_2` | 5.61 s (5.48-5.68) | 6.22 s (6.15-6.34) x1.11 | 9.18 s (8.97-9.33) x1.64 | 12.01 s (11.84-12.21) x2.14 |
| `burn_n` | 9.29 s (9.07-9.34) | 10.10 s (9.94-10.40) x1.09 | 14.89 s (14.79-15.01) x1.60 | 19.84 s (19.39-19.95) x2.14 |

Two bounded trees do not slow each other (x1.02 to x1.15): they fill the
machine and no more. A third costs x1.5 to x1.7 and a fourth x2.0 to x2.4
(L-4 gave x0.8 to x1.1, x1.2 to x1.7 and x1.6 to x2.3). Without the bound the
second slot costs x1.2 to x2.0 (§3.1). That is the rule: **the slots, each
at the share LIM-H7 gives a tree, should not ask for more than the machine
has**.

### 3.4 Proposed default and rule

**LIM-X5 = 2, by the rule `slots = floor(1 / LIM-H7 share)`**: with the
proposed share of one half (§4.4), two slots fill the machine and cost each
other nothing (x1.02 to x1.15 on both platforms where the bound exists);
every further slot costs the others x1.5 or more. The candidate of RFC 013
stands. Memory does not enter the rule: four runtimes of the heaviest
workload took 15 % of the smallest machine measured, and one runtime took at
most 60 MiB (a script) to 400 MiB (a build) above an idle machine.

Where the CPU part is not a bound (macOS; Linux without a delegated cgroup,
which is the state of every Linux machine measured here: §4.1), the second
slot costs a runtime x1.2 to x2.0 and the default of 2 then allows that
slowdown; the architect may prefer 1 there. I measured throughput, not the
machine's responsiveness to a user's own foreground work, which is what the
bound exists to protect; the measurement does not say how much a second tree
costs that work.

The register's range, 1 to 16, was measured to 4 only.

## 4. LIM-H7: the CPU share of a host tool's tree

### 4.1 Linux

**Does the runner delegate a cgroup with `cpu` to the user? No.** On L-ci the
job's own cgroup is `/system.slice/hosted-compute-agent.service`; its
`cgroup.controllers` list `cpuset cpu io memory pids`, but the user (uid 1001,
`runner`) cannot create a directory in it (`mkdir`: permission denied). The
product's own rung says so in `status`: clause M `rlimit_as
(cgroup_not_writable)`, K `subreaper`, tier `full`. The CPU part is
therefore `unavailable` on `ubuntu-latest`, as memory and process count are,
for the same reason. This is the same in every Linux state measured:

| State | What the product's rung (and the kernel) answered |
|---|---|
| L-ci, as the job runs | no write to the cgroup (`mkdir` refused); M `rlimit_as (cgroup_not_writable)` |
| this machine, a terminal's scope | the user may create a cgroup and move into it; the scope offers `memory pids` and **not** `cpu`; enabling a controller for the scope's children is refused (`EBUSY`: the scope holds the terminal's processes); M `rlimit_as (memory.max: permission denied)` |
| this machine, a transient scope with `Delegate=yes` | the same: M `rlimit_as (memory.max: permission denied)`; the runtime itself lives in the cgroup, so no controller can be enabled for its children |
| a transient unit with `Delegate=yes` on L-ci (run `44529348475`, §7) | see §7 |

**Where `cpu.max` can be set.** The worker's limits are written on its own
cgroup `mimlys-<pid>` (`memory.max`, `pids.max`), not on the `tool`
sub-cgroup, which holds the tools and `cgroup.kill`. A `cpu.max` on `tool`
needs `cpu` in the `subtree_control` of the cgroup above it, and the kernel
accepted that only where that cgroup held no process: in a cgroup the user
owns (`systemd-run --user --scope -p Delegate=yes`) with the worker moved
into a leaf *beside* `tool` first and `cpu` enabled afterwards for their
parent, `cpu.max = 100000 100000` on `tool` held a 4-thread tool to one core
(4.55 s of CPU in 4.55 s of wall time, against 1.22 s unbounded; the script
and its output are in the review request). In the other order (`cpu`
enabled for the children first, the worker moved afterwards) the kernel
refused the move (`Operation not supported`) and `tool` had no `cpu.max`.
I did not find a way for a runtime that lives in a user's terminal scope to
arrange this for itself.

**The effect of the bound, by a unit's `cpu.max`.** What the tree takes is
bounded the same when the whole runtime runs in a transient unit with a
quota (`systemd-run -p CPUQuota=<n>00%`, which sets `cpu.max` on the unit),
which is how the numbers below were made, on every Linux machine; the
unit-uncapped case shows that the unit itself costs nothing.

| Case | `burn_n` (4 threads) | `wide` | `demo` | `burn_2` |
|---|---|---|---|---|
| plain | 4.04 s (3.99-4.04) | 9.97 s (9.88-10.16) | 1.23 s (1.19-1.24) | 4.00 s (3.98-4.11) |
| unit, no quota | 4.03 s x1.00 | 10.26 s x1.03 | 1.27 s x1.04 | 4.02 s x1.01 |
| 1 core (`cpu.max` 100000/100000) | 15.53 s x3.84 | 31.68 s x3.18 | 3.15 s x2.57 | 8.97 s x2.24 |
| half the cores (2) | 7.80 s x1.93 | 17.38 s x1.74 | 1.86 s **x1.52** | 4.55 s x1.14 |
| all cores (4) | 4.04 s x1.00 | 10.25 s x1.03 | 1.25 s x1.02 | 4.01 s x1.00 |

Source: L-ci, 3 runs each (ratios to the plain run). L-4: half the cores
x2.37 (`burn_n`), x1.74 (`wide`), x1.50 (`demo`), x1.12 (`burn_2`); one core
x4.95, x3.80, x2.79, x2.77. L-32: half the cores (16) x1.65, x1.07, x1.04
(`burn_n`, `wide`, `demo`); one core x31.7, x8.2, x4.9.

### 4.2 Windows

`JOB_OBJECT_CPU_RATE_CONTROL` is set on the worker's Job Object, which holds
the worker and every tool it starts (a tool's own job, where it makes one,
is nested in it). The
program that did it exists on the throwaway branch only: it reads
`MIMLYS_MEASURE_CPU_RATE` in the runtime and sets the hard cap
(`JOB_OBJECT_CPU_RATE_CONTROL_ENABLE | _HARD_CAP`) after the job's memory and
process limits; its log shows the call succeeded for every worker (the probe's
and the serving one). **The rate is a share of the whole machine**, in
hundredths of a percent: one core of four is 2 500, two cores 5 000, all of
them 10 000.

| Case | `burn_n` (4 threads) | `burn_2` |
|---|---|---|
| uncapped | 5.08 s (5.05-5.17) | 4.95 s (4.94-4.96) |
| 1 core (rate 2 500) | 18.10 s x3.56 | 10.75 s x2.17 |
| half the cores (rate 5 000) | 9.16 s x1.81 | 5.62 s x1.14 |
| all cores (rate 10 000) | 5.13 s x1.01 | 4.96 s x1.00 |

Source: W-ci, 3 runs each. The bound is as good as Linux's: the busy time of
the machine for one `burn_n` is 19.4 s uncapped and 19.7 s at half; only the
wall time stretches. Two such trees at half do not slow each other (§3.3).

### 4.3 macOS

A lowered priority is not a bound. The tree of each case is the whole
runtime run under `nice -n <n>` (the worker and its tools inherit it); M-ci,
3 runs each:

| Case | `burn_n` | `wide` | `demo` | `burn_2` |
|---|---|---|---|---|
| idle machine, `nice 0` | 5.17 s (5.03-5.31) | 9.84 s (9.72-10.76) | 1.91 s (1.67-2.38) | 4.98 s (4.74-5.06) |
| idle, `nice 10` | x0.94 | x1.00 | x0.99 | x0.94 |
| idle, `nice 19` | x0.98 | x1.02 | x0.98 | x0.97 |
| idle, `taskpolicy -b` (background) | x1.01 | x1.01 | x1.08 | x0.99 |
| a competing 3-thread hog at `nice 0`, tree at `nice 0` | x1.87 | x1.44 | x1.26 | x1.65 |
| the same hog, tree at `nice 19` | x1.87 | x1.58 | x1.34 | x1.66 |

**An idle machine still gives the tree every core** (`nice 19` is x0.97 to
x1.02 of the unbounded time), so the report can say `best_effort (priority)`
with these numbers; and with a competitor present the lowered priority gave
the tree no smaller share (x1.87 against x1.87, x1.66 against x1.65; the build
x1.58 against x1.44, the demonstration x1.34 against x1.26):
on this runner `nice` does not even act as a share. `taskpolicy -b` (the
background policy) did nothing on an idle
runner in this run, but in the pilot (run `44517226777`, another instance, one
sample each) it made the same tree x2.0 (`burn_n`), x2.9 (`wide`), x6.2
(`demo`) and x1.2 (`burn_2`) slower. I cannot say what decides it, and it is
not a bound either way: on a 3-CPU virtual machine the two policies are
neither stable nor sized. M-ci is a virtual machine; a Mac of the user's own
may differ (§9).

### 4.4 Proposed default and rule

**LIM-H7 = half the cores, at least one**, as the candidate says, by the rule
"the smallest share at which the M6a demonstration stays within about 1.5
times its unbounded time on a 4-core machine, and at which two bounded trees
fill the machine" (§3.3). On L-ci the demonstration at half the cores is
x1.52; at one core x2.57; at all cores x1.02. A tool that uses two threads
(a test run) loses x1.14 at half; a build that wants every core takes x1.7 to
x1.9 (that is the price of the bound, by definition). On a machine with more
cores half costs less (L-32: the demonstration x1.04, the build x1.07).

The range "one core to all cores" is as measured: all cores is the same as
no bound (x1.00 to x1.03 on both platforms), one core is the sharpest (x2.2
to x3.8 for what was measured). On a machine of one or two cores "at least
one" and "all" coincide.

The CPU part is `enforced` where it exists and `unavailable` with its reason
where it does not (Linux without a delegated, empty cgroup: the common state,
§4.1); on macOS the value cannot be a bound (§4.3).

## 5. The cost of a task switch

An execution from the request to the result, through the real worker handle
of the real runtime (`Worker::execute`, as the runtime calls it), with two
tasks A and B whose workspaces were made by the runtime (on Windows with
their own capability entries), in the order AABBABAA repeated four times in
each of three fresh runtimes. "Kept" follows an execution of the same task;
"replaced" follows another task's, so the worker of the other task is ended
and a worker of this one is started (and, on macOS, the tree swept: 011p);
"cold" is the first execution of a runtime. Median, smallest and largest in
brackets; 45 kept and 48 replaced samples per row (3 cold).

**With the JavaScript interpreter installed** (every runtime of the CI runners
has it; the worker compiles it at its start, LIM-X4's reason):

| Source | Kind | Cold | Kept | Replaced |
|---|---|---|---|---|
| W-ci (per-task) | `run_wasm` | 939 ms (938-940) | 2.2 ms (2.0-2.7) | 952 ms (926-1 046) |
| | `run_script` (JavaScript) | | 2.5 ms (2.3-3.1) | 951 ms (927-1 390) |
| | host tool | | 44 ms (23-46) | 990 ms (951-1 074) |
| M-ci (per-task) | `run_wasm` | 510 ms (464-871) | 4.7 ms (0.7-21.4) | 685 ms (497-1 450) |
| | `run_script` (JavaScript) | | 1.0 ms (0.9-12.9) | 615 ms (484-1 005) |
| | host tool | | 123 ms (31-201) | 742 ms (608-1 000) |
| L-ci, per-task forced | `run_wasm` | 622 ms (618-684) | 1.2 ms (1.0-6.0) | 640 ms (629-794) |
| | `run_script` (JavaScript) | | 1.2 ms (0.9-3.6) | 640 ms (633-674) |
| | host tool | | 27 ms (26-28) | 665 ms (656-724) |
| L-ci, as it is (one worker for every task) | `run_wasm` | 618 ms (611-658) | 1.0 ms | 1.0 ms (0.9-2.7) |
| | host tool | | 27 ms | 27 ms (26-28) |

**Without the interpreter** (a runtime that has not installed one; the worker
starts and compiles nothing):

| Source | Kind | Cold | Kept | Replaced |
|---|---|---|---|---|
| W-ci | `run_wasm` | 27 ms (26-29) | 2.4 ms (2.1-2.8) | 47 ms (46-50) |
| | host tool | | 44 ms (23-45) | 88 ms (66-93) |
| M-ci | `run_wasm` | 13 ms (12-30) | 1.0 ms (0.7-369) | 148 ms (42-292) |
| | host tool | | 96 ms (23-179) | 227 ms (88-366) |
| L-ci, per-task forced | `run_wasm` | 4 ms (4-4) | 2.1 ms (1.0-4.4) | 24 ms (24-24) |
| | host tool | | 24 ms (23-26) | 46 ms (45-48) |

Against LIM-X4 (60 000 ms): a worker start with the interpreter is **0.6 to
1.0 s, 1 to 2 % of the bound**; without it 24 to 150 ms. Almost all of the
cost is the interpreter's compilation (a replaced execution without it
is 24 to 88 ms on Linux and Windows). For admission's design: **every
interleaving of two tasks on macOS or Windows costs about a second of
latency at each switch** (0.6 to 0.8 s on macOS, 0.95 s on Windows); a
runtime that keeps working on one task pays it once. On Linux, where the
worker serves every task, a switch costs nothing (1.0 ms for a script). A
host tool costs 27 ms (Linux), 44 ms (Windows) and 123 ms (macOS) at the
median even when the worker is kept, which is the tool's own start (and, on
macOS, the sandbox's work); the replaced execution adds the worker's start
to it. On macOS the spread is wide (kept: 31 to 201 ms; replaced
without the interpreter: 42 to 292 ms); the medians are not tight numbers.

## 6. LIM-O12: the engine gate

No measurement was made. Nothing seen argues against the default of 1: no
engine was in these runs, and nothing here bears on one consumer GPU's single
request at a time.

## 7. The replication and the pilot

Run `44517226777` (the pilot: one round of each phase, other runner
instances; the Linux one an AMD EPYC 7763 on kernel 7.0.0-1019, the Windows one
build 26100.33828) gave the same picture with the same ordering: the
single runtime of `burn_n` 4.66 s on L-ci (4.02 s in the full run), `wide`
11.65 s (10.23 s); on the instance with the slower processor the CPU-bound
workloads took 1.14 to 1.16 times as long, and the ratios agreed within 0.1
(for example `burn_n` x1.99, x2.96, x3.93 against x2.00, x2.95, x3.92; the
demonstration x1.68, x2.46, x3.17 against x1.59, x2.25, x2.92). Run
`44529348475` repeated the full measurement on a third set of instances with
fewer rounds; its numbers are in the review request and are used here only
where this section says so. (Filled in below.)

## 8. Proposed defaults, and where this touches RFC 013

| Limit | Proposed | Rule |
|---|---|---|
| LIM-X5 | **2** (range 1 to 16: measured to 4) | `floor(1 / LIM-H7 share)`: with the share at one half, two slots fill the machine and cost each other x1.02 to x1.15; the third costs x1.5 to x1.7, the fourth x2.0 to x2.4. Where the CPU part is not a bound the second slot costs x1.2 to x2.0 |
| LIM-H7 | **half the cores, at least one** (range one core to all cores) | the smallest share at which the demonstration stays within about x1.5 (x1.52 on 4 cores) and two trees fill the machine; all cores is no bound (x1.00 to x1.03) |
| LIM-O12 | **1** | nothing measured, nothing seen against it |

What this contradicts or sharpens in RFC 013 and its Amendment 1:

- **Decision 6 puts `cpu.max` "on the `tool` cgroup".** Today the worker's
  limits are on its own cgroup, and `tool` holds the tools and `cgroup.kill`.
  A `cpu.max` on `tool` works only if the worker lives in a leaf beside it
  and the cgroup above holds no process (§4.1); a runtime in a user's
  terminal scope, on the CI runner and in a `Delegate=yes` unit could not get
  that, so **the CPU part will read `unavailable` on Linux wherever memory and
  process count already read `rlimit_as` and `wall_clock`**, which is every
  Linux machine measured here. The sentence "so it never lowers the tier"
  holds; the expectation that the cgroup rung gives a bound on a normal
  Linux machine does not. What did work everywhere is the same quota on a
  unit the user starts the runtime in (`systemd-run --user --scope -p
  CPUQuota=...`), which the book may say.
- **Decision 6, "the CPU share ... measured against the M6a demonstration's
  duration"**: the demonstration cannot run on Windows (§1), so the Windows
  value rests on a native tool through the same path.
- **Decision 6 on Windows**: the Job Object's CPU rate is a share of *the whole
  machine* (hundredths of a percent of all processors), so "half the cores"
  is 5 000 on any machine and "one core" is `10 000 / cores`; the register's
  "one core to all cores" must be converted by the runtime.
- **Decision 6 on macOS**: confirmed, with numbers: a lowered priority is not
  a bound (§4.3), and here not even a share under contention. The wording of
  the clause part, `best_effort (priority)`, is right; "lowered priority"
  names more than one primitive (`nice`, the background policy), and the two
  behaved differently (§4.3).
- **Amendment 1, point 2** (a task switch costs a worker start): measured,
  §5. The cost on macOS and Windows is about a second with the JavaScript
  interpreter installed, which dominates it.
- **Decision 5**: nothing contradicts a slot per execution; the measured
  runtimes held 30 to 400 MiB each above an idle machine.

## 9. What could not be measured, and why

- **The M6a demonstration on Windows**: `cargo` cannot open `NUL` in the
  container (§1; still so on build 26100.33674). `burn_2` and `burn_n` stand
  in for it.
- **A `cpu.max` set by the product.** The product sets none yet. The Linux
  numbers are a quota on the unit the runtime ran in, not a limit the worker
  placed on its own `tool` cgroup; the one `cpu.max` on a `tool` sub-cgroup
  that was set was by hand, locally (§4.1). The Windows numbers come from a
  patch on the throwaway branch that sets the rate on the worker's job, which
  is the call the product would make.
- **The machine's responsiveness under several runtimes**: only the runtimes'
  own throughput was measured, not what a user's foreground work would feel.
- **Real hardware for macOS and Windows.** `macos-latest` is a virtual machine
  with 3 CPUs; `nice` and the background policy may behave otherwise on a Mac
  of one's own; Intel Macs were not measured.
- **More than 4 runtimes**, and machines with fewer than 3 or more than 4
  cores, except L-32.
- **Memory pressure.** No run came near the memory of its machine (the most:
  15 %), so no out-of-memory behaviour, swap or deadline under pressure was
  seen; the rule for memory is therefore a bound seen, not a limit reached.
- **The registry itself** (decision 5's files and locks): it is not built; no
  cost of taking, returning or reaping an entry was measured.
- **LIM-O12**: by the handoff, nothing.
