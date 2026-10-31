# Handoff 006 — Snapshots, rollback, promotion, lifecycle, fork (M3)

**RFC.** [RFC 006](../../accepted/006-workspace-snapshots-and-promotion.md) (Accepted 2026-10-28)
**Specification.** REQ-WS-004 … 007, 009, 010, 012; REQ-TOOL-010; REQ-PERS-007 (M3), 009, 010, 011; REQ-TASK-004; REQ-OBS-005; REQ-SEC-015; REQ-TEST-009; external design §5.4, §7.5, §14.1, §14.3 … §14.6, §16.3, §16.4, §21.3 (CONS-04), §23 (M3); limits LIM-W1 … W5, LIM-P2 … P4, LIM-C5, LIM-U6.
**Crates.** `mimlys-core` (`snapshot.rs`: manifest, entry, diff, pure; SPI growth; loop lines 4/6/7 with snapshots; recovery restore; fork in `create_task`; `history` segment), `mimlys-persist` (`snapshots.rs`: object and manifest store with fsync, the journal, reference counts; migration `002_snapshots.sql`; `deletion.rs`; CONS-04), `mimlys` (`workspace.rs`: snapshot, restore, diff, promote, fork, delete; `commands/{rollback,promote}.rs`, `task delete`, footprint in `task list` and `status`; prompt for promotion and deletion).
**Milestone.** M3. Depends on handoffs 001–005 and 010.

## 1. Scope

In: the snapshot store, manifests, the journal and object reference counts; snapshots at base, before and after (every file hashed); automatic rollback on failure, cancellation and recovery; the user's rollback; promotion with diff, conflicts, backup, confirmation, audit, reversal and `--patch`; workspace lifecycle and explicit deletion with the footprint shown; fork; the end of the M2 approval stopgap; the M3 demonstration; the measurement report (snapshot and restore cost); the requirement and limit amendments.

Out: reflink detection beyond what `std::fs::copy` does on its own; three-way merges; any index or heuristic that skips hashing; any timed deletion.

## 2. Module layout

```text
crates/mimlys-core/src/
  snapshot.rs     snapshot/tests.rs    Manifest{entries}, Entry::{File{path, executable, size, hash}, Dir{path}, Symlink{path, target}}, canonical JSON and hash, diff(a, b) -> Diff{added, modified, deleted}, target_inside_root
  spi.rs          (+) WorkspaceProvider::{snapshot(task, reason) -> SnapshotRecord, restore(task, snapshot), manifest(snapshot), diff_against_base(task), promote(task, plan), materialize_from(task, snapshot, scope), delete(task) -> Footprint}; WorkspaceState; Footprint{workspace_bytes, snapshots, object_bytes_exclusive, blob_bytes}; fakes updated
  runtime/loop_.rs      before-snapshot at line 4 (write effects), after-snapshot at line 6, rollback before the observation on failure and on cancel (profile.on_cancel), quota from the after manifest
  runtime/control.rs    rollback_workspace, promote_workspace (two phases: plan with the diff, apply after confirmation), create_task with from_task/from_snapshot (grants copied, history segment)
  runtime/context_m1.rs history segment (LIM-C5 steps)
  recovery.rs           RecoveryPlan unchanged; persist applies restore_snapshot through the provider
crates/mimlys-persist/src/
  migrations/002_snapshots.sql   snapshots(snapshot_id, task_id, reason, manifest_hash, bytes, files, created_at, action_id, project_root), objects(hash, size, references), workspaces(task_id, state, changed_at), promotions(promotion_id, task_id, backup_snapshot_id, files, applied_at); indexes
  snapshots.rs    snapshots/tests.rs    ObjectStore{put(bytes) -> hash (temp, fsync, rename, 0400), open(hash), verify, unlink}, ManifestStore{write(manifest) -> hash (fsync), read(id)}, Journal{begin(snapshot_id) -> lists added objects; finish; undo}; startup: finish or undo every pending journal (no sweep)
  store.rs        (+) record_snapshot (after the files are durable; increments the objects' references in the same transaction), record_workspace_state, record_promotion, delete_task_data (decrements, returns the objects reaching zero), loads; recovery applies restores; integrity CONS-04 and the reference-count rule
  deletion.rs     deletion/tests.rs     the footprint of a task; the deletion plan (workspace, snapshots, objects reaching zero, blobs, backups; events only with `audit`) and its application, files after rows
crates/mimlys/src/
  workspace.rs    (+) walk-and-hash of every file (links as entries), snapshot(), restore(), diff, promote (conflict check against the project, backup snapshot, temp-and-rename writes), materialize_from, delete, footprint
  commands/rollback.rs, promote.rs (diff display, confirmation, --yes, --summary, --patch), task.rs (delete with the plan shown and confirmation; list and show with the footprint); task new --from-task/--from-snapshot; prompt: the promotion and deletion confirmations
  config.rs       (−) [workspace] retention_days removed (an unknown key names `task delete`)
```

