# Changelog

All notable changes to mimlys are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions follow
semantic versioning; tags carry no `v` prefix. Before 1.0.0, a change to the
external contract (task states, decision fields, tool names and schemas,
effects, rejection codes, event names, CLI commands, JSON output fields) is
announced here under "Changed".

## [Unreleased]

### Added

- `mimlys`: `mimlys toolchain revoke --all [--yes]` (Windows; REQ-CONF-008, handoff 011e): removes every permission the AppContainer holds, on mimlys's own directories and on the toolchain directories (the record's entries and, from a scan bounded to mimlys's own directories and the declared `read_paths`, the entries the record lost), then deletes the container profile and the record file, so that deleting mimlys leaves the user's directories as they were. One confirmation (`--yes` skips it), a report per entry, `--json` with `{removed, failed, profile_deleted, record_deleted}`, exit 1 with the same object if anything could not be removed (the record then holds exactly what failed). Elsewhere the command says nothing was changed outside the data directory.

### Fixed

- `mimlys-mcp` (REQ-MCP-008): a server's `max_call_ms` is now a floor for the deadline of every one of its tools. Until now only a declared tool's deadline was raised to it; an undeclared tool kept the LIM-O5 default (60 000 ms) even when the server declared a longer maximum, so a server with `max_call_ms = 120000` had its undeclared tools cut off at 60 000 ms. The floor is applied at import, in one place.

- `mimlys-core` (REQ-CAP-008): the workspace sentence of the capability text the model reads is generated from the grant set like the rest of it. It was a constant that said "you may read and write inside the workspace" to every task, a `read_only` one included. A task whose granted tools include none with the `workspace_write` effect is now told it may read inside the workspace and may not write; one with such a tool is told it may read and write, as before. This changes the system message, so the first request of a `read_only` task differs from the previous release's.

- `mimlys-core` (REQ-SEC-011, REQ-ENG-004): a panic in the model adapter or in a tool provider is contained where the runtime calls it, in the runtime's one call site of each, not in each adapter. Until now it unwound through the runtime and ended the process (only the worker's own thread was guarded). A panicking backend is `backend_unavailable` for that request (the request is not repeated and the task halts with that reason, as for an unreachable backend); a panicking tool provider is `host_fault{unavailable}` for that execution. The process goes on and the task can be resumed. One error-level log line (`mimlys_core`, "a panic in an adapter was contained", with the adapter) carries the panic's message redacted of configured secrets, on one line and cut at LIM-I5; no panic text or stack trace reaches an observation, an event or a prompt. `mimlys-core` gains the `tracing` facade as a dependency (no I/O).

- `mimlys-core`, `mimlys-exec`, `mimlys` (REQ-SEC-011, REQ-ENG-004): the containment of a panic in an adapter is complete. Every call the runtime makes into a model backend (`describe`, `locality`, `generate`, `last_usage`) or a tool provider (`definitions`, `imports`, `refresh_pins`, `drain_events`, `execute`) is contained: a panic in a call that decides something is `unavailable` for that component (the request or the step fails, the process and the task go on); one in `last_usage` or `drain_events` loses the figure or the events and the step goes on. A call dropped while it is still running (its task cancelled) drops the adapter's future under containment too. `mimlys` and `mimlys-worker` install a panic hook first thing in `main`: a panic that ends a thread writes one JSON line to standard error, redacted of the configured secrets (once the configuration has loaded), on one line and cut at LIM-I5, instead of Rust's default text with its location; a panic the runtime contains and logs itself is not written twice. Panics in the persistence and the workspace adapter are not contained, by design: the process ends and the next start recovers.

