# Handoff 014b — The control API: requests and responses, one handler, the driver and the follower (release 0.14, phase 1)

**RFC.** [RFC 014](../../accepted/014-local-daemon.md), decision 1, read with its **Amendment 1**, items 1, 2 and 9. **From.** The scope of 0.14 (`ROADMAP.md`, "Release 0.14", phase 1), after 014a.

**Specification** (amended before this handoff):
- REQ-ARCH-004 and REQ-ARCH-009;
- external design §4, whole: "Requests and responses", "The driver and the follower", "Steps of one task never overlap", "A cancel waits for the step in flight", "Every other state-changing request on a task whose step is in flight", "The session and the request id";
- §17.1 (`request_id?` on the event envelope);
- REQ-CLI-005, REQ-TASK-006, REQ-PERS-013 (readers direct).

**Owner's decision (2026-11-27):** the scope of 0.14. The control API is made real first, and the embedded mode goes through it.

**Purpose.** 014c puts a socket between a client and the runtime. That is a small step only if the CLI already talks to the runtime through requests and responses, in one process. This handoff makes that true. **Nothing a user sees changes:** the same commands, outputs, codes, events and prompts.

**Embargo:** none.

## Today

As found on `main` at `25517b0`; check them on today's `main`.

- **The CLI calls about twenty `Runtime` methods directly,** through `app.runtime` (`app.rs:240`): `create_task` (`commands/run.rs:112`), `resume_task`, `pause_task`, `cancel_task`, `delete_task` (`commands/task.rs`), `approve_action`, `reject_action`, `answer_question`, `grant_capability`, `revoke_capability`, `rollback_workspace` and `revert_promotion`, `promotion_plan` and `promote_workspace`, `plan_effort` (`target.rs:48`), `model_change` (`main.rs:613`), `record_started`, `record_server_events`, `record_stopped`. Readers call the core's `Reader`.
- **The follow mode drives:** `follow::drive` (`follow.rs:412`) calls `run_step` while a task runs, prompts while it waits, and prints by polling `get_events`. `run_until_wait` exists, and nothing calls it.
- **The stop signals are method calls** (`follow.rs:89-105`): `start_no_new_action`, `interrupt_step`, `interrupt_step_anyway`, `irreversible_in_flight`, with the stages and the watchdog of REQ-CLI-005.
- **`Session` is `{id: "pid:<pid>", os_user}`** (`runtime.rs:37`, built at `app.rs:842`). No request id exists.
- **The store refuses a newer schema** (`sqlite.rs:156`, `reader.rs:102`), so a migration would make a store unreadable to 0.13.0. That is why the request id lives in the event record.

## Items

1. **The requests and the responses** (`mimlys-core`, a module of its own).
   - **`Request`,** one variant per state-changing verb of external design §4, with the arguments the CLI passes today: `create_task`, `resume_task`, `pause_task`, `cancel_task`, `approve_action`, `reject_action`, `answer_question`, `grant_capability`, `revoke_capability`, `rollback_workspace`, `promote_workspace`, `revert_promotion` and `delete_task`.
   - **The stop signals as requests** (REQ-CLI-005): the first asks for a pause at the next boundary, with no new action in the step in flight; the second cancels. Keep the exception for an action that cannot be undone exactly as it is.
   - **The rest of what the CLI asks of the writing runtime:** the effort plan, the model change, the runtime's own events, the probe's start check. Add each as a request, or move it where it belongs (a reader, or the client's own computation). Say which you chose for each, and why.
   - **The envelope:** `{request_id, session, request}`. **The response:** the result, or the error as today's error kinds and details (`app.rs:153-203`), so that one error means the same through any transport.
   - **All serializable** with `serde`, versioned with the runtime. The socket's hello (014c) carries the version, so the types themselves need none.
2. **One handler.**
   - **`Runtime::handle(envelope)`** (blocking where the verb blocks: `cancel_task` waits, as 014a made it) is the only way into the state-changing verbs.
   - **The verb methods become `pub(crate)`,** so that the compiler enforces "no command calls the runtime's methods around it" (external design §4). Tests of the core keep them.
   - **Readers stay direct:** `get_task` and `get_events` for following, `show_task`, `list_tasks`, `metrics`, `footprint`, `activity` and `holdings` stay with `Reader` or with read methods that change nothing. A reader never needs the handler.
3. **The client** (`mimlys`): every command sends its requests through one client, with one transport now: in-process, which calls `handle` on the command's runtime. The client makes the request id (UUIDv7, the ids' own kind) and the session. 014c adds the socket transport behind the same client, so write it with that in view: no command knows which transport it has.
4. **The driver and the follower.**
   - **The driver** (in `mimlys-core`, the scheduler 014c will own across clients) runs the steps of the tasks it owns, one at a time (LIM-X1 = 1), until a task waits or ends. A request that makes a task `running` (create, resume, approve, reject, answer) hands the task to the driver.
   - **The follower** (`follow.rs`) reads the task and its events (a reader), prints them, prompts when the task waits, and sends what the user decides as requests. It sends the stop signals as requests too. It never calls `run_step`.
   - **In embedded mode both live in the command's process.** The driver ends when the command ends, as today. A command without a terminal ends at a wait, as today.