## 3. Behaviours

### 3.1 Objects and manifests (REQ-WS-004, REQ-PERS-009)

`put(bytes)`: hash (SHA-256, LIM-U6), skip when present, else write `objects/<hash>.tmp-<pid>`, fsync, rename to `objects/<hash>`, set read-only. Manifest: entries sorted by path (bytes order), serialized canonically (handoff 001's canonical JSON), written to `manifests/<snapshot_id>.json`, fsynced, its hash recorded. Only then `record_snapshot`. A snapshot whose manifest hash equals the task's previous snapshot reuses that row (no new id). Reading a manifest recomputes and checks the hash; a mismatch is `WorkspaceError::Storage{detail: "manifest_hash"}` and recovery treats the checkpoint as untrusted (REQ-PERS-009).

### 3.2 The walk (RFC 006 §3)

The walk visits the tree with `symlink_metadata` (never following links), applies the exclusions, hashes every regular file (SHA-256, streaming, LIM-W3 is not a bound here), records directories and links as entries with their raw target, and never reads through a link. No index, no cache: the manifest is the truth of the tree at that moment. The snapshot's duration and the tree's size go into the metrics of the step (`setup_ms` stays the sandbox's; a new `snapshot_ms` on the execution record).

### 3.3 The loop (REQ-TOOL-010, REQ-WS-010, REQ-WS-008)

Write-effect action: `snapshot(task, "before")` → the id into `Action.snapshot_before` at line 4 → execute → on success `snapshot(task, "after")` into the execution record at line 6; on failure `restore(task, before)` *before* line 6, `Verdict.rolled_back = true`, notice `rolled_back`; on `cancelled` restore unless `profile.on_cancel == Keep`, and the observation's `rolled_back` says what happened. A `host_fault` or a failed restore: the task halts `unknown_outcome` with the action listed (the disposition's `Halt`). At line 8 the after manifest's `bytes`/`files` above LIM-W1/LIM-W2 halt `workspace_quota`. Retries (REQ-TOOL-009) run after the restore, as before.

### 3.4 Rollback by the user (REQ-WS-006)

`rollback_workspace{task, snapshot?}`: task not `running`/`recovering`; the snapshot must belong to the task (or be a backup of its promotion with `--promotion`); restore; event; the task's state is unchanged (a halted task stays halted). `mimlys rollback <task> [--snapshot ID] [--promotion ID]`.

### 3.5 Promotion (REQ-WS-007, REQ-SEC-015)

Phase 1, `plan`: full rehash; `diff(base, current)`; drop excluded paths (notice `excluded{count}`); for each path in the diff compare the project target with the base entry (hash of the target file versus the base's hash; existence versus absence); conflicts → `conflict{with: task_id? no: paths}` error listing them (ED §18); else a `PromotionPlan{files: [{path, change: added|modified|deleted, bytes}], text_diffs}` bounded for display. Phase 2, `apply(plan_id)`: the plan's manifest hash must still equal the current workspace (else `conflict`); back up the targets' current versions (and record absence) into a `backup` snapshot with `project_root`; write every file through `path.tmp-<pid>` + rename, create directories, apply deletions, recreate relative inside-scope links only; `record_promotion` and `workspace.promoted{files}`. Reversal: `rollback --promotion <id>` restores the backup manifest into the project root (the same restore code with the project root as the target; files absent from the backup but present in the plan's `added` list are removed).

### 3.6 Lifecycle, ownership, deletion (REQ-WS-009, REQ-WS-012, REQ-PERS-010, REQ-OBS-005, all amended)

`workspaces.state`: `allocated` on `create_task` before the copy, `active` after, `archived` on the task's terminal transition (the loop and the control verbs record it), `deleted` by `task delete`. Objects carry a reference count incremented in `record_snapshot`'s transaction and decremented in `delete_task_data`'s; an object whose count reaches zero is unlinked after the transaction commits (a crash in between leaves a zero-count row, finished at the next start by the journal step, not by a sweep). `mimlys task delete <task> [--audit] [--yes]`: the task must be terminal; the plan (workspace bytes, snapshots, objects to unlink with their bytes, blobs, backups and "its promotion P can no longer be reversed") is shown; confirmation; files removed after the rows except objects (rows first, files after); `--audit` also deletes the task's events and the task row. `task list` and `status` show the footprint per task and the directory's total. The requirements text: REQ-WS-009's `destroyed` becomes `deleted` by the user; REQ-WS-012 becomes `task delete`; REQ-PERS-010 becomes "nothing is deleted without the user's command; audit stays unless `--audit`"; REQ-OBS-005 the same; LIM-P2 … P4 rows read "no timer; explicit deletion"; ED §14.6, §16.3 (journal instead of collection), §16.4, §19.1, §19.2 follow.

### 3.6a The journal (REQ-PERS-009)

`snapshot()` begins `snapshots/pending/<snapshot_id>` listing the hashes it is about to add (those not yet present), writes the objects and the manifest, calls `record_snapshot`, then removes the journal. Startup, before recovery: every pending journal with a row is removed; every one without a row has its listed objects unlinked (only those it added) and its manifest removed. The journal is fsynced before the first object is written.

### 3.7 Fork (REQ-TASK-004)

`create_task{from_task, from_snapshot?, ...}`: the source must be terminal or `halted`; `materialize_from(new_task, snapshot, scope)` restores the manifest into the new tree and records it as `base` (same manifest hash, new row); grants: the source's active grants re-issued to the new task, listed in the creation output; history: `context_m1` adds `<<history task="…" steps=N>>` with the goal, the latest plan and the last LIM-C5 steps as `{decision, outcome, tool?, status?}`; `tasks.forked_from` records the source. CLI: `task new <goal> --from-task ID [--from-snapshot ID]` (the scope defaults to the source's).

