# M6c report: the macOS backend

Milestone M6c (RFC 011 §4 and Amendment 2, settled; handoff 011c) was
implemented on 2026-11-07 and is gated by the macOS runner of CI
(`macos-latest`, Apple silicon, macOS 26.7.2, Xcode 26.6), where the clause
suite `tests/m6c_host_tools.rs` and the demonstration run, and by the Rosetta
job (the same suite as Intel code on the same kernel; informative, see §6).
No Mac was available to the developers: everything below that is about the
kernel was measured on CI, by two spikes (S1, S1b) and by the suite itself.
The tables are those of CI runs `42362327695` (the tip of the third slice,
before this report) and, for the spikes, the runs named in §3.

## 1. The backend

| Clause | Primitive | How |
|---|---|---|
| F | `seatbelt` | one Seatbelt profile for the worker and its tools, generated from the specification (canonical paths, escaped strings, `(deny default)`), applied by the worker to itself before its first frame and before any thread exists; `sandbox_init` is resolved with `dlsym`, so a macOS without it reports `seatbelt_missing`, and the worker checks that the profile holds (a directory it could read before must be unreadable now) |
| N | `seatbelt` | `(deny network*)`; Unix sockets only under the write set; the name-lookup socket denied explicitly after them; no DNS mach service. The dependency step's worker adds three rules (outbound TCP to port 443, a connect to the name-lookup socket, the system's certificate file) |
| E | `explicit_environment` | the worker's and every host tool's environment is built from nothing |
| K | `seatbelt_membership` | the **membership kill loop**: the worker lists every pid (`proc_listallpids`), keeps those `proc_pidinfo` can read (which the profile allows for the sandbox's members only), skips itself, `SIGKILL`s each and repeats until a listing finds none or LIM-O2 passes. After a cancel, a deadline and the tool's end, so a re-parented orphan cannot outlive its tool. Enforced: the kernel decides who is a member |
| P | `seatbelt` | the kernel refuses a setuid `execve` from a sandboxed process; no rule is needed, a clause test asserts it |
| M | `rss_watchdog` (best effort) | the members' summed memory above LIM-H2 kills the tree and the execution ends `memory_limit`; polled every LIM-H6 |
| C | `process_count_watchdog` (best effort) | the members' count above LIM-H3 kills the tree and the execution ends `process_limit` (`resource_exhausted{processes}`; never an exit of the tool's own); polled every LIM-H6 |

Every check is the worker's own observation of itself. K, M and C are
reported only if the worker verified, from inside the applied profile, that
the pid listing works, that it can inspect itself, and that neither `launchd`
nor its parent (processes outside the sandbox) can be inspected or signalled;
otherwise all three read `unavailable: membership_unverified: …`. The kill
loop and the watchdogs also refuse to act in a process that did not verify:
outside the profile, "every pid I can inspect" is every process of the user.

**The sweeper.** The runtime is outside the sandbox and cannot list its
members, so a worker that is killed or crashes would leave its tools alive
(confined, but able to write the workspace after their execution ended); Linux
kills the worker's cgroup and Windows closes its job in that case. Right after
the verification and before any thread exists, the worker therefore forks one
**sweeper**: a member in a process group of its own, with its standard
streams on `/dev/null`, holding the read end of a pipe whose write end only
the worker holds (close-on-exec, so no tool inherits it). At end of file,
however the worker ended, the sweeper runs the kill loop (itself excluded) and
exits; the worker's own loop, its counts and its weights exclude the sweeper.
It acts on end of file only, never on a failed read. A worker that cannot start
it (it has threads, or the fork fails) reports K, M and C `unavailable`.
The pipe's behaviour is generic Unix and is tested on Linux too
(`tests/sweeper.rs`); the kill is tested on macOS.

