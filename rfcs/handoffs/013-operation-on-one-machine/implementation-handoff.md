# Handoff 013 — Operation on one machine, part one (M7a, release 0.10)

**RFC.** [RFC 013](../../accepted/013-operation-on-one-machine.md), decisions 1 to 4 (accepted 2026-11-10) · **Specification.** REQ-PERS-013, REQ-WS-013, REQ-WS-014, REQ-CLI-007 (new); external design §14.6, §16, §19; limits LIM-O3 (unchanged) · **Not here.** Decisions 5 to 8 (admission, the CPU part of clause C, the engine gate, the machine view) are part two, M7b, a later handoff.

## Status (2026-11-11): approved; one correction, then the merge

013-1 to 013-4 (PRs #34 to #37) are approved. The type split (`PersistenceRead`), the momentary try-lock that names the holder without holding it, the records-not-a-walk storage view and prune's three guards are what was asked, and each has a mutation that was caught.

**Your questions, decided** (the specification now says so):
1. The momentary shared try-lock: accepted. REQ-PERS-013 says "hold no lock while they read".
2. `mimlys.holder`: accepted; external design §14.1 lists it.
3. Both numbers registered: **LIM-I8** (tasks in the text of `status --storage`, 10) and **LIM-O15** (`events --follow`'s poll, 500 ms). Use the ids in the doc comments.
4. Reflink judged from the filesystem's type, with `unknown`: accepted.
5. The queued flag: REQ-SCHED-001 is **not met** through 0.9; the requirement says so now, and part two (M7b) delivers the flag with its `reason`. The book stays as you wrote it until then.
6. `rollback --dry-run`: dropped from REQ-PERS-013's list.
7. Read commands record no event, `runtime.started` included: accepted, and external design §17 says so.
8. **Correction:** `status` must not start the MCP servers in the current directory when the data directory does not exist yet. It says `servers: not started (no data directory yet)` and lists the declared tools as declared. A server's working directory is the data directory; starting it anywhere else, from a read command, is a side effect nobody asked for. Test and mutation.
9. Unreversed promotions kept even when no longer reversible: accepted.
10. Leftovers after a failed deletion, finished by `task delete`: accepted.

**Also, in the same commit:** the CHANGELOG names the public API change (`Persistence` and `WorkspaceProvider` split into reading and writing traits, `PersistenceError::MigrationPending`) under Changed, for library users.

**Landing:** `main` refuses merge commits, so after the correction the architect lands the stack as a linear series (the tree compared with the CI-verified head), as for handoff 012.

## 1. Read-only open (REQ-PERS-013)

- **A second persistence handle type.** `mimlys-persist` gains a read-only open alongside `open`. It opens the database as a WAL reader (SQLite's read-only mode), takes **no** lock on `mimlys.lock`, and runs **no** recovery, integrity check (LIM-P5 belongs to the writer), reaping or migration. Its handle exposes only reading methods. A read command cannot call a writing method because the type does not have one: no runtime check stands in for that.
- **Which commands use it:** `status`, `events`, `task list`, `task show`, `metrics`, `follow`, `toolchain list`, `check`, and `promote --patch` and `rollback --dry-run` if, and only if, the code shows that they write nothing. Say which of the last two qualified and why.
  - Every other command keeps today's exclusive open and its LIM-O3 wait.
  - `follow` of a task another process runs polls the events through the reader. Keep the interval to a named constant; if it is a new number, register it.
- **Truth on the screen:**
  - while another process holds the lock, `status` adds `store: in use by runtime <pid> since <time>`; the lock file's content gives pid and time, if it does not, have the writer record them;
  - a store that needs recovery (the lock is free and the last owner ended uncleanly, as the existing recovery detects) is read as committed, and `status` says `recovery pending: the next command that writes recovers it`;
  - a read command never prints "locked".
- **Schema:** a reader that meets a newer schema than it knows refuses with the reason and does not migrate. One that meets an older schema reads it if it can; otherwise it says `migration pending`.

## 2. Storage view (REQ-WS-013)

`mimlys status --storage`, read-only:
- the store's path;
- its filesystem, and whether it reflinks (the existing materialization's detection);
- bytes by kind: working trees, snapshot objects exclusive and shared, blobs, interpreters, database and WAL, temporary;
- the tasks that hold the most, with state and age.

The numbers come from what the store already records (manifests, ownership, the per-task footprint). Only `tmp/`, the interpreters and the database files are measured on disk. JSON (`--json`) and text, both stated in `cli.md`. A test checks the sum against a `du` of the data directory, within the WAL's and the filesystem's slack (state the tolerance and why).

## 3. Prune (REQ-WS-014)

`mimlys task prune [--state completed,failed,cancelled,…] [--older-than <duration>] [--audit] [--yes]`:
- lists the matching **terminal** tasks with their footprints and the total;
- asks once (unless `--yes`), then deletes each through the existing `task delete` path, one at a time, reporting each result;
- never touches a task that is not terminal;
- lists a task whose promotion is still reversible as `kept: promotion reversible` and skips it;
- keeps audits unless `--audit`.

A failure midway leaves the remaining tasks untouched and names them. The duration syntax is the one the CLI already uses, or a new one stated in `cli.md`.

## 4. The book (REQ-CLI-007)

- **A "Storage" page:** what lives where (external design §14.1 in the user's words), reflinks per filesystem, what grows, how to reclaim (`status --storage`, `task prune`, `task delete`).
- **A section "Several projects, several jobs" in Getting started:**
  - one store per user;
  - a second terminal can now watch;
  - two runtimes on two data directories do not know each other until part two;
  - the two reasons a task is queued (`concurrency` today; `admission` with part two).

The examples are tested where they are commands.

## 5. Tests (each names its requirement; mutation checks)

- **Read-only open:**
  - a reader opens while a writer holds the lock, and reads a running task's state and events;
  - the reader runs no recovery: a store left unclean stays unclean after `status`, and says `recovery pending`;
  - the reader's handle has no writing method (a compile-time property: a test that the type does not implement the writer's trait is enough);
  - a reader meeting a newer schema refuses.
- **The storage view** sums to the disk within the stated tolerance; **reflink** is detected as the materialization detects it.
- **Prune:**
  - only terminal tasks go;
  - a reversible promotion is kept;
  - `--older-than` and `--state` filter;
  - the listing comes before anything is deleted;
  - a failure midway leaves the rest.
- **Mutations:** the reader takes the lock; the reader runs recovery; prune deletes a non-terminal task; prune deletes a task with a reversible promotion; the storage view walks the working trees instead of using the records (if that changes the result).

## 6. Return

`.git-exclude/review-requests/013-operation-part-one/README.md`: the changes, the gate, CI per platform with skips, the mutations, the commands found to be read-only and why, and every number you had to register.
