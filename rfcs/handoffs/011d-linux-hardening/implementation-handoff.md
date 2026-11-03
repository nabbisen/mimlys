# Handoff 011d — Linux hardening and read-set hygiene (release 0.7.1)

**RFC.** [RFC 011](../../accepted/011-confinement-backends.md), Amendment 1 · **Specification.** REQ-CONF-005 (Linux), REQ-CONF-011, REQ-CONF-012, REQ-TEST-013; external design §13.3, §13.5; limits LIM-H5; threats THR-11, THR-23, THR-24, THR-36 · **Release.** 0.7.1 (patch; the owner tags and publishes) · **Depends on** 011a, 011b (done).
**Origin.** Intake review N-01 and N-02 (`.git-exclude/reviewed/intake-0.7.0/README.md`), reproduced by the outgoing architect. Read both before you start; `evidence/uring_vs_seccomp.c` there is the reproducer for N-01.

## 1. Scope

In:

1. **Seccomp (REQ-CONF-012).** `install_seccomp` in `confine/linux.rs` always denies `io_uring_setup`, `io_uring_enter`, `io_uring_register`, `process_vm_readv`, `process_vm_writev` (errno `EPERM`, like the rest), whether or not network is denied, in every worker including the dependency step's.
2. **Landlock network layer (REQ-CONF-012).** When the detected ABI is 4 or later and the specification denies network, `apply_landlock` also handles `AccessNet::ConnectTcp | BindTcp` with no rules. The N clause's primitive is then `seccomp+landlock_net`; with a lower ABI it stays `seccomp` (still `enforced`: the `io_uring` denial makes the filter complete). Do not weaken any other rule; the ruleset keeps its hard requirement for the filesystem rights.
3. **Non-dumpable processes (REQ-CONF-012).** A function `confine::harden_process()` (safe `rustix::process::set_dumpable_behavior`, in the `confine` module so that REQ-CONF-010 holds) is called first thing in `mimlys`'s `main` and at the start of `worker_main`, before any thread exists and before the specification is applied. Document the cost in the book: no core dump of the runtime, no same-user `ptrace` of it.
4. **The `/proc` part of the read set (REQ-CONF-011).** `confinement_spec` in `app.rs` stops listing `/proc`. Start from none, run the toolchain **under the confinement** (the demonstration, the whole M6a suite, and a `cargo test --offline` of a larger project with a path dependency), read the failure it reports, and add **only** the single `/proc` file whose denial breaks something, then repeat; each entry gets its reason in a comment and the step is recorded in the request (candidates: `/proc/cpuinfo`, `/proc/meminfo`, `/proc/sys/vm/overcommit_memory`). This is the method on purpose: a trace of the *unconfined* tool lists every optional fallback it tries and would give a wider set than the toolchain needs. A tracer, if you already have one, may cross-check the result; nothing is installed for it. Run it with the toolchain of the reference machine and of the CI runner, and write in the book what a user sees when a toolchain needs a file outside the set (the message must name the file and the declared remedy, not a bare `Permission denied`). **Trap:** a Landlock rule is bound to the inode it was made for; a rule on `/proc/self` covers the *worker's* `/proc/<pid>` and nothing of the tool's. Never add a rule for a per-process directory or for `/proc` itself. If the toolchain cannot work without something per-process, stop and send a design request; do not widen.
5. **Declared read paths (REQ-CONF-011, LIM-H5).** One platform-neutral function (in `mimlys-core`, next to `host_tool.rs`, pure, no I/O: it receives the directory's entry names) decides; the `mimlys` crate lists the entry names (depth 1) and calls it from configuration loading (`check` and every command), and from `toolchain grant` (Windows code path, tested on Linux through the function). Refusals: a filesystem root, the home directory, an ancestor of the home directory, or a directory that directly holds a name of LIM-H5 (patterns as in LIM-W5's matcher, plus the literal names). The message names the entry and suggests the narrower path (`~/.cargo/bin`). The limits register is already amended; add the constant `LIM_H5_*` with the usual doc comment.
6. **Probe tool commands** (REQ-TEST-013), in `mimlys-probe-tool`, each printing a line the test asserts and exiting non-zero on failure to do the thing: `uring-socket` (an `AF_INET` socket through `IORING_OP_SOCKET`), `uring-connect <addr>` (the socket and a connect through `IORING_OP_CONNECT` to a loopback listener the test holds), `vm-read <pid>` (`process_vm_readv` on a pid), and the existing `read <path>` used on `/proc/<pid>/environ` and on a credentials file. The `mimlys` crate forbids `unsafe`: the raw-syscall helpers live in `mimlys-exec/src/confine/linux/probe.rs` (the one place allowed), behind a new feature of `mimlys-exec` enabled by `mimlys/test-bins`, each `unsafe` block with its `SAFETY:` line. The reproducer in the intake package shows the syscall sequence.
7. **CI.** The `check` job's matrix gains `ubuntu-24.04-arm` (the repository is public). The seccomp tables are untested on aarch64; this is where they get tested. Anything that fails there is a finding for the review request, not a reason to skip the runner.
8. **Documents.** `host-tools.md` (the new refusal; examples use `~/.cargo/bin` and `~/.rustup`, with the reason; the N cell `seccomp+landlock_net`; the non-dumpable cost), `configuration.md` and `getting-started.md` examples, `m6a-report.md` (a dated note: the clause table on this release), CHANGELOG `Unreleased` with a "Security" entry in the style of 0.7.0 and, under "Changed", the architect's document amendments of 2026-11-03 (REQ-CONF-011 and REQ-CONF-012, LIM-H5, THR-36, the Windows `NUL` exception in external design §23 and the ROADMAP, the RFC moves to `done/`; see `git log` for the commit) (the release commit is the owner's).

