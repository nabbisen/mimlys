# Handoff 014a — What a long-lived process needs (release 0.14, phase 1)

**RFC.** [RFC 014](../../accepted/014-local-daemon.md), decision 6's notes, read with its **Amendment 1**, item 3. **From.** The scope of 0.14 (`ROADMAP.md`, "Release 0.14", phase 1).

**Specification** (amended before this handoff):
- REQ-DEC-001 (steps of one task are sequential), REQ-TASK-006 (a cancel aborts a model request and terminates an execution in flight), REQ-CLI-005;
- external design §4 (the paragraph "Steps of one task never overlap");
- LIM-X3 (per task, never for a start made for no task, cleared at the task's end).

**Owner's decision (2026-11-27):** the scope of 0.14, as recommended.

**Purpose.** Four places of the code are correct today only because one process lives for one command and the store's lock keeps every other process out. A daemon removes both conditions. Each fix is right in embedded mode too, and none changes what a user sees today.

**Embargo:** none.

## Today

These are as found on `main` at `e85433a` (0.13.0); check them on today's `main`.

- **Two steps of one task can overlap.** `run_step` checks only that the task is `running` (`runtime/loop_.rs:224`). The step index is `load_steps().len()` (`:757`), and the only backstop is the schema's `UNIQUE (task_id, idx)` (`migrations/001_initial.sql:26`). A second step would most likely fail with a persistence error rather than be refused.
- **The interruptions make an entry for any task id.** `interruption()` (`runtime.rs:485`) does `.entry(task_id).or_default()` with no check of the task. `interrupt_step`, `interrupt_step_anyway`, `step_interruption`, `start_no_new_action` (`:542`), `no_new_action_requested` (`:497`), `boundary_interruption` (`:506`) and `execution_interruption` (`:514`) all go through it. Nothing removes an entry made for an unknown id.
- **`cancel_task` does not interrupt the step in flight** (`runtime/control.rs:1157-1230`). It transitions the task and closes its records (`close_action`), then `forget_task` drops the interruption entry, so a step in flight keeps tokens that nothing will cancel. The CLI's signal path interrupts before it cancels (`follow.rs:89-102, 426-444`), and that is what makes REQ-TASK-006 true today.
- **LIM-X3 in `mimlys-mcp`** (`server.rs:223-251, 395-474`):
  - `restarts: Mutex<HashMap<TaskId, u32>>` is never cleared;
  - `started_once` is per server, so a server started by a listing for no task makes a task's first start count as a restart.

  The worker's own count (`mimlys-exec/src/worker.rs:247, 644-652`) is never cleared either.
- **`App::server_status`'s comment says "without starting any"** (`app.rs:986`), but `servers.status()` starts the servers.

## Items

1. **A per-task step guard (REQ-DEC-001).**
   - **The guard:** the runtime holds a guard per task for the length of a step. A second `run_step` of a task whose step is in flight is refused at once as `conflict`, with a message that names the task. It never reaches the store.
   - **Not a lock around the store:** a step of another task is not delayed by it.
   - **It is the runtime's,** so that no caller can bypass it: neither the CLI today nor the scheduler of 014b.
2. **The interruptions refuse an unknown or ended task.**
   - **Every request method** (`interrupt_step`, `interrupt_step_anyway`, `start_no_new_action`) refuses a task that is not known, or that is terminal. It returns an error that says which, and creates no entry.
   - **The internal readers** (`step_interruption` and the boundary and execution readers) create nothing either: they read an entry if one exists.
   - **Clearing at transitions** (`transitioned`, `forget_task`) stays as it is.
3. **`cancel_task` interrupts the step in flight first (REQ-TASK-006).**
   - **The order:** before it closes the task's records, it asks the step in flight to stop. A model request is aborted, and an execution is terminated within LIM-O2.
   - **An action that cannot be undone** is recorded with its outcome unknown, as REQ-TASK-006 and REQ-CLI-005 say, and the command's result names it, as today.
   - **One meaning for the user:** a cancel of a task with no step in flight is unchanged, and a cancel during a step ends in `cancelled` with nothing left running.
   - **Say in the request** how you made it safe against the step finishing while the cancel runs. Whichever comes second must find the other's work done; neither may write over it.
4. **LIM-X3 per task.**
   - **What counts:** only a start made for that task, after its first. A start made for no task (a listing, `status`) never counts for any task.
   - **Clearing:** both counts, the server's and the worker's, are cleared when the task ends (`forget_task`).
   - **A refusal once given stays for the task's life,** whatever started the server since (RFC 014, decision 6's note).
   - **The comment of `App::server_status`** says what the code does.

## Tests (by state, §3c)

- **Item 1:** two `run_step` calls of one task at once. One runs; the other is refused `conflict` and writes nothing (the steps, events and actions are counted). A step of another task runs meanwhile.
- **Item 2:** each request method on an unknown id and on a cancelled task: refused, and the registry of interruptions is unchanged (expose its size to tests).
- **Item 3:** a cancel while a model request is in flight (the mock holds the request): the request is aborted, the task ends `cancelled`, and no step writes after it. The same while an execution runs: the tree is gone within LIM-O2. An irreversible action in flight: recorded unknown and named.
- **Item 4:**
  - a server started by a listing, then a task's first start: the task's count is 0;
  - a task's restarts up to LIM-X3, then refused, and still refused after a listing restarted the server;
  - the counts gone after the task ends (exposed to tests).
- **Nothing a user sees changes:** the CLI's integration tests pass unchanged.

## Mutations

Run each, and say which test kills it:
- the guard removed;
- an interruption request creating an entry for an unknown id again;
- `cancel_task` not interrupting;
- a listing's start counted against a task;
- the counts not cleared.

## Book and CHANGELOG

- **The book:** nothing, unless a page states one of the behaviours above. Say what you found.
- **The CHANGELOG, under Unreleased, Fixed** for item 3: a cancel through the core interrupts the step in flight. It cannot be seen through the CLI today, because the signal path interrupts first; say so, and do not claim a user-visible fix. Items 1, 2 and 4 are internal; one line, "for the daemon", unless you find a visible effect.

## Return

- **PRs on `main`,** with their CI. Close your throwaway PRs and delete their branches when their result is in the request (the handoffs' common rules).
- **The request:** `.git-exclude/review-requests/014a-long-lived-process/README.md`, with:
  - the PRs and their CI runs;
  - the mutations, each with what killed it;
  - what is still open, and why;
  - anything in external design §4 that the code showed to be wrong or unclear. Say it, and do not work around it.
