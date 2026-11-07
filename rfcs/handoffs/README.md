# Implementation handoffs

Handoffs are companion execution documents for RFCs (RFC 000, "Companion handoffs"). Their state is the state of their RFC. Each directory holds:

- `implementation-handoff.md` — scope, module layout, types, behaviours with requirement ids, tests, and what to return;
- `task-breakdown-pr-plan.md` — ordered pull requests with dependencies;
- `acceptance-qa-checklist.md` — what the reviewer checks, keyed to requirement ids.

## Milestone M1 — read and implement in this order

| Order | Handoff | RFC | Depends on |
|---|---|---|---|
| 1 | [001 Task state machine and action lifecycle](./001-task-state-machine-and-action-lifecycle/implementation-handoff.md) | [RFC 001](../done/001-task-state-machine-and-action-lifecycle.md) | — |
| 2 | [002 Decision schema, validation and built-in tools](./002-decision-schema-and-validation-order/implementation-handoff.md) | [RFC 002](../done/002-decision-schema-and-validation-order.md) | 001 |
| 3 | [003 Effects, grants and approvals](./003-effects-grants-and-approvals/implementation-handoff.md) | [RFC 003](../done/003-effects-grants-and-approvals.md) | 001, 002 |
| 4 | [004 Persistence and recovery](./004-persistence-checkpoints-and-recovery/implementation-handoff.md) | [RFC 004](../done/004-persistence-checkpoints-and-recovery.md) | 001–003 |
| 5 | [010 CLI, configuration and the M1 demonstration](./010-cli-and-operator-interface/implementation-handoff.md) | [RFC 010](../done/010-cli-and-operator-interface.md) (M1 part) | 001–004 |

## Milestone M2

| Order | Handoff | RFC | Depends on |
|---|---|---|---|
| 1 | [005 Execution worker, sandbox profile, confinement probe](./005-execution-worker-and-sandbox-profile/implementation-handoff.md) | [RFC 005](../done/005-execution-worker-and-sandbox-profile.md) | 001–004, 010 |

## Milestone M3

| Order | Handoff | RFC | Depends on |
|---|---|---|---|
| 1 | [006 Snapshots, rollback, promotion, lifecycle, fork](./006-workspace-snapshots-and-promotion/implementation-handoff.md) | [RFC 006](../done/006-workspace-snapshots-and-promotion.md) | 001–005, 010 |

## Milestone M4

| Order | Handoff | RFC | Depends on |
|---|---|---|---|
| 1 | [007 Context assembly, compression, redaction, measurement](./007-context-assembly-and-observation-pipeline/implementation-handoff.md) | [RFC 007](../done/007-context-assembly-and-observation-pipeline.md) | 001–006, 010 |
| 2 | [008 The OpenAI-compatible backend adapter](./008-model-backend-adapter/implementation-handoff.md) | [RFC 008](../done/008-model-backend-adapter.md) | 007 |
| 3 | [007b The fixed instructions: the goal is the task (0.8)](./007b-instructions-the-goal-is-the-task/implementation-handoff.md) | RFC 007; finding of L1 | 007, 008 |

## Milestone M5

| Order | Handoff | RFC | Depends on |
|---|---|---|---|
| 1 | [009 The MCP client, rypsok as the first external tool server](./009-mcp-client-and-tool-pinning/implementation-handoff.md) | [RFC 009](../done/009-mcp-client-and-tool-pinning.md) | 001–008, 010 |

## Milestones M6a–M6c

| Order | Handoff | RFC | Depends on |
|---|---|---|---|
| 1 | [011a The Linux backend and host tools](./011a-confinement-linux/implementation-handoff.md) | [RFC 011](../accepted/011-confinement-backends.md) | 001–010 |
| 2 | [011b The Windows backend](./011b-confinement-windows/implementation-handoff.md) | RFC 011 §3, §7 | 011a |
| 3 | [011d Linux hardening and read-set hygiene (0.7.1)](./011d-linux-hardening/implementation-handoff.md) | RFC 011 Amendment 1 | 011a, 011b |
| 4 | [011c The macOS backend](./011c-confinement-macos/implementation-handoff.md) | RFC 011 §4, §5 | 011a, 011d |
| 5 | [011e Windows: removing every trace before an uninstall (0.8)](./011e-windows-cleanup/implementation-handoff.md) | RFC 011 §7 | 011b, 011d |
| 6 | [011f Windows: the permission lock fails closed (0.8)](./011f-windows-permission-lock/implementation-handoff.md) | RFC 011 §7; D1 | 011b, 011e |

Common rules for every handoff:

- The specifications are the source of truth: `docs/src/maintainers/requirements.md` (REQ ids), `external-design.md` (ED §), `limits-and-defaults.md` (LIM ids), `glossary.md`. Tests validate the specification, not the code; every test that verifies a `T` requirement names it in its name or doc comment (REQ-TEST-001), for example `req_task_002_illegal_transition_is_refused`.
- Rust 2024, Rust 2018+ module style (`foo.rs` plus `foo/`), tests in `src/<mod>/tests.rs` (REQ-ENG-003), `unsafe` forbidden (REQ-ENG-001), no I/O or protocol dependency in `mimlys-core` (REQ-ARCH-002; `scripts/check-core-deps.sh`).
- Every number comes from the limits register as a named constant with the `LIM-` id in its doc comment; nothing is hard-coded twice.
- Fixed vocabularies are Rust enums with `serde` renames in snake_case (CLR-6); unknown variants are deserialization errors.
- A design defect found while implementing goes back as a question in the review request, not as a local redesign (project rules).
- The review request goes to `.git-exclude/review-requests/<id>-<topic>/README.md` (the dev team's outbox; the architect's review comes back under `.git-exclude/reviewed/<id>-<topic>/`), with an entry point that cites the RFC, the handoff, and the REQ ids and ED sections implemented, so a reviewer who did not design the feature can check conformance.
