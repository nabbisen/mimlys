# Handoff 006 — Acceptance and QA checklist (M3)

- [ ] REQ-WS-004: snapshots at base, before every write-effect action, after every successful one; objects and manifests fsynced before the row; identical manifests share a row.
- [ ] REQ-WS-005 / THR-27: links stored as entries, never followed by snapshot, restore or promotion; an escaping target refused at restore with `symlink_omitted`; both directions tested.
- [ ] REQ-WS-006: restore deletes files absent from the manifest and verifies objects by hash; the user's `rollback` valid only when not `running`/`recovering`.
- [ ] REQ-TOOL-010 / THR-07: a failed write execution is restored before its observation; `rolled_back = true`; a successful one persists; counters count the rolled-back execution (REQ-TASK-008).
- [ ] REQ-WS-010: cancel restores unless `on_cancel = keep`; the observation says which.
- [ ] REQ-WS-007 / REQ-SEC-015 / INV-10 / THR-28: diff against base; conflicts refused and listed; backup snapshot; confirmation required (`--yes` only without a terminal); apply through temp-and-rename; `workspace.promoted{files}`; reversal from the backup; `--patch` writes nothing; excluded paths never promoted; no tool lets the model promote.
- [ ] REQ-WS-008: quota from the after manifest halts `workspace_quota`.
- [ ] REQ-WS-009 / REQ-WS-012 / REQ-PERS-010 / REQ-OBS-005 (amended): lifecycle states end in `deleted` by the user only; `task delete` refused for a non-terminal task, shows its plan, asks, keeps the audit unless `--audit`; reference counts right after every snapshot and deletion; no timer anywhere; the footprint shown.
- [ ] REQ-PERS-007 (M3) / REQ-PERS-008 / INV-07 / INV-12: a write found running is restored and halts `recovered` (or resumes under `auto_resume`); a failed restore halts `unknown_outcome`; nothing non-reversible is replayed.
- [ ] REQ-PERS-009 / CONS-04 / THR-29: the database references only durable manifests; a manifest's hash is checked when read; CONS-04 and the count rule at startup; the journal finished or undone at startup with no orphan left; crash matrix at the new points.
- [ ] REQ-PERS-011: the book states that copying the stopped data directory is a complete backup.
- [ ] REQ-TASK-004: fork from a snapshot; grants copied and shown; history segment bounded by LIM-C5; the source untouched.
- [ ] RFC 005 §11 / REQ-SEC-014: `run_script` and `run_wasm` no longer in the shipped `approval_tools`.
- [ ] Every snapshot hashes every file; the same-size-same-mtime regression test; the cost reported at 10 000 files.
- [ ] The M3 demonstration (ED §23) passes on Linux, Windows and macOS; the report states snapshot and restore cost for a 10 000-file tree.
- [ ] `--json` shapes for `rollback`, `promote`, `task delete`, `task new --from-task` and the footprint documented in the book; CHANGELOG 0.3 announces the removal of `gc` and `retention_days`.
