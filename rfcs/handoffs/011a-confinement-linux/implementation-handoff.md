# Handoff 011a — The Linux backend and host tools (M6a)

**RFC.** [RFC 011](../../done/011-confinement-backends.md) (Accepted)
**Specification.** REQ-CONF-001 … 010; REQ-CAP-002 (`host_tool`); REQ-TEST-013; external design §11 (`run_host_tool`), §13, §19.1 (`[host_tools]`), §23 (M6a); limits LIM-H1 … H4, LIM-O2.
**Crates.** `mimlys-exec` (`confine/linux.rs`: Landlock, seccomp, namespaces, cgroup, subreaper; `confine.rs`: `ConfinementSpec`, the backend reports; `worker_main.rs`: self-confinement before the first frame, `run_host_tool`; `tools.rs`: the `run_host_tool` definition and the argument validation), `mimlys-core` (`host_tool.rs`: declarations and the validation; `confine.rs`: `ClauseState` with the primitive; `Tier` with accepted best-effort; the event), `mimlys` (`[host_tools.<name>]`, `[policy] accept_best_effort`, `mimlys toolchain list`, `status`, the book chapter, the M6a tests and demonstration).
**Milestone.** M6a (release 0.6). Depends on 001–010.

## 1. Scope

In: everything RFC 011 decides for Linux, the platform-independent parts (declarations, validation, the dependency step, the tier rule, `toolchain list`), the clause tests and the demonstration on Linux in CI (`ubuntu-latest`, where unprivileged user namespaces may be restricted and cgroups are not delegated: the subreaper and rlimit paths are what CI exercises; the owner's machine exercises namespaces and the delegated cgroup).

Out: Windows and macOS (011b, 011c); `mimlys toolchain grant|revoke` (Windows only).

## 2. Module layout

```text
crates/mimlys-exec/src/
  confine.rs           ConfinementSpec{read_paths, write_paths, network: NetworkRule, memory_bytes, max_processes}; Report{clauses: BTreeMap<Clause, ClauseState>} ; the portable spawn of M2 unchanged
  confine/linux.rs     apply(spec) -> Report, in order: cgroup (try: create <own cgroup>/mimlys-<pid>, write memory.max/pids.max, move self; else rlimits), namespaces (try: unshare(USER|NET|PID) with uid/gid maps; else subreaper + seccomp-only N), no_new_privs, seccomp (seccompiler: deny socket(!AF_UNIX), ptrace, mount, keyctl, bpf, unshare, setns, kexec, reboot…; errno EPERM), landlock (ruleset with every fs access right the ABI knows; read rules; write rules; restrict_self). Each step records enforced(primitive) / best_effort(primitive) / unavailable{reason}
  confine/linux/tests.rs  unit tests of the spec → rules mapping and of the report
  worker_main.rs       parse --confine <json spec> ; apply before spawning threads or reading frames ; `--probe` applies too and prints the report ; Execute{run_host_tool{...}} → spawn the declared command under the workspace with env, capture LIM-H4, deadline, cancel → kill tree
  tools.rs             the `run_host_tool` ToolDefinition (params: tool: Enum of declared names, args: StringArray ≤ max) ; effect per declaration ; profile Host
crates/mimlys-core/src/
  host_tool.rs (+tests) HostToolDeclaration{name, command, args, allowed_args, arg_pattern: Option<String>, max_args, effect, reversible, timeout_ms, read_paths, env, env_allow, network} ; validate_args(decl, args) -> Result<Vec<String>, ArgViolation> ; reads paths expanded (~)
  confine.rs           ClauseState::{Enforced{primitive}, BestEffort{primitive}, Unavailable{reason}} (serde: state + primitive/reason) ; Probe::tier(accepted: &[Clause]) -> Tier{Core, Full, FullAccepted(Vec<Clause>)}
  policy.rs            Policy.accept_best_effort: Vec<Clause> ; ceiling.host_tools (exists) must name declared tools
  event.rs             runtime.orphans_killed{count}
crates/mimlys/src/
  config.rs            [host_tools.<name>] ; [policy] accept_best_effort ; validation (command absolute or resolvable in read_paths; pattern compiles; effect ∈ {workspace_write, host_write}; timeout in LIM-H1 range)
  app.rs               ConfinementSpec from the configuration (data dir, temp, union of read_paths, system paths /usr /lib /lib64 /bin /etc/ld.so.cache /etc/ssl? no: /etc read-only list as declared); the probe from a real worker --confine --probe ; the tier with accept_best_effort ; the fetch worker (network allow) per execution of a network tool
  commands/toolchain.rs  `toolchain list`
  commands/status.rs   the clause table with primitives ; the tier line with the accepted clauses ; the read set
docs/src/reference/host-tools.md ; docs/src/maintainers/m6a-report.md
```

## 3. Behaviours

### 3.1 Self-confinement (RFC 011 §1, §2)

`mimlys-worker --serve --confine '<spec json>'`: before anything else, `linux::apply(&spec)`; the report is kept and returned on `probe`. Order matters: cgroup first (needs write access to the current cgroup's parent), then namespaces (single-threaded, before tokio), then `no_new_privs`, seccomp (must come after `unshare`), Landlock last (it also restricts the worker's own reads: the interpreters directory and the WASM modules are in the read set). A clause's reason strings: `landlock_unsupported`, `landlock_abi_0`, `seccomp_unsupported`, `userns_denied`, `cgroup_not_delegated`, `rlimit_failed`.

### 3.2 K without a PID namespace

`prctl(PR_SET_CHILD_SUBREAPER, 1)` at start; on cancel, deadline or shutdown, `kill_descendants()`: walk `/proc/*/stat` for every process whose ancestor chain reaches the worker's pid, send `SIGKILL`, repeat until a walk finds none or LIM-O2 passes (then report `tree_kill_incomplete` in the execution's notices and `host_fault{unavailable}`). With a PID namespace, killing the child init does it; with `cgroup.kill`, one write does it.