**`xcrun`'s cache.** With an Xcode selected, `xcrun` resolves the developer
tools through a cache file, `xcrun_db`, in the per-user temporary directory,
which the profile admits read-only. Where it is absent (a Mac where `xcrun`
never ran) a sandboxed `xcrun` cannot create it and the build fails (§7). The
runtime therefore makes it itself, **outside the sandbox**, before it starts
the first worker: it runs `/usr/bin/xcrun --find clang` with a cleared
environment, waiting at most LIM-O8 (10 s; a first run took 1.9 to 2.9 s on
the runner, one with the cache 0.07 s), and `check` and `status` show one line
naming the file and the remedy if it still is not there (the run goes on; the
tool's own error then names the cause). A Mac with the Command Line Tools
selected needs nothing. Never a write rule for the cache in the profile: the
user's own unconfined `xcrun` trusts the file later, and a tool that could
write it could choose what that `xcrun` runs (a unit test asserts that no
rule that writes names the file or its directory).

The profile, in the order that makes the last match the right one:
`(deny default)`; `process-fork`; `process-exec*` of the read and write sets;
`file-read-metadata` anywhere; the root directory as a literal; the read set
readable; the write set readable and writable; `sysctl-read` allowed, then
`kern.procargs` and `kern.proc.` denied; one mach service
(`com.apple.bsd.dirhelper`); `signal` and `process-info*` for the sandbox
only (the broad denial is `(with no-report)`), then the pid listing; the
network rules above.

## 2. The clause table on `macos-latest`

`mimlys status` on the runner, as the clause suite asserts it in
`req_conf_003_macos_status_table` (CI run `42362327695`, job `128373116723`)
and as a debug run of the same code printed it (run `42361156347`):

| Clause | State | Primitive |
|---|---|---|
| F filesystem | enforced | `seatbelt` |
| N network | enforced | `seatbelt` |
| E environment | enforced | `explicit_environment` |
| K termination | enforced | `seatbelt_membership` |
| P privilege | enforced | `seatbelt` |
| M memory | best effort | `rss_watchdog` |
| C count | best effort | `process_count_watchdog` |

CI of the tip of the third slice, run `42362327695` (every job green):

| Job | Passed | Skipped |
|---|---|---|
| check (macos-latest, arm64) | 367 | 0 |
| macOS x86_64 under Rosetta (informative) | 367 | 0 |
| check (ubuntu-latest, ubuntu-24.04-arm), minimum Rust version | 366 | 0 |
| check (windows-latest) | 346 | 0 |

Tier: **full**, with no `accept_best_effort` in the configuration. Host tools
therefore need no accepted clause on macOS. (RFC 011 §5's mechanism stays for
any platform where a mandatory clause is best effort; the first slice of this
backend used it through a test hook, which is gone.)

## 3. What the spikes measured

Spike S1 (macOS 26.7.2, arm64, four CI runs) and S1b (three runs); the
reports are in the review records of the milestone.

- `sandbox_init` with source text works; a syntax error returns a readable
  message with a position. A sandboxed process can apply **no second
  profile**, to itself or to a child, and `sandbox-exec` inside a profile
  aborts: there is one profile for the worker and its tools (the Linux
  two-layer design cannot be copied).
- Wasmtime, its JIT and its mach-port trap handling run under `(deny
  default)` with no rule; the real worker needs nothing beyond the tools'
  set (the kernel's sandbox log shows no denial of the worker except the
  self-check's own).
- Sandbox membership is exact: a re-parented orphan (`setsid`, double fork,
  parent `launchd`) is still signalable; a second worker that applied the
  same profile text is another sandbox and cannot signal it. **`kill(-1,
  SIGKILL)` inside the profile kills the members and the sender**: it is not
  used; the per-pid loop above converged on a fork storm in 0.16 s.
- No listing restricted to the sandbox exists (every form with a target
  parses and never matches); `process-info-listpids` without a target lists
  every pid of the machine. `proc_pidinfo` then succeeds for exactly the
  members (5 of 5, no outsider).
- `sysctl KERN_PROCARGS2` of another process shows its environment unless
  **both** the `process-info*` narrowing and the `kern.procargs` prefix denial
  are in force (measured by the first slice: each alone leaves it readable).
  `KERN_PROC_ALL` lists every process with its name unless the `kern.proc.`
  prefix is denied.
- DNS goes through a mach service and a Unix socket, an egress channel; the
  offline profile has neither. UDP `sendto` under `(deny network*)` fails.
- A setuid `execve` is refused to a sandboxed process (`sudo -n id -u` inside
  does not print `0`; outside it does).
- The read set of `cargo test --offline` is far smaller than a first guess:
  `/System` is not needed (the dyld cache is mapped by the kernel), nor
  `/private/etc` as a whole, nor any file of the home directory; one of two
  interchangeable mach services is (`com.apple.bsd.dirhelper`; the other can
  consult a network directory and is refused).

## 4. The read set, and what the implementation added to it

The system part is the minimum S1b measured: `/usr`, `/bin`, `/sbin`, the
root directory (a literal), `/private/etc/ssl/openssl.cnf`,
`/System/Library/CoreServices/SystemVersion.plist`, the random and zero
devices, the active developer directory (read from the `xcode_select_link`
without running a command, else the Command Line Tools) and, for an Xcode,
its `Contents/SharedFrameworks`; `xcrun`'s cache file `xcrun_db` by name in
the per-user temporary directory (`confstr(_CS_DARWIN_USER_TEMP_DIR)`, which is
not `$TMPDIR`); Rosetta's runtime where it is installed. Then the declared
paths.

The implementation ran the whole suite (M1 to M6c, the worker, the CLI) and
the demonstration against exactly this set on `macos-latest`, with a debug
workflow printing the sandbox's denials, and **no addition was asked for**: the
suite passed and `cargo test --offline` of the demonstration project ended
`completed{verified}` in about four seconds. The denials that remain in the
log are all of files the toolchain asked for and did not need (for example
`/private/etc/passwd` with the `opendirectoryd.libinfo` lookup that goes with
it, framework files under `/System` that `xcodebuild` probes, `/dev/tty`).

A `command` whose canonical target, after its links, leaves the read paths is
refused at load on every platform, naming the resolved path and the directory
to declare: Homebrew's `rustup` links into `/opt/homebrew/Cellar/…`, which the
runner image has, so the demonstration declares the toolchain's own `cargo`
under `~/.rustup/toolchains/…/bin`.

## 5. What the clause tests check (REQ-TEST-013)

All of `tests/m6c_host_tools.rs` runs the probe tool as a host tool under the
real profile, each denial with a control (the same thing done by the tool
outside the profile), each rule shown to fail without it (the mutation
outputs are in the review records).

| Try | Expected |
|---|---|
| `status` | the table of §2, tier `full` |
| read outside the set, `/etc/hosts`, a file of `/System`, a marker in the per-user temporary directory | `permission denied` |
| `openssl.cnf`, `SystemVersion.plist`, `/usr/bin/true` | readable |
| write outside the workspace | `permission denied`; inside: written |
| connect to a live loopback listener | `permission denied`; the listener saw nothing |
| `getaddrinfo` of a name outside `/etc/hosts` | fails |
| a Unix socket under the write set / outside it | works / `permission denied` |
| the operator's `MIMLYS_TEST_SECRET` | unset; the declared variable present |
| `KERN_PROCARGS2` of the runtime | refused (the control reads a same-user process's environment) |
| `proc_pidinfo` and signal 0 to the runtime; `KERN_PROC_ALL` | refused |
| `sudo -n id -u` | `execve` refused; `id` works and prints a uid other than 0 |
| a tool that starts a sleeper in a session of its own (re-parented to `launchd`), then a double interrupt | tool and sleeper gone within LIM-O2, task `cancelled` |
| a tool that ends leaving such a sleeper | the sleeper is gone |
| the worker killed with `SIGKILL` while a tool and such a sleeper run (after an earlier tool has ended) | no member remains within LIM-O2 plus one poll; the execution is `host_fault{worker_died}` |
| the runtime killed with `SIGKILL` while a tool runs | the same |
| the main worker and the dependency step's worker, both with members | neither tool's listing holds a pid of the other's sandbox (nor a worker's), and each sees its own sleeper |
| the sweeper's pipe (Linux and macOS) | acts at end of file only; not inherited by a child that outlives the owner; own process group and no stream of the owner's |
| `fork 100` held open | killed by the watchdog, the execution ends `process_limit` (`resource_exhausted{processes}`, no exit code), children gone; `fork 10` passes |
| 3 000 MiB touched | `memory_limit`; 200 MiB passes |
| `MIMLYS_PROBE_FAIL=F` | tier `core`, `run_host_tool` unlisted |
| the dependency step: a name, port 443, a loopback port that is not 443, the certificates | resolves, connects, `permission denied`, readable; the offline tool does none |
| the approval | its `Tier` line reads `full` |
| the demonstration with `xcrun`'s cache deleted first (an Xcode selected) | completes; the cache exists afterwards (made by the runtime) |
| `status` and `check` when the warming cannot succeed (the debug-only `MIMLYS_TEST_XCRUN` points at a program that fails) | one line each naming the file and the remedy, `xcrun_cache_missing` in the JSON, exit 0; without the hook no line |
| the demonstration | `cargo test --offline` of a Cargo workspace with a path dependency, `completed{verified}`, no accepted clause |

## 6. Residuals and known platform facts

- **Process numbers.** The kill loop needs the pid listing, and no listing
  restricted to the sandbox exists, so a host tool can learn the **numbers** of
  every process on the machine (not their names, owners, parents or
  arguments: `kern.proc.` is denied and `proc_pidinfo` is refused for
  non-members). Recorded in the threat register.
- **The runtime's environment** is readable through `KERN_PROCARGS2` by an
  *unconfined* program of the same user; macOS offers no refusal, and
  removing a variable after reading it changes nothing. A host tool is refused
  it by the profile (the pair of rules in §3).
- A host tool can learn whether a path exists outside its read set
  (`file-read-metadata` is needed by the loader); no content is readable.
- **M and C are best effort**: a poll every LIM-H6 (100 ms, unchanged) can
  miss an excursion shorter than the interval. A member's memory is the larger
  of its resident size and its physical footprint: the resident size alone
  under-reports when the system compresses a process's pages (a 3 GiB tool
  showed under 2 GiB resident on a loaded runner and escaped the first
  version of the watchdog). A tree killed for its count ends `process_limit`
  (external design §7.4), mapped to `resource_exhausted{processes}`; where the
  kernel refuses the extra process instead (Linux, Windows) the tool sees a
  failed `fork` and its own ending is reported.
- **The sweeper's residual.** A tool can kill the sweeper (members of one
  sandbox may signal one another); the worker then no longer has a safety net
  and a worker that dies without an orderly end leaves the tool's processes
  alive, still confined. An orderly end (a cancel, a deadline, the end of a
  tool, the end of the worker's input) sweeps without the sweeper. Recorded in
  the threat register. The two workers' sandboxes (the main one and the
  dependency step's) are apart: neither's listing holds the other's pids, so
  neither's kill loop can reach the other's tools.
- The broad refusal of `process-info*` is `(with no-report)`: the loop and
  the watchdogs ask about every pid at every poll, and each refusal was one
  record in the system log (about 90 000 lines in one suite run; about 9 000
  after).
- One mach service is admitted (`com.apple.bsd.dirhelper`); it is `xcrun`'s,
  and no service that can reach the network or the user's secrets is.
- The dependency step reaches registries over HTTPS only; a git dependency
  over SSH is not reachable.
- Measured on macOS 26.7.2, arm64, one Xcode layout (`Contents/Developer`),
  Homebrew's `rustup`. A Mac with only the Command Line Tools, a different
  macOS, or a cargo linked against another TLS library was not tested; the
  refusal at load and the failing file named in a tool's `stderr` are how an
  administrator finds what to declare. Intel hardware is out of scope
  (REQ-PLAT-002): the Rosetta job runs the suite as Intel code on the same
  kernel, which exercises the Intel build, not an Intel Mac, and its results
  are informative.

## 7. Measurements (handoff 011c-5; reported, nothing changed by them)

On `macos-latest` (macOS 26.7.2, arm64, 3 CPUs, 7 GiB, about 570 processes),
runs `42374250330` and `42374899386` (the second repeats the first; the throwaway workflow that ran them is not in the repository).

- **The cost of a poll** (LIM-H6). One poll is what the watchdogs do: list
  every pid, read the ones the profile lets the worker read. Measured from
  inside the sandbox, 200 polls each: minimum 0.24 and 0.20 ms, **mean 0.80
  and 0.75 ms**, maximum 3.03 and 23.91 ms (one outlier in 200). The worker's
  CPU time while a tool slept for ten seconds under the watchdogs: **0.05 s and
  0.06 s in 10.1 s of wall (0.5 % and 0.6 %)**, and 0.00 and 0.01 s in 5 s
  idle (`ps` resolves 10 ms). A poll costs under 1 % of the 100 ms interval on
  average; the figures are for the architect to set LIM-H6 from.
- **The sweeper's latency.** After `SIGKILL` of the worker, the tool and its
  re-parented sleeper were gone within **42, 58 and 59 ms** in three runs (the bound is LIM-O2 plus
  one poll, 2.35 s).
- **A Mac where `xcrun` never ran (no `xcrun_db`): the demonstration fails.**
  With the cache file deleted first (in both runs), `cargo test --offline` ends `halted`
  (the build fails, exit 101): `xcrun` cannot create its cache in the per-user
  temporary directory (`couldn't create cache file …/xcrun_db-<random>
  (errno=Operation not permitted)`; the profile admits the file by name, read
  only), falls back to `xcodebuild`, which fails loading
  `/Applications/Xcode_26.6.app/Contents/Frameworks/libxcodebuildLoader.dylib`
  (`file system sandbox blocked open()`; the profile admits the developer
  directory and `Contents/SharedFrameworks`, not `Contents/Frameworks`). The
  first `xcrun` run outside the profile creates the file (any shell that runs
  `cc` or `xcrun` once, which every Mac that builds has done). No read-set
  addition was made; the options are the architect's (a create rule for
  `xcrun_db-*` in the per-user temporary directory, or the Xcode's
  `Contents/Frameworks`, or documenting that `xcrun` must have run once).
