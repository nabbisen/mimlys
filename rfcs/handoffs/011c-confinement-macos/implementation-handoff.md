# Handoff 011c — The macOS backend (M6c, release 0.8)

**RFC.** [RFC 011](../../accepted/011-confinement-backends.md) §4, §5 and Amendment 1 (the read-set rules) · **Specification.** REQ-CONF-001 … 012 (macOS), REQ-TEST-013; external design §13.2 … §13.5, §23 (M6c); limits LIM-H1 … H5, LIM-O2; threats THR-11, THR-17, THR-23, THR-24, THR-32 · **Milestone.** M6c (release 0.8). Depends on 011a, 011d (done).
**Rewritten 2026-11-07** (the first version was a three-line stub written before any backend existed). It carries what 011a, 011b and 011d taught, in §2.

## Status, fifth update (2026-11-07): 011c-5 merged; 011c-6

011c-5 is on `main` (`ebb4ab9`). Its two questions are decided (RFC 011 "Amendment 2, implementation decisions", point 8 on `xcrun`'s cache, and LIM-H6 fixed at 100 ms; external design §13.3). **011c-6**, one branch on `main`, request `011c-6-macos`:

1. **`xcrun`'s cache made by the runtime:** where the active developer directory is an Xcode (`…/Contents/Developer`) and `xcrun_db` is absent in the per-user temporary directory, the runtime runs `/usr/bin/xcrun --find cc` itself, outside the sandbox, before it starts the worker (and the dependency step's worker), waiting at most LIM-O8 (new constant). If the cache is still absent afterwards, `check` and `status` show one line naming the file and the remedy (`run xcrun --find cc once`), and the run goes on (the tool's own error then names the cause). Never a write rule for the cache in the profile.
2. **Tests on `macos-latest`:** the cache deleted, then the demonstration completes and the cache exists afterwards; the Command Line Tools case unchanged. Mutation: the warming removed (the demonstration halts, as 011c-5 measured). The `check`/`status` line: a test with the warming made to fail (an injected `xcrun` path for tests only, the way the probe tool's hooks are done).
3. **Documents:** the M6c report (§5b's finding and the decision), the book's macOS section (nothing for the user to do; the line `status` may show), CHANGELOG.

Binding for every later mutation run on macOS: **never disable the membership guard** in a mutation (011c-5 §7: with the guard off, the loop treats every process of the runner as a member).

## Status, fourth update (2026-11-07): 011c-1 to 011c-4 merged; 011c-5

PRs #9, #16, #17 and #18 are on `main` (`e7a9a4a`). The answers to their questions are RFC 011 "Amendment 2, implementation decisions" and external design §7.4; read them first. **011c-5**, one branch on `main`, one review request `011c-5-macos`, one commit per item, each with a test naming its requirement and a mutation check:

1. **The sweeper** (implementation decisions, point 6): forked right after the membership verification, before any thread; its own process group; the pipe's write end close-on-exec and held by the worker only; at end of file the kill loop excluding itself, then exit; the worker's loop excludes it. Tests: the worker killed with `SIGKILL` while a tool's re-parented sleeper runs: no member remains within LIM-O2 plus one poll; the same after the runtime's shutdown. Mutations: no sweeper; the write end inheritable (a living tool then holds it, the sweeper never sees end of file, and the first test fails).
2. **Two sandboxes stay apart:** measure, while the main worker and the dependency step's worker both have members, that neither's listing sees the other's (test `req_conf_009_two_workers_do_not_see_each_others_members`). If they do see each other, stop and report: the loop would kill another execution's tools, and the design changes before anything else.
3. **`process_limit`** (REQ-SBX-003, external design §7.4, §7.5): the variant in the core vocabulary (snake_case, as CLR-6), produced by the count watchdog's verdict and mapped to `resource_exhausted{processes}`; `req_conf_001_count_watchdog_stops_a_fork_storm` asserts it instead of `exited 137`.
4. **Measurements, reported, nothing changed by them:** (a) the worker's CPU time while a tool sleeps 10 s under the watchdogs, and the time of one listing and reading on the runner (the architect fixes LIM-H6 from it); (b) the demonstration with `xcrun_db` deleted first (a Mac where `xcrun` never ran); (c) the demonstration with the Command Line Tools selected (`sudo xcode-select -s /Library/Developer/CommandLineTools` on the runner only), informative. A failure in (b) or (c) is reported with the denial that caused it; no read-set addition without the architect.
5. **Documents:** the M6c report (the sweeper, `process_limit`, the measurements; the dead-worker residual becomes the sweeper's residual, as the threat register now says); the book only if a measurement changes what a user declares; CHANGELOG.

Accepted as built: the `(with no-report)` modifier, the physical footprint, the membership guard, the sweep at the tool's own end, `subpath` for files, the read-only `xcrun_db`, the refusal judged against the tool's own declaration, the new refusal on Linux, the `Tier` line in its own test, the unfiltered metadata and the root literal (already in the threat register), the book without the dead-worker residual, the CHANGELOG as written.

## Status, third update (2026-11-07): what each remaining slice carries

- **011c-1 rebase** (then the architect merges PR #9), folding in the answers to its questions: Q1 `/Library/Apple/usr/libexec/oah` in the read set where it exists (the Rosetta job's skips become runs); Q4 the offline profile denies the `mDNSResponder` socket explicitly, after the Unix-socket allows (settled point 9); Q3 the dependency step's three rules wait for 011c-2; Q2, Q5, Q6 accepted as built.
- **011c-2:** the membership kill loop (settled point 6), the two watchdogs (point 7), `(deny sysctl-read (sysctl-name-prefix "kern.proc."))` (point 4), the dependency step's three rules (point 9).
- **011c-3:** the read set of settled point 8 in `app.rs`, the per-user temporary directory found by `confstr(_CS_DARWIN_USER_TEMP_DIR)` in `confine/macos/sys.rs` (the one place for its `unsafe`); the refusal at load of a command whose canonical target leaves the read paths (external design §13.5; the rule is platform-neutral, so the check is in the shared loader with a test on Linux too); the clause suite; the demonstration without `accept_best_effort`. Any read-set addition the full suite asks for is made one at a time, each with the failure that asked for it, in the request.
- **011c-4:** the M6c report, the book's macOS section (no accepted clause needed), the CHANGELOG.

## Status, second update (2026-11-07)

S1b and 011c-1 are reviewed. **RFC 011 Amendment 2 is settled** (its section "Amendment 2, settled"): points 6 to 9 are decided and may be implemented. 011c-2 and 011c-3 may start once 011c-1 is rebased on `main` and merged. Binding changes against §4 below: K is the membership kill loop and is **`enforced (seatbelt_membership)`**, so the demonstration runs **without** `accept_best_effort` and the test hook `MIMLYS_PROBE_BEST_EFFORT=K` leaves the m6c suite; the read set is the measured minimum of the settled point 8; the dependency step's profile is point 9's three rules. The book's macOS section (011c-4) says host tools need no accepted clause on macOS.

## Status (2026-11-07)

S1 is done and reviewed. **RFC 011 Amendment 2 exists.** Its points 1 to 5 are decided: **011c-1 may start now** (the profile generator, the self-application through `dlsym`, the self-check, F, N, P, the process-information and `kern.procargs` rules, the runtime side of point 2). Points 6 to 9 (K by membership, M and C, the read set's measured part, the dependency step's minimum) wait for **spike S1b** (§3b); do not implement them before the architect has marked them settled here. Where §4 below differs from Amendment 2, Amendment 2 wins: notably §4.2's F/K/M/C rows are replaced by Amendment 2 points 3, 6 and 7, and the read set is Amendment 2 point 8.

## 0. How this handoff works

It has two phases, and the second is not binding until the architect has amended RFC 011 from the first.

1. **Spike S1** (§3): measure on `macos-latest` what Seatbelt actually does with the questions below. Report only; no change on `main`.
2. **Implementation** (§4 … §7): the plan the architect expects, written so that you can see the whole shape and estimate it. After S1 the architect writes RFC 011 Amendment 2, which fixes the design where S1 decided it, and marks this handoff's §4 accordingly. Do not start §4 before that amendment exists.

The reason is the record of 0.7: M6b took fifteen CI rounds because Windows facts were found by implementing, and 011d found two facts (the `/proc` walk for clause K, Landlock's ptrace scoping) that changed the design after the code was written. On macOS the uncertain facts are known in advance, so they are measured first.

## 1. Scope

In: the Seatbelt backend for the worker and its host tools, self-applied like Linux; the explicit environment (E, exists); tree termination (K) by the strongest means S1 supports; the memory and process-count watchdogs (M, C); the honest probe; the accepted best-effort path of RFC 011 §5 when K is best-effort; the clause tests and the demonstration on `macos-latest` (arm64) in CI; the M6c report, the book's macOS section, CHANGELOG.

Out: Intel hardware (REQ-PLAT-002: the Rosetta job runs the suites on the arm64 kernel and is informative for confinement); changes to Linux or Windows behaviour; App Sandbox entitlements, code signing or notarization (mimlys is installed by `cargo install`, unsigned); new configuration keys beyond what Amendment 2 decides.

## 2. What the earlier backends taught (binding)

- **Measure under the confinement, never from an unconfined trace** (011d §1.4): start from the smallest profile, run the tool confined, read the failure, add one rule, record the step and its reason.
- **The worker may need more than its tools** (011d DQ-1): on Linux the worker reads `/proc` for its descendant walk while each tool is restricted to the same sets without it. On macOS the same question is whether a second, stricter profile can be applied to a tool (S1 Q2).
- **Other processes are not part of what project code can see** (REQ-CONF-011): on macOS this is `process-info` of other processes, and in particular `sysctl KERN_PROCARGS2`, which shows **another same-user process's environment** (the runtime holds `env:` secrets there, and macOS has no "non-dumpable" against it). The profile must deny it for anything outside the tool's own tree.
- **Paths have one spelling** (0.7.0, Windows): Seatbelt matches resolved paths. `/tmp`, `/var` and `/etc` are links into `/private`; the data directory's default contains a space (`~/Library/Application Support/mimlys`). Every path in a profile is canonical, and every string is escaped (SBPL string literals: `\` and `"`).
- **Fail closed, report what held** (RFC 011 §1): a missing or ineffective `sandbox_init` is `unavailable{seatbelt_missing}` or `unavailable{seatbelt_ineffective}`, never a crash and never `enforced` on faith; the worker reports from what it observes in itself.
- **Every spawned process is bounded** (`tests/common::run_bounded`), the pipe ends are read on threads, the epoch ticker runs by the wall clock (all already true on macOS; keep them).
- **Mutation checks** for every new clause test (011d §3).

## 3. Spike S1 — what Seatbelt does on macOS 26 (report only)

On a branch `s1-macos-seatbelt`: a scratch test binary (or probe-tool commands behind `test-bins`) and a workflow triggered by pushes to that branch on `macos-latest`. Time-box: **four CI runs**. Each answer is a table row with the command, the result, and the macOS build. Do not merge anything.

| # | Question | Why it matters |
|---|---|---|
| Q1 | Does `sandbox_init` (profile string, flags 0) still apply on this macOS, and does a self-test (a read the profile denies) fail as expected? What error does it give when the profile has a syntax error? | the whole backend; the self-check |
| Q2 | Can a process that is already sandboxed apply a **second, stricter** profile to itself, or to a child between `fork` and `exec` (`sandbox_init` again; `sandbox-exec -p` as the child)? Does the child end up bound by both? | whether tools can be tighter than the worker (the Linux two-layer design) |
| Q3 | **Membership.** With `(allow signal (target same-sandbox))` and `(allow process-info* (target same-sandbox))` and everything else about other processes denied: can the worker list and kill a re-parented orphan of its tool (`setsid`, double fork, parent exits, re-parented to `launchd`)? Does a *second* worker that applied the same profile string count as "same sandbox" (it must not see the first's tree)? What do `kill(-1, SIGKILL)` and `proc_listallpids` do inside such a profile? | if membership is reliable, clause K can be `enforced` on macOS, which RFC 011 §4 assumed impossible; otherwise K stays `best_effort` |
| Q4 | Does the Wasmtime JIT (Cranelift, `MAP_JIT` on arm64) run under `(deny default)`, and with which rule, if any? | the sandbox itself runs in the worker |
| Q5 | Does exec'ing a setuid binary from inside the profile gain privilege? The runner has passwordless `sudo`: compare `sudo -n id -u` outside (control: `0`) and inside. Does `(deny process-exec* (file-mode #o4000))` (or the filter Seatbelt offers) stop it? | clause P |
| Q6 | Network: `(deny network*)` with Unix sockets allowed only under the write set and for `socketpair`. Show: a TCP connect to a loopback listener fails; **`getaddrinfo` fails** (DNS through `/private/var/run/mDNSResponder` is a Unix socket and an exfiltration channel); a Unix socket inside the workspace works. For the dependency step's profile: what minimum lets `cargo fetch` reach the network | clause N; the dependency step |
| Q7 | `mach-lookup`: with every service denied, which ones do `cargo`, `rustc`, `cc` (through `xcrun`) and `ld` need for `cargo test --offline` of the demonstration project? List each with what it is. Do **not** add any; the architect decides the allow-list, refusing any service that can reach the network or the user's secrets on the caller's behalf (`trustd`, `nsurlsessiond`, `securityd`, `opendirectoryd` and the like are candidates to refuse) | a mach service can be a hole in N or F |
| Q8 | The read set `cargo test --offline` needs, by the iterative method: start from the declared `~/.cargo/bin`, `~/.rustup`, the worker's directory, the interpreters and `/usr`; record every addition (`/System`, the dyld cache under `/System/Volumes/Preboot/Cryptexes`, `/Library/Developer/CommandLineTools` or the Xcode path the runner uses, `/private/etc/…`) with the failure that asked for it | F; what the book tells administrators to declare |
| Q9 | Inside the profile: does `proc_pidinfo(PROC_PIDTASKINFO)` work on the worker's own descendants (resident size for M), and is `KERN_PROCARGS2` of the runtime refused to a tool? | M; REQ-CONF-011 on macOS |

Return: `.git-exclude/review-requests/S1-macos-seatbelt/README.md` with the table, the raw logs' run ids, and for each question your reading of the answer, marked as measured or inferred.

## 3b. Spike S1b — the four open points (report only)

Same branch rules as S1; time-box **three CI runs**. Each answer measured, with the run id.

| # | Question | Decides |
|---|---|---|
| B1 | Inside the base profile with `(allow signal (target same-sandbox))`: first verify that signalling `launchd` (pid 1) and the parent outside the sandbox both fail `EPERM`; only then send `kill(-1, SIGKILL)` from the worker stand-in, with a tool tree that holds a re-parented orphan. Afterwards: the tree and the orphan are gone, the stand-in is alive, the runner's own processes (the job's shell, the runner agent) are alive. If the pre-check fails, do not send it, and report | Amendment 2 point 6 (K `enforced`) |
| B2 | Is there a listing restricted to the sandbox: `(allow process-info-listpids (target same-sandbox))` or the closest form the profile language accepts (report a parse error as an answer), and does `proc_listallpids` then return only members? | point 7 (M and C over the members) |
| B3 | Q7 and Q8 completed: the iterative loop with the developer directory from `/private/var/db/xcode_select_link` in the seed, to `cargo test --offline` `Finished` and its test passing; then **necessity**: remove each mach service and each home-directory file of the result one at a time (in one job, a loop) and record whether the run still passes | point 8 (the read set and the mach list) |
| B4 | UDP: under `(deny network*)`, does `sendto` of a UDP datagram to a loopback listener fail (the listener sees nothing)? | point 3 (N) |
| B5 | The dependency step's minimum: the smallest addition to the base profile with which `cargo fetch` of the demonstration project (one registry dependency) succeeds; and that its sibling without it cannot resolve a name | point 9 |

Return: `.git-exclude/review-requests/S1b-macos-seatbelt/README.md`.

## 4. Implementation (binding only after RFC 011 Amendment 2)

### 4.1 Module layout

```text
crates/mimlys-exec/src/confine/macos.rs         apply(spec) -> Applied ; the profile from the spec (pure) ; sandbox_init resolved at run time (dlsym), so a removed symbol is a reported state, not a link failure ; the self-check ; the tree tracker and the watchdogs
crates/mimlys-exec/src/confine/macos/tests.rs   unit tests of the profile (escaping, canonical paths, ordering, the network and process rules) and of the watchdog decisions
crates/mimlys/src/app.rs                        the macOS read set (from S1 Q8)
crates/mimlys/tests/m6c_host_tools.rs           the clause suite (cfg macOS), with tests/common::run_bounded and common::skip
```

Every `unsafe` block (the `sandbox_init` call, `proc_pidinfo`, `proc_listchildpids` or `proc_listallpids`) lives in `confine/macos.rs`, each preceded by its `SAFETY:` line (REQ-CONF-010).

### 4.2 Clauses (the expected outcome; Amendment 2 fixes it)

| Clause | Expected primitive | Notes |
|---|---|---|
| F | `seatbelt` | `(deny default)`; the read set read-only and executable, the write set writable; canonical paths; `process-info` and `signal` only for the tool's own tree |
| N | `seatbelt` | `(deny network*)`; Unix sockets only under the write set and unnamed pairs; DNS refused; the dependency step's profile per S1 Q6 |
| E | `explicit_environment` | exists; `__CF_USER_TEXT_ENCODING` already accepted; no `DYLD_*` ever reaches a tool (the environment is explicit) |
| K | `sandbox_membership` (`enforced`) if S1 Q3 shows membership is reliable; otherwise `process_group_walk` (`best_effort`) | the walk: the tool's process group, then the descendants tracked while the tool runs (polled every LIM-H6) and killed even after re-parenting, repeated within LIM-O2 |
| P | `seatbelt` | only if S1 Q5 shows setuid exec gains nothing under the profile; otherwise `unavailable` and a design request |
| M | `rss_watchdog` (`best_effort`) | the summed resident size of the tracked tree above LIM-H2 kills the tree; the execution ends `memory_limit` |
| C | `process_count_watchdog` (`best_effort`) plus the wall clock | the tracked count above LIM-H3 kills the tree. **Not `RLIMIT_NPROC`**: it counts every process of the user, as on Linux (011a §7), so RFC 011 §4's `RLIMIT_NPROC` is to be amended |

LIM-H6 (the tracking interval) is registered as provisional (100 ms); Amendment 2 confirms or changes it from S1.

### 4.3 Tier

If K is `best_effort`, the tier stays `core` unless `accept_best_effort = ["K"]`, and then reads `full (K best-effort, accepted by policy.accept_best_effort)` in `status` and on the `Tier` line of every host-tool approval (exists, 011a §3.5). If S1 makes K `enforced`, the book's "What you may accept" section changes; the architect writes that change with Amendment 2.

### 4.4 Runtime side

The runtime's environment holds `env:`-referenced secrets and macOS lets a same-user process read another's environment through `KERN_PROCARGS2`. A host tool is refused that by the profile (§4.2 F). An *unconfined* same-user program is not, and no macOS mechanism refuses it: state it as a residual in the M6c report; do not try to work around it (removing variables after start changes nothing, RFC 011 Amendment 1, point 2).

## 5. Tests (REQ-TEST-013; `m6c_host_tools.rs`, each names its requirement, each with a control and a mutation check)

The 011a clause suite on macOS with the probe tool: read outside the set; write outside the workspace; TCP to a live loopback listener; `getaddrinfo` of a name; the operator's secret not in the tool's environment; `KERN_PROCARGS2` of the runtime refused; `setsid` sleepers gone after a cancel (if K is `best_effort`, a second test documents the case the walk cannot cover and asserts the state says so); `fork 100` stopped by the count watchdog; resident memory above LIM-H2 (touch the pages: resident, not virtual) ends `memory_limit`; `sudo -n id -u` inside the profile does not print `0` (runner-specific; skip with the reason elsewhere); the honest probe (`MIMLYS_PROBE_FAIL`); accepted best-effort; the dependency step's network and its offline sibling's lack of it. The demonstration: `cargo test --offline` of the native project ends `completed{verified}`, under `accept_best_effort = ["K"]` if K is best-effort, with the approval's `Tier` line asserted.

The Rosetta job runs the same suite as x86_64 code on the arm64 kernel; its results are informative and stated as such.

## 6. Documents

External design §13.3's macOS row and §23 (M6c) as Amendment 2 fixes them (architect); `docs/src/maintainers/m6c-report.md` (the clause table from CI, what S1 measured, the residuals); `docs/src/reference/host-tools.md` macOS section (what to declare, from S1 Q8, the tier line, what `accept_best_effort` accepts if needed); CHANGELOG `Unreleased`.

## 7. Return

Phase 1: `.git-exclude/review-requests/S1-macos-seatbelt/README.md`. Phase 2: `.git-exclude/review-requests/011c-macos/README.md` with changed files, the gate, CI run ids per platform (macOS arm64 and the Rosetta job named apart) with skip counts, the mutation outputs, the clause table, differences from this handoff and from Amendment 2, limitations. Do not tag, bump the version or publish.
