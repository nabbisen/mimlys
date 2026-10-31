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
| 001 | [Task state machine and action lifecycle](./accepted/001-task-state-machine-and-action-lifecycle.md) | 2026-10-27 |
| 002 | [Decision schema and validation order](./accepted/002-decision-schema-and-validation-order.md) | 2026-10-27 |
| 003 | [Effects, grants and approvals](./accepted/003-effects-grants-and-approvals.md) | 2026-10-27 |
| 004 | [Persistence, checkpoints and recovery](./accepted/004-persistence-checkpoints-and-recovery.md) | 2026-10-27 |
| 005 | [Execution worker and sandbox profile](./accepted/005-execution-worker-and-sandbox-profile.md) | 2026-10-27 |
| 006 | [Workspace snapshots, rollback and promotion](./accepted/006-workspace-snapshots-and-promotion.md) | 2026-10-28 |
| 007 | [Context assembly and the observation pipeline](./accepted/007-context-assembly-and-observation-pipeline.md) | 2026-10-29 |
| 008 | [The model backend adapter](./accepted/008-model-backend-adapter.md) | 2026-10-29 |
| 009 | [The MCP client and tool pinning](./accepted/009-mcp-client-and-tool-pinning.md) | 2026-10-29 |
| 011 | [Confinement backends and host tools](./accepted/011-confinement-backends.md) | 2026-10-29 |
| 010 | [CLI and operator interface](./accepted/010-cli-and-operator-interface.md) | 2026-10-27 (M1 part implemented; M2–M5 parts open) |

## Implemented

| ID | Title | Shipped in |
|----|-------|------------|
| 000 | [RFC lifecycle policy](./done/000-rfc-lifecycle-policy.md) | In effect since project startup (2026-10-02) |

## Archive

| ID | Title | Reason |
|----|-------|--------|
| — | none yet | |
