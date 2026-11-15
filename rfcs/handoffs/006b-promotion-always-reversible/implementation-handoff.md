# Handoff 006b — Promotion is always reversible; snapshots survive unusual names (release 0.10)

**RFC.** [RFC 006](../../done/006-workspace-snapshots-and-promotion.md) · **Found by.** The independent audit before 0.10, which audited commit `f5a94fa`. The owner accepted the triage on 2026-11-14.

**Specification** (amended before this handoff):
- external design §14.5 (the new order, and "The order is the audit's rule") and §16.3 (a promotion found `applying`);
- REQ-WS-007.

**The defects, verified in the code at `f5a94fa`** (`crates/mimlys/src/workspace/snapshot.rs` unless named):
- **A-03, high.** A deleted directory is removed with `remove_dir_all` (`:613-623`), as is a directory that became a file (`:644-648`). The plan lists only the base manifest's paths, so everything else under that directory in the project goes:
  - LIM-W4 and W5 entries never materialized (`.env`, keys, a nested `.git/`);
  - files the user added since.

  None of it is in the backup or on the confirmation screen (`commands/promote.rs:44-55`). `revert_promotion_impl` (`:729-735`) only recreates the empty directory. **The loss cannot be undone.**
- **A-04, high.** The backup is fsynced before the first write, but its row (`record_snapshot`), its seal and the promotion record come after the project writes (`mimlys-core/src/runtime/control.rs:461-495`).
  - A failure or crash midway leaves a changed project with no record. The next start then undoes the journal and deletes the backup manifest.
  - Two type changes always fail partway: a directory replaced by a link (`remove_file` on a directory, `:662-665`), and a file replaced by a directory (`create_dir_all` on a file, `:652-655`).
- **A-15.** The project is compared with the base only at plan time (`:518-532`); an edit made during the confirmation is overwritten.
- **A-16.** Writes follow a project symlink to a directory (`hk -> .git/hooks`): `is_dir` and `create_dir_all` go through it (`:636-656`), and the exclusion is judged on the unresolved path (`:507-512`). An agent can plant the link in an earlier promotion.
- **A-24 and X-03.** The temporary name is the fixed `<name>.mimlys-tmp`, opened with `create` (`:190-191`, `:240-241`).
  - It collides with a real file or a link.
  - It is left behind after a failure.
  - It exceeds NAME_MAX for a name of 245 bytes or more, so **every** restore, fork and promotion of that tree fails. An agent can make its own actions irreversible on purpose: the task halts `unknown_outcome`, and a manual rollback fails too.
- **A-19.** `delete_task` removes the rows first (`control.rs:564-575`), and `delete_files` stops at the first error (`:760-762`, `remove_dir_all(task_dir)?`). The manifests and objects are then orphans for good. Go's read-only module cache in a tool's `HOME` triggers this, and so does `task prune`.
- **From A-39.** An out-of-root link is skipped silently (`:659-661`) yet recorded in `Promotion.files` and the event. `revert_promotion` is unconditional and asks nothing (`commands/rollback.rs:18-33`).

## Items

1. **Nothing unlisted is removed** (A-03).
   - The plan refuses a deleted directory (or one replaced by a file) that holds project entries the plan does not list, and names them, so that the user decides.
   - Removal is of listed paths only: deepest first, never recursive (`remove_file`, `remove_dir` of an empty directory).
   - The revert path follows the same rule.
2. **The backup is committed before the project is touched** (A-04). The order of §14.5:
   1. write the backup;
   2. commit its row and seal it;
   3. record the promotion `applying`;
   4. apply;
   5. record `applied`.

   Then:
   - **An error while applying:** restore the entries already applied from the backup, record `failed`, and say so with the reason.
   - **A promotion found `applying` at start:** the same restore, recorded `failed` and listed (§16.3).
   - **The store:** the promotion record gains its state through a new migration, `004_…`, with `SCHEMA_VERSION` 4. The 0.10 release notes already say that 0.9.0 refuses a 0.10 store; that line now says version 4.
   - **The two always-failing type changes** (a directory → a link, a file → a directory) are applied correctly, or refused at plan time with the reason. Never midway.
3. **The project is re-checked at apply** (A-15). Before each entry is written, it is compared with the base entry (as the backup captured it). A difference stops the promotion through item 2's error path, naming the entry.
4. **Never through a linked parent** (A-16).
   - Every parent component of a project path is checked without following links; a symlink parent is refused at plan time and named.
   - The exclusion (LIM-W4, W5) is judged on the resolved path.
   - The module's claim at `snapshot.rs:5-6` ("links are never followed") is then true.
