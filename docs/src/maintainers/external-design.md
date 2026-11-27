# mimlys — External Design

**Status:** Baseline 1.0 (2026-10-02). Supersedes the draft external design of the startup phase (owner's archive).
**Normative** for everything observable from outside: states, schemas, effects, rejections, tools, the control API, the CLI, the configuration, and the guarantees mimlys gives. Internal structure appears only where a requirement demands it. Every section names the requirements it satisfies; §24 lists every requirement with the section that satisfies it.

Numbers are cited from [Limits and Defaults](./limits-and-defaults.md), threats from the [Threat Register](./threat-register.md), terms from the [Glossary](./glossary.md), requirements from the [Requirements](./requirements.md).

---

## 1. System context

```text
                 User ──────────────┐            Administrator
                 (goals, approvals, │            (configuration, B9)
                  answers, grants)  │                    │
                                    ▼ B1                 ▼
   ┌─────────────────────────────────────────────────────────────────┐
   │  mimlys runtime (in the `mimlys` CLI process in v1)             │
   │  tasks · steps · validation · grants · approvals · context      │
   │  persistence · recovery · events                                │
   └───┬──────────────┬───────────────┬───────────────┬──────────────┘
       │ B2           │ B3            │ B4            │ B6
       ▼              ▼               ▼               ▼
   model backend   built-in tools   worker ──B5──▶ OS   MCP servers
   (local HTTP     (capability      (sandbox,          (rypsok, …)
    or fake)        handles)         host tools)
       B7 ▼                         B8 ▼
   data directory: database,        project scope: materialized in,
   blobs, snapshots, workspaces     promoted out
```

Boundaries B1 to B9 are defined in requirements §3. One runtime per data directory; one user; the model is a replaceable backend; tools are built in, served over MCP, or declared host tools (M6).

### 1.1 What mimlys guarantees to the user

| Guarantee | Requirement |
|---|---|
| Nothing executes that was not validated against the schema, the task's grants and its budgets | REQ-ARCH-005, REQ-DEC-004 |
| Nothing touches the project scope except `promote`, which shows the diff and asks first | REQ-SEC-015, REQ-WS-007 |
| Model text never changes grants, budgets, policy or approvals, and never writes the approval screen | REQ-SEC-002, REQ-APR-002 |
| Every step, execution and model request is bounded; every cut is reported | P5, REQ-TOOL-008 |
| Every transition is durable; a crash never turns an unfinished action into a success, and never replays an irreversible one | REQ-PERS-003, REQ-PERS-007, REQ-PERS-008 |
| A failed write action is rolled back before the model sees its result | REQ-TOOL-010 |
| Generated code runs only in the sandbox with no network and no environment; host tools only under confinement | REQ-SBX-001, REQ-SBX-004, REQ-CONF-004 |
| What the installation can enforce is detected and shown, never assumed | REQ-CONF-003 |
| Every stop has a reason the user can read | REQ-TASK-007 |

### 1.2 What mimlys expects from the user and the administrator

| # | Expectation |
|---|---|
| 1 | Grant only what the task needs; prefer the `workspace` profile |
| 2 | Read the approval screen; the model's stated reason is labelled and untrusted |
| 3 | Review the diff before promoting |
| 4 | Keep secrets out of the project scope or in the excluded patterns |
| 5 | Run a local backend, or set `remote_backend_allowed` knowingly |
| 6 | Declare effects honestly for external tools; an undeclared tool is treated as destructive |
| 7 | Keep the toolchain paths declared for host tools minimal |

---

## 2. Actors and channels

| Actor | Channel | May do |
|---|---|---|
| User | Control API through the CLI | Create, pause, resume, cancel tasks; approve, reject, answer; grant and revoke within the ceiling; rollback; promote; read events, metrics, status |
| Administrator | Configuration file | Set the ceiling, profiles, exclusions, backends, servers, host tools, retention |
| Model | Decision schema only | Propose `act`, `plan`, `ask_user`, `complete`, `give_up` |
| Runtime | — | Everything else |

The model has no channel to any operation of the control API (REQ-SEC-007).

---

## 3. API tiers (REQ-ARCH-004)

| Tier | Consumers | Content |
|---|---|---|
| Control API | Clients | The verbs of §4 |
| Adapter SPI | Implementations | `ModelBackend`, `ToolProvider`, `Executor`, `Persistence`, `WorkspaceProvider` (the snapshot store of REQ-ARCH-004, widened by RFC 006 to materialization, promotion, fork and deletion), `ApprovalChannel` |
| Loop API | Embedding applications | `run_step`, `run_until_wait`, with the same validation as the CLI path |

The adapter SPI, informative shape:

```text
ModelBackend    generate(request) -> text | error ; cancel() ; usage()
ToolProvider    definitions() ; execute(authorized_action, context) -> execution_record
Executor        run(execution_request) -> execution_record ; cancel(execution_id) ; probe()
Persistence     load(task) ; persist(transition) ; append(event) ; put_blob / get_blob
WorkspaceProvider materialize ; snapshot(root, reason) -> snapshot_id ; restore(snapshot_id, root) ; diff(a, b) ; promote ; materialize_from ; delete
ApprovalChannel request(rendered) -> decision        (v1: the CLI prompt)
```

`execute` and `run` accept only `AuthorizedAction` values that the validator constructs (REQ-ARCH-005).

---

## 4. Control API

| Verb | Valid when | Effect | Requirement |
|---|---|---|---|
| `create_task` | — | Creates the task in `created`, materializes the workspace | REQ-TASK-001 |
| `get_task` | any | Returns the task with its current step, pending item, counters, plan, grants | — |
| `list_tasks` | — | Returns tasks with state, goal, timestamps | — |
| `resume_task` | `created`, `paused`, `halted` | → `running`; from `halted` may extend `steps` and `deadline` within the ceiling | REQ-TASK-005 |
| `pause_task` | `running` | → `paused` at the next step boundary | REQ-TASK-005 |
| `cancel_task` | any non-terminal | → `cancelled` | REQ-TASK-006 |
| `approve_action` | `waiting_user` (approval, or from 0.13 a plan-first task's plan) | Records the approval, with the user's optional note; the action starts, or the gate of plan first ends; → `running` | REQ-APR-007, REQ-TASK-015 |
| `reject_action` | `waiting_user` (approval, or from 0.13 a plan-first task's plan) | Records the rejection with note and `halt` flag; → `running` (for a plan: still gated, the model asked for another) or `halted` | REQ-APR-007, REQ-TASK-015 |
| `answer_question` | `waiting_user` (question) | Records the answer; → `running` | REQ-APR-010 |
| `grant_capability` | any non-terminal | Issues a grant within the ceiling | REQ-CAP-004 |
| `revoke_capability` | any non-terminal | Revokes a grant | REQ-CAP-005 |
| `rollback_workspace` | not `running`, not `recovering` | Restores a snapshot | REQ-WS-006 |
| `promote_workspace` | not `running`, not `recovering` | Diff, confirm, backup, apply | REQ-WS-007 |
| `promotion_plan` | not `running`, not `recovering` | The diff and the conflicts a promotion would meet, without writing (`promote --patch`, `--summary`) | REQ-WS-007 |
| `revert_promotion` | not `running`, not `recovering` | Restores a promotion's targets from its backup snapshot (`rollback --promotion`) | REQ-WS-007 |
| `delete_task` | terminal | Removes the task's workspace, snapshots, blobs and backups after showing the footprint; the audit stays unless asked | REQ-WS-012 |
| `get_events` | any | Ordered events since a position | REQ-OBS-002 |
| `get_metrics` | — | Derived metrics | REQ-OBS-003 |
| `get_runtime_status` | — | Version, backend, tier, probe results, running tasks, pending items | REQ-OBS-006 |

**Requests and responses (from 0.14; RFC 014, Amendment 1).**
- **One handler.** The control API is a set of requests and responses, serializable and versioned with the runtime, handled in one place. Every client sends them: the CLI through an in-process transport in embedded mode, and through the daemon's socket in served mode (§19.5, from 0.14). No command calls the runtime's methods around it.
- **The driver and the follower.** The driver (the scheduler) runs the steps of the tasks it owns; a step is never run by a client. A client that follows a task reads its events, prompts the user, and sends the stop signals as requests (REQ-CLI-005):
  - the first asks the task to pause at its next boundary, its step starting no new action;
  - the second cancels it, which interrupts the step in flight (REQ-TASK-006).

  In embedded mode the driver lives as long as the command.
- **Steps of one task never overlap** (REQ-DEC-001). The runtime refuses a second step of a task whose step is in flight, as `conflict`, and refuses a step while a cancel of its task is in progress. An interruption request for a task that is unknown or ended is refused, and creates nothing.
- **A cancel waits for the step in flight** (REQ-TASK-006). It claims the task, so that no step begins, asks the step to stop, and waits for it to end, at most LIM-O19. Only then does it write. If the step has not ended by then, the cancel is refused `conflict`, the task is untouched, and the request stays so that the step ends `cancelled` when it can. The wait blocks its thread, and a cancel is never made from inside the step it waits for.
- **Every other state-changing request on a task whose step is in flight** (a pause, a grant, an approval) takes effect at the step's end and is never written over by it (from 014b, which gives the driver the requests).

**The session and the request id.**
- **The session.** Every request carries it: the client process and its operating-system user. Under a daemon the session is verified, not claimed (§19.5).
- **The request id.** Every state-changing request carries a request id, made by the client. The id is recorded with the change it caused, in the same write. A request whose id is already recorded returns the recorded outcome and applies nothing again.
- **A repeated request** is the client's own, with its own id, after a broken connection. A client never tells the user that it does not know whether a request took effect.

---

## 5. Task state machine (REQ-TASK-002)

```text
created ──▶ running ──▶ completed | failed | cancelled        terminal, immutable
              │  ▲
              │  ├── waiting_user     an approval, a question or a plan (plan first) is pending
              │  ├── paused           the user paused it
              │  └── halted           the runtime stopped it, with halt_reason
              └── recovering          found running at startup; resolves to halted or running
```

### 5.1 Transitions

| From | To | Trigger | Actor |
|---|---|---|---|
| `created` | `running` | `resume_task` (the CLI does this at creation unless `--hold`) | user |
| `created` | `cancelled` | `cancel_task` | user |
| `running` | `waiting_user` | an action needs approval; an `ask_user` decision; the first `plan` of a task created with `--plan-first` (`wait_kind: plan`, from 0.13) | runtime |
| `running` | `paused` | `pause_task`, at the next step boundary | user |
| `running` | `halted` | a halt reason of §5.2 | runtime |
| `running` | `completed` | `complete` decision, check passed or absent | runtime |
| `running` | `failed` | `give_up` decision; unrecoverable error | runtime |
| `running` | `cancelled` | `cancel_task` | user |
| `waiting_user` | `running` | `approve_action` (an action, or a plan: the task may now act); `reject_action` without halt (for a plan: the model is asked for another, and the task stays gated); `answer_question` | user |
| `waiting_user` | `halted` | `reject_action` with halt; expiry | user, runtime |
| `waiting_user` | `cancelled` | `cancel_task` | user |
| `paused` | `running` | `resume_task` | user |
| `paused` | `cancelled` | `cancel_task` | user |
| `halted` | `running` | `resume_task` | user |
| `halted` | `cancelled` | `cancel_task` | user |
| `running` | `recovering` | runtime start | runtime |
| `recovering` | `halted` | disposition of §16.3 (default) | runtime |
| `recovering` | `running` | `recovery.auto_resume` and a clean disposition | runtime |
| `recovering` | `cancelled` | `cancel_task` | user |

**Plan first (REQ-TASK-015; RFC 015 decision 6 and Amendment 1, from 0.13).** A task created with `--plan-first` (`task.created` records `plan_first: true`) is gated until the user approves a plan:
- **Before the plan:**
  - an `act` or a `complete` is rejected `state{reason: plan_required}`, which counts toward LIM-T2;
  - `ask_user` and `give_up` act on nothing and are allowed.
- **Its first `plan`** is recorded as a plan version (REQ-DEC-011), and the task waits as `waiting_user{wait_kind: plan}` (`pending: plan{version}`), with nothing acted.
- **`approve [--note TEXT]`** ends the gate.
- **`reject --note TEXT`** keeps it: the step of the plan ends `approval_rejected`, and the model is asked again, with the note as its `rejection_note` segment, as for an action.
- **`reject --halt`, `task cancel` and the expiry of a pending item (LIM-A1)** act as for an approval.
- **After the gate:** only the first approved plan is gated. Later plans are recorded and shown, never gated, and a fork is gated only when it is created with `--plan-first` itself.
- **The goal is never rewritten.** An approval's note reaches the model as a labelled `user` segment (`kind: approval_note`), bounded as a rejection's note is (LIM-D2, 500 characters).

`waiting_user`, `paused`, `halted` and `created` keep their state across a restart. A pause requested during a step is a visible `pause_requested` flag until the boundary.

### 5.2 Halt reasons (REQ-TASK-007)

`budget_steps`, `budget_rejections`, `budget_failures`, `budget_deadline`, `convergence`, `approval_rejected`, `approval_expired`, `question_expired`, `recovered`, `unknown_outcome`, `workspace_quota`, `workspace_unreadable`, `definitions_too_large`, `backend_unavailable`, `tool_unavailable`, `policy`. A halt `policy` carries `halt_detail` (from 0.13) naming the rule that stopped the task, `remote_backend_forbidden` (with the backend and the profile) or the completion check's refusal; a halt `backend_unavailable` of a task whose backend is no longer configured carries `backend_not_configured`, and of one whose backend cannot be made `backend_unusable` with the reason (§19.4), in the task's view and in its state change, so that a script can tell the causes apart as the text line does.

### 5.3 Terminal fields

| State | Field | Values |
|---|---|---|
| `completed` | `completion` | `verified` (the completion check passed), `asserted` (no check configured) |
| `failed` | `failure_reason` | `model_gave_up`, `unrecoverable` (with detail), `workspace_lost` |
| `cancelled` | `cancelled_by` | session reference |

### 5.4 Fork (REQ-TASK-004)

`create_task{from_task, from_snapshot?}` creates a new task from a snapshot of a terminal or halted task; the source is untouched. A halted source still holds its scope (§5.1, REQ-TASK-012), so a fork onto an overlapping scope is a `conflict` whose detail names the source and both remedies: `--scope` elsewhere, or `mimlys task cancel <source>` first.

---

## 6. Steps and decisions

### 6.1 Step phases (REQ-DEC-001)

`assemble → decide → validate → wait? → execute → observe → evaluate`. A step record holds the phase, the decision hash, the rejection code or action id, the observation id, and the step outcome: `continued`, `rejected`, `waited`, `completed`, `failed`, `halted`, `paused`, `cancelled`.

### 6.2 Decision schema (REQ-DEC-002), version 1

The model returns exactly one JSON object.

| `decision` | Fields | Constraints |
|---|---|---|
| `act` | `tool` (string), `arguments` (object), `reason` (string, optional) | `tool` visible to the task; `arguments` per the tool's input schema; `reason` ≤ LIM-D2 |
| `plan` | `steps` (array of strings) | ≤ LIM-D3 items and characters |
| `ask_user` | `question` (string) | ≤ LIM-D4 |
| `complete` | `summary` (string) | ≤ LIM-D4 |
| `give_up` | `reason` (string) | ≤ LIM-D2 |

Examples:

```json
{"decision": "act", "tool": "read_file", "arguments": {"path": "src/lib.rs"}, "reason": "inspect the failing function"}
{"decision": "act", "tool": "run_script", "arguments": {"language": "javascript", "source": "console.log(1+1)", "mode": "read_only"}}
{"decision": "plan", "steps": ["read the failing test", "fix the off-by-one in sum()", "run the tests"]}
{"decision": "ask_user", "question": "Should sum() return 0 or null for an empty list?"}
{"decision": "complete", "summary": "Fixed sum() for empty input; the test passes."}
{"decision": "give_up", "reason": "The test needs a network service that is not available."}
```

Unknown fields are refused. The whole object is bounded by LIM-D1. `reason` is the model's text: shown separately on approval screens, never used by the runtime for any decision.

### 6.3 Validation order (REQ-DEC-004)

| # | Stage | Rejection on failure |
|---|---|---|
| 1 | Task is `running`, no pending user item | `state` |
| 2 | Size ≤ LIM-D1; exactly one JSON object; no duplicate keys | `malformed` |
| 3 | Decision schema (fields, types, enumerations, bounds) | `schema` |
| 4 | Tool visible through a grant | `unknown_tool` |
| 5 | Arguments against the tool's input schema | `schema` |
| 6 | Canonicalization: paths lexically normalized against the workspace root, NFC, case per filesystem; invalid names refused | `invalid_argument` |
| 7 | Authorization: paths inside the workspace or a granted host root and not excluded (an absolute path or a `..` escape is `path_outside_workspace`, identically whether or not it exists); destination arguments covered by a `network` grant; a host tool granted and its arguments within its declaration (§13.5). The effect rules are the ceiling and the grants: no separate rule of the profile exists (audit A-35, 2026-11-14) | `denied` |
| 8 | Semantic bounds: numeric ranges, pattern validity, size of `content` ≤ LIM-W3 | `invalid_argument` |
| 9 | Effect, `reversible`, `idempotent` and the approval requirement derived (§7.2) | — |
| 10 | Budget reservation: the deadline, and the workspace quota for a write (the step budget halts at the step boundary, §6.5; the executions of an action, LIM-T3, bound its retries, §7.1) | `budget` |
| 11 | Approval or question: the task waits; a rejection yields `approval_rejected` | `approval_rejected` |
| 12 | Execution through the executor; existence of paths established here | failure causes of §7.5 |

No filesystem access happens before stage 12. Stage 7 produces the same `denied` whether or not a path exists (REQ-DEC-007).

### 6.4 Rejection vocabulary (REQ-DEC-008)

| Code | When | `detail` keys | Hint |
|---|---|---|---|
| `malformed` | Not exactly one JSON object; too large; duplicate keys | `problem` from `not_object`, `multiple_values`, `trailing_text`, `duplicate_key`, `too_large`; `limit` | Return exactly one JSON object with a `decision` field |
| `schema` | Missing or unknown field, wrong type, bad enumeration | `path`, `expected`, `allowed` | The field at `path` must be `expected` |
| `unknown_tool` | Tool not visible | `tool`, `visible_tools` | Use one of `visible_tools` |
| `invalid_argument` | Out of bounds or invalid | `path`, `min`, `max`, `allowed` | Change the argument at `path` |
| `denied` | Outside the workspace or grants; excluded path; destination not granted; promotion proposed; arguments whose names cannot all be put to the user within LIM-I6; a host-tool argument outside its declaration | `reason` from `path_outside_workspace`, `path_excluded`, `grant_missing`, `destination_not_granted`, `scheme_not_https`, `destination_not_canonical`, `promote_not_allowed`, `arguments_do_not_fit`, `argument_not_allowed`; `grant_kind`; for `argument_not_allowed` the `argument` position and `value`, or `max`; for `destination_not_canonical` the canonical form in `value` (bounded by LIM-I9 like every string of a detail) | One fixed hint per reason: a relative path; other files; ask for the missing grant (its kind in `grant_kind` when there is one); a granted domain; the `https` form of the URL; the declared literals and pattern; for `arguments_do_not_fit`, another tool or a question to the user |
| `state` | Decision not allowed now | `reason` from `pending_user_item`, `completion_check_running`, `plan_required` (from 0.13: an `act` or a `complete` before the plan of a plan-first task is approved) | Wait for the pending item; for `plan_required`: state your plan first (decide `plan`) |
| `budget` | Would exceed a budget | `budget` from `deadline`, `workspace` | Finish or give up |
| `approval_rejected` | The user rejected the action, or the plan of a plan-first task (from 0.13) | none: the user's note (optional) reaches the model as its own `user` segment (`rejection_note`), never inside runtime feedback | Choose another approach |

Hints are at most LIM-I5 characters and come from a fixed table; a `denied` hint is chosen by its `reason`, so the model is told what to change, never a generic sentence (CLR-5). A hint names only keys its detail always carries. The model reads a rejection as the feedback segment `{code, hint, detail}`: the detail's fixed keys above, each string and list bounded by LIM-I9. The detail is stored with the step (persistence schema 3), so the request after a restart is the one the live run would have sent; a step stored before 0.10 has the code and its hint only. A fork's history and a compressed step keep their fixed summaries (REQ-CTX-004).

### 6.5 Counters and convergence (REQ-DEC-010)

| Counter | Increments | Resets | Limit | Halt reason |
|---|---|---|---|---|
| `steps` | every step | never | LIM-T1 | `budget_steps` |
| `consecutive_rejections` | a rejected step | an accepted step | LIM-T2 | `budget_rejections` |
| `consecutive_failures` | a failed action | a succeeded action | LIM-T6 | `budget_failures` |
| `identical_decisions` | same decision hash as the previous step with an unchanged snapshot | any other decision | LIM-T4 | `convergence` |

---

## 7. Actions

### 7.1 Lifecycle (REQ-TASK-002, D-03)

```text
proposed ──▶ rejected
   │
   ├──▶ waiting_approval ──▶ rejected | authorized
   └──▶ authorized ──▶ running ──▶ succeeded | failed{cause} | cancelled | unknown
```

`unknown` is terminal for the action; its task-level disposition is §16.3. It says that what the action did is not known: the action was running when the runtime stopped, or it cannot be undone (§7.2: "After `unknown`" is halt `unknown_outcome`) and its execution was ended by the runtime rather than by the tool (§7.5: a deadline, a resource limit, a lost worker or connection, an interruption; amended 2026-11-14, review of handoff 010e). Action content is immutable from `proposed`. A task cancellation also cancels an action that is `proposed`, `waiting_approval` or `authorized`, so that no action is left without a terminal status (clarified 2026-10-27 during the handoff 001 implementation).

### 7.2 Effects (REQ-TOOL-002)

| Effect | Meaning | Approval | After `unknown` | Parallel (DR-001) |
|---|---|---|---|---|
| `none` | No side effect and no data read | no | re-execution permitted | yes |
| `workspace_read` | Reads inside the workspace | no | re-execution permitted | yes |
| `workspace_write` | Writes inside the workspace; reversible through the snapshots, which cover everything materialized: paths excluded by LIM-W4 and LIM-W5 (`target/`, `node_modules/`, secret patterns) are neither snapshotted nor restored, so a write there is not undone | no, unless the profile lists the tool | rollback to the pre-execution snapshot; re-execution permitted | no |
| `host_read` | Reads a granted host path | no | re-execution permitted | yes |
| `host_write` | Writes a granted host path; reversible only when the tool backs up first | yes, unless reversible | halt `unknown_outcome`: the runtime does not undo a host write, even one the tool backed up first, so a backup only spares the approval (reserved, DR-017; amended 2026-11-14, as `plan_recovery` already does) | no |
| `external_read` | Sends arguments to an external service, mutates nothing | no, unless the profile requires it | idempotent: re-execution permitted; else halt `unknown_outcome` | yes |
| `external_write` | Mutates something outside the host | always | halt `unknown_outcome` | no |

"Re-execution permitted" means the user, or `recovery.auto_resume`, may let the task continue; nothing re-executes by itself (REQ-PERS-008). One function of the core derives approval, recovery and parallelism from the effect.

### 7.3 Action record

`action_id`, `task_id`, `step_id`, `tool`, `arguments` (canonical JSON), `action_hash`, `grant_set_hash`, `effect`, `reversible`, `idempotent`, `approval_required`, `approval_id?`, `deadline_ms`, `status`, `executions` (count), `created_at`, `finished_at?`.

### 7.4 Execution record and termination (REQ-SBX-003)

`execution_id`, `action_id`, `attempt`, `started_at`, `ended_at`, `termination`, `exit_code?`, `snapshot_before?`, `snapshot_after?`, `stdout_blob?`, `stderr_blob?`, `metrics{wall_ms, fuel?, memory_peak_bytes?, stdout_bytes, stderr_bytes, setup_ms?}`.

`metrics.fuel` is the fuel the execution consumed, or **absent (`null`, unknown, never `0`)** when the engine cannot give it. Wasmtime keeps the fuel counter in a register of the running function and writes it to the store only on a call, a return, `unreachable` and when the fuel runs out. A run that ends anywhere else, in guest code (`deadline` and `cancelled`, which are epoch interruptions; a trap such as an out-of-bounds access; a refused memory growth), leaves the store's counter stale, so its figure is not recorded. The figure is recorded for `exited`, `fuel_exhausted`, `unreachable` and for an end raised by a host call (`output_limit`). `wall_ms` says how long a run that ended without a fuel figure lasted.

| `termination` | Meaning | `exit_code` |
|---|---|---|
| `exited` | The program ended by itself | present |
| `trapped` | A WASM trap (detail names it) | absent |
| `fuel_exhausted` | LIM-S1 reached | absent |
| `deadline` | LIM-S2, LIM-O4, LIM-O5 or LIM-H1 reached | absent |
| `memory_limit` | LIM-S3 or LIM-H2 reached | absent |
| `output_limit` | LIM-S4 or LIM-H4 reached | absent |
| `process_limit` | LIM-H3 reached where the tree is ended for it (the macOS count watchdog); where the kernel refuses the extra process instead (Linux, Windows), the tool sees a failed `fork` and its own ending is reported | absent |
| `cancelled` | Cancelled by the user or by a task transition | absent |
| `host_fault` | The worker, sandbox or tool infrastructure failed (detail: `invalid_module`, `unavailable`, `worker_died`) | absent |

### 7.5 Failure causes and dispositions

| Cause | From | Disposition |
|---|---|---|
| `tool_error` | `exited` with non-zero code; a tool's structured failure; `trapped`; MCP `isError` | feedback |
| `timeout` | `deadline` | retry when idempotent and executions < LIM-T3, else feedback |
| `resource_exhausted{limit}` | `fuel_exhausted`, `memory_limit`, `output_limit`, `process_limit`, workspace quota | feedback; workspace quota halts |
| `sandbox_fault{detail}` | `host_fault` | retry once for `worker_died` when idempotent, else feedback; `unavailable` halts with `tool_unavailable` |
| `tool_unavailable` | an MCP server or host tool not reachable | retry per REQ-TOOL-009, then halt `tool_unavailable` |
| `output_invalid` | result violates the output schema | feedback |

**An action that cannot be undone and was ended by the runtime** (an external write, a host write, an external read that is not idempotent: "After `unknown`" halt `unknown_outcome` in §7.2) has an unknown outcome when its execution started and the runtime, not the tool, ended it: at `deadline`, `fuel_exhausted`, `memory_limit`, `output_limit`, `process_limit`, `cancelled` or `host_fault{worker_died}`, or by an MCP connection lost after the call was sent, a failed write of the request included, since it cannot tell whether the whole request reached the server (`host_fault{worker_died}` too; review of handoff 010e-5). It is recorded `unknown`, not `failed`, its observation's status is `unknown` (the observation and `action.finished` keep the cause that says how the runtime ended it: `timeout`, `resource_exhausted`, `sandbox_fault`), and the task halts `unknown_outcome` with the action listed, as after a crash (§16.3, REQ-PERS-008): a `failed` record would let the model try again an effect that may have happened. The table above applies to such an action only when the tool itself reported its end (an exit code, a structured failure, MCP `isError`, an output that violates the schema), or when nothing was started (`host_fault{invalid_module}`, `host_fault{unavailable}`, and a call that never left: the server not reachable, the request not handed to the transport (every `send:` failure of the SDK), or its restart limit LIM-X3 reached, each the cause `tool_unavailable`). A `trapped` end is the guest's own. (2026-11-14, review of handoff 010e.)

A failed `workspace_write` execution is rolled back before feedback (REQ-TOOL-010). Dispositions never consult the model.

---

## 8. Observations (REQ-CTX-006, REQ-CTX-009)

```json
{
  "observation_id": "…", "action_id": "…", "tool": "run_script",
  "status": "failed", "cause": "resource_exhausted", "cause_detail": {"limit": "fuel"},
  "rolled_back": true,
  "payload": {"termination": "fuel_exhausted"},
  "stdout": "Processing item 4500…\n[… 48213 bytes omitted …]\n…",
  "stderr": "",
  "notices": [{"code": "output_truncated", "stream": "stdout", "total_bytes": 52213}],
  "metrics": {"wall_ms": 30012, "fuel": 10000000000, "memory_peak_bytes": 1048576, "stdout_bytes": 52213, "stderr_bytes": 0}
}
```

| Field | Asserted by | Class |
|---|---|---|
| `status`, `cause`, `cause_detail`, `rolled_back`, `notices`, `metrics` | runtime | envelope, fixed vocabularies |
| `payload` | tool, validated against its output schema | external data |
| `stdout`, `stderr` | tool | external data, excerpts per LIM-C2 |

Every observation's payload, from any provider, is bounded last by the runtime at LIM-I19, after its redaction (`Observation::bound_payload`; handoff 013c): a tool's own bound comes first, the runtime's acts only on a payload still over it, and `content_truncated` is said once (the tool's total, when it said one). An observation may carry `full_payload` (an external tool's whole cleaned result, kept for the user when the copy the model reads was cut, itself bounded by LIM-S4; never in a context segment; handoff 009i). A result whose structure alone exceeds LIM-I19 reaches the model as `{"truncated": "…", "total_chars": N}`. `content_truncated.total_chars` is the sum of the full lengths of the strings and keys a result's bound cut, or its serialized size when it was replaced.

Notice codes: `output_truncated{stream, total_bytes, captured_bytes?, copy_cut?}` (`captured_bytes`, the bytes kept, only on the notice the worker sends when the capture itself was cut: a script's stream past LIM-S4, a host tool's past LIM-H4; handoff 004d-4; `copy_cut`, only on the excerpt's notice and only when true, the user's stored copy itself cut by a redaction that lengthened the text: handoff 004d-5), `content_truncated{total_chars}`, `entries_truncated{total}`, `entries_skipped{reason, count}` (reason from `symlink`, `too_large`, `unreadable`), `invalid_utf8_replaced{stream}`, `characters_removed{count}`, `symlink_omitted{path}` (materialization), `secret_redacted{name}`, `rolled_back`, `retried{attempt}`, `host_tool_not_started{reason}` (with `host_fault{unavailable}`: the command, its temporary directory or the spawn), `content_omitted{count}` (the content blocks of an MCP result that are not passed on as its payload, wherever they stand; handoff 009h), `stopped_before_start` (with status `cancelled` and no execution: a stop signal came after the action's step had begun and before the action began, §19.2; review of handoff 010e-3; or after the action was recorded begun but before anything was sent, when a wait before the send (an MCP server's pin check) ends at the first stop signal, review of handoff 010f). Every code carries at most the keys named here.

---

## 9. Capabilities and policy

### 9.1 Grants (REQ-CAP-002)

| Kind | Scope | Effect |
|---|---|---|
| `tool{name}` | one tool | the tool is visible and usable |
| `host_read{root, patterns}` | a directory outside the workspace, within a ceiling root | `host_read` actions on matching paths; **reserved**: no tool has that effect yet (DR-016) |
| `host_write{root, patterns}` | a directory outside the workspace, within a ceiling root | `host_write` actions on matching paths; **reserved**: no tool has that effect yet (DR-016, DR-017); promotion is the user's verb and needs no grant |
| `network{domains, ports}` | destination allowlist (M5) | `external_*` actions whose destination arguments match |
| `host_tool{name}` | one declared host tool (M6) | `run_host_tool` for that tool: the grant alone makes the gateway visible; `run_host_tool` is never a `tool` grant and never listed in `tools` |

Implicit, never granted explicitly: reading and writing inside the task's own workspace. A reserved kind is part of the vocabulary (REQ-CAP-002) but not of the interface until a tool uses it: the CLI refuses to issue it and the configuration refuses a non-empty `host_read_roots` or `host_write_roots`, each with the reason, so that no grant ever changes nothing. A grant record carries `grant_id`, `task_id`, `kind`, `scope`, `issued_by`, `issued_at`, `status` (`active`, `revoked`, `expired`), `valid_until?`.

### 9.2 Ceiling and profiles (REQ-CAP-003, REQ-CAP-009)

The ceiling lists, per profile, the tools, host roots, domains and host tools a task may be granted, and the maximum budgets. Shipped profiles:

| Profile | Tools | Writes | Approvals | Budgets |
|---|---|---|---|---|
| `read_only` | `read_file`, `list_dir`, `search_files` | none | — | defaults |
| `workspace` (default) | the eight built-in tools of M1 and M2 | workspace only | tools listed by the administrator | defaults |
| `project` | `workspace` plus promotion | workspace; promotion with confirmation | promotion always | defaults |

A profile's `tools` are the `tool` grants issued at creation and its `host_tools` the `host_tool` grants (none in any shipped profile); the user adds within the ceiling. `approval_tools` may name `run_host_tool`: approval then covers every host tool.

### 9.3 Grant lifecycle

`active → revoked | expired`. A grant never widens (REQ-CAP-006). The active set's hash (REQ-CAP-007) is recorded on every action and approval.

---

## 10. Approvals and questions

### 10.1 Rendered request (REQ-APR-002, REQ-APR-003)

```text
Approval required · task 018f… · step 7
Tool         delete_path
Effect       workspace_write · reversible: yes · idempotent: yes
Will         delete 3 files under the workspace
Arguments    path = "build" (resolved: …/workspaces/018f…/tree/build) · recursive = true
Grants used  tool:delete_path
Model's stated reason (untrusted): "remove stale build outputs"
Expires      never
[a]pprove   [r]eject   [h]alt (reject and halt)   [g]rant …
```

Every external string is quoted; control, invisible and bidirectional characters are removed with a notice; paths are absolute and canonical; the whole request is at most LIM-I6 characters.

**No argument is ever hidden** (audit A-02, the owner's decision of 2026-11-14). The request names every argument. Destinations and paths come first. Each value is bounded to its own share of LIM-I6, and a cut value says that it is cut and how many characters are left out; the request is never cut so that an argument disappears. Before deciding, the user can see every argument in full, sanitized: the prompt's `[v]iew` choice and `mimlys task show <task> --arguments` print them. An action whose arguments cannot all be named within LIM-I6 is not put to the user: it is rejected with a stated reason.

### 10.2 Approval record (REQ-APR-004 to REQ-APR-006)

`approval_id`, `task_id`, `action_id`, `action_hash`, `grant_set_hash`, `rendered`, `model_reason`, `requested_at`, `expires_at?`, `status` (`pending`, `approved`, `consumed`, `rejected`, `expired`, `invalidated`), `decided_at?`, `decided_by?` (session and OS user), `note?`, `halt?`, `arguments?` (every argument in full, sanitized, in the card's order, with the resolved path of a path parameter; what `[v]iew`, `task show --arguments` and `pending_approval.arguments` show; review of handoff 003b).

Lifecycle: `pending → approved → consumed`, or `pending → rejected | expired | invalidated`. Invalidation happens when the action hash or the grant-set hash no longer matches, or a used grant is revoked.

### 10.3 Questions (REQ-APR-010)

`question_id`, `task_id`, `step_id`, `text`, `asked_at`, `expires_at?`, `status` (`pending`, `answered`, `expired`), `answer?`, `answered_by?`. The answer enters the next step labelled `user`.

---

## 11. Built-in tools (REQ-TOOL-003)

All paths are relative to the workspace root; absolute paths and `..` segments that leave the root are `denied` at stage 7.

| Tool | Input | Output | Effect | Milestone |
|---|---|---|---|---|
| `read_file` | `path`, `start_line?` (≥ 1), `max_chars?` (≤ LIM-I19; a ceiling, not a promise: the longest prefix whose serialized payload fits LIM-I19 is returned, with `truncated`, the `line_range` actually returned and `content_truncated`; handoff 013c) | `content`, `truncated`, `total_chars`, `line_range`; `tool_error{line_out_of_range}` with `total_lines` when `start_line` is past the end | `workspace_read` | M1 |
| `write_file` | `path`, `content` (≤ LIM-W3) | `bytes_written`, `created`, `parents_created` (missing parent directories are created) | `workspace_write` | M1 |
| `replace_in_file` | `path`, `find`, `replace`, `occurrences?` (default 1; 0 = all) | `replaced` | `workspace_write` | M1 |
| `list_dir` | `path`, `depth?` (1–3) | `entries[]{path, type (file, dir, symlink), size}`, `truncated` (LIM-I7); links are listed without their target and never descended | `workspace_read` | M1 |
| `search_files` | `pattern` (regex), `glob?`, `max_results?` (≤ LIM-I7) | `matches[]{path, line, text, truncated?}`, `truncated`; links, files over LIM-W3 and unreadable files are skipped and counted in `entries_skipped` | `workspace_read` | M1 |
| `delete_path` | `path`, `recursive?` | `deleted` | `workspace_write` | M1 |
| `run_script` | `language` (`javascript`, `python`), `source`, `stdin?`, `args?`, `mode` (`read_only`, `read_write`; default `read_write`) | execution result (§7.4 fields) | `workspace_read` or `workspace_write` by `mode` | M2 |
| `run_wasm` | `module` (path), `args?`, `stdin?`, `mode` | execution result | by `mode` | M2 |
| `run_host_tool` | `tool` (a declared name the task holds a `host_tool` grant for), `args?` | execution result | as declared (§13.5) | M6; visible through `host_tool` grants (§9.1), at tier `full` only |

`replace_in_file` fails with `tool_error{no_match}` or `tool_error{ambiguous}` when `find` matches a different number of times than requested. Every tool is idempotent except `run_*`; every built-in tool's resource profile is `builtin` except `run_*` (`sandbox`, `host`).

### 11.1 External tools (MCP)

External tools appear with the name `<server>.<tool>` and the declaration of REQ-MCP-004. They are visible only with a `tool` grant; `rypsok.engine_status` is not granted by default. RFC 009 fixes the rest:

- **Servers are children** (`[servers.<name>]`, §19.1): an explicit environment (`env_allow` copied from the runtime's, `env_secrets` by reference, nothing else), started at the first need within LIM-O6 through the pinned SDK (`rmcp`, the version rypsok ships with) in their own process group or Job Object, ended at runtime exit by closing stdin and then killing the group after LIM-O16 (per server `shutdown_ms`, longer than the server's own graceful shutdown: rypsok's is 3 s by default); a dead server is restarted at the next need, at most LIM-X3 times per task in one runtime process (a new process, such as a `task resume` from the CLI, starts with the whole allowance, as the worker's does): a task's first need of the server is free, and every start made for that task after it counts, the one made by the pin check before a call included; a start made for no task (a listing, `status`, the import) never counts for any task, a refusal once given stays for the task's life, and the counts are cleared when the task ends (from 0.14, handoff 014a); whatever ended the process (its death, or the kill after LIM-O2: a server that does not honour a cancellation is a fault too). A listing made for no task (`status`, the import at startup) is never counted, and a start it attempts that fails makes no later start a restart; a start that succeeds, for anyone, does (review of handoff 009h). A server that cannot be reached or started at the pin check is `tool_unavailable`, not a changed definition: the tool stays pinned (review of handoff 010e-5). Past the limit its tools fail `host_fault{unavailable}` with the cause `tool_unavailable` and the reason `restart_limit_reached`: no call was made, so no outcome is unknown (§7.5; amended 2026-11-14, review of handoff 010e-4). Standard error is logged through the redactor; `server.started{server, version}` and `server.stopped{server, reason}` are recorded.
- **Import at task creation.** Each listed tool becomes `<server>.<tool>`: description sanitized and cut to LIM-I2 (`description_truncated` in the import record), the input schema converted to the flat parameter model (string, integer, boolean, enum, string array, one nested object; at most LIM-I3 parameters, depth LIM-I4), the output schema kept whole for validation, annotations recorded and never used, the server's instructions recorded and never placed in the context. A tool outside the model is **unavailable with a named reason** (`schema_unsupported`, `too_many_parameters`, `destination_unknown`, `destination_not_string`, `output_schema_unsupported`, `definition_invalid`, `definition_too_large` past LIM-I20, `too_many_enum_values` past LIM-I21), shown by `status`, never shown to the model. A parameter's name and a nested key are ASCII letters, digits, `_`, `.` or `-`, at most LIM-I10; an enumeration value is sanitized and cut to LIM-I5; an output schema is held to LIM-I13, LIM-I14 and LIM-I15, and validating one result to LIM-I16 visits (`output_invalid`: "the schema was too costly to check"). A listing reads at most LIM-I11 pages and LIM-I12 tools, the whole listing within LIM-O6; its reasons, which `status` and `tool_unavailable`'s payload show, are `list:timeout`, `list:cancelled`, `list:too_many_pages`, `list:too_many_tools`, `startup_timeout` and `startup_cancelled` (review of handoff 009g). Every definition is hashed (LIM-U6) and pinned with `tool.pinned`; the records sit on the task's versions (`tool_imports`).
- **Every request to a server is bounded** (audit A-08, the owner's decision of 2026-11-14): a listing, each of its pages and a call each have a deadline (LIM-O6 for a listing, LIM-O5 for a call) and observe the task's interruption; a listing has a bound on its pages and tools; no lock is held across a wait that is not bounded, so a server that stops answering never holds a step, another call or the runtime's exit.
- **A destination is a string** (audit A-11): a destination parameter whose schema is not a string makes the tool unavailable at import, with the reason; at stage 7 a destination argument present with another type is `destination_not_granted`.
- **A destination is judged as it is sent** (A-12, handoff 009h): a destination argument is admitted only when its parse serializes back to the same text (an empty path's `/` aside) and holds no user information; otherwise it is `denied` with `destination_not_canonical`, whose hint gives the canonical form, so the grant check and the server read one address. The same holds for a `redirected` result. The string is never rewritten: the user approved what the model wrote.
- **What a server writes on standard error is bounded** (A-27, handoff 009h): a line is cut at a length bound and marked, and the lines past a rate bound are counted in one line; the bounds are LIM-I17, LIM-I18 and LIM-O18.
- **Effects from the administrator** (`[servers.<name>.tools.<tool>]`: `effect`, `idempotent`, `destination_arguments`, `timeout_ms`). Undeclared: `external_write`, not idempotent, no destinations, LIM-O5. `reversible` is always false. A declaration admits the tool to the ceiling; `policy.ceiling.tools` may name an undeclared tool, which then runs under the restrictive defaults (`check` warns). Shipped profiles list no external tool.
- **Pinning holds.** A definition changes only after a restart or `notifications/tools/list_changed`; at the next step boundary the changed tool is unpinned once (`tool.unpinned`), omitted from the definitions, and named to the model in a `feedback kind="tool_unavailable"` segment; the hash is compared again at call time (`tool_error{definition_changed}` on the race). A new task pins the new definition.
- **What reaches the model is bounded as a whole** (A-26, A-32; handoff 009i). A result is bounded in total, after its output schema is checked, by cutting its longest strings first so that it stays valid JSON; its cuts are said in one `content_truncated` and its removals in one `characters_removed`, with totals; the full bounded result is kept for the user. One imported tool's rendering and the number of values of one enumeration are bounded at import: past either the tool is unavailable with the reason. A grant that would make the visible definitions exceed LIM-I1 is refused, naming the tool, so that the definitions never fail a step. The bounds are LIM-I19 (a result), LIM-I20 (one tool's rendering) and LIM-I21 (an enumeration's values); the definitions visible at a step boundary over LIM-I1 anyway halt the task once, `definitions_too_large`.
- **Calls.** One `tools/call` per execution under the deadline of REQ-TOOL-006 (never below the server's `max_call_ms`); `structuredContent` (else the text content parsed as JSON, else the text) validated against the pinned output schema (`output_invalid` otherwise); `isError` → `tool_error{<kind>: <message>}` with the error object as the payload, so the model reads rypsok's `invalid_request`, `denied`, `unavailable`, `redirected`, `unsupported_content` and `too_large` as they are meant; every string and key sanitized and redacted, then the output schema checked, then the result bounded in total (the bullet below); the server's standard output is read through a bound of LIM-I22 bytes per message: past it the stream ends with an error, the call ends as for a server that ended, the process and its group are killed at once, `server.stopped` keeps its reason `died`, and the log says why at `warn` (handoff 013c); redirects never followed: the model re-proposes and stage 7 validates the new destination. Cancellation sends `notifications/cancelled`; a server that has not stopped within LIM-O2 is killed.

---

## 12. Sandbox profile (REQ-SBX-001 to REQ-SBX-012)

| Item | Value |
|---|---|
| Engine | Wasmtime, pinned version; wasip1 core modules (D-16) |
| Guards | fuel LIM-S1; epoch deadline LIM-S2 with tick LIM-S7 (armed past the ticks the ticker owes, which it delivers at once after a stretched sleep, plus one: an execution never ends before its deadline and at most two ticks after it); memory LIM-S3; each independent |
| Filesystem | preopened `/workspace` (read-write or read-only by `mode`); preopened ephemeral `/tmp` discarded after the execution; nothing else |
| Network | none; no socket capability |
| Environment | empty; `args` and `stdin` as declared |
| Output | stdout and stderr captured, LIM-S4 each; `output_limit` on overflow |
| Clocks | granularity LIM-S6; fixed clock and random seed in replay mode |
| Host functions | WASI only |
| Interpreters | pinned builds verified by SHA-256 and stored under `<data_dir>/interpreters/<sha256>.wasm`; the binary carries each shipped build's artifact and hash (`mimlys interpreters install`); JavaScript (QuickJS-NG, wasip1) at M2, Python when a maintained build is pinned; compiled once per worker process and shared read-only by its executions |
| Modules | ≤ LIM-S5; validated before instantiation |
| Overhead | setup time measured per execution; target LIM-S8 |

`mimlys interpreters install <language>` downloads the shipped build's artifact within LIM-O9 and verifies its SHA-256 before anything is stored. A transient failure (no connection, a timeout, HTTP 429 or 5xx) is retried LIM-O10 times with the waits LIM-O10 states; any other status is not retried. When the download still fails, the error is `unavailable` with `component: "download"` (§18), naming the URL, the last status and the remedy `--from FILE` (an artifact obtained another way, verified by the same hash); never `internal`, which says the runtime is at fault.

---

## 13. Worker and confinement

### 13.1 Worker (REQ-ARCH-008)

One worker process per runtime, spawned through the confinement interface at the first execution (its start, including the compilation of the interpreters, is bounded by LIM-X4), talking over its standard input and output in frames of a 4-byte big-endian length followed by one JSON object whose first field is `version`: the requests `probe`, `execute` (with an optional `replay{seed, clock_ms}`), `cancel`, `shutdown`, and the responses `probe_result{clauses, engine_version, interpreters, interpreter_errors, cgroup?}`, `execution_record`, `error` (RFC 005; message types in `mimlys-exec::protocol`). The protocol is version 2 (handoff 004d-4): an `execution_record`'s streams travel as standard padded base64 text, at about a byte a byte (in 0.11 and earlier they travelled as arrays of numbers, and a stream above about 600 KB ended the worker). A frame is at most `2 × base64(max(LIM-S4 as configured, LIM-H4)) + 64 KiB`, LIM-S4 held within its range, and both sides derive the limit from the same bounds; a frame is read as it arrives, never allocated by its declared length. A peer of another version is refused by name at the handshake, before any work (the runtime: `the worker speaks protocol version n, this runtime 2`; the worker: the same, then it exits 2), and a frame above the limit ends the execution as `worker_died`, never a hang. One execution is in flight at a time (LIM-X2); `cancel` names it and must end it within LIM-O2. The worker's death ends in-flight executions with `host_fault{worker_died}` and is counted against LIM-X3; beyond LIM-X3 the task's executions are `host_fault{unavailable}`. The `mimlys-worker` binary is built by the `mimlys` crate from `mimlys-exec::worker_main`, so one install ships both.

### 13.2 Confinement contract (REQ-CONF-001)

| Clause | Guarantee | Mandatory |
|---|---|---|
| F | Explicit read set (data directory subtree, temp, declared toolchain and system paths) and explicit write set (the worker: the workspaces and temp; a host tool: its own task's workspace only, its tree and its temporary directory); nothing else visible or writable, and no Unix socket reachable outside the write set (§13.3) | yes |
| N | No network, loopback included | yes |
| E | Explicit environment; nothing inherited | yes |
| K | Whole process tree terminated when an execution ends, by itself or on cancel, deadline or runtime exit | yes |
| P | No privilege gain | yes |
| M | Memory ceiling for the tree (LIM-H2) | best-effort allowed; status says so |
| C | Process count (LIM-H3) and wall clock (LIM-H1) | wall clock mandatory (runtime timer); count best-effort allowed |

### 13.3 Backends (REQ-CONF-005; RFC 011)

The worker confines itself at start from a specification the runtime passes (the read set: the interpreters, the worker binary as a file (RFC 017), the declared toolchain paths, the system's directories; the write set: the workspaces and the temporary directory; whether network is denied; LIM-H2 and LIM-H3), before reading its first frame and before any thread exists, and reports per clause what held with the primitive's name. Each rung is tried from the strongest down:

| Platform | F | N | E | K | P | M | C |
|---|---|---|---|---|---|---|---|
| Linux (M6a) | Landlock, at the kernel's ABI, abstract Unix sockets scoped where the ABI allows (`landlock`); the read set holds no `/proc` entry that shows another process (REQ-CONF-011) | seccomp denying `socket` outside `AF_UNIX` and, always, `io_uring_setup`, `io_uring_enter` and `io_uring_register` (`seccomp`); with Landlock ABI 4 or later also TCP `bind` and `connect` handled with an empty allow-set (`seccomp+landlock_net`); the dependency step's worker keeps the `io_uring` denial and omits the rest (REQ-CONF-012) | explicit environment | `cgroup.kill` of the `tool` sub-cgroup of the worker's own cgroup, which every process a host tool starts joins before it runs (`cgroup_kill`); else the worker as child subreaper with a `/proc` walk that kills every descendant within LIM-O2 (`subreaper`): re-parenting cannot escape a subreaper | `no_new_privs` and the seccomp filter denying `ptrace`, `process_vm_readv`, `process_vm_writev`, `mount`, `keyctl`, `bpf`, `unshare`, `setns`, module and kexec loads (`no_new_privs+seccomp`); the runtime and the worker are non-dumpable (`PR_SET_DUMPABLE` 0), so a same-user process cannot read their memory or environment (REQ-CONF-012) | `memory.max` of the worker's cgroup where the session delegates the controller (`cgroup_memory_max`), else `RLIMIT_AS` set in each host tool's process between fork and exec, never in the worker (`rlimit_as`, best-effort; handoff 011n) | `pids.max` of the worker's cgroup where delegated (`cgroup_pids_max`), else the runtime's wall clock alone (`wall_clock`, best-effort) |
| Windows (M6b) | an AppContainer per installation (`mimlys-<hash of the data directory's identity>`: its volume's serial number and file id, so every spelling of the directory names one container; handoff 011o), the container SID granted read on the toolchain paths, the interpreters and, as files with an entry that nothing inherits, the worker binary and each configured interpreter module (RFC 017: their directories are never written), and read on `<data>/tmp`, and nothing on the workspaces: each task's `tree` and `tmp` carry that task's capability with read, write, execute and delete (handoff 011o), never `WRITE_DAC`, `WRITE_OWNER` or `GENERIC_ALL`, so a tool can change no file's permissions (scope of 0.11), by access-control entries that `toolchain grant` and `revoke` add and remove (`appcontainer`) | no network capability on the container (`appcontainer`); the dependency step's worker carries the internet client capability (loopback stays closed to containers) | explicit block; `TEMP`, `TMP`, `USERPROFILE`, `LOCALAPPDATA`, `APPDATA` inside the workspace's temporary directory, `SystemRoot` forwarded | a Job Object with kill-on-close and breakaway forbidden, joined before the first instruction runs (`job_object`); every host tool in a job of its own nested in the worker's | a restricted, low-integrity primary token together with the container where the system accepts one (`restricted_token+appcontainer`), else the container's own lowbox token (`appcontainer`) | `JobMemoryLimit` of the worker's job (`job_object`) | `ActiveProcessLimit` of the worker's job (`job_object`) |
| macOS (M6c) | one Seatbelt profile for the worker and its tools, self-applied, `(deny default)`, canonical read and write sets, the active developer directory read-only; other processes' information and `kern.procargs*` refused (`seatbelt`) | `(deny network*)`, DNS refused, Unix sockets only under the write set (`seatbelt`); the dependency step's own worker adds exactly outbound TCP to port 443, the name-lookup socket and the system's certificate file | explicit environment | every member of the worker's sandbox but the worker, found by listing the pids and keeping those the profile lets it inspect, killed until none remains within LIM-O2: the tool's tree, re-parented orphans included (`seatbelt_membership`); and, whenever the worker ends, the runtime's own sweep of the same members from outside the sandbox (rule A-21 below; handoff 011p) | the kernel refuses a setuid `execve` from a sandboxed process (`seatbelt`) | the members' memory above LIM-H2, each member's the larger of its resident size and its physical footprint, polled every LIM-H6 (`rss_watchdog`, best-effort) | the members' count above LIM-H3 (`process_count_watchdog`, best-effort) (RFC 011 Amendment 2, settled) |

The Linux read set (REQ-CONF-011) is: the data directory's `interpreters` and `tmp`, the worker binary and each configured interpreter module as files (RFC 017: never their directories, so nothing beside the install is readable by accident), the declared toolchain paths, `/usr`, `/lib`, `/lib64`, `/bin`, `/sbin`, the random and zero devices, and nothing of `/etc`, `/opt` or `/proc`: it was measured, under the confinement, that `cargo test --offline` needs none. The worker alone keeps `/proc` for its descendant walk (clause K, `subreaper` rung); a host tool's process is restricted, between fork and exec, to the same sets without it (a second Landlock layer; layers intersect). Landlock's own ptrace scoping already refuses a confined process another process's `environ`, `maps`, `fd` and `mem`; what the narrowing removes is the `cmdline`, `status`, `stat` and network tables of the user's other processes. Of `/etc`, only the dependency step's own worker reads anything, and only the files of this list that exist on the machine: the resolver's configuration (`/etc/resolv.conf`) and the certificate bundle `/etc/ssl/certs/ca-certificates.crt`, each because a measured failure asked for it (handoff 011j); and, **unmeasured**, from their families' documented layouts, `/etc/pki/tls/certs/ca-bundle.crt` (Fedora, RHEL) and `/etc/ssl/ca-bundle.pem` (openSUSE) (handoff 011l). `status` lists them as `dependency_step_read_set_extra`. No other worker has them, and a tool that needs another file of `/etc`, or lives under `/opt`, declares it in `read_paths`. A `command` under an undeclared `/opt` path is refused at load with the directory to declare. A secret that reached a process's environment is protected by the process being non-dumpable (the runtime, the worker) and by Landlock's scoping, not by its removal from the environment: `/proc/<pid>/environ` shows the environment the process started with. An MCP server child is `exec`'d and so dumpable again; what protects its environment is the host tools' read set.

**Three rules from the audit before 0.10** (audit before 0.10, owner's decision of 2026-11-14):
- **No Unix socket outside the write set** (A-01). A pathname Unix socket is a way out of every clause: a service of the user's session reachable through one can start a program outside the confinement. On Linux the worker's two Landlock layers handle `ResolveUnix` (ABI 9 and later) with no rule beyond the write set; below ABI 9, seccomp denies `socket(AF_UNIX)` (`socketpair` stays allowed), so a tool there creates no Unix socket at all, even in its own workspace (measured: `cargo test --offline` and the dependency step need none). The primitive names which: F `landlock_resolve_unix` or `landlock+seccomp_no_unix_socket`; N `seccomp[+landlock_net]+` the same. With neither, F and N are `unavailable` (`no_unix_socket_guard`), never `best_effort`, so `accept_best_effort` cannot accept the hole. Abstract sockets stay scoped. macOS already admits Unix sockets only under the write set; Windows containers reach no Unix socket the container is not granted. F and N report `enforced` only when this holds.
- **A host tool writes only its own task's workspace** (A-06). On Linux the host tool's own Landlock layer, applied between fork and exec, holds of the data directory only its task's workspace (tree and temporary directory). On macOS it is made by **a worker per task**: a sandboxed process can apply no second profile, to itself or to a child (spike S1, re-measured in handoff 011m), so the worker is spawned for one task and the first execution of another task replaces it. Of the data directory, a tool writes only its task's tree and its execution's temporary directory; `<data>/tmp` is readable, not writable, on both platforms. On macOS a socket path in a tool's tree can exceed `sun_path`'s 104 bytes, so a tool binds a socket by a relative name. On Windows (handoff 011o, 0.11) it is made by a worker per task as well, whose container token carries **its task's capability**: a capability SID derived from the task's identifier, and no other capability but the dependency step's network one. The task's `tree` and `tmp` (never the task's directory itself, which holds its workspace record) each carry one inheritable entry for that SID with the write mask, written on the empty directory before anything is copied into it, so the system's propagation has nothing to walk. The container's own SID has no entry on `<data>/workspaces` or on any task's directory, and only the read mask on `<data>/tmp`. A tool of one task therefore opens no other task's tree or temporary directory, to read or to write. The first start of 0.11 migrates once: the container's entry on `<data>/workspaces` is removed, each existing task's `tree` and `tmp` receive their entries, and the step is recorded in `toolchain-acls.json`. A start interrupted during it repeats it, and `toolchain revoke --all` removes the per-task entries with the rest. Measured on the Windows job (handoff 011o, spike W1): no entry is needed on `<data>/workspaces` or the task's directory to reach the tree (traversal is not checked), a tool can list neither of them, so it learns no other task's identifier, and an entry written before the copy costs nothing; the migration costs about 40 µs per file.
- **A host tool's argument names no path outside its workspace** (A-34, scope of 0.11): every argument, and each part after `=` or a short option's letter, is refused when it holds a `..` component or begins with `/`, `\`, `~`, a drive (`X:`) or `\\`, on every platform, whatever `arg_pattern` admits; a declaration that needs such arguments says so with `allow_paths = true`, shown by `status`.
- **The runtime's wait for a host tool covers the worker's** (handoff 011n): the runtime waits the tool's deadline plus twice LIM-O2 from its request (the worker starts its own clock after resolving and spawning, and may take LIM-O2 to end the tree); a worker the runtime must end after the tool's deadline has passed is recorded `deadline`, not `worker_died`, with the note in the log.
- **A task's paths are opened, not compared** (scope of 0.11): the tree and temporary directory given to a tool's layer (Linux), worker (macOS) or entries (Windows) are opened level by level without following links, and a component other than a plain name is refused.
- **The tree ends when the worker is killed, on macOS too** (A-21; handoff 011p, 0.11; spike M1). The worker's sweeper is a member of its sandbox, so a tool can signal it and then the worker, and outlive both. The runtime, outside the sandbox, therefore sweeps the worker's members itself whenever a worker ends (exit, crash, kill, deadline, replacement for another task), and the next start does it for a worker a crashed runtime left (from `worker.json`, before the record is removed). **Each worker's profile names a marker of its own:** a mach service name, `com.mimlys.marker.<32 hex>` from 16 random bytes, admitted by one `(allow mach-lookup (global-name …))` rule. It is not a path, because `sandbox_check` answers "denied" for a path that does not exist, whatever the profile says. **A process is a member only on three answers of the private `sandbox_check`, asked in this order and no further:** it is in a sandbox; its profile allows the marker; and its profile refuses the canary, the marker followed by `.canary`. The canary is there because about 25 of a runner's system processes allow any name (measured), and an unsandboxed process allows everything. A call that fails counts as no. The loop is `sweep_with`'s, within LIM-O2, over every pid of the user but the runtime. A pid whose information cannot be read (a zombie) is skipped. **Each signal is guarded:** the start time read with the first look, and the three answers, are read again immediately before `SIGKILL`, and a pid whose start time changed is skipped. macOS has no process handle, so a gap remains between that read and the signal: measured at most 126 µs natively and 532 µs under Rosetta, against at least 25.6 s before a pid is reused (pids are allocated in sequence). The threat register states it. **Each serving worker's start verifies the test** after its probe answered, with the worker as the known member and the runtime as the known outsider. `status` holds its probe worker (`--hold`, released by closing its input) to verify on it. The next start's sweep has no member alive and verifies the outsider's half. If the verification fails, the outside sweep is not armed and kills nothing; `status` says in one line that the sweep outside the sandbox is unavailable, with the reason (`outside_sweep_unavailable` in `--json`, present only then); the in-sandbox sweeper alone runs, as in 0.10; the tier and K are unchanged. A member the sweep cannot end within LIM-O2 is `tree_kill_incomplete`, at `warn`. The in-sandbox sweeper stays as the first layer, for a runtime that dies before its worker. A CI test on the macOS jobs fails if `sandbox_check` stops telling a member from an outsider (the owner's decision of 2026-11-16).
- **The tree ends with the execution** (X-01). When a host tool's process exits by itself, its tree is terminated as on a cancel (`cgroup.kill` of `tool`, or the descendant walk); a process that survives within LIM-O2 is reported (`tree_kill_incomplete`, logged at `warn`) on every platform, and the tool layer has no right on the worker's cgroup (A-36).

The macOS read set is RFC 011 Amendment 2, settled point 8. `xcrun` resolves the developer tools through a cache file in the per-user temporary directory, which the profile admits read-only; where the active developer directory is an Xcode and the cache is absent (a Mac where `xcrun` never ran), the tools fail without it (measured). The cache holds one entry per tool asked for, keyed with the asker's environment (measured). The runtime therefore makes it before the worker starts, and `check` does the same before it reports: outside the sandbox, with an empty environment (`PATH=/usr/bin:/bin`), it runs `/usr/bin/xcrun --find <tool>` for each tool of a fixed list (`clang`, which the `cc` shim asks for, and the tools a Rust or C build runs through `/usr/bin` shims), natively (through `/usr/bin/arch -arm64` when the runtime itself is translated by Rosetta), each call waiting at most LIM-O8; a call that fails or is killed leaves no partial file, which a sandboxed `xcrun` could neither use nor repair. A host tool is never given a write rule for the cache, because the user's own unconfined `xcrun` trusts it later and a tool that could write it could choose what that `xcrun` runs. If the cache still cannot be made, `check` and `status` say so in one line with the exact command to run (`env -i PATH=/usr/bin:/bin /usr/bin/xcrun --find clang`); the run goes on.

On Windows the parent must act, because the container, the token and the job are attributes of process creation: the runtime starts the worker suspended inside them and the worker reports what it finds in its own token and job, never what the parent intended. The cgroup rung is tried control by control: the worker creates `<own cgroup>/mimlys-<pid>` with two leaves, `worker` and `tool`, and moves itself into `worker` (handoff 013d; a worker that cannot make the leaf stays in the parent and says `cgroup_worker_leaf_not_made`), then sets each limit on its own, so a session that refuses `memory.max` still gives K its `cgroup.kill`; the directories are removed when the worker ends, swept at the next start when a crash left them, and a probe's own cgroup goes with the probe. Host tools run as children of the confined worker in the task's workspace tree, with `HOME` and `TMPDIR` in the workspace's temporary directory, `PATH` built from the declared read paths that are themselves a `bin` directory (`~/.cargo/bin`) or hold one, then `/usr/bin:/bin`, unless the declaration's `env` sets `PATH`, and the declaration's environment; their standard streams are captured to LIM-H4 each; the deadline and a cancellation terminate the tree.

### 13.4 Probe and tier (REQ-CONF-003; RFC 011 §5)

The worker started with `--trace` (which the runtime passes when its log admits `debug` for the worker) writes one line per phase of an execution to its standard error (received, set up, returned, record sent: identifiers and durations, never content), forwarded at `debug`. The probe is a real worker started with the specification and `--probe`: it reports every clause as `enforced (primitive)`, `best_effort (primitive)` or `unavailable: reason`; `get_runtime_status` shows the table. The tier is `full` when every mandatory clause is `enforced`, else `core`; a mandatory clause that is `best_effort` gives `full` only when `policy.accept_best_effort` names it, and the tier then reads `full (K best-effort, accepted by policy.accept_best_effort)` in `status` and in every host-tool approval. An `unavailable` clause can never be accepted. Host tools exist only at `full`. Every host-tool approval carries a `Tier` line with the same text, so the user approving the run sees the confinement it runs under.

### 13.5 Host tools (REQ-CONF-006 … 008; RFC 011 §6 … §9)

`[host_tools.<name>]` declares `command` (absolute, or a name resolved against the read paths that are a `bin` directory or hold one; never the inherited `PATH`; a command whose canonical target, after its links, leaves the read paths is refused at load naming the resolved path to declare, on every platform, so it never fails later with a bare permission error: Homebrew's `rustup` links into `/opt/homebrew/Cellar/…`), `args` (fixed), `allowed_args` (literals the model may add), `arg_pattern` (a regular expression any further argument must match), `max_args`, `effect` (`workspace_write`: the tool writes only inside the workspace, reversible through the snapshots; `external_read` exactly when `network = true`, and only then: any other pairing is a startup error naming the valid value, so that the file says what the runtime does; `host_write` is deferred until a declaration can name write paths, DR-017, because the worker's write set holds nothing but the workspaces and a declaration must never promise what the confinement refuses), `timeout_ms` (LIM-H1), `read_paths`, `env`, `env_allow` and `network`. A declaration admits the name to `policy.ceiling.host_tools`; `run_host_tool{tool, args}` is visible through a `host_tool{name}` grant alone (a profile's `host_tools` issues them at creation; the gateway is never granted by name, §9.1), validated at stage 7 against the grant and the declaration (`denied{argument_not_allowed}` with the `argument` position and `value`, or `max`), its effect derived at stage 9 from the declaration. Host tools run offline; a declaration with `network = true` is the dependency step (REQ-CONF-007): effect `external_read`, approval per profile, run by a one-shot worker whose confinement keeps every clause but N. The worker's record under the data directory (`worker.json`: the pid, on Linux its start time, on macOS its marker, each optional, so records of 0.10 and 0.11 read each other) lets the next runtime terminate what a crash left (`runtime.orphans_killed{count}`). `mimlys toolchain list` shows the read set in force. A declared `read_paths` entry that is a filesystem root, the home directory or an ancestor of it, or that directly holds a name of LIM-H5 (`~/.cargo` holds `credentials.toml`), or that is the data directory, lies inside it or contains it (`data_directory`, handoff 011o-4: each path compared by its longest existing ancestor resolved, case-insensitively on Windows and macOS, and the data directory named as configured), is refused by `check`, at load and by `toolchain grant` with the entry named and the narrower path suggested (`~/.cargo/bin`) (REQ-CONF-011). On Windows a declared `read_paths` entry that lacks the container's access-control entry makes its tool unavailable with `acl_not_applied: <path>`: `status` and `toolchain list` say so and name `mimlys toolchain grant <path>`, a `host_tool` grant for it is refused with the reason, and the model never sees it; `grant` and `revoke` are confirmed, recorded under `<data>/toolchain-acls.json` (written by a replace, never in place; a record found corrupt or unreadable stops the start and the read commands that show it, `status` and `toolchain list`, naming the file and the remedy, and is never rewritten: the owner's decision of 2026-11-16; a permission counts as applied only when the directory's own entry, not an inherited one, holds the wanted rights and no `WRITE_DAC` or `WRITE_OWNER`) so that `revoke` leaves the path as it was, and mimlys's own paths (applied at startup: `<data>/tmp` and `<data>/interpreters`, never the workspaces since 0.11, and the worker binary and the interpreter modules as files) are refused by `toolchain revoke <path>` with the reason; only `toolchain revoke --all` removes them. A `toolchain revoke <path>` of a path whose directory holds no entry of the container writes nothing and says so (`applied: false`, `reason: "no_entry"`; the record's entry for the path, if any, is dropped; handoff 011o-4). The writers of one directory's permissions, across the runtimes of a machine, hold a named mutex for the whole write; a writer that finds the mutex held says so once in the log, waits up to LIM-O7 and then refuses the write with `acl_not_applied` and the reason (`the permissions of <dir> are being written by another runtime`; at startup the start fails with it), never writing on a base read while another write was landing (the decision, `confine/dacl_wait.rs`, is proved with Verus never to write without the lock and to refuse when the wait runs out: RFC 016, REQ-TEST-018; the call order is the tests'); a lock the system will not create or wait on is refused the same way, with its own reason (`the permissions lock of <dir> cannot be taken: <system error>`), never a write without it. Either refusal is the error kind `unavailable` (§18). A task's own entries (on its `tree` and `tmp`, for its capability) are written with its workspace and never recorded: `toolchain list` counts them (`container.per_task_entries`), `revoke --all` finds them on disk and removes them (`per_task_entries: {removed, failed}` in its JSON when there were some), and the record's `task_entries_migrated` says that 0.11's one-time migration (§13.3) has finished; until it has, `status` says so in one line (`permissions_migration_pending` in `--json`, present only when true) and `toolchain list --json` carries `container.migration_pending`. After a complete `revoke --all` the record is gone, and the next start migrates again. Orphan reaping is Linux work: on Windows the job's kill-on-close ends the tree when the runtime's handle closes, a crash included.

---

## 14. Workspace

### 14.1 Layout (REQ-WS-001)

```text
<data_dir>/                    mode 0700 / owner-only DACL
  mimlys.db  mimlys.db-wal     database (§16)
  mimlys.lock
  blobs/<id>                   bounded outputs
  snapshots/objects/<hash>     content-addressed objects, read-only
  snapshots/manifests/<id>     manifests
  snapshots/pending/<id>       the journal of a snapshot being written (RFC 006 §6)
  interpreters/<hash>.wasm     pinned interpreter builds
  workspaces/<task_id>/tree    the task's working tree
  workspaces/<task_id>/tmp     ephemeral per-execution temp
  worker.json                  the live worker's record, for the next start to reap (§13.5; Linux)
  toolchain-acls.json          the container's permission record (§13.5; Windows)
  tmp/                         the temporary directory of the worker and the host tools (the write set with workspaces/)
```

The default data directory is the platform's user data directory (`$XDG_DATA_HOME/mimlys`, `%LOCALAPPDATA%\mimlys`, `~/Library/Application Support/mimlys`).

`mimlys.holder` (pid and RFC 3339 time of the process holding `mimlys.lock`, written by the writer after it takes the lock and removed before it releases it) lets a reader name the holder without reading a lock file that Windows keeps closed to others (REQ-PERS-013).

### 14.2 Materialization (REQ-WS-002, REQ-WS-003)

The project scope is copied into `tree` (reflink where the filesystem supports it, otherwise a copy; never a hard link). LIM-W4 paths and LIM-W5 patterns are skipped. Symbolic links whose target stays inside the scope are recreated; others are omitted with `symlink_omitted`. A scope above LIM-W1 or LIM-W2 is refused at creation with the measured size. A name that is not valid UTF-8 refuses the creation, naming its path with the invalid bytes escaped; a part of the scope that cannot be read refuses it with the path and the kind of the failure, never an internal error (scope of 0.11, handoff 006c).

### 14.3 Snapshots (REQ-WS-004, REQ-WS-005)

A snapshot is a manifest of sorted entries `path, type (file | dir | symlink), executable, mode?, size, hash | target` (`mode`: the file's permission bits masked to 0o777, absent in a manifest made before 0.11, so that its hash is unchanged; a link's target is kept byte for byte) plus objects stored once by hash under `snapshots/objects/<sha256>`, the manifest under `snapshots/manifests/<snapshot_id>.json` with its own hash in the row. Every file is hashed on every snapshot; no index or modification-time heuristic exists (RFC 006 §3). Snapshots are taken at materialization (`base`, or `fork`), before every write-effect action (`before`, recorded with the authorization), after every successful one (`after`, in the execution record) and of a promotion's targets (`backup`); two consecutive snapshots with the same manifest share one record. A snapshot is a journaled transaction: journal, objects and manifest fsynced, then the row, then the journal removed; the next start finishes or undoes a pending journal. Snapshot and restore never follow symbolic links; a target that is absolute or escapes the root is refused at restore and reported.

### 14.4 Rollback (REQ-WS-006)

Restore materializes the manifest exactly: files absent from it are deleted, objects are verified by hash, and the event `workspace.rollback` is recorded. **Verify, then replace** (scope of 0.11, handoff 006c): every object a restore or a promotion's reversal needs is verified, and written into a staging place inside the root, before anything is removed, so that a missing or corrupt object leaves the tree (or the project) as it was. The walk sees every kind of entry, a named pipe, a socket or a device included, so a restore removes one the manifest does not hold; it never blocks on one. A link chain is resolved through the manifest's own links, with a bound on the hops, and refused when it leaves the root. A file's mode is restored from the manifest. A backup snapshot of a promotion is not a target of `rollback --snapshot`: it holds only the project's changed paths; `rollback --promotion` reverses it.

### 14.5 Promotion (REQ-WS-007)

```text
diff(base, current) ──▶ conflicts? (target differs from base) ──▶ refuse, list conflicts
                   ├──▶ a deleted directory holds project entries the plan does not list ──▶ refuse, list them
                   └──▶ show diff ──▶ user confirms ──▶ back up targets into a snapshot
                        ──▶ commit the backup (its row, sealed) and record the promotion `applying`
                        ──▶ apply, entry by entry: re-check against the base; never through a linked parent;
                            remove only listed paths, deepest first, never recursively
                        ──▶ record `applied` ──▶ event workspace.promoted{files}
                        on an error: restore the applied entries from the backup, record `failed`, say so
```

**The order is the audit's rule** (A-03, A-04, A-15, A-16, the owner's decision of 2026-11-14): nothing in the project is written before the backup is committed and the promotion is recorded, and nothing is removed that the plan does not list, so that every promotion can be undone. A promotion found `applying` at start is reverted from its backup, recorded `failed` and listed (§16.3). **There is always a way back:** a promotion whose own restore failed stays reversible from its kept backup by `mimlys rollback <task> --promotion <id>`, and the message names that command (review of handoff 006b). `rollback --promotion` lists what it restores and asks; a path edited since the promotion refuses it unless `--overwrite-edited`. Promotion is reversible through the backup snapshot. A promoted file keeps the project file's own mode, except that a change of its executable bit made by the task is carried over (where the mode allows reading, as `chmod +x` does); a new file takes the workspace file's mode without the group's and others' write bits. The plan shows exactly the mode changes the promotion makes, and no other (scope of 0.11, review of handoff 006c). A path of the project that is not a regular file, a directory or a link (a named pipe, a socket, a device) is refused in the plan, never opened. It is the only operation that writes to the project scope (INV-10). LIM-W4 paths and LIM-W5 patterns are never promoted.

### 14.6 Quotas and lifecycle (REQ-WS-008, REQ-WS-009)

LIM-W1 (read from the latest snapshot at the step boundary), LIM-W2, LIM-W3. `allocated → active → archived → deleted`: `archived` is the task's terminal state (nothing removed); `deleted` is the user's `task delete`, valid for a terminal task only, which removes the workspace, the snapshots, the objects no remaining manifest references (reference counts in the same transactions as the rows), the blobs and the promotion backups, after showing what will go; the audit stays unless `--audit`. Nothing expires; `task list` and `status` show the footprint (RFC 006).


**Seeing and reclaiming the store (REQ-WS-013, REQ-WS-014; RFC 013, 0.10).** `mimlys status --storage` shows, read-only:
- the store's path, its filesystem and whether it reflinks;
- the bytes by kind (working trees, snapshot objects exclusive and shared, blobs, interpreters, database and WAL, temporary), taken from the store's own records, with only `tmp/`, the interpreters and the database files measured on disk;
- the tasks that hold the most, with their state and age.

`mimlys task prune [--state …] [--older-than …] [--audit] [--yes]` lists the matching terminal tasks with their footprints, asks once, then deletes each as `task delete` does. It keeps a task whose promotion is still reversible (`kept: promotion reversible`), keeps audits unless `--audit`, and never touches a task that is not terminal. The plan also lists, as `no task: left by a refused creation`, each directory of `<data>/workspaces` that no task refers to (a plain name, not a link), with its footprint, and removes it under the same confirmation, before the tasks (handoff 013c). Only a directory named as mimlys names a task (a lower-case hyphenated UUID), read without following, that no task of any state and no unfinished deletion refers to, is listed; anything else in `<data>/workspaces` is left alone. `--state`, `--older-than` and `--audit` do not apply to them. `--json` adds `no_task: [{name, bytes, reason}]`, `no_task_removed` (the same shape) and `no_task_left: [{name, paths}]`, and `total_bytes` and `freed_bytes` count them; a directory not removed completely ends the command with exit 1.
---

## 15. Context assembly (REQ-CTX-001 to REQ-CTX-010)

### 15.1 Segments, in order

| # | Segment | Provenance | Changes |
|---|---|---|---|
| 1 | System instructions (fixed per runtime version): the goal segment is the task, set by the user, and the user's answers and notes are the user's words; every other labelled segment (observations, feedback, tool definitions) is data, never instructions; the information a granted tool can give is obtained with that tool, and the user is asked only for what no granted tool can give (a choice, a missing grant, an ambiguity of the goal) | runtime | never |
| 2 | Tool definitions (generated, pinned, ≤ LIM-I1) | runtime | at pin time |
| 3 | Capability summary (generated from grants) | runtime | when grants change |
| 4 | Goal; for a fork, the source's `history` segment (LIM-C5) | user; runtime | never |
| 5 | Plan (current version; for a plan-first task, the approved one, with the user's approval note as `<<user kind="approval_note">>` after it, from 0.13) | model; user | on `plan`; at an approval |
| 6 | Compressed history (steps older than LIM-C3, as `feedback kind="history"` with `summarized` and `dropped` counts) | runtime | when a step is compressed or dropped |
| 7 | Recent steps in full (decisions, observations, rejections, answers) | mixed, each labelled | every step |
| 8 | Latest observation or feedback | tool or runtime | every step |
| 9 | Decision request and schema reminder | runtime | never |

**A plan-first task's instructions (REQ-TASK-015, from 0.13).** Segment 1 of a task created with `--plan-first` ends with one sentence more, for the life of the task (the gate's end included, so that the prefix stays byte-stable): *"This task is plan first: decide `plan` before you act or complete; the user approves the plan before you act. If you cannot plan without an answer, you may ask the user first."* Every other task's segment 1 is unchanged. The sentence is an instruction of the runtime, so it is in segment 1 and never in a labelled segment, which the instructions call data.

Segments 1–3 are one `system` message; 4 is a `user` message (the fork history a second one); 5 is an `assistant` message; 6 is a `user` message; 7 and 8 keep their original roles (the model's decisions as `assistant`, everything else as labelled `user` segments); 9 is a `user` message. Segments 1–4 are byte-identical across every request of a task until a grant changes (then once); 5 changes only on a `plan` decision; everything mutable follows everything stable (C-07). The step being decided is not yet part of the history. RFC 007.

### 15.2 Provenance delimiting (REQ-CTX-002)

Each labelled segment is one JSON object between marker lines:

```text
<<observation tool="run_script" action="…" status="failed">>
{ … }
<</observation>>
```

Bodies are JSON; markers stand alone on their lines. Because JSON string encoding escapes line breaks, no external content can produce a line equal to a marker. Labels are `observation`, `feedback` (runtime: `capabilities`, `rejection`, `history`, `tool_unavailable`, `decision_request`), `user` (goal, answers, notes), `tool_definitions`, `plan`, `history` (a fork's source). Summaries in the `history` feedback are built by the runtime from structured fields; the model's own messages are never relabelled.

### 15.3 Compression (REQ-CTX-003, REQ-CTX-004)

The budget is LIM-C1 characters over all message contents (a backend's `context_chars` overrides it for that backend, and a level's bounded by it, §19.4; `metrics` reports characters per prompt token so it can be set from a model's window). Rules, applied in order until the request fits: outputs truncated to LIM-C2 head and tail (at observation time); runs of three or more identical consecutive lines in an excerpt collapsed to one line plus `[… repeated N times …]`; every step older than the last `recent_steps` (LIM-C3; the task's own from 0.13, §19.4) reduced to a fixed summary of at most LIM-C4 characters built from `step`, `decision`, `tool`, `path`, `status`, `cause`, `rejection`, `outcome`, notice codes and the first line of output (the first line goes first when the limit is tight); then the oldest summaries dropped, `recent_steps` at a time, so the history segment changes in batches. Never dropped: the capability summary, every step that halted the task, every rejected decision (its code), and every failed action that no later success on the same tool and `path` superseded. When the retained set alone exceeds the budget the request is sent anyway and the step records `over_budget`. No model is ever asked to summarize (D-12). Every step records `context_chars`, `chars_before_fit`, `compressed_steps`, `dropped_steps`, `over_budget`, `prefix_chars` (characters shared with the task's previous request in this process), `model_ms` and the backend's `usage` when reported (REQ-CTX-010).

Redaction (REQ-CTX-007): the runtime holds every configured secret's resolved value with its name (`backends.<name>.api_key`); before an observation is persisted its payload strings, both excerpts and notice texts have every occurrence of a value (and its JSON-escaped form) replaced by `[redacted:<name>]`, with one `secret_redacted{name}` notice per secret found. Log lines pass through the same redactor. A secret shorter than eight characters is refused by the configuration: it could not be redacted without redacting noise.

### 15.4 The `tool_calls` transport (REQ-MDL-009; RFC 012, 0.9)

The same segments, in the same order and with the same provenance labels, with these differences:

- Segment 1's text replaces the description of the envelope with "call exactly one of the provided functions" and keeps every other statement of REQ-CTX-002.
- Segment 2 is not a system segment: the definitions travel as the request's `tools`, one function per pinned tool (its name with `.` written as `__`) plus the reserved `plan`, `ask_user`, `complete` and `give_up`.
- An earlier decision is an `assistant` message carrying its call (with the call's id). Its observation, or its rejection, is the `tool` message answering that id, still labelled (`<<observation …>>`, `<<feedback kind="rejection">>`).
- The stored plan (segment 5) is an earlier `plan` call with its acknowledgement as the `tool` message.
- Segment 9 does not exist.

A request therefore never ends with an `assistant` message, which both measured engines refuse under constrained output. The request asks for one call (`parallel_tool_calls: false` where the engine accepts it) and sends no `tool_choice`. The character count of REQ-CTX-003 includes the `tools` array.

---

## 16. Persistence and recovery

**Readers and the writer (REQ-PERS-013; RFC 013, 0.10).** One process writes a store at a time, holding `mimlys.lock` (LIM-O3). Commands that only read open the database as WAL readers without that lock, through a handle type that has no writing method.
- They run no recovery, integrity check, reaping or migration.
- A store that needs recovery is read as committed, and `status` says `recovery pending: the next command that writes recovers it`.
- While the lock is held, `status` names the holder (`store: in use by runtime <pid> since <time>`).
- A writer the lock refuses names the holder in the same words (from 0.13, every writer), with the error kind `conflict`.
- A reader never reports a store as locked.

### 16.1 Entities (REQ-PERS-001)

`task`, `step`, `decision`, `action`, `execution`, `observation`, `approval`, `question`, `grant`, `plan_version`, `snapshot`, `blob`, `event`. Every entity carries its schema version (REQ-MAIN-002).

### 16.2 Write ordering per action (REQ-PERS-004)

```text
1  action proposed (content, hash)        5  action running (intent)
2  validation result or rejection          6  execution record, observation
3  approval request; decision              7  action terminal status
4  action authorized; snapshot_before id   8  step outcome; counters; task state
```

Each line is one transaction, durable when it returns (LIM-P1). Execution starts only after line 5.

### 16.3 Recovery at startup (REQ-PERS-006, REQ-PERS-007)

```text
integrity check (LIM-P5) ──▶ CONS-01 … CONS-08 ──▶ for every task in running: recovering
   action in running → unknown
     effect reversible (workspace_write) → restore snapshot_before → halted(recovered)
     effect that cannot be undone (external_write; host_write, with or without the tool's backup; external_read not idempotent) → halted(unknown_outcome), action listed
     no action in flight → halted(recovered)
   a promotion recorded `applying` → its applied entries restored from its backup → recorded `failed`, listed (§14.5)
   a task found in recovering (a crash during recovery) → recovered again from the start; every step above is idempotent (audit A-18)
   recovery.auto_resume = true and a restored or empty disposition → running instead of halted
expired approvals and questions → rejected(expired) → halted
orphan processes of a previous runtime → terminated where identifiable
pending snapshot journals → finished when their row exists, undone otherwise (no orphan, nothing collected)
```

### 16.4 Retention (REQ-PERS-010)

| Class | Data | Retention |
|---|---|---|
| Operational | blobs of raw outputs | until `task delete` |
| Recovery | snapshots, workspaces | until `task delete`; never while the task is non-terminal |
| Audit | events | until `task delete --audit` |
| Durable | configuration, secrets by reference | until changed |

No timer and no collector: everything on disk belongs to one task (RFC 006 §6).

---

## 17. Events, metrics and logs

### 17.1 Events (REQ-OBS-001)

Events are written by a writing runtime only; a read command (REQ-PERS-013) records none, `runtime.started` and `runtime.stopped` included.

`task.created{effort?}` (from 0.13: the level, who chose it and the resolved envelope, §19.4), `task.state_changed{from, to, reason?, rollback_failed?}`, `task.admission_changed{state: waiting | admitted | unadmitted, slots_taken?, waited_ms?, reason?}` (§19.3, 0.12; `waited_ms` on `admitted`; a wait the stop request or the deadline ends records no closing event, the action's `stopped_before_start` and the task's pause or halt say how it ended), `task.pause_requested`, `task.deleted{audit}`, `step.started`, `step.finished{outcome}`, `decision.received{hash, transport, text_chars?}` (from 0.9: `transport` is `tool_calls` or `envelope`; `text_chars` counts text beside a structured call, which is not part of the decision), `decision.rejected{code, reason?, grant_kind?}`, `action.proposed`, `action.authorized`, `action.started`, `execution.finished{attempt, termination}`, `action.finished{status, cause?}`, `approval.requested`, `approval.decided{decision}`, `plan.approval_requested{version}` and `plan.approval_decided{version, decision}` (plan first, from 0.13; an approval's or a rejection's note is not in the event, as for an action), `approval.invalidated`, `question.asked`, `question.answered`, `grant.issued`, `grant.revoked`, `workspace.materialized`, `workspace.snapshot{reason}`, `workspace.rollback`, `workspace.unreadable{path, kind?}`, `workspace.promotion_started{files}`, `workspace.promoted{files}`, `workspace.promotion_failed{reason}`, `task.recovered{disposition}`, `tool.pinned`, `tool.unpinned`, `server.started{server, version?}`, `server.stopped{server, reason}`, `runtime.started{tier, backend}`, `runtime.stopped`, `runtime.orphans_killed{count}`, `snapshot.journals_recovered{finished, undone, objects_removed}` (from 0.10: the start finished or undid snapshot journals a crash left, REQ-WS-012; recorded only when it did). `backend` is the descriptor `{kind, model?, locality}` of the backend in force (REQ-SEC-010, RFC 008); the same descriptor is recorded on every task's versions at creation.

Every event carries `event_id`, `at` (LIM-U4), `actor` (`user{session, os_user}`, `runtime`, `model`, `tool{name}`), `task_id?`, `step_id?`, `action_id?`, `policy_hash`.

### 17.2 Metrics (REQ-OBS-003)

Per task and aggregated: tasks by state; events by name; rejections by code; halts by reason; approvals by decision; finished actions by status; and under `context`, over the steps that carry measures: `measured_steps`, `context_chars_avg`, `prefix_ratio` (Σ shared prefix / Σ context over steps after a task's first: what a prefix cache may reuse), `compression_ratio` (Σ characters before the rules / Σ after), `over_budget_steps`, `compressed_steps`, `dropped_steps`, `model_ms_avg`, `prompt_tokens`, `completion_tokens`, `cached_tokens` (as the backend reported them), `chars_per_prompt_token` (the number to set `context_chars` from a model's window). From 0.13, `by_effort.<level>` aggregates per level (§19.4). Execution latency and sandbox overhead stay in each observation's `metrics`.

### 17.3 Logs (REQ-OBS-004)

Structured lines on standard error: timestamp, level, event, durations, counts, outcome, and, for a line written while a task's step runs, the fields of the step's span: `task_id`, `step_id` and, once there is an action, `action_id`. Never prompt content, file content or secrets at the default level. `logging.debug_prompts = true` writes each request's messages and the model's reply as `debug` lines of their own target, whatever `logging.level` is (the switch is the request), through the same redaction, and warns at start. A line that a reader of another process's standard error writes (the worker's reader, an MCP server's) carries the ids of the one execution or call in flight on that process when the line is read, and no ids when none or more than one is in flight: a missing id rather than a wrong one. The one residual error is a line written during one call and read only during the next, which needs the reader to lag by a whole call; the protocols carry no request id that could settle it.

---

## 18. Errors of the control API

```json
{"error": {"kind": "state", "message": "task is waiting for the user; answer or cancel it", "detail": {"state": "waiting_user", "pending": "question"}}}
```

| Kind | When | `detail` keys |
|---|---|---|
| `invalid_request` | a parameter fails validation | `parameter`, `min`, `max`, `allowed` |
| `not_found` | unknown task, action, snapshot or grant | `id` |
| `state` | the verb is not valid in the current state | `state`, `pending` |
| `denied` | a grant or budget beyond the ceiling; promotion not allowed | `reason`, `ceiling` |
| `conflict` | overlapping project scope; promotion conflicts; the store held by another process (from 0.13) | `with`, `paths`, `holder_pid` |
| `unavailable` | the model backend (`model_backend`: an adapter's contained panic, or a `BackendError`) or the tools' provider (`tool_provider`: a contained panic) not usable; every `unavailable` carries `detail.component`, and these four words are the whole set (handoff 004d). A worker or an MCP server that cannot be reached is not this error: it is an execution's outcome (`host_fault`) or an import's reason, shown in the observation and by `status`; on Windows, the permissions of a directory being written by another runtime past LIM-O7 or their lock not obtainable, or a new workspace whose per-task entries cannot be written (`component: "permissions"`; retrying later is the remedy; handoff 011o-4); an interpreter download that failed after its retries (`component: "download"`, §12) | `component` |
| `internal` | a runtime defect | `reference` (an event id; never a stack trace) |

Messages are at most LIM-I5 characters and never carry secrets, file content or provider text.

---

## 19. Operator interface

### 19.1 Configuration (REQ-CFG-001 to REQ-CFG-004)

One TOML file, located by `--config`, `MIMLYS_CONFIG` or the platform default. Its shape (the exact schema is fixed by RFC 010; every default is the limits register's value):

```toml
config_version = 2                     # 2 from 0.13; a version-1 file (one `[backend]`) is read through 0.14

[runtime]
data_dir = "…"                         # platform default when absent

[backends.default]                     # a level that names no backend runs on `default` (RFC 015)
kind = "openai_compatible"             # or "fake" with `script = "decisions.json"` (the scripted decisions)
url = "http://127.0.0.1:8080/v1"
model = "…"
locality = "local"                     # a host that is not loopback must be "remote"; "remote" needs policy.remote_backend_allowed
decision_transport = "auto"            # auto | tool_calls | envelope (0.9, REQ-MDL-009); `status` shows the one in force
structured_output = "auto"             # auto | schema | none; the envelope transport only
timeout_ms = 600000                    # LIM-M1
max_tokens = 4096                      # LIM-M2
context_chars = 32000                  # LIM-C1; `metrics` shows chars per prompt token
temperature = 0.0                      # 0.0–2.0; 0 for the most repeatable decisions
api_key = "env:MIMLYS_BACKEND_KEY"     # by reference only; at least 8 characters

# [backends.small]                     # more backends: one section each, the same keys
# url = "http://127.0.0.1:11434/v1"
# model = "qwen2.5:3b-instruct"

# [effort.quick]                       # RFC 015: an overlay; a key it leaves out is the profile's
                                       # shipped: steps = 22 (LIM-E4), deadline = "10m" (LIM-E5); a key here replaces it
# backend = "small"                    # every level local, or every level on one remote endpoint
# steps = 20                           # LIM-T1, within the ceiling
# deadline = "30m"                     # LIM-T5
# context_chars = 16000                # LIM-C1, at most the backend's
# recent_steps = 4                     # LIM-C3
# [effort.standard]                    # ships with no keys: the behaviour before levels
# [effort.deep]                        # ships with no keys: chosen unconfigured, it is refused (name a larger model's backend)
# backend = "large"

[policy]
default_profile = "workspace"
remote_backend_allowed = false
# accept_best_effort = ["K"]           # a best-effort mandatory clause accepted by name (§13.4)
[policy.ceiling]
tools = ["read_file", "write_file", "replace_in_file", "list_dir", "search_files", "delete_path", "run_script", "run_wasm"]
host_tools = []                        # declarations admit names here by themselves
network_domains = []
host_read_roots = []                   # reserved: refused when non-empty (§9.1)
host_write_roots = []                  # reserved: refused when non-empty (§9.1)
max_steps = 200
# effort_max = "deep"                  # the highest level any profile may allow (§19.4); absent means deep
[policy.profiles.workspace]
tools = ["read_file", "write_file", "replace_in_file", "list_dir", "search_files", "delete_path", "run_script", "run_wasm"]
host_tools = []                        # declared host tools granted at creation (`host_tool` grants)
steps = 40                             # LIM-T1
consecutive_rejections = 3             # LIM-T2
consecutive_failures = 5               # LIM-T6
identical_decisions = 3                # LIM-T4
executions_per_action = 2              # LIM-T3
approval_tools = []                    # tools that always need approval in this profile; `run_host_tool` covers every host tool
on_reject = "continue"                 # continue | halt
on_cancel = "rollback"                 # rollback | keep
promote = false                        # the `project` profile sets true
external = false                       # may the task communicate outside the host (a remote backend); false in every shipped profile
# effort_max = "standard"              # the highest level this profile allows, at most the ceiling's (§19.4)
[policy.exclusions]
materialize = [".git/", ".hg/", ".svn/", "target/", "node_modules/"]   # LIM-W4
secrets = [".env", ".env.*", "*.pem", "*.key", "id_*", ".ssh/", ".aws/", ".gnupg/"]  # LIM-W5
[policy.recovery]
auto_resume = false

[sandbox]
fuel = 10000000000                     # LIM-S1
deadline_ms = 30000                    # LIM-S2
memory_mib = 256                       # LIM-S3
output_bytes = 1048576                 # LIM-S4

[workspace]
max_bytes = 1073741824                 # LIM-W1
max_files = 200000                     # LIM-W2

[servers.rypsok]                       # RFC 009; a declaration admits the tool to the ceiling
command = "rypsok"
args = ["serve", "--config", "/path/to/rypsok.toml"]   # MCP over stdio; a rypsok file the user owns, with follow_cross_host_redirects = false
env_allow = ["PATH"]                   # copied from mimlys's environment; nothing else is inherited
env_secrets = { BRAVE_API_KEY = "env:BRAVE_API_KEY" }   # by reference; redacted everywhere
startup_ms = 15000                     # LIM-O6
shutdown_ms = 5000                     # LIM-O16: longer than rypsok's own graceful shutdown (3 s by default)
# max_call_ms = 11000                  # the server's declared maximum; no deadline below it
[servers.rypsok.tools.web_search]
effect = "external_read"               # external_read | external_write; undeclared = external_write
idempotent = true
timeout_ms = 10000                     # LIM-O5
[servers.rypsok.tools.web_fetch]
effect = "external_read"
idempotent = true
destination_arguments = ["url"]        # validated against the task's network grants
timeout_ms = 15000

[host_tools.cargo_test]                # RFC 011; a declaration admits the name to the ceiling
command = "~/.cargo/bin/cargo"         # absolute, or a name resolved in read_paths/*/bin
args = ["test", "--offline"]           # fixed; the tool runs offline (clause N)
allowed_args = ["--lib", "--release"]  # literals the model may add
arg_pattern = "[A-Za-z0-9_:]+"         # a test name, nothing else
max_args = 2
effect = "workspace_write"             # writes only target/ inside the workspace: reversible
timeout_ms = 600000                    # LIM-H1
read_paths = ["~/.cargo/bin", "~/.rustup"] # the toolchain; nothing else outside the system directories is readable; a directory holding credentials (`~/.cargo` holds `credentials.toml`) is refused, REQ-CONF-011
env = { RUSTUP_HOME = "~/.rustup", RUSTUP_TOOLCHAIN = "stable" }
[host_tools.cargo_fetch]               # the dependency step (REQ-CONF-007): network allowed, external_read, approval per profile
command = "~/.cargo/bin/cargo"
args = ["fetch"]
effect = "external_read"
network = true
read_paths = ["~/.cargo/bin", "~/.rustup"]
env = { RUSTUP_HOME = "~/.rustup" }

[logging]
level = "info"
debug_prompts = false                  # true logs prompts and model output (redacted) at any level, and warns at start

[interpreters.javascript]              # optional: overrides the shipped build (RFC 005)
module = "/path/to/qjs-wasi.wasm"
sha256 = "…"
```

Validation refuses an unknown `config_version`, an unknown key, an inline secret, a budget above its range, a `remote` backend without `remote_backend_allowed`, a backend declared `local` whose URL's host is not loopback (`localhost`, `127.0.0.0/8`, `::1`; a loopback URL may be declared `remote`, for a tunnel), a version-2 file with `[backend]` or a version-1 file with `[backends]` (naming the version that form needs), two backends on one URL with different `max_in_flight`, levels whose backends are not all local and not all on one remote endpoint (naming each level and its endpoint), a level that names no backend when there is no `default`, a level whose `context_chars` is above its backend's, a resolved secret shorter than eight characters, and a host tool with an undeclared effect. A `remote` backend is further refused for a profile whose `external` is false (REQ-SEC-010), at a task's creation and again at every start of it, before anything is sent: a configuration edited between two starts cannot send a task's content where its profile forbids. A version-1 file is read with its `[backend]` as `backends.default`, and `check` prints one deprecation line saying how to write it in version 2. Before a command that runs steps, and for `status`, the backend is probed within LIM-M4: `GET {url}/models`, then the structured-output mode (`auto`: one-token completions with `json_schema`, then `json_object`, then none; `schema`: verified, and a refusing server is a startup error naming the fix). The outcome is recorded in `runtime.started`, shown by `status`, and announced once by `run` when `auto` ended at plain text. An unreachable backend does not refuse the command: the first step halts `backend_unavailable` (§20).

### 19.2 Command line (REQ-CLI-001 to REQ-CLI-006)

| Command | Behaviour |
|---|---|
| `mimlys run <goal> [--scope DIR] [--profile P] [--grant …] [--effort LEVEL] [--plan-first]` | Create and start a task, follow its events, prompt when it waits; `--effort` from 0.13 (§19.4) |
| `mimlys task new <goal> [--hold] [--effort LEVEL] [--plan-first] [--from-task T [--from-snapshot S]]` · `show` · `list` · `resume [--extend-steps N] [--extend-deadline D]` · `pause` · `cancel` · `delete [--audit] [--yes]` · `output <task> [<action>] [--attempt N] [--stdout \| --stderr \| --result]` | Task control; `delete` shows the footprint and asks; `output` shows an action's whole copies (REQ-PERS-005; `--attempt` from handoff 004d-4): without a flag, what the action has (its result and each stream, each with its size and whether the model's copy was cut), with a flag that copy itself, the text as it is to standard output (on a terminal a carriage return that ends no line is shown as `\r`), the result as JSON; the action defaults to the task's latest that ran, and the copy is the latest execution's; a reader, served by the read-only open (handoff 004d). An action is named by its id or a prefix of at least four characters; refusals: an unknown action or a whole copy that is not there `not_found` (never shown as if whole), an action that never ran or an external tool's stream `state`, a prefix too short or ambiguous `invalid_request`. On a terminal a last line without a newline is ended; piped, nothing is added. `--json` without a flag: `{task_id, action_id, execution_id, tool, attempt, attempts: [{attempt, execution_id, status}], copies: [{kind: result \| stdout \| stderr, bytes, cut, capture_cut, captured_bytes?, copy_cut, source: blob \| excerpt \| full_payload \| payload, flag, unavailable?: no_streams \| not_kept \| removed}]}` (`capture_cut`: the tool printed more than its bound and the first `captured_bytes` are kept; `copy_cut`: the stored copy was itself cut, which only a redaction that lengthens the text past three times the bound can do, handoff 004d-5); `--attempt N` picks the action's execution of that attempt (an attempt it does not have is `not_found`, naming those it has), and the text index names the attempts when there is more than one; with a flag: the same header with `kind, bytes, cut, source` and `text` (a stream) or `result` (the JSON value), exact |
| `mimlys approve <task> [--note T]` · `reject <task> [--note T] [--halt]` · `answer <task> <text>` | User items: an action awaiting approval, a question, or (from 0.13) a plan-first task's plan, shown as an approval card is. `approve --note` passes the note to the model as a labelled `user` segment (`approval_note`) for an action as for a plan. `task show` shows the plan in force of every task, and for a plan-first task whether it is approved |
| `mimlys grant <task> tool NAME` · `host_tool NAME` · `network DOMAIN[:PORT]` · `revoke <task> <grant>` | Grants within the ceiling; `host_read` and `host_write` are refused as reserved (§9.1) |
| `mimlys rollback <task> [--snapshot ID \| --promotion ID]` · `promote <task> [--yes] [--summary] [--patch]` | Workspace: restore, or reverse a promotion; promote after the diff and the confirmation, or print a patch |
| `mimlys events <task> [--follow]` · `metrics` · `status` · `runtime reap` | Observability (`runtime reap`: the registry's stale entries removed by hand and listed, §19.3); `status` shows the tier and every clause by letter and word with its primitive, each backend with the probe's findings (from 0.13 every configured backend, probed at once within one LIM-M4: reachable, model listed, resident, the transport in force, the engine's context, and the last fitness probe as `acts, measured on this machine on <date>`; resident is Ollama's word from `/api/ps`, always `yes` for a llama-server, which holds one model, and `unknown` for another engine; structured-output mode for the envelope transport), and every configured server (started for the listing within LIM-O6) with its tools, the tools it refused to import and why, and whether its list changed |
| `mimlys model probe [<backend>]` | The fitness probe (REQ-MDL-011; RFC 015, from 0.13): per backend, one scripted mini-task against the real model, in a scope the runtime makes under `<data_dir>/tmp/probe-<id>/` with one small file, under a profile of the built-in read and write tools only that cannot promote, within LIM-E2 steps and LIM-E3; the class `acts` (completed, and the runtime found the file as asked), `incomplete` (it proposed an action, accepted or rejected, but did not finish), `plans_only` (it never proposed an action), each of these two with how it ended: `not_finished` (the bounds), `not_verified` (it said it was done, and the file was not as asked), `asked_user` (no one answers a probe) or `gave_up`, `no_structured_call` (only `malformed` replies) with the steps, the model time per step and the date, written to `<data_dir>/probe-results.json`; an engine it cannot reach is said (`unreachable`, exit code 1) and not recorded, so an earlier measurement of that backend stays; the probe's task and scope are deleted when it ends, and it writes no event of a user's task. Without a name it probes every local backend, one after the other, and says which remote ones it left out; a remote backend is probed only when named and when `remote_backend_allowed`, and the command says first that it sends a fixed mini-task and nothing of the user's. It is a writer: while another process holds the store it is refused, naming the holder. Its requests take the engine gate (§19.3); its tools take no admission slot |
| `mimlys check` · `--version` | Configuration check; version (`gc` never shipped: deletion is explicit, RFC 006) |
| `mimlys interpreters list` · `install <language> [--from FILE]` | Interpreter builds for `run_script`: verified by hash, stored under the data directory |
| `mimlys toolchain list` · `grant <path> [--yes]` · `revoke <path> [--yes]` · `revoke --all [--yes]` | The read set host tools run under and each declared tool's paths (REQ-CONF-008); on Windows whether each carries the container's permission, and the confirmed, reversible grant and revoke of one; `revoke --all` (Windows) removes, after showing the list and asking, every entry mimlys added (the user's and its own, recorded or found on mimlys's directories and the declared read paths) and deletes the installation's AppContainer profile and the record, so that removing mimlys leaves the machine as it was; the next start of mimlys applies its own entries again; elsewhere `grant` and `revoke` say that nothing is needed |

Exit codes: 0 success, 1 runtime or configuration error, 2 usage error. `--json` on every command yields stable field names. **A stop signal** (Ctrl-C; SIGTERM on Unix) asks running tasks to pause: no new step begins, the step in flight starts no new action, and the task pauses when what is in flight has finished; a second signal cancels them; the process exits within LIM-O1. An action the step had not started when the signal came is closed `cancelled` without starting (nothing ran, so the record is exact), and its observation says that the user's stop signal came before it began. **The check and the start are one step** (A-33, handoff 010f): a first signal either finds an action not begun, which then never begins, or begun and recorded `running`, which the process then waits for or interrupts by its kind; no signal falls between the two. A wait made before anything is sent (an MCP server's pin check) observes the first signal too, and ends with the action not begun (`stopped_before_start`), never as a `tool_unavailable` halt. An approved action whose step has not begun is not closed: the step does not begin, the task pauses with the action approved, and a resume continues it as any approved action (review of handoff 010e-4). After the first signal, a step still running LIM-O1 less one second later has what can be undone in it interrupted: its model request, a read, a workspace write (rolled back): the actions whose "After `unknown`" in §7.2 is re-execution or rollback. The task then pauses. The grace counts the whole step, including the store's writes around it, so on a loaded machine a pause can interrupt such an action that would have finished a moment later: the price of a bounded exit, with nothing lost. The process says that the step was interrupted once the step has ended so, never before, and that its workspace changes were rolled back only when they were. An action that cannot be undone, the actions whose "After `unknown`" is halt `unknown_outcome` (an external write, a host write, an external read that is not idempotent), is never interrupted by the first signal. The process says why it waits, in words for the action's effect: `waiting for <tool> to finish: it changes something outside this machine and cannot be undone` (an external write), `waiting for <tool> to finish: it changes files outside the task's workspace and cannot be undone` (a host write), `waiting for <tool> to finish: it has sent a request outside this machine that cannot be taken back` (an external read that is not idempotent), each ending `; press Ctrl-C again to interrupt it anyway`. The wait is bounded by the action's own deadline, at which the action's outcome is unknown (§7.5). A second signal interrupts it: the action is recorded with its outcome unknown and the task halts `unknown_outcome` with the action listed (§16.3's rule for a crash, REQ-PERS-008), so that the user, not the record, decides what happened. `mimlys task cancel` ends the task `cancelled` as asked, and records and names such an action the same way. An interruption request is spent by the step it ends and is cleared at every task transition (a pause, a resume, a halt, a cancellation, and the user's approval, rejection or answer), so it never reaches a step that began later (handoff 010d: a request that stayed set turned every later step into an interrupted one and the pause was never taken). The messages name what happened in the user's words, never a boundary. **At an approval or question prompt** nothing runs, so a stop signal neither pauses nor cancels: it ends the wait, the task stays `waiting_user` with its request, nothing typed at that moment is applied, one line on standard error says how to decide later (`mimlys approve`, `reject` or `answer`), and the process exits 0 within LIM-O1 (review of handoff 003b). A task lists its actions of unknown outcome (`unknown_actions`) for its whole life, a resume included: resuming does not tell what they did (review of handoff 010e, 2026-11-14).

---

### 19.3 Several runtimes of one user (REQ-SCHED-006, REQ-SCHED-007, REQ-CONF-013, REQ-OBS-009; RFC 013 part two with its Amendments 1 and 2)

**The registry.** It is one directory per user, owner-only, under the user's runtime directory: `$XDG_RUNTIME_DIR/mimlys/` (else `$XDG_DATA_HOME/mimlys/run/`), `%LOCALAPPDATA%\mimlys\run\`, or `~/Library/Application Support/mimlys/run/`. It is outside every confinement's write set. It holds one entry per slot taken, per engine gate held, and per runtime waiting for either (a `waiting` entry counts against nothing; it lets another process's `status` and `task show` say who waits, for what, behind whom). An entry is two files: an empty lock file its holder keeps locked for as long as the entry lives, and the entry's text, written once by a rename and never changed (`{v, kind: slot | gate, state: held | waiting, pid, data_dir, task_id, since, limit, key?, url?}`; a gate names its endpoint as `scheme://host:port`, never a credential or a path). Two files, because on Windows a locked file cannot be read by another process. An entry whose lock is free, or whose lock file is missing, is stale: the system drops a dead holder's lock, so the rule is exact where a pid can be reused, and the pid is shown, not consulted. A text that cannot be parsed, is over 4 KiB, disagrees with its name, is a link or belongs to another user is refused. Stale and refused entries are reaped at every take, under the registry's own mutex, and logged at `info`; a reader lists them and removes none (handoff 013d). `mimlys runtime reap` reaps by hand and lists what it removed. Every operation on the registry that reads or takes (take, list, reap) is bounded by LIM-O11; returning an entry is closing its lock and removing two files, which makes it stale at once whatever the removal does, so it needs no timer.

**Admission (decision 5).** An execution takes a slot before its action is recorded `running`, and returns it when the execution ends, so an idle runtime, a `follow` or a pending approval costs nothing.
- **At most LIM-X5 slots are taken at once per user** (`[runtime] admission_slots`, within LIM-X5's range).
- **A task whose execution waits for a slot stays `running`.** Its view (`task show`, `status`) says `waiting: admission (LIM-X5 = 2 slots taken by runtimes <pids>)`, and the event `task.admission_changed{state: waiting | admitted, slots_taken}` is recorded at each change.
- **The wait is a wait before the send** (Amendment 1): the first stop request ends it, with the action not begun (`stopped_before_start`) and the task paused. Nothing but the task's own deadline bounds it, and the deadline keeps counting.

**The engine gate (decision 7).** A model request takes the gate of its backend's URL for the user before it is sent (`max_in_flight` of the backend, default LIM-O12, within its range; from 0.13 every `[backends.<name>]` entry on one URL carries the same value, REQ-CFG-007), and returns it when the answer is read.
- **The wait** is the metric `engine_wait_ms` and a log line.
- **It ends at the first stop request, at once,** since nothing has been sent (Amendment 1); a request in flight is interrupted at the grace, as before. The task's deadline ends it too, and the task halts `budget_deadline`, as for the slot: never `backend_unavailable`, since the engine never received the request (handoff 013d-4).
- **The request's own deadline (LIM-M1) starts when the request is sent,** not when it starts waiting. The probe in `status` takes no gate.

**Fail-open, visible.** A registry that cannot be read or written within LIM-O11 lets the runtime proceed as if alone and says so at `warn`.
- **`status` only reads the registry** (handoff 013d-4): it makes no directory and writes no file, and judges from the directory's metadata. It says `admission: ready (<taken> of <limit> slots taken)`, or `admission: not yet used (no runtime of yours has taken a slot)` when the directory does not exist yet (JSON `{state: ready, not_yet_used: true}`), or `admission: unavailable (<reason>)`; the first take decides for real.
- **While it lasts,** `status` shows `admission: unavailable (<reason>)`, and the task's events record `task.admission_changed{state: unadmitted, reason}`.
- **A liveness defect of the registry never stops the user's work.** The residual risk is THR-37's.

**The CPU part of clause C (decision 6, REQ-CONF-013; Amendment 3).** The share is LIM-H7, `[policy.ceiling] cpu_share`, a whole percent of the machine's cores (1 to 100, default 50), never less than one core and never more than all of them. It is one value for every task; no task raises it. The probe reports it as `C.cpu` beside the clauses (`status`: `clause C cpu`; `--json`: `cpu: {state, primitive?, reason?}`), and nothing that decides the tier reads it.
- **Linux:** the worker's cgroup has two leaves, `worker` (the worker) and `tool` (the tools), and `cpu.max` goes on `tool` (`enforced (cgroup_cpu_max)`); `memory.max` and `pids.max` stay on the parent. cgroup v2 passes `cpu` to the leaves only when the cgroup above the worker's holds no process, and that cgroup is the runtime's own, which always holds the runtime: so **in practice the part is `unavailable` with its reason on Linux** (`cgroup_cpu_not_enabled (the runtime's cgroup holds processes)`, `cgroup_cpu_not_delegated`, …). The book shows the way that bounds the whole runtime instead: `systemd-run --user --scope -p CPUQuota=…`.
- **Windows:** a hard CPU rate on the worker's Job Object (`enforced (job_object)`), converted to the whole machine's share and read back; a rate that does not read back is `unavailable` with the two numbers.
- **macOS:** the worker's nice value lowered to 10 before its profile is applied, inherited by its tools and read back (`best effort (priority)`); never the background policy.

**The machine view (decision 8, REQ-OBS-009).** `status` gains a section with:
- the user's runtimes that are executing or waiting (pid, data directory, task, since when);
- each engine gate, with the requests in flight, its width, its holders and the runtimes waiting for it;
- the slots taken against LIM-X5.

`metrics` gains `admission_wait_ms`, `admission_waits`, `engine_wait_ms` and `engine_waits`, per task with `--task` and over all tasks without, as its other counts. The words `running`, `waiting` and `slot` are used nowhere else with another meaning.

### 19.4 Effort levels (REQ-TASK-013, REQ-CFG-007, REQ-OBS-010, REQ-PERS-015; RFC 015 with Amendment 1, from 0.13)

A task is created at one of three levels, `quick`, `standard` or `deep`: fixed identifiers, ordered `quick` < `standard` < `deep`. A level is a budget, never a permission: no level grants a tool, a path, a destination or an effect.

**The envelope.** A level is an overlay of five keys, `[effort.<level>]`:
- `backend` (a name from `[backends]`), `steps` (LIM-T1), `deadline` (LIM-T5, a duration as `--deadline` takes it), `context_chars` (LIM-C1) and `recent_steps` (LIM-C3, 1 to 50);
- **precedence:** a flag (`--steps`, `--deadline`) wins over the level, the level over the profile, and the profile over the register's default. The guard counters (LIM-T2, T3, T4, T6) are the profile's alone;
- **shipped:** `standard` and `deep` have no keys; `quick` has `steps = 22` (LIM-E4) and `deadline = "10m"` (LIM-E5). A key in the file replaces the shipped one; a key left out is the profile's.
- **A level other than `standard` with no key, shipped or configured, is refused when chosen** (`deep` until it is configured), naming `[effort.<level>]` and what to set in it; no task is created (§20).

**`check` refuses** (each naming the level, the key and the bound):
- a level name other than the three;
- a `backend` that is not configured;
- a `context_chars` above its backend's;
- a `steps` above the ceiling's `max_steps`, a `deadline` outside LIM-T5's range, a `recent_steps` outside 1 to 50;
- levels whose backends are not all local or all on one remote URL (REQ-CFG-007);
- an `effort_max` that is not a level, or a profile's `effort_max` above the ceiling's.

**`effort_max`** (`[policy.ceiling]` and `[policy.profiles.<name>]`; absent means `deep`). A task's level must be at most the lower of the two:
- `--effort` above it is refused, naming the profile or the ceiling that bounds it; no task is created (§20);
- **without `--effort`** the level is `standard`, or the bound when it is lower (`chosen_by: default`, and the creation's first line says which).

**Choosing.** `run --effort <level>` and `task new --effort <level>`. A fork takes its source's level unless `--effort` is given, its envelope resolved from the configuration now. A level name that is not one of the three is a usage error (exit 2).

**Recorded and fixed.** `task.created` and the task record carry `effort{level, chosen_by: user | default | fork, envelope{backend, steps, deadline_ms, context_chars?, recent_steps}}`, every key resolved but one: what the task runs with, not what the file said.
- **`context_chars` is recorded only when the level sets it** (review of 015c). Otherwise the task's budget is its backend's `context_chars` as it is at each step, so that lowering it for an engine whose window is too small reaches the tasks already made. A level's own value is bounded at each step by its backend's as it is then (`check` refuses a larger one; an edit after the task's creation can lower the backend's).
- The envelope never changes during the task. A later change of `[effort.*]` applies to new tasks; `resume --extend-steps` and `--extend-deadline` extend as they always did, and `task show` shows both the envelope and the extension.
- The backend is resolved by its name at every start (Amendment 1, item 3; §20). The step records `backend`, `model` and `transport` (REQ-PERS-015; a step that sent no request records no `transport`), so that a task whose backend's model changed shows it. The notice of a changed model is printed by the command that starts the task; a task that recovery resumes on its own has none, and `task show` lists every model.
- **A task found `running` whose backend is no longer configured** (recovery resumed it before the file changed) is halted `backend_unavailable`, with `halt_detail` `backend_not_configured` naming the backend and the configured ones. **A task found `running` whose backend is configured but cannot be made** (for example a key by reference that the environment does not hold) is halted the same way, with `halt_detail` `backend_unusable` and the reason (review of 015c-6). Neither is ever left `running`, and nothing is sent. The line printed gives both remedies: fix the configuration and resume, or fork the task onto another level (`task new --from-task <task> --effort <level>`).
- A task record without `effort` (written before 0.13) reads as `standard`, `chosen_by: default`, with the envelope of its own budgets. Every field is optional and the schema version does not change.

**Shown.**
- **`run` and `task new`** print the level and its envelope as their first line about the task (a notice of the configuration may come before it): `effort    quick (you chose): steps 22, deadline 10m, backend default (qwen2.5-7b-instruct)`.
- **`task show`** answers what the task cost (REQ-OBS-010): the level and who chose it, the steps decided (the steps the task has finished; a step in flight is not yet decided) and the steps counted against the budget, each named, since the step that ends a task is not counted (`7 steps decided; 6 of 22 counted against the budget`), the context budget in force as the configuration is now (the level's, or `the backend's` with its value; in `--json`, `cost.context_chars` and `cost.context_source: level | backend`), the time against the deadline, the model time, the prompt, cached and completion tokens where reported, and the backend and model, every one of them when the task used more than one.
- **`status`** lists the three levels, each with its backend and its keys, and `not configured` for a level that would be refused.
- **`metrics`** aggregates under `by_effort.<level>`: tasks by state, steps, model time, and prompt, cached and completion tokens. With `--task`, the task's own level.

**What a level is, for the user** (the book; the M8 report):
- **`quick` stops a task that wanders sooner.** It does not make a completing task faster. A smaller model's backend in `[effort.quick]` makes it faster.
- **`deep` is the larger model you name.**
  - The fitness probe tells whether a model acts, never whether it finishes a task, and the engine matters as much as the model.
  - On one consumer GPU, a level on another model pays one swap at the task's first request; two `llama-server` processes pay none.

## 20. Failure and degradation

| Condition | Visible behaviour | Requirement |
|---|---|---|
| Backend unreachable | retries per LIM-M3, then halt `backend_unavailable` | REQ-MDL-003 |
| A model backend answers with a redirect (from 0.13) | not followed; the request fails naming the host the redirect points to, as an unreachable backend does; nothing is sent there | REQ-SEC-010 |
| A level chosen that has no key, shipped or configured (from 0.13; `deep` until it is configured) | `run --effort <level>` and `task new --effort <level>` refused, naming `[effort.<level>]` and what to set in it (a backend, or a budget); no task is created | REQ-TASK-013 |
| A level above the profile's or the ceiling's `effort_max` (from 0.13) | `run --effort <level>` and `task new --effort <level>` refused, naming the bound and where it is set; no task is created | REQ-TASK-013 |
| A task's backend unreachable at a creation that starts it (from 0.13; `run`, and `task new` without `--hold`, at every level and with or without `--effort`) | refused with the backend's name and the reason; no task is created. The line names `[backends.<name>]` to fix, and names `[effort.<level>]` only when that level is what chose the backend. `task new --hold` contacts no backend | REQ-MDL-010 |
| A task's backend no longer configured at a start (from 0.13) | the start refused, naming the backend and the configured ones; the task stays as it was; nothing moves it to another backend | REQ-MDL-010 |
| A task's backend now `remote` under a profile without `external`, at a start | the task halts `policy`, naming the backend and the profile, before any request is sent | REQ-SEC-010 |
| A task's backend serves another model under the same name, at a start (from 0.13) | the task goes on; a notice at the start names both models; each step records its model, and `task show` says the task used more than one | REQ-MDL-010, REQ-PERS-015 |
| Model output unparsable | rejection `malformed`, feedback; LIM-T2 rejections halt | REQ-DEC-003 |
| Out-of-scope path | `denied`, no filesystem access, no existence leak | REQ-DEC-007 |
| Infinite loop in generated code | `fuel_exhausted` or `deadline`; feedback | REQ-SBX-002 |
| Allocation bomb | `memory_limit`; feedback | REQ-SBX-002 |
| Output flood | `output_limit`; excerpt plus blob; feedback | REQ-SBX-005 |
| Failed write action | rolled back; observation says so | REQ-TOOL-010 |
| Worker dies | `host_fault{worker_died}`; worker restarted up to LIM-X3 | REQ-ARCH-008 |
| Confinement clause unavailable | host tools unavailable; tier `core`; reason shown | REQ-CONF-004 |
| Mandatory clause best-effort | tier `core`, unless `policy.accept_best_effort` names it: then `full (… accepted …)` shown in `status` and in approvals | RFC 011 §5 |
| Host tool argument outside the declaration | rejection `denied{argument_not_allowed}` naming the position and the value, or the bound | REQ-CONF-006 |
| Runtime crash | recovery of §16.3; nothing replayed | REQ-PERS-007 |
| Approval expires | halt `approval_expired` | REQ-APR-006 |
| Workspace quota | halt `workspace_quota` | REQ-WS-008 |
| Part of the workspace cannot be read (an action changed its permissions, or another failure of a read) | halt `workspace_unreadable`, the event `workspace.unreadable{path, kind?}`, a line naming the path and the remedy (restore its permissions, then resume, for a permission problem; the failure's own words otherwise); after a rollback that failed, the line also says that the workspace was not rolled back (`rollback_failed` on the halt's state change); never an internal error that leaves the task `running` (review of handoff 002e) | REQ-WS-004 |
| Second runtime on the data directory | refuses to start, `conflict`, naming the holder (`the store is in use by runtime <pid> since <time>`) | REQ-PERS-001 |
| Engine context smaller than one request needs (the budget's tokens at the transport's lowest measured characters per token, plus `max_tokens`) | `status` shows `context: <n> tokens per request (needs about <m>)` and a warning with the engine's remedy (llama.cpp's `-c` for all its slots, or Ollama's `num_ctx`); `run` and the commands that drive steps say it once on standard error; the run goes on. The context is the engine's word (llama.cpp's `/props`; Ollama's `/api/ps` for a loaded model, else the Modelfile's `num_ctx`), else `unknown`, never assumed | REQ-MDL-003 |
| MCP definition changed | tool unavailable for the task; `tool.unpinned` | REQ-MCP-002 |

---

## 21. Security

### 21.1 Order of evaluation

The order of §6.3, preceded by the session check of the control API and followed by result normalization and audit persistence. No later stage compensates for a skipped earlier stage.

### 21.2 Invariants (REQ-SEC-004)

| # | Invariant |
|---|---|
| INV-01 | No executor call without an authorized action constructed by the validator |
| INV-02 | Model text never changes grants, budgets, policy, approvals or tool visibility |
| INV-03 | An out-of-scope path is `denied` before any filesystem access, identically whether it exists or not |
| INV-04 | Every execution has a deadline and output limits; sandbox executions have fuel and memory limits too |
| INV-05 | Every transition is durable before its consequences are visible |
| INV-06 | An approval binds to one action hash and one grant-set hash and is consumed once |
| INV-07 | No non-reversible effect is re-executed while its outcome is unknown |
| INV-08 | Snapshot and restore never follow symbolic links; the database references only materialized snapshots |
| INV-09 | Nothing enters the model context without a provenance label; grants, halts, rejections and unresolved failures are never pruned |
| INV-10 | Nothing writes to the project scope except promotion |
| INV-11 | Grants never exceed the ceiling and never widen |
| INV-12 | A restart never turns an unfinished action into a success |
| INV-13 | The sandbox and the worker have no network; external network exists only through tools with a matching `network` grant, or through a declared dependency step (`network = true`) under its own `host_tool` grant, whose destinations the runtime cannot bound and whose declaration and status say so (REQ-CONF-007, THR-35) |
| INV-14 | Secrets never appear in prompts, observations, logs or child environments |

### 21.3 Consistency rules (REQ-PERS-006)

| # | Rule |
|---|---|
| CONS-01 | A `completed` task has no step, action, approval or question that is not terminal |
| CONS-02 | No action is `running` under a task in a terminal state |
| CONS-03 | Every `succeeded` execution belongs to an `authorized` action whose approval, if required, is `consumed` |
| CONS-04 | Every rollback references an existing, verified snapshot |
| CONS-05 | Every approval references exactly one action and matches its hash |
| CONS-06 | No action is authorized under a revoked or expired grant |
| CONS-07 | Every action's status distinguishes `proposed`, `running`, terminal and `unknown` durably |
| CONS-08 | Every counter is within its budget, or the task is `halted` with the matching reason |

---

## 22. Versioning (REQ-MAIN-001, REQ-MAIN-002)

Independently versioned:
- the decision schema (`DECISION_SCHEMA_VERSION`, 1);
- the action, execution and observation schemas (`ACTION_SCHEMA_VERSION`, 2);
- the event schema (`EVENT_SCHEMA_VERSION`, 2);
- the configuration schema (`config_version`, 1; 2 from 0.13, with 1 still read through 0.14, REQ-CFG-007);
- the persistence schema (`SCHEMA_VERSION`, 4);
- the worker protocol.

Every task records the decision, event and action schema versions, the policy hash, the tool-definition hashes and the runtime version in force at creation (`task.versions`). A task written before the event and action versions were recorded reads with the values of its release.

**When a version changes.** An additive field, or a value a reader may ignore, raises nothing. A renamed or removed field, event or code, or a changed meaning, raises the version by one and is announced in the changelog (REQ-MAIN-001). The history before this rule is numbered by release: events 1 up to 0.8.0 and 2 from 0.9.0; actions 1 up to 0.7.1 and 2 from 0.8.0 (handoff 004b). Every name, field, state, effect, code and command in this document is part of the external contract.

**A reader meets a newer release's word (REQ-MAIN-006, from 0.13).** An event, a task state, a wait kind or a reason that a command reading the store does not know was written by a newer release. The command shows it by its name and its raw fields, marked `unknown to this release`, and goes on: a listing never fails for it, and neither does `task show` of the task that holds it. A writer (a command that runs steps) refuses to act on such a task, naming the word and saying which release wrote it is needed. Input from outside the store (the configuration, the model's reply, a tool's result) keeps refusing an unknown value, as every fixed vocabulary does. **The shape (handoff 004e).**
- **Text:** the word as written, then `(unknown to this release)`.
- **JSON:** the record as written, and the object that holds the word gains `unknown_to_this_release: [{field, word}]`.
  - `field` runs from that object's root: `.` between names and `[i]` for a list (`wait_kind`, `pending.pending`, `notices[0].code`). A map or a list of words is named by the field that holds it (`events_by_name`).
  - A record that holds no such word never carries the key, so a store of an earlier release reads byte for byte as before.
- **Where the key sits:** `events` puts it on the record, `task list` on the row, and `task show` on the task. `task show` also gives `unknown_records: [{kind, key, record}]` for the task's other records that hold one.
- **A writer refuses with the error kind `state`,** naming the record, the field and the word. This covers every command that changes or runs a task, `task delete` included, and `prune` leaves such a task and lists it. A task is refused when any of its records holds such a word, events included, because a writer never acts on a task it cannot read whole.
- **Recovery at a start** leaves such a task as it is and says so once per start on standard error.
- **Free text is not marked.** A field that the schema gives as text, even where it names a state (`task.state_changed`'s `from` and `to`), is shown as written; the contract's inventory before the freeze decides whether such fields are typed.

---

## 23. Acceptance demonstrations

| Milestone | Demonstration |
|---|---|
| M1 | With the fake backend: create → `act` → validate → approval prompt → execute → observe → `complete`; the process is killed at each of the eight persistence lines of §16.2 and resumes correctly; `read_file{path: "../../etc/shadow"}` and a non-existent out-of-scope path yield byte-identical `denied` |
| M2 | On Linux, Windows and macOS: a JavaScript infinite loop ends `fuel_exhausted` or `deadline`; an allocation bomb ends `memory_limit`; a print flood ends `output_limit`; `status` shows the tier |
| M3 | A failed `run_script` in `read_write` mode leaves the workspace at its pre-execution snapshot; `promote` refuses a conflicting file and otherwise shows the diff, asks, backs up and applies; a workspace symlink to `/etc` is neither snapshotted nor restored through |
| M4 | A local model repairs a scripted defect in a JavaScript workload within LIM-T1 steps; the prefix of segments 1–4 is byte-stable across steps; the measurement report exists |
| M5 | `rypsok.web_search` and `rypsok.web_fetch` run under a `network` grant; a `web_fetch` to an ungranted domain is `denied`; `rypsok.engine_status` is not listed to the model |
| M6a, M6b, M6c | `cargo test --offline` of a native project runs under every mandatory clause; `status` lists the clauses enforced. **Exception, M6b (0.7.0):** on Windows the clause suite passes with the probe tool, but `cargo` cannot run inside an AppContainer on current Windows (the `NUL` device is refused), so this demonstration is not met there; investigated on 2026-11-03 on Windows Server 2022 and 2025 (the same on both): `NUL`, `\\.\NUL` and `\\?\GLOBALROOT\Device\Null` are refused to the container for read and write, and `cargo` opens `NUL` as the standard input of every `rustc`; no remedy that keeps clauses F, N and P was found, so this is an exception without a remedy, revisited if Windows changes the container's access to the device |

---

## 24. Requirements coverage

Every requirement of the baseline and the section of this document that satisfies it. "Internal" marks requirements with no externally visible form; they are verified by tests and inspection.

| Requirement | Title | Milestone | External design |
|---|---|---|---|
| REQ-ARCH-001 | Layer separation | M1 | §3 API tiers; §13.1 Worker; §22 Versioning |
| REQ-ARCH-002 | Core without I/O or protocol dependencies | M0 | §3 API tiers; §13.1 Worker; §22 Versioning |
| REQ-ARCH-003 | Asynchronous runtime | All | §3 API tiers; §13.1 Worker; §22 Versioning |
| REQ-ARCH-004 | Three API tiers | M1 | §3 API tiers; §13.1 Worker; §22 Versioning |
| REQ-ARCH-005 | Runtime authority by construction | All | §3 Adapter SPI; §21.2 INV-01 |
| REQ-ARCH-006 | Deterministic control logic | M1 | §16 Persistence; §23 M1 demonstration |
| REQ-ARCH-007 | Pinned engines | M2, M5 | §3 API tiers; §13.1 Worker; §22 Versioning |
| REQ-ARCH-008 | Worker process | M2 | §13.1 Worker |
| REQ-TASK-001 | Creation | M1 | §4 Control API; §5 Task state machine |
| REQ-TASK-002 | States and transitions | M1 | §4 Control API; §5 Task state machine |
| REQ-TASK-003 | Terminal immutability | M1 | §4 Control API; §5 Task state machine |
| REQ-TASK-004 | Fork | M3 | §4 Control API; §5 Task state machine |
| REQ-TASK-005 | Pause and resume | M1 | §4 Control API; §5 Task state machine |
| REQ-TASK-006 | Cancel | M1 | §4 Control API; §5 Task state machine |
| REQ-TASK-007 | Halt reasons | M1 | §4 Control API; §5 Task state machine |
| REQ-TASK-008 | Budgets | M1 | §4 Control API; §5 Task state machine |
| REQ-TASK-009 | Deadline | M1 | §4 Control API; §5 Task state machine |
| REQ-TASK-010 | Completion | M1 | §5.3 Terminal fields |
| REQ-TASK-011 | Give up | M1 | §4 Control API; §5 Task state machine |
| REQ-TASK-012 | One active task per project scope | M1 | §18 Errors (`conflict`) |
| REQ-DEC-001 | Step structure | M1 | §6 Steps and decisions |
| REQ-DEC-002 | Decision schema | M1 | §6 Steps and decisions |
| REQ-DEC-003 | Strict parsing, no repair | M1 | §6 Steps and decisions |
| REQ-DEC-004 | Validation order | M1 | §6 Steps and decisions |
| REQ-DEC-005 | Tool visibility | M1 | §6 Steps and decisions |
| REQ-DEC-006 | Argument validation | M1 | §6 Steps and decisions |
| REQ-DEC-007 | Authorization before existence | M1 | §6 Steps and decisions |
| REQ-DEC-008 | Rejection vocabulary | M1 | §6 Steps and decisions |
| REQ-DEC-009 | Feedback | M1 | §6 Steps and decisions |
| REQ-DEC-010 | Counters and convergence | M1 | §6 Steps and decisions |
| REQ-DEC-011 | Plan | M1 | §6 Steps and decisions |
| REQ-DEC-012 | Canonical hashes | M1 | §6 Steps and decisions |
| REQ-CAP-001 | No ambient authority | All | §9 Capabilities and policy |
| REQ-CAP-002 | Grant kinds | M1 | §9 Capabilities and policy |
| REQ-CAP-003 | Ceiling | M1 | §9 Capabilities and policy |
| REQ-CAP-004 | Issuance | M1 | §9 Capabilities and policy |
| REQ-CAP-005 | Revocation | M1 | §9 Capabilities and policy |
| REQ-CAP-006 | No broadening | All | §9 Capabilities and policy |
| REQ-CAP-007 | Grant-set hash | M1 | §9 Capabilities and policy |
| REQ-CAP-008 | Capability text generated | M1 | §9 Capabilities and policy |
| REQ-CAP-009 | Profiles | M1 | §9 Capabilities and policy |
| REQ-CAP-010 | Policy versioning | M1 | §9 Capabilities and policy |
| REQ-APR-001 | When approval is required | M1 | §10 Approvals and questions |
| REQ-APR-002 | Request content | M1 | §10 Approvals and questions |
| REQ-APR-003 | Rendering safety | M1 | §10 Approvals and questions |
| REQ-APR-004 | Binding | M1 | §10 Approvals and questions |
| REQ-APR-005 | One-shot | M1 | §10 Approvals and questions |
| REQ-APR-006 | Expiry | M1 | §10 Approvals and questions |
| REQ-APR-007 | Human decisions | M1 | §10 Approvals and questions |
| REQ-APR-008 | No inferred approval | All | §10 Approvals and questions |
| REQ-APR-009 | Authority | M1 | §10 Approvals and questions |
| REQ-APR-010 | Questions | M1 | §10 Approvals and questions |
| REQ-TOOL-001 | Tool definition | M1 | §7 Actions; §11 Built-in tools |
| REQ-TOOL-002 | Effect vocabulary | M1 | §7 Actions; §11 Built-in tools |
| REQ-TOOL-003 | Built-in tools | M1, M2 | §7 Actions; §11 Built-in tools |
| REQ-TOOL-004 | File tools use directory handles | M1 | §7 Actions; §11 Built-in tools |
| REQ-TOOL-005 | Result validation | M1 | §7 Actions; §11 Built-in tools |
| REQ-TOOL-006 | Deadlines | M1 | §7 Actions; §11 Built-in tools |
| REQ-TOOL-007 | Cancellation | M1 | §7 Actions; §11 Built-in tools |
| REQ-TOOL-008 | Output bounds | M1 | §7 Actions; §11 Built-in tools |
| REQ-TOOL-009 | Runtime retry | M1 | §7 Actions; §11 Built-in tools |
| REQ-TOOL-010 | Transaction equals action | M3 | §7 Actions; §11 Built-in tools |
| REQ-SBX-001 | Wasmtime in the worker | M2 | §12 Sandbox profile; §7.4 Termination |
| REQ-SBX-002 | Triple guard | M2 | §12 Sandbox profile; §7.4 Termination |
| REQ-SBX-003 | Termination reporting | M2 | §12 Sandbox profile; §7.4 Termination |
| REQ-SBX-004 | WASI configuration | M2 | §12 Sandbox profile; §7.4 Termination |
| REQ-SBX-005 | Output capture | M2 | §12 Sandbox profile; §7.4 Termination |
| REQ-SBX-006 | Module validation | M2 | §12 Sandbox profile; §7.4 Termination |
| REQ-SBX-007 | Instance isolation | M2 | §12 Sandbox profile; §7.4 Termination |
| REQ-SBX-008 | Replay mode | M2 | §12 Sandbox profile; §7.4 Termination |
| REQ-SBX-009 | Overhead measurement | M2 | §12 Sandbox profile; §7.4 Termination |
| REQ-SBX-010 | No host compilers | M2 | §12 Sandbox profile; §7.4 Termination |
| REQ-SBX-011 | Resource accounting | M2 | §12 Sandbox profile; §7.4 Termination |
| REQ-SBX-012 | Fail closed | M2 | §12 Sandbox profile; §7.4 Termination |
| REQ-CONF-001 | Confinement contract | M2 | §13 Worker and confinement |
| REQ-CONF-002 | Worker outer layer | M2 | §13 Worker and confinement |
| REQ-CONF-003 | Probe and tier | M2 | §13 Worker and confinement |
| REQ-CONF-004 | Fail closed for host tools | M6a | §13 Worker and confinement |
| REQ-CONF-005 | Backends | M6a, M6b, M6c | §13 Worker and confinement |
| REQ-CONF-006 | Declared host tools | M6a | §13 Worker and confinement |
| REQ-CONF-007 | Offline rule | M6a | §13 Worker and confinement |
| REQ-CONF-008 | Toolchain visibility | M6a | §13 Worker and confinement |
| REQ-CONF-009 | Tree termination | M2 | §13 Worker and confinement |
| REQ-CONF-010 | Unsafe code boundary | M2 | §13 Worker and confinement |
| REQ-CONF-011 | Read sets hold no credentials | M6a, M6b | §13.3 Backends; §13.5 Host tools |
| REQ-CONF-012 | Process memory and descriptors are not reachable | M6a | §13.3 Backends |
| REQ-WS-001 | Location and permissions | M1 | §14 Workspace |
| REQ-WS-002 | Materialization | M1 | §14 Workspace |
| REQ-WS-003 | Exclusions | M1 | §14 Workspace |
| REQ-WS-004 | Snapshots | M3 | §14 Workspace |
| REQ-WS-005 | Symlink safety | M3 | §14 Workspace |
| REQ-WS-006 | Rollback | M3 | §14 Workspace |
| REQ-WS-007 | Promote | M3 | §14 Workspace |
| REQ-WS-008 | Quotas | M1 | §14 Workspace |
| REQ-WS-009 | Lifecycle | M3 | §14 Workspace |
| REQ-WS-010 | Cancellation and the workspace | M3 | §14 Workspace |
| REQ-WS-011 | Case and path rules | M1 | §14 Workspace |
| REQ-WS-012 | Garbage collection | M3 | §14 Workspace |
| REQ-CTX-001 | Prompt structure | M4 | §15 Context assembly; §8 Observations |
| REQ-CTX-002 | Provenance | M1 | §15 Context assembly; §8 Observations |
| REQ-CTX-003 | Budget and pruning | M4 | §15 Context assembly; §8 Observations |
| REQ-CTX-004 | Rule-based compression | M4 | §15 Context assembly; §8 Observations |
| REQ-CTX-005 | Tool definitions | M1 | §15 Context assembly; §8 Observations |
| REQ-CTX-006 | Observation excerpts | M1 | §15 Context assembly; §8 Observations |
| REQ-CTX-007 | Secrets never enter the context | M1 | §15 Context assembly; §8 Observations |
| REQ-CTX-008 | Invisible characters | M1 | §15 Context assembly; §8 Observations |
| REQ-CTX-009 | Structured results preserved | M1 | §15 Context assembly; §8 Observations |
| REQ-CTX-010 | Prefix stability measured | M4 | §15 Context assembly; §8 Observations |
| REQ-PERS-001 | Store | M1 | §16 Persistence and recovery |
| REQ-PERS-002 | Schema version | M1 | §16 Persistence and recovery |
| REQ-PERS-003 | Durable transitions | M1 | §16 Persistence and recovery |
| REQ-PERS-004 | Write ordering | M1 | §16 Persistence and recovery |
| REQ-PERS-005 | Blobs | M1 | §16 Persistence and recovery |
| REQ-PERS-006 | Integrity at startup | M1 | §16 Persistence and recovery |
| REQ-PERS-007 | Recovery procedure | M1 (basic), M3 (with snapshots) | §16 Persistence and recovery |
| REQ-PERS-008 | No automatic replay | All | §16 Persistence and recovery |
| REQ-PERS-009 | Snapshot references | M3 | §16 Persistence and recovery |
| REQ-PERS-010 | Retention | M3 | §16 Persistence and recovery |
| REQ-PERS-011 | Backup | M3 | §16 Persistence and recovery |
| REQ-PERS-012 | Versions recorded | M1 | §16 Persistence and recovery |
| REQ-SCHED-001 | Concurrency limits | M1 | §4 Control API (queueing); §13.1 Worker |
| REQ-SCHED-002 | Sequential steps | M1 | §4 Control API (queueing); §13.1 Worker |
| REQ-SCHED-003 | Memory budget | M2 | §4 Control API (queueing); §13.1 Worker |
| REQ-SCHED-004 | Cancellation propagation | M1 | §4 Control API (queueing); §13.1 Worker |
| REQ-SCHED-005 | Resource release | M1 | §4 Control API (queueing); §13.1 Worker |
| REQ-OBS-001 | Events | M1 | §17 Events, metrics and logs; §18 Errors |
| REQ-OBS-002 | Event stream | M1 | §17 Events, metrics and logs; §18 Errors |
| REQ-OBS-003 | Metrics | M1, M4 | §17 Events, metrics and logs; §18 Errors |
| REQ-OBS-004 | Logs | M1 | §17 Events, metrics and logs; §18 Errors |
| REQ-OBS-005 | Audit retention | M3 | §17 Events, metrics and logs; §18 Errors |
| REQ-OBS-006 | Runtime status | M2 | §17 Events, metrics and logs; §18 Errors |
| REQ-OBS-007 | Replay | M4 | §17 Events, metrics and logs; §18 Errors |
| REQ-OBS-008 | Refusals counted | M1 | §17 Events, metrics and logs; §18 Errors |
| REQ-SEC-001 | Threat register | All | §21 Security; §20 Failure and degradation |
| REQ-SEC-002 | Model text is never authority | All | §21 Security; §20 Failure and degradation |
| REQ-SEC-003 | Fail closed | All | §21 Security; §20 Failure and degradation |
| REQ-SEC-004 | Invariants tested | All | §21 Security; §20 Failure and degradation |
| REQ-SEC-005 | Path safety | M1 | §21 Security; §20 Failure and degradation |
| REQ-SEC-006 | Network denial | M2 | §12 Sandbox profile; §13.2 Clause N |
| REQ-SEC-007 | Approval channel | All | §2 Actors and channels; §4 Control API |
| REQ-SEC-008 | Local files | M1 | §21 Security; §20 Failure and degradation |
| REQ-SEC-009 | Model output size | M1 | §6.3 Validation order |
| REQ-SEC-010 | Non-local backends | M4 | §19.1 Configuration (locality) |
| REQ-SEC-011 | Process stability | All | §21 Security; §20 Failure and degradation |
| REQ-SEC-012 | Dependency hygiene | M0 | §21 Security; §20 Failure and degradation |
| REQ-SEC-013 | Security regression corpus | M1 | §21 Security; §20 Failure and degradation |
| REQ-SEC-014 | Approval fatigue is a defect | All | §10 Approvals; §7.2 Effects |
| REQ-SEC-015 | Promotion is the only exit | All | §14.5 Promotion; §21.2 INV-10 |
| REQ-PRIV-001 | Minimization | M1 | §14.2 Materialization; §15 Context; §17.3 Logs; §19.1 Configuration |
| REQ-PRIV-002 | Secret patterns and redaction | M1 | §14.2 Materialization; §15 Context; §17.3 Logs; §19.1 Configuration |
| REQ-PRIV-003 | Environment | M1 | §14.2 Materialization; §15 Context; §17.3 Logs; §19.1 Configuration |
| REQ-PRIV-004 | Logs | M1 | §14.2 Materialization; §15 Context; §17.3 Logs; §19.1 Configuration |
| REQ-PRIV-005 | Disclosure | M4, M5 | §14.2 Materialization; §15 Context; §17.3 Logs; §19.1 Configuration |
| REQ-PRIV-006 | Retention | M3 | §14.2 Materialization; §15 Context; §17.3 Logs; §19.1 Configuration |
| REQ-MDL-001 | Backend SPI | M1 | §3 Adapter SPI; §19.1 Configuration (backend) |
| REQ-MDL-002 | Fake backend | M1 | §3 Adapter SPI; §19.1 Configuration (backend) |
| REQ-MDL-003 | OpenAI-compatible adapter | M4 | §3 Adapter SPI; §19.1 Configuration (backend) |
| REQ-MDL-004 | Structured output modes | M4 | §3 Adapter SPI; §19.1 Configuration (backend) |
| REQ-MDL-005 | Locality | M4 | §3 Adapter SPI; §19.1 Configuration (backend) |
| REQ-MDL-006 | Usage metrics | M4 | §3 Adapter SPI; §19.1 Configuration (backend) |
| REQ-MDL-007 | Output is untrusted text | All | §3 Adapter SPI; §19.1 Configuration (backend) |
| REQ-MDL-008 | No model inside the core | All | §3 Adapter SPI; §19.1 Configuration (backend) |
| REQ-MDL-009 | Decision transport | M4 (0.9) | §15.4 The `tool_calls` transport; §19.1 Configuration (backend) |
| REQ-PERS-013 | Read-only open | M7a | §16 Persistence (RFC 013 decision 1) |
| REQ-PERS-014 | One owner of a store under the daemon | M9 | §16 Persistence (RFC 014 decision 2) |
| REQ-PERS-015 | The backend recorded per step | M8 | §16 Persistence (RFC 015 decision 4) |
| REQ-WS-013 | Storage view | M7a | §14.6 Quotas and lifecycle (RFC 013 decision 2) |
| REQ-WS-014 | Prune | M7a | §14.6 Quotas and lifecycle (RFC 013 decision 3) |
| REQ-CLI-007 | Storage and operation pages | M7a | §19 Operator interface (RFC 013 decision 4) |
| REQ-CLI-008 | Daemon lifecycle | M9 | §19 Operator interface (RFC 014 decision 7) |
| REQ-CLI-009 | Output to a closed pipe | M8 (0.13) | §19.2 Command line |
| REQ-SCHED-006 | Admission per user | M7b | §19 Operator interface; §20 (RFC 013 decision 5) |
| REQ-SCHED-007 | Engine gate | M7b | §20 (RFC 013 decision 7) |
| REQ-SCHED-008 | One scheduler under the daemon | M9 | §4 Control API (RFC 014 decision 6) |
| REQ-CONF-013 | CPU part of clause C | M7b | §13.2 Clauses; §13.3 (RFC 013 decision 6) |
| REQ-OBS-009 | Machine view | M7b | §17 Events, metrics and logs (RFC 013 decision 8) |
| REQ-OBS-010 | Cost visibility | M8 | §17 (RFC 015 decision 7) |
| REQ-ARCH-009 | Two deployment modes | M9 | §1 System context; §4 (RFC 014 decision 1) |
| REQ-SEC-016 | The daemon's transport | M9 | §21 Security (RFC 014 decisions 3, 8) |
| REQ-TASK-013 | Effort levels | M8 | §19.1 Configuration; §19.4 Effort levels (RFC 015 decision 1, Amendment 1) |
| REQ-TASK-015 | Plan first | M8 | §5; §6.4 Rejection vocabulary (RFC 015 decision 6, Amendment 1) |
| REQ-CFG-007 | Effort and backends configuration | M8 | §19.1 Configuration (RFC 015 decisions 1, 3) |
| REQ-MDL-010 | Several backends | M8 | §19.1; §20 (RFC 015 decision 3) |
| REQ-MDL-011 | Fitness probe | M8 | §19.2 Command line (RFC 015 decision 5, Amendment 1) |
| REQ-MCP-001 | Servers as children | M5 | §11.1 External tools; §19.1 Configuration (servers) |
| REQ-MCP-002 | Discovery and pinning | M5 | §11.1 External tools; §19.1 Configuration (servers) |
| REQ-MCP-003 | Definition import | M5 | §11.1 External tools; §19.1 Configuration (servers) |
| REQ-MCP-004 | Effect declaration | M5 | §11.1 External tools; §19.1 Configuration (servers) |
| REQ-MCP-005 | Destination validation | M5 | §11.1 External tools; §19.1 Configuration (servers) |
| REQ-MCP-006 | Results | M5 | §11.1 External tools; §19.1 Configuration (servers) |
| REQ-MCP-007 | Cancellation | M5 | §11.1 External tools; §19.1 Configuration (servers) |
| REQ-MCP-008 | Deadlines | M5 | §11.1 External tools; §19.1 Configuration (servers) |
| REQ-MCP-009 | Visibility | M5 | §11.1 External tools; §19.1 Configuration (servers) |
| REQ-MCP-010 | No MCP server surface | All | §11.1 External tools; §19.1 Configuration (servers) |
| REQ-CFG-001 | Configuration file | M1 | §19.1 Configuration |
| REQ-CFG-002 | Defaults | M1 | §19.1 Configuration |
| REQ-CFG-003 | Secrets by reference | M1 | §19.1 Configuration |
| REQ-CFG-004 | Policy | M1 | §19.1 Configuration |
| REQ-CFG-005 | No hot reload | M1 | §19.1 Configuration |
| REQ-CFG-006 | Message catalog | M1 | §19.1 Configuration |
| REQ-CLI-001 | Commands | M1 | §19.2 Command line |
| REQ-CLI-002 | Approval prompt | M1 | §19.2 Command line |
| REQ-CLI-003 | Follow mode | M1 | §19.2 Command line |
| REQ-CLI-004 | Output | M1 | §19.2 Command line |
| REQ-CLI-005 | Shutdown | M1 | §19.2 Command line |
| REQ-CLI-006 | Tier display | M2 | §19.2 Command line |
| REQ-PLAT-001 | Rust | All | §13.3 Backends; §14 Workspace; Internal: build configuration |
| REQ-PLAT-002 | Platforms | M1 | §13.3 Backends; §14 Workspace; Internal: build configuration |
| REQ-PLAT-003 | Tier per platform | M2, M6 | §13.3 Backends; §14 Workspace; Internal: build configuration |
| REQ-PLAT-004 | Filesystem semantics | M1 | §13.3 Backends; §14 Workspace; Internal: build configuration |
| REQ-ENG-001 | Unsafe code | M0 | Internal: build configuration and CI |
| REQ-ENG-002 | CI gate | M0 | Internal: build configuration and CI |
| REQ-ENG-003 | Test layout | All | Internal: build configuration and CI |
| REQ-ENG-004 | Panic policy | M1 | Internal: build configuration and CI |
| REQ-ENG-005 | Typed boundaries | All | Internal: build configuration and CI |
| REQ-ENG-006 | Documentation | All | Internal: build configuration and CI |
| REQ-TEST-001 | Specification-derived tests | All | Internal: verified by the test suite named in each requirement |
| REQ-TEST-002 | Test doubles | M1 | Internal: verified by the test suite named in each requirement |
| REQ-TEST-003 | State machines | M1 | Internal: verified by the test suite named in each requirement |
| REQ-TEST-004 | Decision parsing | M1 | Internal: verified by the test suite named in each requirement |
| REQ-TEST-005 | Authorization and paths | M1 | Internal: verified by the test suite named in each requirement |
| REQ-TEST-006 | Approvals | M1 | Internal: verified by the test suite named in each requirement |
| REQ-TEST-007 | Crash recovery | M1, M3 | Internal: verified by the test suite named in each requirement |
| REQ-TEST-008 | Sandbox | M2 | Internal: verified by the test suite named in each requirement |
| REQ-TEST-009 | Workspace | M3 | Internal: verified by the test suite named in each requirement |
| REQ-TEST-010 | Context | M4 | Internal: verified by the test suite named in each requirement |
| REQ-TEST-011 | Budgets and convergence | M1 | Internal: verified by the test suite named in each requirement |
| REQ-TEST-012 | MCP client | M5 | Internal: verified by the test suite named in each requirement |
| REQ-TEST-013 | Confinement | M2, M6a, M6b, M6c | Internal: verified by the test suite named in each requirement |
| REQ-TEST-014 | CI gate | M0 | Internal: verified by the test suite named in each requirement |
| REQ-TEST-015 | Privacy | M1 | Internal: verified by the test suite named in each requirement |
| REQ-TEST-016 | Determinism and replay | M1 | Internal: verified by the test suite named in each requirement |
| REQ-TEST-017 | Security regression corpus | M1 | Internal: verified by the test suite named in each requirement |
| REQ-TEST-018 | Formal proofs, kept narrow | All (0.10) | §13.5 (the permission-lock decision); `repository-guards.md` (RFC 016) |
| REQ-MAIN-001 | Contract versioning | M1 | §22 Versioning |
| REQ-MAIN-002 | Independent schema versions | M1 | §22 Versioning |
| REQ-MAIN-003 | Adding a built-in tool | All | §22 Versioning |
| REQ-MAIN-004 | Adding a confinement backend | All | §22 Versioning |
| REQ-MAIN-005 | Documents in step | All | §22 Versioning |
| REQ-MAIN-006 | Readers tolerate newer records | M8 (0.13) | §22 Versioning |