Out: macOS and Windows backends (011c, the Windows remedy); the Windows cleanup command (THR-36, a later handoff); narrowing `/etc` and `/opt`; MSRV and publish jobs and the naming script (separate warm-up tasks); any change to the control API, the CLI or the configuration beyond the new refusal.

## 2. Behaviours

- The reported clause table of a Linux worker is unchanged except the N primitive; `status --json` keeps its shape. Update the book's sample tables.
- `MIMLYS_PROBE_FAIL` and `MIMLYS_PROBE_BEST_EFFORT` keep working.
- The refusal of item 5 is a configuration error at load (exit 1), naming the declaration, the path and the entry found; `check` shows it the same way. An existing user configuration with `read_paths = ["~/.cargo"]` therefore stops loading: that is intended, and the CHANGELOG says what to write instead. If the toolchain needs more of `~/.cargo` (registry for offline dependencies), the tool declares `~/.cargo/registry` as an additional path; verify that works with `CARGO_HOME` and say what you found in the book.

## 3. Tests (REQ-TEST-013; each names its requirement)

All Linux, run on `ubuntu-latest`, `ubuntu-24.04-arm` and the reference machine; a test that must skip (kernel without `io_uring`, below tier `full`) prints why, and the review request counts skips separately from passes.

| Test | Expected |
|---|---|
| `req_conf_012_io_uring_socket_is_denied` | `uring-socket` inside a host tool fails (`EPERM`); the same command run outside the confinement succeeds on a kernel with `io_uring` (a control: the test skips with the reason where it cannot) |
| `req_conf_012_io_uring_connect_is_denied` | `uring-connect` to a live loopback listener: the listener sees no connection |
| `req_conf_012_cross_process_memory_is_denied` | `vm-read <runtime pid>` fails |
| `req_conf_012_runtime_is_not_dumpable` | from the *test process* (same user): `/proc/<pid>` of the runtime and of the worker is owned by root (the kernel's mark of a non-dumpable process) and `/proc/<pid>/environ` cannot be opened; there is no `Dumpable` line in `status` |
| `req_conf_011_proc_environ_is_unreadable` | `read /proc/<runtime pid>/environ` inside a host tool fails. The pid reaches the tool through a declared `env_allow` variable the runtime holds: start the runtime as `sh -c 'MIMLYS_TEST_RUNTIME_PID=$$ exec mimlys …'` |
| `req_conf_011_credentials_beside_toolchain_are_unreadable` | a `credentials.toml` placed beside the declared toolchain directory cannot be read |
| `req_conf_011_declaration_with_credentials_is_refused` | configuration with `read_paths` holding such a file, the home directory, an ancestor of it, `/`: each refused with the message; `~/.cargo/bin` accepted. Unit tests for the pure function in `mimlys-core` |
| `req_conf_012_landlock_net_layer` | on ABI ≥ 4 the probe reports `seccomp+landlock_net` and a TCP connect from a tool that bypasses the seccomp rule (use a test-only build of the tool that skips the seccomp layer, or assert the ruleset in a unit test of `apply_landlock`'s construction) fails |
| the M6a suite and the demonstration | unchanged and green: `cargo test --offline` ends `completed{verified}` with the narrowed read set |

**Mutation check (mandatory evidence).** For each of the first six tests, remove the rule it guards (the syscall from the deny list; `harden_process`; the `/proc` narrowing), run the test, and show it fail; restore. Put the failing output in the review request. A test that passes with the defect in place is worthless here.

## 4. Constraints and prohibited shortcuts

- No `unsafe` outside `crates/mimlys-exec/src/confine/`; `scripts/check-unsafe.sh` stays green.
- Do not turn the seccomp default from allow to deny, do not remove `/etc` or `/opt`, do not add a Landlock rule for `/proc` as a whole to make a test pass.
- Do not remove a variable from the runtime's environment as a "fix": it does not change `/proc/<pid>/environ` (verified; RFC 011 Amendment 1, point 2).
- No new dependency except under the feature of item 6 if you choose a crate over raw `libc` (state it; dependencies are the architect's decision).
- A design question goes into the review request as a question, with what you tried.

## 5. Return

Review request at `.git-exclude/review-requests/011d-linux-hardening/README.md` with: changed files; every command of the gate and its output; the list of `/proc` entries kept, each with the reason; the mutation-check outputs; CI run ids for the three Linux/Windows/macOS platforms **and** `ubuntu-24.04-arm`, stating which platform ran which suite; the skips; differences from this handoff; limitations. Do not tag, bump the version or publish.
