# M6a report: the Linux backend and host tools

Milestone M6a (RFC 011, handoff 011a) was implemented on 2026-10-29 and
amended by the design review of 0.6 on 2026-10-31 (handoff 011a §8). This
report records what the Linux backend enforces on the reference machine and
what CI's runner is expected to enforce, and the demonstration of external
design §23 (M6a).

## 1. The clause table on the reference machine

Kernel 7.3 (CachyOS), Landlock and seccomp built in. The terminal's
session scope lets the worker create a cgroup of its own and move into it,
but refuses to enable the `memory` and `pids` controllers for it (the scope
still holds the terminal's processes). The probe is a real worker's report
of its own confinement:

| Clause | State | Primitive | Note |
|---|---|---|---|
| F | enforced | `landlock` | the kernel's ABI detected as a hard requirement; abstract Unix sockets scoped |
| N | enforced | `seccomp` | `socket` outside `AF_UNIX` answered `EPERM`, loopback included |
| E | enforced | `explicit_environment` | nothing inherited by the worker or its tools |
| K | enforced | `cgroup_kill` | every process a host tool starts joins the worker's `tool` sub-cgroup before it runs and dies with one `cgroup.kill`; the worker is also the subreaper, and the walk runs after the kill |
| P | enforced | `no_new_privs+seccomp` | `ptrace`, `mount`, `keyctl`, `bpf`, `unshare`, `setns`, module and kexec loads denied |
| M | best effort | `rlimit_as` | `memory.max` refused (`permission denied`: the controller is not enabled for the scope's children); `RLIMIT_AS` of LIM-H2 set per host tool |
| C | best effort | `wall_clock` | `pids.max` refused likewise; the runtime's timer holds |

Tier: **full**. The cgroup rung is tried control by control (design review
of 0.6): before it, one refused write dropped the whole cgroup and K read
`subreaper` here. On GitHub's `ubuntu-latest`, where the runner's cgroup is
not writable by the job, the ladder is expected to end at `landlock`,
`seccomp`, `subreaper`, `rlimit_as`, `wall_clock`: tier `full` as well. A kernel without Landlock reports F `unavailable:
landlock_unsupported` and the tier `core`, with host tools invisible.

## 2. What the clause tests showed (REQ-TEST-013)

Inside a declared host tool (the probe tool, `tests/m6a_host_tools.rs`):

| Try | Result |
|---|---|
| read a file outside the read set | `permission denied` |
| write outside the workspace | `permission denied`; inside the workspace: written |
| connect to a loopback port with a listener alive | `permission denied` (no socket at all) |
| read the operator's `MIMLYS_TEST_SECRET` | unset; the declared `PROBE_OK` present |
| `setsid` a child and sleep, then two stop signals to `run` | every sleeper gone within 5 s (one `cgroup.kill` of the `tool` sub-cgroup here, the subreaper walk on a runner); the task `cancelled` |
| fork 100 | counted against `pids.max` where a cgroup is delegated |
| allocate 3 000 MiB | the tool ends non-zero (`RLIMIT_AS` or the cgroup) |
| `MIMLYS_PROBE_FAIL=F` | tier `core`, F `unavailable: injected failure`, `run_host_tool` not listed, a proposal rejected `unknown_tool` |
| `MIMLYS_PROBE_BEST_EFFORT=K` | tier `core`; with `accept_best_effort = ["K"]`: `full (K best-effort, accepted by policy.accept_best_effort)` in `status` and on the `Tier` line of the host tool's approval; `["M"]` refused by `check`; an unavailable clause never accepted |
| `--all`, `../x`, four arguments, an undeclared name | each `denied` at stage 7 with the argument's position and value, or the bound, and the hint for `argument_not_allowed`; the task going on between them |
| a `network = true` tool | reaches the listener from its own worker; the offline tool in the same task cannot |
| a leftover tree recorded in `worker.json` | killed at the next start, `runtime.orphans_killed` recorded |

## 3. The demonstration (external design §23, M6a)

A Cargo workspace with a path dependency (`arith`) and a failing test in
`src/lib.rs`; declarations `cargo_test` (`~/.cargo/bin/cargo test
--offline`, `workspace_write`, read paths `~/.cargo` and `~/.rustup`,
`RUSTUP_HOME` and `RUSTUP_TOOLCHAIN` in the environment); the scripted
model runs the test (fails), replaces the off-by-one, runs the test again
(passes), completes; the completion check runs `cargo_test` once more.

| Measure | Value |
|---|---|
| Outcome | `completed{verified}` |
| Actions | failed, succeeded, succeeded, succeeded (the check) |
| Wall time of the task | 0.82 s on the reference machine (the toolchain warm) |
| Where the build went | `tree/target` inside the workspace; skipped by snapshots and promotion |
| What the toolchain could see | `~/.cargo`, `~/.rustup`, the system directories; not the home directory |

## 4. Limits

LIM-H1 … LIM-H4 are kept as registered. LIM-O2 (2 s) was enough for every
tree kill observed; the subreaper walk ended within one pass in the tests.

## 5. Release 0.7.1 (2026-11-03): the clause table after the hardening

Handoff 011d (RFC 011 Amendment 1). On the reference machine (kernel 7.3,
Landlock ABI 4 or later, a session that delegates a cgroup but not the `memory` and
`pids` controllers) the probe reports:

| Clause | State | Primitive | What changed |
|---|---|---|---|
| F | enforced | `landlock` | the read set no longer holds `/proc` for a host tool; the worker keeps it for its own walk and a host tool restricts itself to the same sets without it (a second Landlock layer) |
| N | enforced | `seccomp+landlock_net` | `io_uring_*` and `process_vm_*` denied always; TCP `bind` and `connect` handled with no rule from ABI 4 (Linux 6.7), otherwise `seccomp` alone, still complete |
| E, K, P, M, C | unchanged | | |

The new clause tests (REQ-TEST-013) each have a control, the same command
outside the confinement, so that a denial is the confinement's: `uring-socket`
and `uring-connect` (a live listener sees no connection), `vm-read` of a
descendant, a same-user process's `cmdline` and `environ`, and a
`credentials.toml` beside the declared toolchain directory. The read set for
the demonstration is `~/.cargo/bin` and `~/.rustup`: the earlier §3 declared
`~/.cargo`, which holds the crates.io token and is refused since this release.
The measured `/proc` set for `cargo test --offline` of a project with a path
dependency is empty: cargo and rustc fall back when `/proc` files are
unreadable.