5. **A unique temporary name** (A-24, X-03).
   - A short name unique per write: not derived from the entry's name, so it never exceeds NAME_MAX.
   - Opened with `create_new` in the entry's directory.
   - Removed on every error path.
   - A tree holding a 255-byte name restores, forks and promotes.
6. **`task delete` leaves no orphan** (A-19).
   - Remove the manifests, objects and blobs the task alone references first.
   - Make a directory writable before removing it.
   - Attempt every removal, and report each failure by path.
   - The rows go last, or the deletion is journaled so that the next start finishes it. Say which you chose.
7. **From A-39.**
   - An out-of-root link is refused at plan time, never silently skipped and recorded as promoted.
   - `rollback --promotion` shows what it will restore and asks for confirmation, as `promote` does. Later edits to those paths are listed as conflicts and refused unless confirmed.

## Tests (by state, §3c; each names its requirement)

- **1:** the auditor's case. The project has `d/a.txt`, `d/.env` and a later `d/new.txt`; the agent deletes `d`. The plan refuses and names `d/.env` and `d/new.txt`; the project is unchanged.
- **2:**
  - an injected failure after the first write: the project is restored, the promotion is `failed`, and the backup is present;
  - a crash between step 3 (`applying`) and step 5 (`applied`): the next start restores it and lists it;
  - each type change succeeds, or is refused at plan time.
- **3:** an edit made in the project after the plan: the promotion stops and names it.
- **4:** `hk -> .git/hooks` plus `hk/pre-commit` added. The plan refuses, and `.git/hooks/pre-commit` never exists.
- **5:**
  - a 255-byte file name: restore, fork and promote succeed;
  - a pre-existing `foo.mimlys-tmp` file or link: untouched, and `foo` is correct.
- **6:** a read-only directory under the task's workspace. `task delete` removes everything, and the store holds no manifest or object of the task.
- **7:** an out-of-root link is refused at plan time; `rollback --promotion` lists and asks.

REQ-WS-003, REQ-WS-005, REQ-WS-006, REQ-WS-007, REQ-WS-012, REQ-SEC-015, INV-10.

## Mutations

- `remove_dir_all` restored;
- the backup's row committed after the apply;
- the apply-time re-check removed;
- symlink parents followed;
- the fixed temporary name;
- rows deleted before the files.

## Book and CHANGELOG

- The promotion page says what is refused and why, and that a failed promotion restores itself.
- CHANGELOG: Fixed, citing the audit's ids; Changed: `SCHEMA_VERSION` 4.

Return: `.git-exclude/review-requests/006b-promotion-audit/README.md`.

## After review (2026-11-15): 006b-5

006b-1 to 006b-4 are landed. Three changes follow. The first keeps this handoff's title true. The specification states them (external design §14.5, "There is always a way back"; §17.1, your two events).

1. **A promotion whose own restore failed stays reversible.** Today a failed apply whose restore also fails is recorded `failed`, and `rollback --promotion` refuses every `failed` promotion. `rollback --snapshot` restores the workspace, not the project. So the user is left with a half-applied project and no command to undo it. The same holds at start (`recover_promotions`).
   - Record whether the restore succeeded: `restored: bool` in the record, defaulting to `true` for older records.
   - `rollback --promotion <id>` accepts a `failed` promotion with `restored: false`. It runs the same restore from its kept backup, with the same listing, confirmation and edited-path rule.
   - The error and `task show` name that command, for example `…; the backup is kept: run mimlys rollback <task> --promotion <id> to restore the project`.
   - The next start does not retry by itself, so a lasting error does not repeat at every start. It lists the promotion once with the command.
2. **A deletion that left files says the task is deleted.** `NotRemoved` reads today as if `task delete` had failed. Say first that the task is deleted, then which paths are left to the next start, for example `task <id> is deleted; 2 path(s) could not be removed and are left to the next start: …`. This applies to `task delete` and to `task prune`, which names the task.
3. **A Windows junction as a project parent is refused like a link.**
   - Test it on the Windows job: a project directory that is a junction, with a path below it in the plan, is refused at plan time.
   - If cap-std's `is_symlink` does not see a junction, check the reparse point instead.

**Tests:**
- 1: an injected failure of the restore after a failed apply: `failed` with `restored: false`, then `rollback --promotion` restores the project. The same after start.
- 2: the exact message.
- 3: as said.

**Mutations:**
- a `failed` promotion with `restored: false` refused by `rollback --promotion`;
- the junction not detected.

**Release text** (correct your §5): the first "Fixed" line says a directory "is removed with `remove_dir_all` only after the plan names every entry". That is not what the code does; say "removed only when empty, after its listed entries".

Return: `.git-exclude/review-requests/006b-5-promotion/README.md`.
