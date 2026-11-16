# Handoff 006c — The workspace: restore verifies first, the walk sees everything, modes are kept (release 0.11, phase 1)

**RFC.** [RFC 006](../../done/006-workspace-snapshots-and-promotion.md). **Found by.** The audit before 0.10 (A-14, A-20, A-24, A-38, X-06), the reviews of 006b, 006b-5, 002e and 013b-1, and the re-check of 2026-11-16 (`ROADMAP.md`, "Release 0.11", phase 1).

**Specification** (amended before this handoff):
- external design §14.2 (non-UTF-8 names; the scope's read errors);
- §14.3 (`mode?`; link targets byte for byte);
- §14.4 (verify, then replace; the walk sees every kind of entry; link chains; modes; backups are not rollback targets);
- §14.5 (promotion and modes; a special file of the project refused);
- REQ-WS-006.

**Embargo:** none.

## Items

The lines below are as found on `main` at `70ab081`, in `crates/mimlys/src/workspace/snapshot.rs` unless named.

1. **X-06: a backup is not a rollback target.** `control.rs:421` finds any snapshot. Refuse a `Backup` record with an `InvalidRequest` that names `rollback --promotion <id>`. Optionally, `task show` marks the backups.
2. **A named pipe in the project never blocks `promote`.**
   - `project_entry` (`:1105-1130`) hashes any entry that is not a directory or a link, with a blocking open. A path of the plan that is a named pipe, socket or device is refused in the plan, naming it.
   - Every remaining open of a project file uses the non-blocking open with an `fstat` check, as 002e's `open_regular` does, which also covers a race.
3. **A-14: verify, then replace.** `restore_into` (`:363-403`) removes before the objects are checked. `converge` in `Mode::Revert` (`:904-950`, then `:982`) does the same on the user's project.
   - Verify and write every needed object into a staging directory inside the root. Then remove and rename into place.
   - A staging directory left by a crash is an extra entry, which the next restore removes.
   - Do the same for a reversal (`Source::Objects`).
4. **A-38: one walk that sees everything, used by every walk.** Today these recurse separately: `walk` (`:107`), `measure` and `copy_tree` (`workspace.rs:215`, `:254`), `tree_size` (`:302`) and `unlisted_in` (`:1387`). Replace them with one iterative walk, with no recursion on the stack, that:
   - returns named pipes, sockets and devices as entries, so that a restore removes one the manifest does not hold, and no `create_dir` fails on one (`:413`, `:972`);
   - refuses a name that is not valid UTF-8, naming its path, at task creation (§14.2), instead of today's lossy name and a later `ReadFailed{NotFound}` with the wrong advice;
   - keeps a link's target byte for byte (`:128`, `:1115` rewrite `\` to `/`; convert only where Windows needs it);
   - at task creation, refuses a part of the scope that cannot be read with the path and the kind of the failure (`workspace.rs:221-235`, `:260-268`, `unlisted_in` `:1393-1401`; today `Storage{walk}`, an internal error);
   - resolves a link chain through the manifest's own links, with a bound on the hops, and refuses one that leaves the root (`mimlys-core/src/snapshot.rs:214-258` is lexical only). Put the bound in a constant, and I register it at review. This sub-item is the largest. If 006c runs long, it goes last in the PR order, not out.
5. **The Windows directory link.** `recreate_symlink_in` (`workspace.rs:373-385`) always makes a file link.
   - Write links last, and choose `symlink_dir` when the target, resolved in the manifest or in the destination after its writes, is a directory.
   - No format change. This runs on the Windows job only.
6. **A-20: modes are kept.** `TempFile::set_mode` (`:197-212`) forces 0o644 or 0o755 and ignores the error.
   - **The manifest:** `Entry::File` gains `mode: Option<u32>` (masked to 0o777, absent before 0.11 so that old hashes are unchanged), taken at snapshot.
   - **Restore** applies it.
   - **Promotion** keeps the project file's own mode. A new file takes the workspace's mode without group or other write bits, and a change of mode alone is in the plan. Include `mode` in `same_content`.
   - **The reversal** restores the backup's mode.
   - **Errors:** a failed `chmod` is an error, not ignored.
   - **No store change:** manifests are files, and `Promotion.after` gains an optional field. No `SCHEMA_VERSION` move.
7. **A-24's residual.** A promotion's restore after a crash also removes its own temporary files (`.mimlys-tmp-<pid>-<n>`) in the parents it touched.
8. **`task prune` goes on past a task deleted with files left.** `execute` (`commands/prune.rs:155-161`) breaks on any error.
   - Count `NotRemoved` as deleted with files left: list its paths, go on, and finish as `Partial` (exit 1) with a line that says what is left to the next start.
   - The JSON report gains `left` (additive).
9. **The halt lines say what happened.**
   - **After a failed rollback:** a restore that fails halts the task: `workspace_unreadable` when the tree could not be read, `unknown_outcome` otherwise. Today `unknown_outcome`'s line ("an action that cannot be undone did not finish by itself", `messages.rs:118-120`) is false for a reversible action, and `workspace_unreadable`'s does not say that nothing was rolled back. Record that the halt followed a failed rollback (an additive field on the halt's event), and give both lines a clause: `the workspace was not rolled back`.
   - **For a tree unreadable for a reason other than its permissions:** `workspace.unreadable{path}` gains an optional `kind` (`event.rs:254-257`). `halt_text` becomes neutral, and `unreadable_line` gives "restore its permissions" only for a permission problem, the kind's own words otherwise.
   - Both fields are additive, so no event schema version moves.

## Tests (by state; each names its requirement)

- **1:** rolling back to a backup snapshot is refused with the message.
- **2:** a project with a named pipe at a path the plan touches: the plan refuses it at once, and nothing blocks.
- **3:**
  - a missing object, and then a corrupt object, in a restore: the tree is unchanged;
  - in a reversal: the project is unchanged;
  - a staging directory left behind is removed by the next restore.
- **4:**
  - a walk of a tree deeper than any recursion would bear, measured, not assumed;
  - a named pipe or socket made by an action is removed by a rollback;
  - a non-UTF-8 name refuses creation, naming the path (Linux);
  - a link target with `\` survives;
  - an unreadable directory in the scope refuses creation with the path;
  - the audit's chain `s/x -> ..` plus `y -> s/x/..` is refused.
- **5:** the Windows job: a directory link is restored as a directory link.
- **6:**
  - a 0600 file stays 0600 through a promotion and a reversal;
  - a new file of 0666 is promoted as 0644;
  - a mode-only change is in the plan;
  - an old manifest's hash is unchanged.
- **7:** a crash after a temporary file was written leaves none after the restore.
- **8:** a prune over two tasks, the first with files left, deletes both, lists the left paths and exits 1.
- **9:** each halt's line, by state: the event's fields, and the printed line.

## Mutations

- a backup admitted;
- a blocking open of a project file;
- remove before verify (the restore, and the reversal);
- the recursive walk;
- a special file skipped;
- a lossy name;
- `\` rewritten;
- the lexical chain test;
- a file link for a directory;
- modes forced again;
- a prune that stops;
- the halt line without the clause.

## Book and CHANGELOG

- Book:
  - the storage page and the promotion page (modes, special files, a failed rollback);
  - `cli.md` (`prune`'s `left`; `rollback --snapshot` refusing a backup).
- CHANGELOG: Fixed (each item) and Changed (non-UTF-8 names refused; modes kept; the halt lines; `left`).

Return: `.git-exclude/review-requests/006c-workspace/README.md`.

## After review (2026-11-16): 006c-6

006c-1 to 006c-5 are landed. Five small changes follow. The specification is amended: external design §14.5 (modes), §17.1 and §20; LIM-W6.

1. **The plan shows exactly what the promotion does** (your question 1).
   - Keep your rule: the project's own mode, with an executable-bit change carried.
   - A mode difference that promotes nothing is **not** in the plan. A path changed in mode only, with nothing carried, is not listed.
   - A carried executable bit shows as `old mode` / `new mode`.
2. **A backup is not a fork source either.** `run --from-snapshot`, or a fork (`control.rs`, the fork's `find`), refuses a promotion's backup with the same message as `rollback --snapshot` (X-06's sibling).
3. **`remove_tree` uses the one walk.** Task deletion's removal is recursive and works by path today. Make it iterative over handles, as the walk is, so that a tree deeper than the stack, or than a path may be, is removed.
4. **`task delete`'s message is bounded:** at most ten paths in the text, every path in the JSON, as `prune`'s lines are.
5. **The scope's read errors at creation are `invalid_request`** (your question 4): the scope is the request's input. `state` stays for a task's tree.

**Answers that need no code:**
- question 2 (a mode difference is not a conflict): accepted;
- questions 3, 5, 6 and 7: accepted as you did them;
- `MAX_LINK_HOPS` is registered as LIM-W6, and its comment cites it.

**Tests and mutations:**
- 1: a mode-only change outside the carried bit is absent from the plan.
- 2: a fork from a backup is refused.
- 3: the deep tree of your prune test is deleted whole, and `NotRemoved` no longer occurs for it. Your prune test then needs another way to leave files: choose one and say it.
- 4: the bound on the text.
- 5: the error kind.

Return: `.git-exclude/review-requests/006c-6-workspace/README.md`.
