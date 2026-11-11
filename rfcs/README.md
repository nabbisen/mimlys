# mimlys RFCs

RFCs record design decisions and the reasoning behind them. Their lifecycle
(folders, states, numbering, cross-references) is defined by
[RFC 000](./done/000-rfc-lifecycle-policy.md). This directory uses the
**5-folder variant** declared in that policy: `proposed/` → `accepted/` →
`done/`, with `archive/` for withdrawn or superseded RFCs. The folder an RFC
lives in is the source of truth for its state; the Status field inside the
file mirrors it.

Numbering starts at `001` and is never reused. Optional implementation
companions live under `handoffs/NNN-slug/` and inherit the state of their RFC.

## Proposed

| ID | Title | Priority |
|----|-------|----------|
| — | none at the moment | |

## Accepted

| ID | Title | Accepted on |
|----|-------|-------------|
| 013 | [Operation on one machine](./accepted/013-operation-on-one-machine.md) | 2026-11-10 (part one, M7a, in 0.10; part two, M7b, after) |
| 015 | [Adaptive effort, part one](./accepted/015-adaptive-effort.md) | 2026-11-10 (M8, after 0.10) |
| 014 | [The local daemon](./accepted/014-local-daemon.md) | 2026-11-10 (M9, decided at 0.10's disposition) |
| 016 | [Formal proofs with Verus, kept narrow](./accepted/016-formal-proofs-with-verus.md) | 2026-11-11 (0.10: the permission-lock decision; rules for any other proof) |

## Implemented

| ID | Title | Shipped in |
|----|-------|------------|
| 000 | [RFC lifecycle policy](./done/000-rfc-lifecycle-policy.md) | In effect since project startup (2026-10-02) |
| 001 | [Task state machine and action lifecycle](./done/001-task-state-machine-and-action-lifecycle.md) | 0.7.0 (M1) |
| 002 | [Decision schema and validation order](./done/002-decision-schema-and-validation-order.md) | 0.7.0 (M1) |
| 003 | [Effects, grants and approvals](./done/003-effects-grants-and-approvals.md) | 0.7.0 (M1) |
| 004 | [Persistence, checkpoints and recovery](./done/004-persistence-checkpoints-and-recovery.md) | 0.7.0 (M1) |
| 005 | [Execution worker and sandbox profile](./done/005-execution-worker-and-sandbox-profile.md) | 0.7.0 (M2) |
| 006 | [Workspace snapshots, rollback and promotion](./done/006-workspace-snapshots-and-promotion.md) | 0.7.0 (M3) |
| 007 | [Context assembly and the observation pipeline](./done/007-context-assembly-and-observation-pipeline.md) | 0.7.0 (M4) |
| 008 | [The model backend adapter](./done/008-model-backend-adapter.md) | 0.7.0 (M4) |
| 009 | [The MCP client and tool pinning](./done/009-mcp-client-and-tool-pinning.md) | 0.7.0 (M5) |
| 010 | [CLI and operator interface](./done/010-cli-and-operator-interface.md) | 0.7.0 (M1 to M5) |
| 011 | [Confinement backends and host tools](./done/011-confinement-backends.md) | 0.8.0 (M6a to M6c) |
| 012 | [Decisions as native tool calls](./done/012-decisions-as-native-tool-calls.md) | 0.9.0 |

## Archive

| ID | Title | Reason |
|----|-------|--------|
| — | none yet | |
