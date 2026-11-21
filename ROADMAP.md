# mimlys — Roadmap

**Status:** Authorized by the owner on 2026-10-02 (startup review, decisions D-01 … D-24; D-06 and D-07 in their v2 form). Release 0.7.0 (M1 to M6b) is the first tagged release, 2026-11-03, and was not published; 0.7.1 (the same milestones plus the Linux hardening) is the first release on crates.io; 0.8.0 (M1 to M6c: the macOS backend completes RFC 011) was released on 2026-11-08 (tag `0.8.0`, crates.io, GitHub); 0.9.0 (M1 to M6c plus decisions as native tool calls, RFC 012) was released on 2026-11-10 (tag `0.9.0`, crates.io, GitHub); 0.10.0 (M1 to M7a, RFC 013 part one, plus the fixes of the independent audit) was released on 2026-11-15; 0.11.0 (0.10.0 plus the audit's remainder: A-06 on Windows and A-21 on macOS closed, the audit's smaller findings, and the gaps found in the reviews of 0.11) was released on 2026-11-18, and 0.11.1 (0.11.0 with the WebAssembly engine's security update, Wasmtime 50.0.3) on 2026-11-21; 0.12.0 (0.11.1 plus the user's copies, `task output`, and operation on one machine, M7b: admission per user, the engine gate, the CPU part of clause C, the machine view) was released on 2026-11-21; 0.1 to 0.6 were accepted milestones without a published release. **Replanned on 2026-11-08** (the owner's review of the replanning report): 0.9, the independent audit in parallel, 0.10, then the operation theme's RFC and the daemon or DR-016/017, then 1.0 by the definition below.
Milestones are the unit of planning, release and acceptance. Every requirement in `docs/src/maintainers/requirements.md` names the milestone in which it is verified.

## Principles

- Milestones follow the order of the safety guarantees: control before autonomy, recovery before models, integration last.
- Each milestone is a release `0.<M>.x` of the Cargo workspace on crates.io, published as a whole, with tags that carry no `v` prefix, and a `CHANGELOG.md` entry.
- A milestone is accepted when every requirement mapped to it passes its verification method and its demonstration (external design §23) succeeds.
- The model enters only after the executor is proven with the fake backend; no GPU is on the critical path before M4.
- The platform tier (`core` or `full`) is detected at startup and shown with reasons, never assumed.
- Numbers in the limits register are provisional until the measurement reports of M2 and M4.

## Milestones

| M | Name | Scope | Demonstration | Release |
|---|---|---|---|---|
| M0 | Baseline | Decisions taken; requirements, external design, glossary, limits register, threat register, architecture chapter; this roadmap; mdbook skeleton; README, NOTICE, CHANGELOG; Cargo workspace skeleton with the CI gate on Linux, Windows and macOS (format, lints, tests, dependency audit, license check, `unsafe` rule, core-dependency rule, RFC invariants, documentation build and cross-reference check); RFCs 001–004 proposed | Documents accepted; CI green on the empty workspace on all three platforms | — |
| M1 | Deterministic core | Task state machine; decision schema and strict parsing; twelve-stage validation with authorization before existence; effect model; grants and ceiling; approvals and questions with the rendered prompt; built-in file tools over directory handles; SQLite persistence with write ordering and basic recovery; events and metrics; the fakes (backend, tool, approval, clock, crash injection); the CLI with follow mode; worker IPC skeleton | A scripted task runs create → decide → validate → approve → execute → observe → complete; the process is killed at every persistence point and resumes correctly; an out-of-scope path yields `denied` without an existence leak | 0.1 |
| M2 | Sandbox | The worker process; Wasmtime with fuel, epoch, memory limiter, WASI preopens, coarse clock, empty environment; the JavaScript interpreter adapter; `run_script`, `run_wasm`; termination reporting; the OS-confinement outer layer for the worker where the platform allows, with the probe and tier status; the security suite on all three platforms; the overhead report | An infinite loop, an allocation bomb and an output flood each end with the correct termination on every platform; `mimlys status` shows the tier | 0.2 |
| M3 | Transactional workspace | Content-addressed snapshots; transaction equals action; rollback; promotion with diff, conflicts and backup; unknown-outcome recovery by effect; cancellation restore; symbolic-link tests in both directions; database–snapshot reconciliation; retention and garbage collection; fork | A failed write action leaves the workspace at its pre-execution snapshot; a crash mid-write recovers; promotion refuses a conflict and otherwise asks and applies; symlink escapes fail in snapshot and in restore | 0.3 |
| M4 | Model and context | OpenAI-compatible backend adapter with structured-output modes; context assembly with stable prefix, provenance delimiting and rule-based compression; budgets and convergence end to end; locality policy; usage metrics; the measurement report that fixes the context and prefix limits | A local model repairs a scripted defect in a JavaScript workload within its budgets; the prefix is byte-stable across steps | 0.4 |
| M5 | Integration | The MCP client: stdio children with scrubbed environments, discovery and pinning, definition sanitization, effect declarations, destination validation, output-schema validation, cancellation; rypsok as the first external tool server; user and integrator documentation | `rypsok.web_search` and `rypsok.web_fetch` run under a `network` grant; an ungranted domain is `denied`; `engine_status` is invisible to the model | 0.5 |
| M6a | Host tools, Linux | Confinement backend (Landlock, seccomp, namespaces where available, cgroups or rlimits) meeting every clause of the contract; declared host tools (`cargo test` first); the offline dependency step | `cargo test --offline` of a native project under every mandatory clause; the clause tests pass in CI | 0.6 |
| M6b | Host tools, Windows | AppContainer, Job Object and restricted-token backend; reversible toolchain ACL management; redirected profile and temp paths | The same demonstration on Windows. **Not met in 0.7.0:** `cargo` cannot run inside an AppContainer (the `NUL` device); the clause suite passes with the probe tool. Investigated 2026-11-03 on Windows Server 2022 and 2025: the same refusal, no remedy that keeps F, N and P; an exception without a remedy, revisited if Windows changes | 0.7 |
| M6c | Host tools, macOS | Seatbelt backend, one profile for the worker and its tools; tree termination by sandbox membership (enforced, RFC 011 Amendment 2); memory and process-count watchdogs over the members (best-effort) | The same demonstration on macOS | 0.8 |
| M7a | Operation on one machine, part one | Read-only open of the store; storage view; `task prune`; the book's storage and operation pages (RFC 013 decisions 1 to 4) | A second terminal watches a running task; the storage view sums to what the disk holds; prune frees what it listed | 0.10 |
| M7b | Operation on one machine, part two | Admission per user (LIM-X5), the CPU part of clause C (LIM-H7), the engine gate, the machine view (RFC 013 decisions 5 to 8) | Two runtimes: the second waits and says why; a build stays within its CPU share | 0.12 |
| M8 | Adaptive effort, part one | Effort levels, a backend per level, the fitness probe, plan-first (RFC 015) | The M4 task under each level, measured; the probe classifies the models measured so far | 0.13 |
| M9 | The local daemon | Served mode over a local socket, one scheduler (RFC 014) | Approve from a second terminal; one queue for the user's tasks | 0.14 |

**0.7.1** is a hardening patch between M6b and M6c (RFC 011 Amendment 1, handoff 011d): the Linux backend's clause N and read set, found in the intake review after 0.7.0. It adds no milestone. 0.7.0 is tagged but not published to crates.io; 0.7.1 (2026-11-04) is the first release published there.

## Release 0.8 (scoped 2026-11-07)

| | |
|---|---|
| Objective | Confined host tools on macOS (M6c), which completes the M6 confinement on the three platforms, and the release's open promises |
| In | M6c, the macOS backend (handoff 011c: spike S1, then RFC 011 Amendment 2, then the implementation); removing every Windows trace before an uninstall (handoff 011e, RFC 011 §7's promise, THR-36); the Windows permission lock failing closed instead of losing an entry (handoff 011f, investigation D1, THR-36); the interpreter download bounded and retried (handoff 005b); the fixed instructions saying the goal is the task (handoff 007b); the first slice of tests for requirements and invariants that no test names (REQ-TEST-001, REQ-SEC-004) |
| Out | the M4 demonstration against a real local model and LIM-C1, **moved to 0.9** (owner's decision of 2026-11-08: under the decision envelope no measured model acted, spike S2; the remedy is RFC 012); the independent code audit (after M6c, owner's decision); DR-001 … DR-017; narrowing `/etc` and `/opt`; one HTTP stack instead of two; any remedy for `cargo` on Windows (none exists) |
| Done when | the M6c demonstration and clause suite pass on `macos-latest`; 011e's tests pass on Windows; the book and the README say plainly that no measured local model completed a task under 0.8's decision format, and that 0.9 changes it; every CI job green with skips counted; the threat register's review for 0.8; the CHANGELOG |
| Major risks | Seatbelt facts were unknown in advance: spikes S1 and S1b measured them (2026-11-07), and clause K is enforced on macOS by sandbox membership, so `accept_best_effort` is not needed there; what remains is the implementation's CI rounds and machines unlike the runner (Command Line Tools only, other macOS releases) |
| Next | M6c is the last milestone of the original plan: after 0.8 the architect brings the owner the replanning report (roadmap governance) and the independent audit follows (owner's decision) |

## Release 0.9 (scoped 2026-11-08)

| | |
|---|---|
| Objective | A capable local model finishes a task: decisions requested as native tool calls |
| In | RFC 012 (accepted 2026-11-08): the decision transport, the functions, the strict reply, the context per transport, the budget counting the tools; the M4 demonstration against a real local model (run B, with Qwen2.5-7B-Instruct), which fixes LIM-C1; the book's model example |
| Out | reading a call written into the model's text (RFC 012, rejected); models that do not return structured calls (the Coder variants measured) |
| Done when | run B completes on llama.cpp and Ollama and the M4 report records it; LIM-C1 fixed; prefix stability measured for both transports; every CI job green with skips counted; the threat register's review for 0.9; the CHANGELOG |
| Major risks | engines and chat templates change what a model returns (S2: the same file behaved differently under two templates); one task and three runs per cell are thin evidence, so run B repeats across both engines |
| Next | 0.10; the independent audit runs in parallel with 0.9 |

## The independent audit (in parallel with 0.9; owner's decision of 2026-11-08)

| | |
|---|---|
| Who | a fresh reviewer session given the code at tag `0.8.0`, the specifications, the RFCs and the handoffs, and none of the project's own review conclusions |
| Scope | `mimlys-exec/confine` on the three backends and the sweeper; the validator's twelve stages; approval binding; snapshot and restore; promotion (intake N-03) |
| Record | the reviewer's raw report kept unchanged under `.git-exclude/reviewed/audit-0.8/raw/` with its SHA-256; the architect's triage beside it; the owner decides each finding |
| Next | findings that need code join 0.10; the model adapter and the context assembly are audited after 0.9 lands |
| Retargeted 2026-11-14 | the brief was never run against `0.8.0`; by the owner's decision it audited the code 0.10 will ship, commit `f5a94fa` (`.git-exclude/reviewed/audit-0.10/`). 39 findings plus the architect's six (X-01 to X-06); none refuted. The owner accepted the triage's dispositions on 2026-11-14 |

## Release 0.10 (scoped 2026-11-08)

| | |
|---|---|
| Objective | Hygiene, the audit's findings, and the operator's view of one machine (the bridge of the operation theme) |
| In | The audit's code findings. The hygiene set of the replanning report §3 C: Windows container entries without propagation (an RFC 011 follow-up RFC); one HTTP stack; narrowing `/etc` and `/opt` on Linux; naming the remaining untested requirements; a data-driven hostile corpus; the event-schema and action/observation-schema versions that REQ-MAIN-002 requires and the code lacks (found in handoff 012's review); one `sha2` line (the workspace is on 0.11 since handoff 008c; the remaining 0.10 is wasmtime's cranelift, to follow a wasmtime release); short timing bounds in the test files other than `sweeper.rs`, read by process state instead (handoff 011i's rule). Spike S3 (handoff 011k): one Verus proof, of the Windows permission-lock decision, to measure the cost of formal verification before any wider use (the owner's rule: start minimal). The bridge items: **B1** read commands open the store without the exclusive lock (amends REQ-PERS and external design §14; LIM-O3 unchanged); **B2** a storage view (`status --storage` or `mimlys storage`; external design §19, a new REQ in §21); **B3** `task prune` for terminal tasks, the plan shown first (external design §19.2, a new REQ; it reuses `task delete`); **B4** the book's "Storage" page and "Several projects, several jobs" section; **B5** `status` warns when the engine's context cannot hold the prefix (external design §20). What naming the untested requirements and writing the flow page found in the code (review of 2026-11-12): a rejection's reason and detail reach the model, persisted and counted, and convergence sees the tree (handoff 002c); the next start finishes or undoes a pending snapshot journal (004c); the log's step ids and `debug_prompts` (010c); the test-hygiene follow-up (002d). Each item's handoff lists its REQ and LIM amendments |
| Out | the operation theme's admission, CPU bound and engine gate (its RFC); the daemon; DR-016/017 |
| The audit (2026-11-14) | **Blocks the release:** A-01 and X-02 (the Linux escape through a Unix socket), X-01 (a tool's tree outlives its end), A-02 (the approval card hides arguments), A-03 and A-04 (promotion deletes unlisted files; no way back after a failure), A-08 (an MCP request without a deadline), A-11 (an array destination skips the grant). **Fixed in 0.10:** A-05, A-06 on Linux and macOS, A-07, A-09, A-10, A-13 (a to c), A-15, A-16, A-17, A-18, A-19, A-22, A-24, A-28, A-31, A-35, A-36, A-37, the `HOME` and out-of-root-link items of A-39, X-03, X-04, X-05. Handoffs 011m, 006b, 003b, 009g, 002e |
| Done when | each item's acceptance; every CI job green with skips counted; the threat register's review for 0.10; the CHANGELOG |
| Next | 0.11 (the audit's remainder); then the operation theme's RFC first, then the daemon RFC alongside DR-016/017 (the daemon first if concurrent use proves to be the need) |

## Release 0.11 (scheduled 2026-11-14; scoped 2026-11-16: the audit's remainder, before any new theme)

| | |
|---|---|
| Objective | Close what the audit before 0.10 left open, on a date, so that no accepted residual stays open without one (1.0, condition 2) |
| Scope | Every item re-checked against 0.10.0's code (`.git-exclude/reviewed/scope-0.11/README.md`). Closed by 0.10: A-24, and the `revert_promotion` and `HOME` parts of A-39. Closed by a stated decision: A-29 (c), a server that changes its tools without announcing it (the model sees only the definition pinned at import); the MCP part of A-39, a process a server detaches leaves the group kill (MCP servers are unconfined programs the administrator declares); `read_file` over LIM-W3 stays `too_large` (read it with a host tool) |
| Phase 1, steady and safe | **011n** (Windows and confinement): the Windows mask without `WRITE_DAC`/`WRITE_OWNER`; A-23; the `GetNamedSecurityInfoW` retry under the lock; exit code 259; a closed stdin; handle leaks and alignment; `RLIMIT_AS` in `pre_exec`; A-34; task paths opened without following links; the runtime's deadline margin for a host tool. **009h** (MCP and context): A-12, A-25, A-27, A-29 (a, b), A-30, the dropped content blocks and `setsid` of A-39, one wait for an unanswering server, the stack-margin guard. **006c** (workspace): X-06, the `promote` hang on a named pipe, A-14 (also the reversal), A-38, the Windows directory link, A-20 (modes), `task prune` going on, the halt lines after a failed rollback and a non-permission read error |
| Phase 2, the start gate and the context | **010f**: A-33, and the pin check observing the weakest stop request without halting a task `tool_unavailable`. **009i**: A-26 (a total bound on a result), A-32 (one tool can no longer fail every step) |
| Phase 3, the two residuals | **011o**: A-06 on Windows, a worker per task with a per-task capability, after a spike on the Windows job; a one-time migration at the first start. **011p**: A-21 on macOS, the sweeper outside the sandbox, after a spike of a membership test from outside; if none is reliable, the residual stays, stated as accepted. The spikes run during phase 1 |
| Before the candidate | Found in the reviews of phase 2 and judged by the architect against the owner's philosophy (2026-11-18), **handoff 013c, part 1**: every tool's result bounded as the model reads it (LIM-I19; `read_file` at most LIM-I19, no longer 64 000); a refused creation leaves no workspace, and `prune` removes one an earlier release left; an MCP server's message bounded before it is parsed (LIM-I22); the book says truthfully what is kept for the user. Phase 3 finished within the release (both spikes met no stop condition) |
| Cut point | If phase 3 needs more than the release's time, 0.11 ships after phase 2, and phase 3 is 0.12, dated in the threat register (the owner's decision of 2026-11-16) |
| Done when | each handoff's acceptance; A-21: a test on the macOS job in which a tool signals the sweeper and is refused, and a tool's tree is swept after the worker is killed, or the residual restated as accepted; A-06 on Windows: a test on the Windows job in which a tool of one task cannot open another task's tree; every CI job green with skips counted; the threat register's review for 0.11; the CHANGELOG |
| Next | 0.12: **the user's copies** (REQ-PERS-005: an output stream's whole bounded output written as a blob, and one command that shows a result's or a stream's whole copy), then the operation theme's RFC, then the daemon RFC alongside DR-016/017, as below |

## Release 0.12 (scoped 2026-11-18: the user's copies, then operation on one machine)

| | |
|---|---|
| Objective | Keep and show the user the whole of what a tool returned or printed (REQ-PERS-005), then bound several runtimes of one user on one machine (RFC 013 part two, M7b: 1.0's condition 5) |
| Scope | `.git-exclude/reviewed/scope-0.12/README.md`, every item checked against 0.11.0's code. RFC 013 part two is amended for 0.11's rules (the waits end at the first stop request; a worker per task on macOS and Windows; the registry outside every write set) |
| Phase 1, steady | **004d**: an output stream's whole bounded output written as a blob, redacted as its excerpt is; `mimlys task output <task> [<action>] [--stdout \| --stderr \| --result]` shows a stream's or a result's whole copy; `detail.component` on every `unavailable`. **013d-0**: the measurement spike for LIM-X5 and LIM-H7, and the cost of a task switch with a worker per task, early |
| Phase 2, the operation theme | **013d**: admission per user (the registry, LIM-X5), the CPU part of clause C (LIM-H7), the engine gate (LIM-O12), the machine view, `runtime reap`, the visible fail-open (LIM-O11) |
| Cut point | If the spike or M7b runs long, 0.12 ships after phase 1 and M7b is 0.13, with the later releases moved by one |
| Done when | each handoff's acceptance; two runtimes, the second waiting and saying why; a build within its CPU share; `task output` showing a stream's and a result's whole copy; every CI job green with skips counted; the threat register's review for 0.12; the CHANGELOG |
| Next | 0.13: adaptive effort, part one (RFC 015, M8; scoped below). 0.14: the local daemon (RFC 014, M9). Then the release that prepares 1.0: the contract's inventory, review and freeze (condition 3), a tested example in the book for every command (condition 4), the M4 task with a small pinned model on all three platforms in CI (condition 1), the removal of `config_version` 1, and the dependency refresh before the freeze; then 1.0.0 |

## Release 0.13 (scoped 2026-11-21: adaptive effort, part one)

| | |
|---|---|
| Objective | A task costs what it is worth: three effort levels, each an envelope of budgets on a named backend; a fitness probe that says whether a model acts on this machine; a plan shown before anything acts (RFC 015 with its Amendment 1, M8: 1.0's condition 6) |
| Scope | `.git-exclude/reviewed/scope-0.13/README.md`, every item checked against 0.12.0's code. Amendment 1 reads RFC 015 against that code and the owner's philosophy: a level is an overlay of `backend`, `steps`, `deadline`, `context_chars` and `recent_steps`; a level never changes where a task's data goes; locality is checked against the URL and the profile's rule at every start; nothing is chosen silently; no store migration |
| Phase 1, steady | **015a**: `[backends.<name>]` and `config_version` 2 (version 1 read), `status` per backend, locality checked against the URL and REQ-SEC-010 at every start, `mimlys model probe`. **004e**: readers show a newer release's events and values by name instead of failing (REQ-MAIN-006), and output to a closed pipe ends quietly (REQ-CLI-009), both carried from 0.12. **015b**: `--plan-first`, the `plan` wait, `plan_required`, the plan in `task show` |
| Phase 2, measure | **015-0**, the M8 report on the machine of run B: the M4 task at each level, the swap between a smaller and a larger model, the probe's class for the models measured so far; it sets LIM-E2, LIM-E3 and the `quick` and `deep` envelopes |
| Phase 3, the levels | **015c**: `[effort.<level>]`, `--effort`, `effort_max`, the backend, model and transport per step, the cost in `task show`, `metrics` per level |
| Cut point | If phase 3 runs long, 0.13 ships after phase 2, and the levels move to 0.14 with the daemon after them |
| Done when | each handoff's acceptance; the M4 task under each level, measured; the probe classifies the models measured so far; a 0.12 configuration loads unchanged (one with a non-loopback URL declared `local` refused, saying why); every CI job green with skips counted; the threat register's review for 0.13; the CHANGELOG |
| Next | 0.14: the local daemon (RFC 014, M9). Then the release that prepares 1.0, then 1.0.0 |

## After 0.11 (the themes; their releases set in the scope of 0.12, 2026-11-18)

- **The operation theme** (RFC 013 part two, **0.12**): admission per user (at most a new LIM-X5 executions at once), a CPU bound per host-tool tree (a new LIM-H id; LIM-H6 is the macOS tracking interval), one engine request in flight per endpoint, and the machine view in `status`. Defaults are measured first.
- **The daemon** (RFC 014, **0.14**, before 1.0; superseding DR-002 in part): one daemon per operating-system user over a local socket with a peer-credential check, no network, no multi-user; approvals stay with the human at a client.
- **DR-016 / DR-017**: host paths read and written under grants (their own RFC; not scheduled, and not a condition of 1.0).
- **Adaptive effort** (owner's direction of 2026-11-10, D-E1 to D-E4): RFC 015 part one in **0.13**, before 1.0 (levels as envelopes, a backend per level, the fitness probe, the plan-first gate) after the operation theme; the automatic level (`auto`) and parallel `deep` attempts in a later RFC citing it. Escalation local-only until 1.0.

## 1.0 (definition adopted on 2026-11-08)

1.0 when: (1) the M4 task runs end to end through llama.cpp and Ollama, with native tool calls, on all three platforms in CI with a small pinned model, and a 7B-class local model completes the repair (Linux, run B of the M4 report; amended 2026-11-18, the owner's decision in the scope of 0.12); (2) the independent audit is done and its findings are closed or accepted; (3) the external contract (REQ-MAIN-001) is frozen; (4) the book covers every command with a tested example; (5) the behaviour of several runtimes on one machine is specified, bounded and documented (the operation theme delivered); (6) adaptive effort, part one (RFC 015, M8) is delivered; (7) the local daemon (RFC 014, M9) is delivered (conditions 6 and 7 added 2026-11-18, the owner's decision in the scope of 0.12).

## RFCs, in order

| RFC | Title | Milestone | Records |
|---|---|---|---|
| 000 | RFC lifecycle policy | — | adopted at startup |
| 001 | Task state machine and action lifecycle | M1 | D-02, D-03 |
| 002 | Decision schema and validation order | M1 | D-01, M-01, R-02, M-11 |
| 003 | Effects, grants and approvals | M1 | D-04, D-05 |
| 004 | Persistence, checkpoints and recovery | M1 | D-15, M-09, R-11, R-12 |
| 005 | Execution worker and sandbox profile | M2 | D-10, D-16, D-06 v2 |
| 006 | Workspace snapshots and promotion | M3 | D-08, D-09 |
| 007 | Context assembly and observation pipeline | M4 | D-12, C-07, M-18 |
| 008 | Model backend adapter | M4 | D-21, A-09 |
| 009 | MCP client and tool pinning | M5 | M-06, K-01 … K-07, D-13 |
| 010 | CLI and operator interface | M1 → M5 | D-17, D-18, M-13 |
| 011 | Confinement backends | M6a–M6c | D-06 v2, D-07 v2 |
| 012 | Decisions as native tool calls | 0.9 | M4 report §6, spike S2 |
| 013 (accepted) | Operation on one machine (part one in 0.10: read-only open, storage view, prune; part two after: admission, CPU bound, engine gate, machine view) | 0.10 and after | owner's review of 2026-11-08 |
| 015 (accepted) | Adaptive effort, part one (effort levels, a backend per level, the fitness probe, the plan-first gate; Amendment 1, 2026-11-21) | 0.13 | owner's direction of 2026-11-10 (D-E1 to D-E4) |
| 014 (accepted) | The local daemon (per user and store, local socket) | decided at 0.10's disposition | owner's review of 2026-11-08; DR-002 in part |

RFCs enter `rfcs/proposed/`, move to `rfcs/accepted/` on the owner's sign-off, and to `rfcs/done/` when the release that ships them is tagged (an RFC whose milestones span several releases stays in `accepted/` with a Status line saying what shipped) (RFC 000, five-folder variant).

## Gates per milestone

1. Architect: RFC(s) → owner accepts → handoff under `rfcs/handoffs/NNN-slug/` citing requirement ids and external-design sections, with acceptance checks and expected tests.
2. Dev team: implements and tests from the handoff; returns a review-request package whose entry point cites the RFC, the specifications and the handoff.
3. Architect: review with a risk rating per dimension and a Go/No-Go; a design defect goes back to the RFC first, then to the handoff.
4. Owner: authorizes the release; the changelog, this roadmap and the threat register's review log are updated.

## Not in v1

Daemon mode and sessions; multi-user; plan nodes with dependencies or parallel steps; an MCP server surface; a built-in network adapter; model-based summarization; GUI and additional languages; compiled-language adapters; remote access; vector memory; cryptographic audit integrity; policy simulation; metrics export; Windows aarch64 (see requirements §25).

## Risks

- Host-tool confinement on three platforms is the largest body of platform-specific work; staging it after a cross-platform core and exercising each backend as the worker's outer layer from M2 keeps it off the critical path of M1–M5.
- The portable-core tax (case-insensitive filesystems, Windows path rules, file locking, ACLs) lands mostly in M1 and M3, estimated at 20 to 30 % of those milestones.
- Small local models may not emit reliable JSON without grammar support; the fakes prove the runtime first, and strict parsing with fixed feedback bounds the damage.
- rypsok's schedule is independent; M5 depends only on its published contract.
- Content-addressed snapshots are the largest piece of self-written infrastructure; M3 is isolated so it can slip without blocking M4.