5. **Requests on a task whose step is in flight** (external design §4):
   - **a pause** is noted at once and taken at the boundary;
   - **a grant or a revocation** waits for the step to end, then applies from a fresh read;
   - **no write is made to a task while its step holds it,** except by the step. Use 014a's slot.
   - In embedded mode today this happens only through the follower's own signals. Test the rule in the core with two threads.
6. **Request ids** (external design §4 and §17.1).
   - **Recorded:** every event that a state-changing request's change writes carries `request_id`.
   - **Repeated:** a request whose id is already recorded returns `already applied` with the event's position and the task now, and applies nothing. A refused request wrote nothing, so a repeat is answered afresh.
   - **The lookup** uses an index the store already has (the events of the task, `events_task`). For `create_task`, which has no task yet, find a way that needs no new index and is bounded, and say what it is. **No migration, and no schema version:** a 0.13.0 binary must still read a store that 0.14 wrote, including the new field. Check this with the 0.13.0 binary, as 015d did.
   - **The embedded client never repeats a request** (there is no connection to break), but the rule is there and tested, for 014c.

## Tests (by state, §3c)

- **Nothing changes for a user:** every existing test passes unchanged, the integration tests included, and the signal tests alone and in the foreground (`m5_signals`, `m6a_host_tools`, `panic_hook`). Where a test compares outputs, they are byte for byte the same. If a test must change, say which and why. It should not.
- **The one way in:** a test that the verbs are not public (a compile-fail test, or a `cargo public-api`-style check if you prefer: say which).
- **Request ids:** each state-changing verb records its id on the events it writes. A repeat of an applied request returns `already applied` and writes nothing (the events counted). A repeat of a refused request is refused again, afresh. `create_task` repeated makes no second task.
- **The rule of item 5:** a grant during a step applies after it, from a fresh read; a pause during a step is taken at the boundary; the step's end writes over neither.
- **The driver and the follower:** a follower that ends (the command ends) ends the embedded driver, as today. A task that waits returns control to the follower's prompt.
- **0.13.0 on a 0.14 store:** the commands of 015d's table, on a store whose events carry `request_id`. They work as before.

## Mutations

Run each, and say which test kills it:
- a verb public again, and a command calling it;
- the request id not recorded;
- a repeat applied twice;
- a grant during a step written over by the step's end;
- the follower calling `run_step`.

## Book and CHANGELOG

- **The book:** nothing, unless a page states how a command reaches the runtime.
- **The CHANGELOG:** under Added, `request_id` on the events of a change (visible in `events --json`), additive. Otherwise one line, "internal, for the daemon".

## Return

- **PRs on `main`,** as many as you need, each with its CI. This is a large change, so a stack is welcome: for example, (1) the types and the handler; (2) the client and every command through it; (3) the driver and the follower; (4) the request ids. Close your throwaway PRs and delete their branches when their result is in the request (the handoffs' common rules).
- **The request:** `.git-exclude/review-requests/014b-control-api/README.md`, with:
  - the PRs and their CI runs;
  - the mutations, each with what killed it;
  - the choices that items 1, 3 and 6 leave to you, each with its reason;
  - the 0.13.0 check;
  - what is still open, and why;
  - anything in external design §4 that the code showed to be wrong or unclear. Say it, and do not work around it.