### 3.3 `run_host_tool` (RFC 011 §6)

Stage 5 validates `tool` (a declared name the task holds a `host_tool` grant for, else `unknown_tool`) and `args` (`HostToolDeclaration::validate_args`); stage 7 checks the grant; stage 9 derives the effect from the declaration (`workspace_write` → reversible, approval per profile; `host_write` → approval required, non-reversible unless declared). The worker spawns `command` with `args ++ validated args`, `current_dir = workspace tree`, `env_clear` + `env` + `env_allow` values + `HOME`/`TMPDIR` pointing into the workspace's temp, stdio piped, in a new process group; captures each stream to LIM-H4 with `output_truncated`; ends it on deadline (`Termination::Deadline`) or cancel (`Cancelled`) through §3.2; exit code → `Exited{exit_code}`, non-zero → `tool_error{exit_code:N}`.

### 3.4 The dependency step (RFC 011 §8)

A declaration with `network = true` is executed by `Worker::spawn_once(spec with network: Allow)`: the runtime starts a second worker for that execution only, with the same spec otherwise, and ends it after the execution; its report is checked to be the main worker's minus N, else the execution is `host_fault{unavailable}`.

### 3.5 Tier and status (RFC 011 §5)

`Probe::tier(&policy.accept_best_effort)`: `Full` when every mandatory clause is `Enforced`; `FullAccepted(clauses)` when the only non-enforced mandatory clauses are `BestEffort` and listed in `accept_best_effort`; else `Core`. `status` prints `tier            full (K best-effort, accepted by policy.accept_best_effort)` and, per clause, `enforced (landlock)`. Host-tool approval renderings carry the same tier line. An `Unavailable` clause listed in `accept_best_effort` is a configuration error at load.

### 3.6 Orphans (RFC 011 §9)

`<data>/worker.json` records `{pid, start_time, pgid, cgroup?}` while the worker lives and is removed at shutdown; at startup a record whose pid still has that start time (`/proc/<pid>/stat` field 22) is killed with its group and cgroup, `runtime.orphans_killed{count}` recorded.

## 4. Tests (REQ-TEST-013)

Against a small host tool built for the tests (`mimlys-probe-tool`, a test binary that does what its first argument says: `read <path>`, `write <path>`, `connect <addr>`, `env <name>`, `setsid-sleep`, `fork <n>`, `alloc <mib>`, `setuid`):

- Per clause reported `enforced`: the forbidden thing fails as expected and the allowed thing works (a read inside the set, a write inside the workspace, an `AF_UNIX` socketpair).
- K: `setsid-sleep` then cancel → the execution ends `cancelled` within LIM-O2 and no process of the tree remains (checked by pid).
- The probe is honest: `MIMLYS_PROBE_FAIL=F` → F unavailable, `run_host_tool` absent from the definitions, `status` tier `core` with the reason; a machine without Landlock (simulated by the injection) behaves the same.
- Argument validation: literals accepted, a pattern match accepted, anything else `schema` rejection naming the argument; `max_args`; `..` and absolute paths refused unless the pattern allows them.
- The dependency step: a `network = true` tool runs with N absent in its own worker's report and is `external_read` with approval under `workspace`; a `network = false` tool cannot connect.
- Accepted best-effort: with M `best_effort` the tier stays `full`; with K `best_effort` (simulated) the tier is `core` unless `accept_best_effort = ["K"]`, then `full (K best-effort, accepted …)` and the approval line shows it.
- Orphans: a worker record left behind is killed at startup and the event recorded.
- The demonstration (external design §23, M6a): a Cargo project in the scope with a failing test and a vendored dependency (`cargo vendor` committed, `.cargo/config.toml` pointing at it); declarations `cargo_test` (`cargo test --offline`, `workspace_write`, read paths `~/.cargo`, `~/.rustup`) and `cargo_fetch` (`network = true`); the scripted model runs `cargo_test` (fails), edits the file, runs `cargo_test` (passes), completes with a `run_host_tool{cargo_test}` completion check → `completed{verified}`; the report records the clause table, the primitives in force on the CI runner and on the reference machine, and the timings.

## 5. Return

