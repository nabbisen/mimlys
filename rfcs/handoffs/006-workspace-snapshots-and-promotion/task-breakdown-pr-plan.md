# Handoff 006 — PR plan (M3)

| PR | Content | Depends on | Size |
|---|---|---|---|
| 006-1 | `mimlys-core::snapshot`: manifest, entries, canonical hash, diff, root rule; SPI growth with the fakes; LIM-C5; the requirement and limit amendments (REQ-WS-009, 012, REQ-PERS-010, REQ-OBS-005, LIM-P2 … P4) | — | M |
| 006-2 | `mimlys-persist::snapshots`: object and manifest stores with fsync, the journal, reference counts; migration 002; `record_snapshot`, `record_workspace_state`, `record_promotion`, `delete_task_data`; CONS-04 and the count rule; crash points | 006-1 | M |
| 006-3 | `mimlys::workspace`: the walk, `snapshot`, `restore`, `materialize_from`; symbolic-link tests both directions | 006-1, 006-2 | L |
| 006-4 | The loop and control: before/after snapshots, rollback on failure and cancel, quota from the manifest, `rollback_workspace`, recovery restore; the shipped profiles' `approval_tools` back to empty | 006-3 | L |
| 006-5 | Promotion: plan, conflicts, backup, apply, reversal, `--patch`; `promote` and `rollback --promotion` commands with the diff display and confirmation | 006-3 | L |
| 006-6 | Lifecycle, `task delete` with the plan and confirmation, the footprint in `task list` and `status`; the journal step at startup; `retention_days` removed | 006-2 | M |
| 006-7 | Fork: `create_task{from_task, from_snapshot}`, grants copied, history segment, `task new --from-task` | 006-3, 006-4 | M |
| 006-8 | The M3 demonstration, the measurement report, book chapters (workspace, promotion, deletion, backup), CHANGELOG 0.3 | 006-4 … 006-7 | M |
