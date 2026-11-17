# Handoff 011o — Windows: a host tool reaches only its own task (release 0.11, phase 3)

**RFC.** [RFC 011](../../done/011-confinement-backends.md). **Found by.** The audit before 0.10 (A-06; Linux and macOS fixed in 0.10 by handoff 011m), the review of 011n (D1, D5, D7), and the scope of 0.11 (`ROADMAP.md`, "Release 0.11", phase 3).

**Specification** (amended before this handoff):
- external design §13.3, the rule "A host tool writes only its own task's workspace": the Windows sentence;
- REQ-CONF-006.

**Owner's decision (2026-11-16):** a worker per task with a per-task capability SID, after a spike on the Windows job. The upgrade's one-time migration runs at 0.11's first start, and `toolchain revoke --all` cleans it up. If phase 3 runs long, it becomes 0.12. Your report decides that, so say early if it will.

**Embargo:** none. The residual is public in the 0.10 release notes and the threat register.

## Today

These are as found on `main` at `d7bcea6`; check them on today's `main`.

- **The container's write set is every workspace.** `app.rs:300` puts `<data>/workspaces` in the write set, and `toolchain::prepare` (`toolchain.rs:236-290`) gives the container SID one inheritable write entry there. So a host tool of task A can read and change task B's tree.
- **The per-task worker already exists.** `Worker::per_task` (`worker.rs:576-588`) makes a worker serve one task and be replaced at another task's first execution, and `spec_for` narrows its specification to the task. macOS uses it, and test builds can choose it on any platform. Windows does not use it yet.
- **A task's directory is `workspaces/<task>/`.** It holds `tree/`, `tmp/` (the executions' temporary directories) and the workspace record (`INFO_FILE`). A tool must never write the record.

## Stage 1: spike W1, on a throwaway branch measured by the Windows job

Make a branch you will not merge, and measure on `windows-latest` (not locally):

1. **Two capabilities, one container.** Derive two capability SIDs from two names with `DeriveCapabilitySidsFromName` (for example `mimlys-task-<id>`). Give directory A one inheritable entry for capability 1 with today's write mask, and directory B one for capability 2. Then start a process in the container with capability 1 only.
   - Expected: it reads, creates and deletes in A; it cannot open B for reading or writing, nor list it.
   - Also: run it with the restricted, low-integrity token where the system accepts one with the container, as the worker runs today.
2. **Traversal.** With no container or capability entry on the parent (`<data>/workspaces`, `<data>/workspaces/<task>`), the process still opens `<data>/workspaces/<task>/tree` by its full path.
   - If it cannot, measure the smallest entry that admits it. The candidate is traverse only on the parent, not inheritable, and not list.
3. **No other task by name or listing.** The process cannot list `<data>/workspaces`, so it learns no other task's identifier.
   - If traversal needs an entry that also lists, say so: that is a stop condition.
4. **The cost.** Time the entry written on an empty directory followed by copying 10 000 files, against the entry written after the copy. Expected: the first costs the copy alone.
5. **`<data>/tmp` read-only.** Run the existing host-tool tests (`m6b_host_tools`, `m6c_host_tools`) with the container's entry on `<data>/tmp` changed to the read mask.
   - Expected: they pass, because an execution's temporary directory is under the task.
6. **The migration's cost.** Measure removing the container's inheritable entry from a `<data>/workspaces` that holds 20 task trees of 1 000 files each. That removal propagates.

**Stop and report after the spike, instead of going on to stage 2, if:**
- item 1 does not hold;
- traversal needs an entry that also lists;
- the migration of item 6 takes more than LIM-O7 per task;
- or anything else contradicts the specification's Windows sentence.

Otherwise go on, and put the spike's numbers in the request.

## Stage 2: the items

1. **A worker per task on Windows.**
   - `per_task` is on for Windows, as for macOS.
   - The worker's container token carries the task's capability SID, and no other capability but the dependency step's network one.
   - Its specification's write set is the task's `tree` and `tmp` only.
2. **The entries are made with the task.**
   - When a workspace is made (creation, fork, restore into a new tree), `tree` and `tmp` each receive the task's entry, written while they are empty, through `acl::ensure_entry`'s lock and handle-based write.
   - The task's directory and its record receive none.
   - A failure is the task's creation failing with `acl_not_applied` and its reason, never a task whose tools cannot write.
   - Deleting a task removes its entries with its tree.