- **Command Line Tools selected** (`xcode-select -s
  /Library/Developer/CommandLineTools`; informative): the demonstration
  **passes** (`completed{verified}` in 2.4 s) with the same read set, and with
  `xcrun_db` still absent, so the detour through `xcodebuild` that fails above
  is specific to the Xcode layout.

## 8. `xcrun`'s cache, made by the runtime (handoff 011c-6; the decision on §7's finding)

Decided (RFC 011 Amendment 2, implementation decisions, point 8; external
design §13.3): the runtime creates the cache itself, outside the sandbox,
before the first worker starts. Rejected: a write rule for the cache (a tool
could write what the user's unconfined `xcrun` later trusts: an escape) and
adding the Xcode's `Contents/Frameworks` to the read set (it repairs one
fallback of unknown extent and leaves every `cc` call on the slow path).

**What the measurement of §7 left out, found by implementing it** (CI runs
`42382348791`, `42382685206`, `42382906017` and, for the final code, `42383168067`, in `macos-latest`, the runtime's own warming switched off to
try variants): the handoff named `xcrun --find cc`, and the demonstration
**still halted** with a cache made that way. The `cc` the build runs is a shim
that asks `xcrun` for **`clang`**; the cache holds one entry per tool asked
for (with the SDK and the developer directory), and a lookup that is not in it
makes the sandboxed `xcrun` try to write a new cache file, which it cannot.

