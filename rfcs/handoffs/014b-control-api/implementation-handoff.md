# Handoff 014b — The control API: requests and responses, one handler, the driver and the follower (release 0.14, phase 1)

**Status (2026-11-28): done.** Items 1 to 6 landed at `3a2efec`, 014b-5 at `0297d3a`, 014b-6 at `7756a8c` and 014b-7 at `9244d7b` (`.git-exclude/reviewed/handoff-014b/README.md` and its `-5`, `-6` and `-7`).

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

## After review: 014b-5

The review of 2026-11-27 (`.git-exclude/reviewed/handoff-014b/README.md`) approved the stack #224 to #227 and landed it by fast-forward (`main` = `3a2efec`). External design §4 now holds your eight points, and its table has the stop requests and the asks. Five things remain.

1. **A repeat never answers a change that took effect as an error** (your §7, points 3 and 4, and `client.rs:323`).
   - **What is wrong:** a repeat of a verb that returns no task answers `conflict`, "applied already", where the first answer was a success. A user whose connection broke would read a failure for a grant that worked. And a promotion that failed after it wrote is answered `AlreadyApplied` at its first event, which calls a failure applied.
   - **Change** (external design §4, "The recorded outcome"):
     - **applied:** a success. The task now, for a verb that returns one; otherwise the recorded event of the change (the grant, the snapshot, the promotion), so that the command prints what it would have printed, or says "done earlier" with what was done;
     - **failed after it wrote:** the recorded failure, with its reason;
     - **refused:** answered afresh, as now;
     - **a repeated deletion whose task is gone:** the client says that the task is gone, not `not_found`.
   - **Tests:** each case, through the client.
2. **The pinning test decides by state** (your §1).
   - **What is wrong:** `m5_pinning_breaks_visibly` passes or fails by when the test server's `list_changed` is read, against when the next boundary asks. The driver's yield made the failure rarer; it did not remove it.
   - **Change:** the test server sends `notifications/tools/list_changed` **before** its reply to `change_definition`, so that the boundary after that step has the notice whatever the timing.
   - **The yield:** keep it only if it has a reason of its own, not the test's. A real server may notify after its reply, and reading a waiting notice before a boundary is right for it. If you keep it, its comment says that reason and not the test's.
   - **Evidence:** the test run many times on CI (as 014a-5 did on Windows), and the yield removed for a run, to show that the test no longer needs it.
3. **The core tests REQ-CLI-005's exception** (the lead's mutation: `apply_noted` cancelling a task the step halted survived every core test).
   - **The test** (`in_flight.rs`): a stop noted during a step with an action that cannot be undone in flight leaves the task `halted` `unknown_outcome`, never `cancelled`. Only the integration test `m5_signals` held this rule; 014c's second client relies on it.
4. **The CHANGELOG says what changed in the events:** besides `request_id`, a first stop signal during a step now writes one `task.pause_requested`, where it could write two. Under Changed, as a script that counts events would see it.
5. **One comment:** `commands/events.rs` says `EVENTS_POLL_MS` is not in the register. It is LIM-O15, so the comment names it.

**Accepted as you did them:**
- **The four asks:** they depend on what the runtime was started with, which a client never sends.
- **The runtime's own events as host methods:** no client can write `runtime.started`.
- **`RuntimeError` itself serialized.**
- **The creation's lookup,** bounded by the number of tasks, with the plan read in a test.
- **A request that finds a step returning** waits for it. It is in §4 now.
- **The 0.13.0 check.**

**Mutations:** for items 1 and 3, one each, and say which test kills it. For item 2, the before and after on CI.

**Return:** `.git-exclude/review-requests/014b-5-after-review/README.md`.

## After review: 014b-6

The review of 2026-11-27 (`.git-exclude/reviewed/handoff-014b-5/README.md`) approved #228 and landed it by fast-forward (`main` = `0297d3a`).
- **The yield's removal is accepted:** nothing a run shows needed it.
- **Your points 2 to 4 are decided** in external design §4, "The recorded outcome".
- **Point 1, the pinning test's premise:** decided too, but its change goes into 014c's handoff, as its first item, beside that handoff's other test work.
- **Point 5 is 014c's.**