Review package under `.git-exclude/reviewed/m6a-011a/` with the clause table from CI and from the reference machine, the demonstration transcript, the report, and the configuration lines a user needs.

## 7. Amendments made during implementation (2026-10-29)

- **No namespace rung.** The only safe `unshare` in the system-call crate is deprecated in favour of an `unsafe` one, and clause N is enforced by the seccomp filter alone (`socket` outside `AF_UNIX` answered `EPERM`, loopback included). The network namespace is not used in M6a; the state reads `enforced (seccomp)`. Abstract Unix sockets are scoped by Landlock where the ABI allows (ABI 6 and up), so that no socket outside the sandbox can be reached through them; filesystem Unix sockets are governed by the read and write sets.
- **The Landlock ABI is detected** by asking for each known ABI as a hard requirement from the newest down (the crate's own query is private); the ruleset is then built as a hard requirement at that ABI, so `enforced (landlock)` means fully enforced.
- **No PID namespace rung either** (its init semantics interact badly with the worker's own death); K is `cgroup_kill` where a cgroup is delegated, else the subreaper walk. The walk skips zombies.
- **`RLIMIT_AS` is set per host tool** around its spawn (the worker cannot carry it: the engine reserves large address ranges); M reads `best_effort (rlimit_as (reason))` where no cgroup is delegated. C without a cgroup is `best_effort (wall_clock)`: `RLIMIT_NPROC` counts every process of the user and would reach the user's session.
- **The probe is a real worker run**: `mimlys-worker --probe --confine <spec> [--host-tool …]` prints its report; `Worker::probe_confined` parses and caches it. The debug-build injections `MIMLYS_PROBE_FAIL=<letter>` and the new `MIMLYS_PROBE_BEST_EFFORT=<letter>` are forwarded to the worker's explicit environment (REQ-TEST-013's honest-probe and accepted-best-effort tests).
- **The worker protocol carries the core's `ClauseState`** (one vocabulary, one meaning); the exec crate's copy is gone.
- **Where the limits live.** LIM-H1 … H4 became constants; the `cargo_test` example of the external design is `workspace_write` (RFC 011 Q-7) and a `cargo_fetch` dependency step is shown beside it.
- **The read set** is built by the runtime from the data directory's `interpreters`, the configured interpreter modules' directories, the worker binary's directory, the temporary directory, the declared toolchain paths and the system directories (`/usr`, `/lib`, `/lib64`, `/bin`, `/sbin`, `/etc`, `/proc`, `/opt`, the random devices); the write set is the workspaces directory, the temporary directory and `/dev/null`. The store file under the data directory is not in the read set.
- **`~` is expanded** in `command`, `read_paths` and the declared `env` values; `env_allow` values are resolved from the runtime's environment into the declaration before the worker sees it (the worker's environment is explicit).
- **The host tool's `PATH`** is the declared read paths' `bin` directories plus `/usr/bin:/bin`, so a toolchain finds its helpers (`cc`, `ld`); `HOME`, `TMPDIR`, `TMP` point into the workspace's temporary directory (so `cargo` without `CARGO_HOME` keeps its caches there, writable, while the toolchain under `~/.rustup` is read-only).
- **Argument violations are `denied{argument_not_allowed}`** at stage 7 (the hook has the declarations), with the argument's position and value in the detail.
- **The test binaries' feature is `test-bins`** (the MCP test server and the probe tool); the M6a tests skip with a message on a machine below tier `full`, and the demonstration skips without a rustup toolchain under the home directory.

## 8. Amendments from the design review of 0.6 (2026-10-31)

- **The cgroup rung is tried control by control.** `join_cgroup` creates `<own cgroup>/mimlys-<pid>` and its `tool` sub-cgroup, moves the worker in, then sets `memory.max` and `pids.max` each on its own; a session that refuses a controller (the "no internal processes" rule of a desktop scope) keeps the directory. Before, one refused write dropped the whole rung and K read `subreaper` on the reference machine.
- **K through `cgroup.kill` is real.** Every host tool's process joins the `tool` sub-cgroup between fork and exec (`pre_exec`, the module's one `unsafe` block, justified), so the whole tree dies with one write whatever it forked; the process-group kill and the subreaper walk follow. K reads `cgroup_kill` only when the worker can still open `tool/cgroup.procs` and `tool/cgroup.kill` after Landlock (the worker's cgroup is in the write set); else `subreaper`. The worker's cgroup travels in `probe_result` and the record, is removed when the worker ends, swept at the next start when a crash left it, and a probe's own goes with the probe.
- **`run_host_tool` is visible through `host_tool` grants alone** (ED §9.1 as written); it is never a `tool` grant nor a `tools` entry, and profiles gain `host_tools`. `approval_tools` may still name it.
- **`host_write` and `reversible` left the declaration** until write paths exist (DR-017): the write set holds only the workspaces, so the key promised what Landlock refused.
- **Every host-tool approval carries a `Tier` line** (REQ-CONF-004, ED §13.4); it was in the status only.
- **Argument denials carry `argument`, `value` or `max`** and the hint for `argument_not_allowed`, instead of the violation text under `grant_kind` and the workspace hint.

