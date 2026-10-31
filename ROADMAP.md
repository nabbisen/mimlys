# mimlys — Roadmap

**Status:** Authorized by the owner on 2026-10-02 (startup review, decisions D-01 … D-24; D-06 and D-07 in their v2 form).
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
| M6b | Host tools, Windows | AppContainer, Job Object and restricted-token backend; reversible toolchain ACL management; redirected profile and temp paths | The same demonstration on Windows | 0.7 |
| M6c | Host tools, macOS | Seatbelt backend; resident-size watchdog for the memory clause; tree termination by process group, documented as best-effort | The same demonstration on macOS | 0.8 |

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

RFCs enter `rfcs/proposed/`, move to `rfcs/accepted/` on the owner's sign-off, and to `rfcs/done/` when their milestone ships (RFC 000, five-folder variant).

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