Two things remain, each small.

1. **A failure keeps its kind on a repeat** (your point 3).
   - **What is wrong:** a promotion that failed as `conflict` answers its repeat as `internal` (`RuntimeError::Recorded`), with the same words. One request, two kinds: a script that tells a conflict from an internal fault would be misled.
   - **Change:** the failure's event records the error's kind as the optional field `kind` (`workspace.promotion_failed{reason, kind?}`, external design §17.1). The repeat answers with that kind, and with `internal` only for an event that holds none (written before this change). A promotion that stopped before its apply was recorded keeps the kind of what stopped it.
   - **Check:** the 0.13.0 binary reads a store with the field, as 014b did for `request_id`.
2. **A request whose task was deleted since** (your point 4).
   - **What is wrong:** a repeated grant (or any verb with no task result) whose task was deleted between the request and its repeat answers `not_found`. The grant took effect, so the truth is that it was applied and that the task is gone.
   - **Change:** while the task's events are kept (a default deletion), the repeat is answered as applied earlier, and the client says that the task is gone since. After `delete --audit`, nothing of the task is known, and `not_found` stays.

**Your point 2, decided to follow the code:** a repeated creation finds its task while the task's record stays, deleted or not. External design §4 says so now. Add the test you did not write: a creation, then a default deletion, then the repeat, is answered with that task, and makes none. After `--audit`, the repeat makes a new task.

**Tests:** one for each case above. **Mutation:** for item 1, the kind dropped on the event. Say which test kills it.

**Return:** `.git-exclude/review-requests/014b-6-after-review/README.md`.

## After review: 014b-7

The review of 2026-11-28 (`.git-exclude/reviewed/handoff-014b-6/README.md`) approved #229 and landed it by fast-forward (`main` = `7756a8c`).
- **Your correction of item 2's premise holds:** a grant was already answered as applied, and rollback, promote and revert were not.
- **Your point 3, the event of a stop before the apply, is accepted.** It is true, and external design §17.1 now says that it can follow a backup with no promotion started.
- **Your points 6 and 7 are 014c's and 014d's:** the book's page and the reachable `Gone` arms.

Four things close phase 1. Neither field is released, so a rename costs nothing now.

1. **`kind` becomes `error_kind`** on `workspace.promotion_failed` (your point 5).
   - **What is wrong:** `workspace.unreadable{kind}` holds the words of an input/output error. A second `kind` on another event, holding the error kinds of §18, is one name with two meanings for a script that reads both.
   - **Change:** the field is `error_kind`, in the event, the code, the tests and the CHANGELOG (external design §17.1).
2. **The detail comes back too** (your point 1).
   - **What is wrong:** a repeated `conflict` keeps its kind and words, but its `detail` (`paths`, `kept`) is `{}`. A script that reads `detail.paths` after a repeat would find none.
   - **Change:** the failure's event records the detail as the optional field `error_detail`, and the repeat answers with it. The repeat is then the first answer, whole.
3. **An unknown kind is named** (your point 2, and the lead's gap: no test feeds it).
   - **Change:** a kind this release does not know is answered `internal`, and its message names the word that was recorded. The event keeps the word, as it does.
   - **The test:** an event written with a kind of a newer release. The answer is `internal`, naming it.
4. **`Response::into_result` cannot call a success an error** (your point 4).
   - **What is wrong:** it turns an applied request with no reply into `conflict`, "applied already". No caller should be able to do that, and 014c must not meet it.
   - **Change:** remove it if nothing needs it. If something does, make it answer the applied request as `Client::applied_as` does.

**Check:** the 0.13.0 binary reads a store with `error_kind` and `error_detail`, as before.

**Tests:** one for each item. **Mutations:** for items 2 and 3, one each, and say which test kills it.

**Return:** `.git-exclude/review-requests/014b-7-after-review/README.md`.
