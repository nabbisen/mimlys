# mimlys — Roadmap

**Status:** Authorized by the owner on 2026-10-02 (startup review, decisions D-01 … D-24; D-06 and D-07 in their v2 form). Release 0.7.0 (M1 to M6b) is the first tagged release, 2026-11-03, and was not published; 0.7.1 (the same milestones plus the Linux hardening) is the first release on crates.io; 0.8.0 (M1 to M6c: the macOS backend completes RFC 011) was released on 2026-11-08 (tag `0.8.0`, crates.io, GitHub); 0.1 to 0.6 were accepted milestones without a published release.
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
| Next | the replanning report decides what follows |

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