| Warming (unsandboxed, environment cleared) | Cache | Demonstration |
|---|---|---|
| none | none | `halted` |
| `xcrun --find cc` | 1333 bytes | `halted` |
| `cc --version`, `xcrun --find clang`, `cc -x c -fsyntax-only -`, a compile of an empty program | 1342 bytes | `completed` |
| `--find cc`, `--sdk macosx --show-sdk-path`, `--find clang`, `--find ld` and one more, made **with the runner's environment** | 3478 bytes | `halted` |

So the warming is **`xcrun --find clang` with an empty environment** (the
environment of a host tool is explicit and empty; the last row suggests that
the cache is keyed by it too: the entries made under the runner's own
environment, `clang` among them, did not serve the sandboxed asker, and what in
that environment matters was not isolated). This is a deviation from the handoff's text
(`--find cc`), argued by the table. The remedy line that `status` shows names
the command that was measured to work (`env -i PATH=/usr/bin:/bin
/usr/bin/xcrun --find clang`), not a bare `xcrun --find cc`, which would leave
the user with the cache of the first row.

**Limitation.** The cache holds what has been asked. A build that asks the
sandboxed `xcrun` for a tool the warming did not ask for (for `libtool`, `ar`,
`strip` or `lipo` by name, rather than by the `cc` shim) finds it missing and
fails as in §7, until an unconfined `xcrun` has been asked the same thing with
the same (empty) environment. The demonstration needs only `clang`. The
warming asks for what the demonstration and the `cc` shim need and for nothing
more; extending it is a one-line change per tool, to be made when a toolchain
is found that needs it.

**Command Line Tools selected** (informative, `sudo xcode-select -s
/Library/Developer/CommandLineTools` on the runner only): the demonstration
completes in 3.6 s with no cache and the runtime makes none (the two cache
tests record a skip with that reason).