### 3.8 Recovery (REQ-PERS-007, M3 part)

`recover_all` takes the workspace provider: for `AfterUnknown::Rollback` it restores `snapshot_before` (from the action) and halts `recovered` (or resumes under `auto_resume`); a failed restore halts `unknown_outcome`; the event `workspace.rollback` is recorded in between. CONS-04 in `integrity.rs`: every `workspace.rollback` event and every `promotions.backup_snapshot_id` references a snapshot row whose manifest file exists and whose hash matches.

## 4. Tests (REQ-TEST-009; crash points)

- Manifest canonical form and hash; diff of added/modified/deleted; `target_inside_root` for `..`, absolute and prefix forms.
- Snapshot → change → restore yields identical hashes; files absent from the manifest are deleted; executables keep their bit (unix); objects verified by hash (a corrupted object makes the restore fail and the tree untouched for that entry).
- Symbolic links in both directions: a workspace link to `/etc/passwd` is stored as an entry, its target never read, and restore recreates it only when inside the root; a project link outside the scope is omitted at materialization; promotion never follows a link in either tree.
- Every snapshot hashes every file: a file rewritten with the same size and modification time is still seen (the heuristic's failure case, as a regression test); the report measures the cost at 10 000 files.
- Loop: a failed write action (`write_file` to a path, then a scripted failure) leaves the tree at the before snapshot and the observation says `rolled_back: true`; a cancelled write under `on_cancel = keep` keeps it; quota halt from the after manifest.
- Promotion: conflicts refused and listed (the user edited a target after creation); a clean promotion shows the diff, needs confirmation, backs up, applies, records files; `rollback --promotion` restores the project; excluded paths never promoted; `--yes` without a terminal; `--patch` prints a patch and writes nothing; the model cannot propose it (no such tool).
- Crash injection at the new points `snapshot.before_row`, `snapshot.after_row`, `delete.before_rows`, `delete.after_rows`, `promotion.before_apply`, `promotion.after_apply`: the database never references a missing manifest; a pending journal is finished or undone at the next start and no orphan object remains (count the files); recovery restores and halts correctly.
- Lifecycle and deletion: states; `task delete` refused for a non-terminal task; the plan matches what is removed; shared objects survive while another task's manifest references them; `--audit`; the footprint in `task list` and `status`.
- Fork: the new workspace equals the snapshot; grants copied; history segment present and bounded; the source untouched.
- The M3 demonstration (ED §23): a failed `run_script` in `read_write` mode leaves the workspace at its pre-execution snapshot; `promote` refuses a conflicting file and otherwise shows the diff, asks, backs up and applies; a workspace symlink to `/etc` is neither snapshotted nor restored through.
- The report: snapshot and restore time for a 10 000-file, 100 MiB tree.

## 5. Recommended crates

`similar` (text diffs for the promotion display); everything else exists (`sha2`, `cap-std`, `globset`, `rusqlite`).

## 6. Return

Review-request package under `.git-exclude/reviewed/m3-006/` with the demonstration output, the snapshot and restore measurements, the crash matrix at the new points, the amended requirement texts, and the `--json` shapes of the new commands.

## 7. Amendments from the implementation (2026-10-28)

1. **Files in the provider, rows and events in the runtime.** `WorkspaceProvider::snapshot` prepares a snapshot (objects and manifest durable, journal open) and returns the record and its object list; the runtime records the row with its event and then calls `seal`. The same split holds for `materialize_from`, `promote` (the backup is a prepared snapshot) and `delete_files` (after `delete_task_data` returned the released rows). The provider never writes a row, the runtime never writes a file, and events stay the runtime's.
2. **`Action.snapshot_before`** is a field of the action record (set with the authorization), not a column; `Execution.snapshot_before`/`snapshot_after` fill from it.
3. **`archived` is derived**, never recorded: the `workspaces` table holds `allocated`, `active` and `deleted`; a terminal task's workspace is archived by definition.
4. **The footprint is computed by the runtime** from the task's manifests and the object reference counts (`object_references`), the tree's bytes (`tree_bytes`) and `blob_bytes`; `task list` computes it per task without walking manifests' objects more than once each.
5. **Reversal of a promotion** restores the backup manifest's entries and removes the paths the promotion added (those have no backup entry), through the same verified copy as a restore.
6. **Fork's base** is recorded with reason `fork` (the manifest file copied under the new id, no object written); the diff of a promotion is against the first snapshot whatever its reason (`base` or `fork`).
7. **The persistence crash harness** exercises the `snapshot` and `delete` points; `promotion` is exercised only through the CLI tests (the harness has no project root), and the M1 crash matrix exempts the points its demonstration does not reach.
8. **The pinned JavaScript build has no file modules**, so the "failed `run_script` in `read_write` mode" of the M3 demonstration is carried by a `run_wasm` module that writes a file and exits 1; the rollback path is the same.
9. **`gc` never shipped** (RFC 006 §6): it left the command set and the external design; `task delete` with the plan shown replaces it; `[workspace] retention_days` is refused with a message naming `task delete`.
10. **Fork's grants** are the source's active grants, re-issued to the new task by the session; the request's extra grants add to them within the ceiling.
