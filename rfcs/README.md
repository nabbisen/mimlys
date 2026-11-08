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
| 011 | [Confinement backends and host tools](./accepted/011-confinement-backends.md) | 2026-10-29 (§1–§3 and §6–§10 shipped in 0.7.0 on Linux and Windows; the macOS backend, §4, implemented on `main` for 0.8; moves to `done/` when 0.8 ships) |
| 012 | [Decisions as native tool calls](./accepted/012-decisions-as-native-tool-calls.md) | 2026-11-08 (for 0.9) |

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

## Archive

| ID | Title | Reason |
|----|-------|--------|
| — | none yet | |