3. **The container's own entries.** None on `<data>/workspaces` or a task's directory, except the traverse entry if spike item 2 found one necessary. `<data>/tmp` gets the read mask.
4. **The one-time migration at 0.11's first start.**
   - What it does: remove the container's entry from `<data>/workspaces`, give each existing task's `tree` and `tmp` their entries, and set `<data>/tmp` to the read mask.
   - It is recorded in `toolchain-acls.json` (a new field; an older file reads as "not migrated").
   - It is repeated at the next start if it did not finish, and it is idempotent.
   - A record that cannot be used still stops the start (the owner's decision of 2026-11-16).
   - `status` says, in one line, while it has not finished.
5. **`toolchain revoke --all`** also removes the per-task entries, and `toolchain list` shows them as one summary line ("per-task entries on N tasks"), not one line per task.
6. **D1: the container's profile name** is keyed on the data directory's identity or its canonical path, not its configured spelling (`profile_name`, `windows.rs:564`).
   - This changes the container SID once. The migration of item 4 therefore re-applies the runtime's own entries and the administrator's granted entries under the new SID, and removes the old SID's entries and profile.
   - Two spellings of one data directory then give one container (test).
7. **D5:** the runtime's own directories (`workspaces`, `tmp`, `interpreters`) are refused when they are a link or a junction, at their creation and at every start, naming the path.
8. **D7:** `task_names` requires the temporary directory to be `workspaces/<task>/tmp/...` (`task_paths.rs:97`), not anything inside the task's directory.

## Tests (by state, §3c)

- **On the Windows job:** a host tool of task A cannot open task B's tree or `tmp`, for reading or writing, and cannot list `<data>/workspaces`. It reads and writes its own tree. This is ROADMAP's "done when" for A-06.
- A tool cannot write its task's workspace record.
- **The migration:** a data directory prepared as 0.10 leaves it, with two tasks, becomes the new layout. Each task's tool then writes only its own tree. A second start changes nothing (checked by reading the entries back). A start interrupted after the first task finishes the migration.
- **D1:** two spellings of the data directory (case, a trailing separator, a `subst` drive if the job allows it) give one profile name.
- **D5:** each of the three directories as a junction is refused, naming it.
- **D7:** a temporary path elsewhere in the task's directory is refused.
- **On Linux and macOS:** the existing tests of 011m pass unchanged.

## Mutations

Each of these must be killed:
- the capability left out of the token;
- the entry put on the task's directory instead of `tree` and `tmp`;
- the container's entry on `<data>/workspaces` kept;
- the migration's record not written, which repeats it forever;
- `profile_name` back on the spelling;
- the link check of D5 removed;
- `task_names`' old rule.

## Book and CHANGELOG

- **The book:** the Windows confinement page and `toolchain`'s page, stating what is isolated and the migration.
- **CHANGELOG:**
  - **Security:** A-06 on Windows.
  - **Changed:** the migration, with an **upgrade: read this** note. It says the first start may take a while on a data directory with many tasks, and what `status` says meanwhile.
- The threat register's residual row is closed at review, not by you.

## Return

`.git-exclude/review-requests/011o-windows-task-isolation/README.md`, with:
- the spike's numbers;
- the PRs and their CI runs;
- the mutations;
- the design questions.

## After review: 011o-4

The review of 2026-11-18 (`.git-exclude/reviewed/handoff-011o/README.md`) approved stages 1 and 2 and landed them. Three small changes follow, as one PR on `main`.

1. **A read path never covers the data directory,** on every platform.
   - **Today:** `check_read_path` refuses a root, the home directory and an ancestor of it, but not the data directory, a path inside it, or an ancestor of it. `task_spec` keeps the declared read paths. So a declaration's `read_paths` of `~/.local/share` (Linux), or a `toolchain grant` of `%LOCALAPPDATA%` (Windows), lets every task's tool read every other task's tree and the store. That undoes A-06's read half by a mistake in a configuration.
   - **The change:** refuse such an entry wherever a read path is refused today (`check`, the load, `toolchain grant`), naming the entry and the data directory, with a new refusal reason. Compare after normalizing, and case-insensitively on Windows and macOS, as `check_read_path` already does.
   - **Tests by state:** the data directory, a path inside it and an ancestor of it are each refused at load, with the reason, on every job; `toolchain grant` refuses them on the Windows job; a sibling of the data directory is accepted. First show, on the Linux job, a tool of one task reading another task's file through such a read path, with the refusal removed.
2. **A workspace whose entries cannot be written is `unavailable`,** with component `permissions`, as every other `acl_not_applied` is (your question 1). Today it is `internal` (`workspace storage failure: …`). Give `WorkspaceError` its own variant, not a prefix to parse.
3. **`acl::revoke` writes nothing when the SID holds no entry on the path** (your question 6), so a `revoke` of a path without one does not rewrite every file beneath it. Tests: the descriptor is unchanged, read back; the `toolchain revoke` of such a path says it held no entry.

**Mutations:** each of the three changes undone must be killed by its test.

**CHANGELOG:** Fixed (2, 3); Changed (1, with an **upgrade: read this** note: a configuration with such a read path no longer loads, and the message names a narrower path).

**Return:** `.git-exclude/review-requests/011o-4-windows-task-isolation/README.md`.