- `mimlys`, `mimlys-exec` (REQ-WS-001, THR-31): the data directory is owner-only from the start. On Unix the data directory, the parents the runtime creates for it and `workspaces/`, `tmp/` and `interpreters/` are created with mode 0700 (given to `mkdir`; before, the data directory was created with the umask's mode, normally 0755, and tightened by the store a moment later, and the three subdirectories stayed 0755 under the 0700 root); an existing directory with another mode is brought to 0700 at every start. On Windows the data directory now gets a protected DACL granting the current user full control and nobody else, inherited by what is created beneath, at every start and before the AppContainer's entries are added to the subdirectories (until now it had whatever its parent's DACL inherited; the default location under the user's profile was private by that, a configured one need not have been). The system and administrator accounts therefore have no entry in it; the owner can always change the DACL. The call is a new function in `crates/mimlys-exec/src/confine/windows/acl.rs`.

### Changed

- `mimlys` (REQ-WS-001): `check` and `status` print one warning line when a configured `runtime.data_dir` is inside the temporary directory (a cleaner of temporary files can delete tasks); the directory is still accepted. The JSON output of both gains the boolean `data_dir_in_temporary_directory`.

## [0.7.1] - 2026-11-04

Release 0.7.1 is the hardening of the Linux backend that the intake review
after 0.7.0 asked for (RFC 011 Amendment 1, handoff 011d). 0.7.0 is tagged and
not published; 0.7.1 is the first release on crates.io.

### Security

- `mimlys-exec` (Linux, REQ-CONF-012): clause N could be defeated by project
  code running in a host tool. A socket created through `io_uring`
  (`IORING_OP_SOCKET`, Linux 5.19 and later) never passes the `socket` rule of
  the seccomp filter, so the filter now always denies `io_uring_setup`,
  `io_uring_enter` and `io_uring_register` (in the dependency step's worker
  too) together with `process_vm_readv` and `process_vm_writev` (another
  process's memory, which the denial of `ptrace` did not cover). Where
  Landlock's ABI is 4 or later (Linux 6.7) and network is denied, the ruleset
  also handles TCP `bind` and `connect` with no rule, a second layer under the
  filter; `status` then reads `enforced (seccomp+landlock_net)` for clause N,
  `enforced (seccomp)` on an older kernel.
- `mimlys`, `mimlys-exec` (REQ-CONF-012): the runtime and the worker mark
  themselves non-dumpable first thing, so another program of the same user
  cannot read their memory, environment or descriptors through `/proc`
  (`env:`-referenced secrets sit in the runtime's environment). Cost: no core
  dump of either, and no `ptrace` of either as the same user.
- `mimlys-exec` (Linux, REQ-CONF-011): the processes of the machine left the
  read set. `/proc` is no longer readable by a host tool: its command lines
  and network tables stay out of project code. The worker keeps `/proc` for its
  own descendant walk (clause K on the `subreaper` rung); a host tool's process
  restricts itself to the same sets without it before it runs, as a second
  Landlock layer. (Landlock's ptrace scoping already refused another process's
  `environ`, `maps` and `fd`; the read set was where `cmdline` and the rest
  leaked.)
- `mimlys-core`, `mimlys` (REQ-CONF-011, LIM-H5): a declared read path may not
  expose credentials. A filesystem root, the home directory, an ancestor of it,
  or a directory that directly holds a name of LIM-H5 or LIM-W5 (`~/.cargo`
  holds `credentials.toml`) is refused when the configuration loads and by
  `toolchain grant`, naming the entry found and the narrower path to declare.

### Changed

- **A configuration that followed the 0.7.0 book fails to load**: write
  `read_paths = ["~/.cargo/bin", "~/.rustup"]` instead of `["~/.cargo",
  "~/.rustup"]` (and `mimlys toolchain grant %USERPROFILE%\.cargo\bin` on
  Windows); and a dependency step is declared `effect = "external_read"` with
  `network = true`, never `workspace_write` (REQ-CONF-006, CLR-1: `external_read`
  exactly when `network = true`; any other pairing is an error naming the
  declaration and the value to write). `effect_view()` no longer derives the
  effect from `network`.
- **A host tool's `PATH` and a named `command`** are built from the declared read
  paths that are themselves a `bin` directory (`~/.cargo/bin`) or hold one,
  then `/usr/bin:/bin` (Linux and macOS code; Windows keeps its rule), unless the
  declaration's `env` sets `PATH`. Before, `<read path>/bin` was appended to
  every read path, so the `~/.cargo/bin` spelling of the book left `rustc` off
  `PATH` on a machine without a system `rustc`.
- A host tool offline that needs a file outside its read set fails with the
  file's name (the toolchain's own message); the book now shows the message and
  the remedy, how to supply dependencies offline (vendoring; the read-only
  `~/.cargo/registry`, which works only for crates already unpacked) and what
  non-dumpable costs.
- Test infrastructure: `common::skip` records a skipped test in
  `skips.log` beside the test output (libtest has no skipped state); the probe
  tool gains `uring-socket`, `uring-connect`, `vm-read`, `hold` and `read-proc`
  behind the `probe-syscalls` feature of `mimlys-exec`, enabled by
  `mimlys/test-bins`; a debug build honours `MIMLYS_PROBE_NO_CGROUP` (the
  worker skips the cgroup rung, so clause K's `subreaper` rung is testable on a
  machine that delegates a cgroup).
- Platform matrix (REQ-PLAT-002): macOS x86_64 is built and tested under Rosetta 2 on the arm64 runner (a CI job of its own); Intel-hardware confinement behaviour is stated as untested, and no Intel runner image is used (they end in 2027). CI also runs on Linux aarch64 (`ubuntu-24.04-arm`), counts skipped tests per platform, builds on the minimum Rust version, dry-runs the workspace publish and pins its actions by commit.
- The interpreter recipe now rebuilds the pinned QuickJS-NG byte for byte (wasi-sdk 29, as upstream's release job; the wasi-sdk package pinned by SHA-256 and the source by commit): the scheduled check had failed since its first run because the recipe used wasi-sdk 25.
- The Windows `cargo` demonstration is an exception without a remedy: the refusal of the `NUL` device to an AppContainer is the same on Windows Server 2022 and 2025, and no way that keeps clauses F, N and P was found (external design §23).
- Document amendments of 2026-11-03 (the architect's): REQ-CONF-011 and
  REQ-CONF-012, LIM-H5, THR-36 and the review-log row; the Windows `NUL`
  exception recorded in external design §23 and the ROADMAP's M6b row; RFC 011
  Amendment 1; RFCs 001 to 010 moved to `rfcs/done/` as implemented in 0.7.0;
  the rollback wording of external design §7.2 and REQ-TOOL-010 (paths excluded
  by LIM-W4 and LIM-W5 are neither snapshotted nor restored); REQ-PERS-009's
  wording on collection; this changelog's statement that 0.7.0 is not
  published.

## [0.7.0] - 2026-11-03

The first tagged release (not published to crates.io): milestones M1 to M6b. Releases 0.1 to
0.6 were accepted milestones that were never tagged or published; their
content is recorded here under 0.7.0.

### Added

- `mimlys-exec`: the Windows confinement backend of RFC 011 §3 (handoff 011b): the worker started suspended inside an AppContainer per installation (no network capability; the dependency step's worker with `internetClient`), a Job Object with kill-on-close, breakaway forbidden and the LIM-H2/LIM-H3 limits, under a restricted low-integrity token where the system accepts it with the container; every host tool in a nested job killed as one; the worker's self-report from its own token and job; the access-control entries of `confine::windows::acl`; `ConfinementSpec.data_dir`; `probe_result.cgroup`.
- `mimlys`: `mimlys toolchain grant <path> [--yes]` and `revoke <path> [--yes]` (Windows: the container's read permission on a toolchain directory, confirmed, recorded under `<data>/toolchain-acls.json`, reversed exactly; elsewhere they say nothing is needed); `toolchain list` and `status` show whether each path carries its permission and name a tool unavailable with `acl_not_applied: <path>`; a `host_tool` grant for such a tool is refused with the reason; the write set is the workspaces and `<data>/tmp` on every platform; the M6b test suite (`tests/m6b_host_tools.rs`, Windows) and the book's Windows section (handoff 011b).
- `mimlys-exec`: the Linux confinement backend of RFC 011, applied by the worker to itself before its first frame from a specification the runtime passes: a delegated cgroup (`memory.max`, `pids.max`, `cgroup.kill`) or the fallbacks, the worker as child subreaper with a `/proc` walk that terminates every descendant within LIM-O2, `no_new_privs`, a seccomp filter (no sockets outside `AF_UNIX`; no `ptrace`, `mount`, `keyctl`, `bpf`, `unshare`, `setns`, module or kexec loads), Landlock at the kernel's ABI with the read and write sets and abstract Unix sockets scoped; every clause reported with its primitive; `run_host_tool` executed in the worker (workspace as working directory, explicit environment, streams captured to LIM-H4, `RLIMIT_AS` per tool where no cgroup bounds memory); the worker record and the reaping of a previous runtime's tree (handoff 011a).
- `mimlys-core`: `HostToolDeclaration` with the argument rules of RFC 011 §6; `ClauseState` with the primitive; `TierReport` with accepted best-effort; `policy.accept_best_effort` and `policy.host_tools`; the denial `argument_not_allowed`; the event `runtime.orphans_killed{count}`; limits LIM-H1 … H4 as constants (handoff 011a).
- `mimlys`: `[host_tools.<name>]` (a declaration admits the name to the ceiling), `[policy] accept_best_effort`, the probe from a real worker, the dependency step's worker, `mimlys toolchain list`, `status` with the primitives, host tools and the read set; the probe tool and the M6a tests (every clause inside a host tool, cancellation of a `setsid` tree, the honest probe, the argument rules, the dependency step, accepted best-effort, orphans) and the demonstration `cargo test --offline` of a native project with a path dependency ending `completed{verified}`; the book chapter "Host tools: fixing a failing test" (handoff 011a).

- `mimlys-mcp`: the MCP client of RFC 009 over the pinned SDK (`rmcp =3.5.0`, rypsok's version): servers as child processes with an explicit environment and secrets by reference, in their own process group or Job Object, started at the first need within LIM-O6, restarted at most LIM-X3 times per task, ended at exit; discovery and import under bounds (descriptions sanitized and cut to LIM-I2, the flat parameter model, LIM-I3/LIM-I4, output schemas kept for validation, annotations recorded and never used, unsupported constructs unavailable with a named reason); effects from `[servers.<name>.tools.<tool>]`, undeclared tools `external_write`; pinning per task with `tool.pinned`, a changed definition unpinned at the next boundary with `tool.unpinned` and named to the model (`feedback kind="tool_unavailable"`); calls with deadlines, `notifications/cancelled` and the kill within LIM-O2; results validated against the pinned output schema, `isError` mapped to `tool_error{kind}` with the error object as payload, strings sanitized and bounded; the scripted test server behind the `test-server` feature serving rypsok 0.7.0's tools from its committed transcripts (handoff 009).
- `mimlys-core`: the bounded JSON-Schema validator (`schema`), `ImportRecord` and `TaskVersions.tool_imports`, `ResourceProfile::External`, the events `server.started` and `server.stopped`, the provider hooks `imports`, `refresh_pins` and `drain_events`, limits LIM-O5 and LIM-O6 as constants (handoff 009).
- `mimlys`: `[servers.<name>]` with `command`, `args`, `env_allow`, `env_secrets`, `startup_ms`, `max_call_ms` and `[servers.<name>.tools.<tool>]`; a declaration admits the tool to the ceiling; `check` prints the admitted external tools with their source and warns about undeclared ones; `status` lists every server with its tools, the unavailable ones and their reasons; the book chapter "External tools: rypsok"; the M5 tests and the demonstration behind `MIMLYS_RYPSOK_BIN` (handoff 009).

- `mimlys-core`: context assembly of external design §15 as a pure function of the persisted history: nine segments in one order with segments 1–4 byte-stable across a task's requests, the compressed `history` feedback segment, the structural compression rules (repeated lines, fixed step summaries within LIM-C4, batches of LIM-C3 dropped) with the retained set, `over_budget`, the redactor for configured secrets applied to observations before persistence with the `secret_redacted` notice, the per-step measures (`context_chars`, `chars_before_fit`, `compressed_steps`, `dropped_steps`, `over_budget`, `prefix_chars`, `model_ms`, `usage`) and the derived `context` metrics; limits LIM-C1, LIM-C3, LIM-C4, LIM-M4 as constants (handoff 007).
- `mimlys`: `[backend] context_chars`; secrets resolved once at load and refused under eight characters; every log line redacted; `metrics` prints the context and model measures (handoff 007).
- `mimlys-model`: the OpenAI-compatible chat-completions adapter (RFC 008): one request per decision with the messages unchanged, the deadline, the completion bound, the structured-output modes `auto` (resolved at the probe: `json_schema`, then `json_object`, then none), `schema` and `none`, a bounded read, usage in OpenAI's and llama-server's field names, a bearer token from the configured reference; the startup probe (model listing and mode within LIM-M4); a scripted mock server behind the `mock-server` feature for tests (handoff 008).
- `mimlys-core`: `BackendDescriptor{kind, model?, locality}` from every backend, recorded in `runtime.started` and on each task's versions; the profile key `external`; a remote backend refused at task creation for a profile without it (handoff 008).
- `mimlys`: `[backend] temperature`; the backend probe before step-running commands and `status`; `status` with the backend's probe findings; the M4 demonstration against the mock server and, behind `MIMLYS_M4_BACKEND_URL`/`MIMLYS_M4_MODEL`, against a local engine; the M4 report (handoff 008).

- `mimlys-core`: snapshot manifests, diffs and the symbolic-link rule as pure data; the `WorkspaceProvider` contract for snapshots, restore, promotion, fork and deletion (files in the provider, rows and events in the runtime); the loop's transaction per write action (a `before` snapshot with the authorization, an `after` snapshot on success, a restore before the observation on failure and, unless `on_cancel = keep`, on cancel; a failed restore halts `unknown_outcome`); the quota from the latest snapshot; the control verbs `rollback_workspace`, `promotion_plan`, `promote_workspace`, `revert_promotion`, `delete_task`; fork with copied grants and the `history` segment (LIM-C5); the footprint (handoff 006).
- `mimlys-persist`: migration 002 (`snapshots`, `objects` with reference counts, `workspaces`, `promotions`); the object and manifest stores with fsync and the journal that the next start finishes or undoes; `record_snapshot` with the counts in the same transaction; `delete_task_data`; CONS-04 with the reference-count rule; recovery restores a write found running; crash points `snapshot`, `promotion`, `delete` (handoff 006).
- `mimlys`: every-file hashing, restore that makes the tree equal to the manifest, two-phase promotion with conflict refusal, backup, temp-and-rename writes, unified diffs and reversal, fork materialization, deletion of released files; `rollback`, `promote [--yes] [--summary] [--patch]`, `task delete [--audit] [--yes]` with the plan shown, `task new --from-task [--from-snapshot]`; the footprint in `task show` and `task list` (handoff 006).
- `mimlys-exec`: the Wasmtime sandbox pinned at 50.0.0 with three independent guards (fuel, epoch deadline, memory limiter) and the exact termination mapping of external design §7.4, the WASI boundary (`/workspace` by mode, an ephemeral `/tmp`, declared args and stdin, empty environment, bounded streams that end an execution `output_limit`, clocks at LIM-S6, a replayable random source, no sockets), the worker protocol's framing and the worker's entry point, the runtime side of the worker (lazy start bounded by LIM-X4, one execution at a time, cancel within LIM-O2, death as `host_fault{worker_died}`, restarts up to LIM-X3 per task), `run_script` and `run_wasm` with `mode` deriving the effect, pinned interpreter builds verified by SHA-256 (JavaScript: QuickJS-NG v0.17.0), the confinement interface with the probe and the portable clauses (E and K on every platform through an explicit environment and a process group or a Job Object; P on Linux through `no_new_privs`), the security suite of REQ-TEST-008 (handoff 005).
- `mimlys-core`: the confinement vocabulary (`Clause`, `ClauseState`, `Probe`, the tier rule) and the probe in the runtime's status; limits LIM-S5 … S7, LIM-X3, LIM-X4, LIM-O2 as constants (handoff 005).
- `mimlys`: the `mimlys-worker` binary, `interpreters list|install`, `[interpreters.<language>]`, the `[sandbox]` values in force, the clause table and the engine version in `status`, the M2 demonstration tests (an infinite loop ends `fuel_exhausted` or `deadline`, an allocation bomb `memory_limit`, a print flood `output_limit`; `run_wasm` from the workspace; a read-write module writes only inside the workspace; the honest probe) and the overhead report (handoff 005).
- `scripts/check-unsafe.sh`: `unsafe` only under `crates/mimlys-exec/src/confine/`, each block with a `SAFETY` justification (REQ-CONF-010).
- M0 baseline documents under `docs/src/maintainers/`: requirements (214
  identified requirements with milestone and verification), external design,
  glossary, limits and defaults, threat register, architecture overview.
- `ROADMAP.md` with milestones M0 to M6c and the RFC order.
- RFC 000 (lifecycle policy) adopted; RFCs 001–005 and 010 accepted (2026-10-27); RFC 006 accepted (2026-10-28); RFCs 007, 008, 009 and 011 accepted (2026-10-29).
- M1 implementation handoffs under `rfcs/handoffs/` for RFCs 001–004 and 010.
- `mimlys-core`: identifiers, clock, canonical hashing, the task and action state machines, counters and budgets, failure causes and dispositions, the event vocabulary (handoff 001; 44 specification-derived tests).
- `mimlys-core`: decision schema v1 with strict parsing, tool definitions and the flat schema model, lexical path canonicalization with platform rules and case modes, the twelve-stage validator with the sealed `AuthorizedAction`, observations with sanitization and excerpts, context segments, the model-backend and tool-provider SPI (handoff 002).
- `mimlys-exec`: the six built-in file tools over `cap-std` directory handles with deadlines and cancellation (handoff 002).
- `mimlys-model`: the fake backend with script files (handoff 002).
- `mimlys-core`: effect declaration rules and the derivation function, capability grants and the grant set with host-path and destination matching, the policy ceiling with the shipped profiles and exclusion matching, the validator hooks over them, approval requests with runtime rendering, hash binding, one-shot consumption and expiry, questions, and the completion rule (handoff 003).
- `mimlys-core`: the `Persistence` interface with the eight ordered write methods, task versions recorded at creation, plan versions, and the pure recovery plan (handoff 004).
- `mimlys-core`: the runtime: the step loop over the SPI (boundary checks, the M1 context, one model request per step with LIM-M3 retries, the twelve-stage validation, the eight ordered persistence lines, approval continuation in the same step, the completion check as a runtime-proposed action, dispositions and retries, counters and halts, cancellation tokens) and the control API (create with scope-overlap refusal, resume with LIM-X1, pause, cancel, approve, reject, answer, grant, revoke, task view, list, events, metrics, status); in-memory fakes for the backend, the workspaces and the persistence (handoff 010; 25 runtime tests).
- `mimlys`: the command line of external design §19.2 with `--json` on every command and exit codes 0/1/2; the TOML configuration with `config_version`, refused unknown keys, secrets by reference, defaults from the limits register and stated ranges; `mimlys check`; the approval and question prompts from the runtime's rendering; follow mode with pause and cancel on stop signals within LIM-O1; local workspaces materialized by copy with LIM-W4/LIM-W5 exclusions, the symbolic-link rule, the size refusal before writing and the case probe; JSON logs on standard error; the message catalog; the `mimlys-testloop` harness and the M1 demonstration tests (the scripted task with approval, `verified` completion, the existence-leak check, questions, rejection, the environment allow-list, crash injection at every persistence point) (handoff 010).
- Book chapters for 0.1: getting started, decisions and tools, configuration, command line.
- `mimlys-exec`: the worker protocol's message types (round-trip only; the worker arrives in M2) (handoff 010).
- `mimlys-persist`: the SQLite store with WAL and synchronous FULL, owner-only files, the runtime lock, forward-only migrations, one transaction per transition with the event inside it, write-ordering enforcement, blobs written and fsynced before their rows, events and queries, the consistency rules CONS-01 … CONS-08 at startup, recovery application with expiry of waiting items, and crash injection behind the `crash-injection` feature with a self-spawning test at every point (handoff 004).

### Changed

- `mimlys-exec`: a `host_fault{worker_died}` is logged at `warn` with how the worker ended (its exit status, or that it did not answer within LIM-O2); clause E's unavailability names the inherited variables (names only); on macOS `__CF_USER_TEXT_ENCODING`, set by the system's libraries inside the process, does not count as inherited; the worker's phase trace (`--trace`, passed at log level `debug`, forwarded at `debug`); `confine::windows::acl::has_entry` on a directory without explicit entries; CI runs every test binary (`--no-fail-fast`) with a one-hour job limit; the Windows worker's environment carries `TEMP`, `TMP`, `USERPROFILE`, `LOCALAPPDATA` and `APPDATA` inside `<data>/tmp` as §13.3 states (AppContainer creation refused the block without them, `ERROR_ENVVAR_NOT_FOUND`); a failed wait on a Windows child is an error, not a spin, and the wait after termination is bounded by LIM-O2 with a `warn` naming a process that survives; the worker's pipes hold 4 MiB on Windows; the probe drains the worker's streams on threads and the timed wait decides (a worker that neither writes nor exits is `worker probe timed out` with its CPU times and standard error, never a blocked runtime); `Sandbox::with_opt_level` for the test suite's diagnosis; the epoch ticker advances by the wall clock (a stretched sleep delays a tick but never loses one, so a deadline of N ticks is N × LIM-S7 of wall time); the mock model server sets accepted sockets blocking (macOS inherits the listener's flag); debug builds optimize the Cranelift crates so a worker start stays well within LIM-X4 under parallel tests; the Windows backend keeps the parent's ends of the worker's pipes as synchronous files (`confine::windows::PipeEnd`, read and written with `ReadFile` and `WriteFile`), never as the standard library's child-stream types, whose alertable overlapped I/O stalls on an anonymous pipe handle after its first transfers (the cause of every Windows worker failure in CI); every suite runs the binaries under `tests/common::run_bounded`, so a hang fails one test with its output instead of holding the job to its limit; a host tool's captured streams are read to their end within LIM-O2 of the process's end and the reader is then left to finish on its own, so a survivor of a termination holding the pipe delays nothing; `status --json` lists each host tool as `{name, command, network, unavailable?}`, as the text output already did; `.gitattributes` checks out one line ending everywhere; `toolchain grant` and `revoke` keep the directory's inherited entries and its protection flag (the DACL write names `UNPROTECTED_DACL` or `PROTECTED_DACL` as the directory had it), and the writers of one directory's DACL hold a named mutex per path across the runtimes of a machine (two installations granting their containers the worker's directory at once lost one another's entry); a host tool that does not start carries the reason in the notice `host_tool_not_started{reason}`; the epoch deadline is armed past the ticks the ticker owes (delivered at once after a stretched sleep) plus one, so an execution never ends before its deadline and at most two ticks after it; `cargo` cannot run inside an AppContainer on current Windows (the `NUL` device is refused), which the book and the M6b report state and the demonstration test reports instead of claiming; a host tool's execution deadline is its declaration's `timeout_ms` (the gateway definition's LIM-H1 applied to every declared tool, so a declared 10 s let a tool run to its end); the DACL merge of `grant` and `revoke` starts from the directory's explicit entries alone, the system recomputing the inherited ones (`SetEntriesInAclW` copies an inherited entry as an explicit one); the permission record names a directory in one spelling (canonical, without the verbatim prefix, compared without regard to case on Windows), so `revoke` recognises mimlys's own directories under the short form the data directory may be configured in and refuses them, and the commands show the path without `\\?\`. The first CI runs on three platforms (2026-10-31 to 2026-11-03).
- Minimum Rust 1.97 (REQ-PLAT-001 and `rust-version`, from 1.90): the effective minimum since Wasmtime 50; accepted with the design review of 0.6.
- Wasmtime and `wasmtime-wasi` pinned at `50.0.1` (from `50.0.0`; REQ-ARCH-007): the patch fixes RUSTSEC-2026-0420 (wasip3 UDP `send`), RUSTSEC-2026-0422 (typed functions with `i31` or `nullref` arguments), RUSTSEC-2026-0423 (cross-engine struct-type confusion) and RUSTSEC-2026-0424 (borrowed resources between components); none is reachable from mimlys (wasip1 only, no GC types, one engine, no components, no sockets), and no feature is gained. The engine version in `status`, `probe_result` and the records reads `50.0.1`.
- Design review of 0.6 (2026-10-31): `run_host_tool` is visible through a `host_tool` grant alone and is never a `tool` grant or a `tools` entry (`check` and `grant` refuse it with the rule; `approval_tools` may still name it); profiles gain `host_tools` (declared host tools granted at creation). `host_read` and `host_write` grants are reserved until a tool carries their effect: `grant`, `--grant` and the prompt refuse them with the reason, and non-empty `policy.ceiling.host_read_roots`/`host_write_roots` are refused (DR-016, DR-017). Host-tool declarations lose `host_write` and `reversible`: a host tool writes only inside the workspace in this release. Every host-tool approval carries a `Tier` line. `denied` rejections carry one hint per `reason` and, for `argument_not_allowed`, `argument`, `value` or `max` in the detail (`promote_not_allowed` and `argument_not_allowed` join §6.4). The Linux cgroup rung is tried control by control and host tools run in a `tool` sub-cgroup killed as one (K `cgroup_kill` where the session lets the worker create a cgroup, as on a desktop); the worker's cgroup is recorded, removed at its end and swept at the next start. `status` shows each clause by letter and word. A command that would run a step with the fake backend and no `script` is refused up front instead of halting `backend_unavailable`. INV-13 and REQ-SEC-006 state the dependency step's exception; THR-35 added. `WorkspaceProvider` is the SPI's name for the baseline's `SnapshotStore`.
- The worker's protocol carries the core's `ClauseState` (with `primitive`); `status` prints `enforced (landlock)` instead of `enforced`; `runtime.started{tier}` and the status tier follow `policy.accept_best_effort` (RFC 011).
- External design §13 rewritten for the self-confining worker; §19.1's `cargo_test` example is `workspace_write` (it writes only `target/` inside the workspace), and a `cargo_fetch` dependency step is shown.
- The `test-server` feature of the `mimlys` crate is `test-bins` (it builds the MCP test server and the probe tool).
- Event vocabulary: `server.started{server, version?}` and `server.stopped{server, reason}` added; `status` JSON gains `servers`; the task's `versions` gain `tool_imports` (RFC 009).
- REQ-MCP-004: `reversible` is not declared for external tools (always false); a declaration admits the tool to the ceiling (RFC 009 §4).
- `status` JSON: `backend_kind` and `backend_locality` are replaced by one `backend` object `{kind, model?, locality, reachable, model_listed?, structured_output?, fell_back, detail?}`; the text line shows the same. `runtime.started` gains `backend`; the task's `versions` gain `backend` (RFC 008).
- `[policy.profiles.<name>] external` is a new key (default false); a `remote` backend now also needs a profile with `external = true` (REQ-SEC-010).
- Follow mode no longer refuses the `openai_compatible` backend (the M1 stopgap is gone).
- Ownership instead of collection (RFC 006 §6): nothing expires and nothing is swept; a terminal task is deleted by the user with `task delete`; `gc` never shipped and leaves the command set; `[workspace] retention_days` is removed (an unknown key naming `task delete`); REQ-WS-009, REQ-WS-012, REQ-PERS-010 and REQ-OBS-005 amended; LIM-P2 … LIM-P4 carry no timer.
- Event vocabulary: `task.deleted{audit}` added.
- Shipped profiles `workspace` and `project` no longer list `run_script` and `run_wasm` in `approval_tools`: rollback exists (RFC 006 §9 ends the M2 stopgap of RFC 005 §11).
- Limits register: LIM-X4 (worker start, including interpreter compilation) added; the memory limiter traps the guest at `memory.grow` instead of returning -1, so an allocation bomb ends `memory_limit` rather than an ordinary out-of-memory exit.
- Event vocabulary (external design §17.1): `execution.finished{attempt, termination}` is the event of persistence line 6 (one per execution, from the tool actor); `action.finished{status, cause?}` is emitted once, at line 7. Metrics no longer count an action twice (handoff 010).
- Approvals (external design §10.2): a grant issued or an unrelated grant revoked while an approval is pending withdraws the request and asks again under the new grant set, so the user is never asked to approve under grants that no longer apply; revoking a grant the action uses rejects it `denied` (handoff 010).
- Task state machine: a resume that would halt again at once (deadline passed, step budget used up) is refused with a message naming the extension to give; `task.pause_requested` is recorded as an event (handoff 010).
- Validator SPI: the budget-reservation hook receives the canonical arguments, so a write larger than the remaining workspace quota is refused before execution (handoff 003).
- External design §8: notice `entries_skipped{reason, count}` added for listings and searches; §11: `write_file` creates missing parents and reports `parents_created`, `list_dir` lists symbolic links as type `symlink`, `read_file` fails with `line_out_of_range` past the end, `search_files` reports skipped entries and per-match truncation; §6.3 stage 7 names absolute paths explicitly.
- External design §7.1: an action that is `proposed`, `waiting_approval` or `authorized` is also cancelled when its task is cancelled.
- Cargo workspace skeleton (`mimlys-core`, `mimlys-persist`, `mimlys-exec`,
  `mimlys-model`, `mimlys-mcp`, `mimlys`) and the CI gate on Linux, Windows
  and macOS.
