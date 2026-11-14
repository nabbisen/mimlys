# mimlys — Requirements Specification

**Status:** Baseline 1.0 (2026-10-02). Supersedes the draft requirements, the draft architecture overview and the draft external design of the startup phase (owner's archive).
**Normative.** Precedence: owner constraints and project rules, then this document, then the [external design](./external-design.md), then the [architecture overview](./architecture.md) (informative), then the startup study (informative; its code samples are non-normative).

## 0. How to read this document

Every requirement has an identifier, a milestone in which it is verified, and a verification method:

| Code | Verification |
|---|---|
| T | Automated test in CI |
| I | Inspection of code, configuration or documents by the reviewer |
| A | Analysis: a threat-model review or a measurement report |
| D | Demonstration against a live backend, tool server or platform |

Milestones M0 to M6c are defined in `ROADMAP.md`; `All` marks a requirement that holds from M1 on. Numbers are never written here; they are cited as `LIM-` identifiers of [Limits and Defaults](./limits-and-defaults.md). Threats are cited as `THR-` identifiers of the [Threat Register](./threat-register.md). Terms are those of the [Glossary](./glossary.md). Decisions `D-nn` and findings (`C-`, `M-`, `R-`, `A-`, `X-`, `T-`, `K-`) refer to the startup review (owner's archive, `reviewed/startup/`).

Normative words: **shall** (binding), **should** (binding unless a recorded reason exists), **may** (permitted).

Every identifier of the draft requirements specification is accounted for in [Appendix A](#appendix-a--disposition-of-the-draft-requirements).

---

## 1. Purpose and scope

mimlys is a local AI-agent execution and safety runtime. It accepts a goal from a user, lets a model propose one decision at a time, validates each decision against a fixed schema, the task's grants and its budgets, executes the resulting action inside explicit boundaries, records what happened, and lets the user see and control every step. It runs on the user's machine; the model is a replaceable backend; tools are built in, served over MCP, or, later, declared host tools.

In scope: task lifecycle and budgets; the decision schema and validation; capability grants and the policy ceiling; approvals and questions; built-in tools; the WASM sandbox; the execution worker and its confinement; workspaces, snapshots, rollback and promotion; context assembly; persistence and recovery; events, metrics and audit; the model-backend and MCP-client adapters; the CLI and configuration.

Out of scope, as non-goals: implementing or training a model; a search engine or browser (rypsok is a separate project and an external tool); a GUI, TUI or web UI; a general operating-system sandbox beyond the use of existing mechanisms; an opinionated agent framework; multi-user or remote operation (deferred, §25); a built-in network adapter (D-13); model-based summarization inside the core (D-12).

---

## 2. Principles

| # | Principle |
|---|---|
| P1 | **The runtime is the authority.** Model output is a proposal. Grants, budgets, policy and approvals change only through the user's or administrator's channel |
| P2 | **Default deny.** No grant, no operation; no network unless a tool is granted it; nothing touches the user's tree except `promote` |
| P3 | **Freeze semantics, replace technology.** States, schemas, effects, validation order and recovery semantics are fixed by this baseline; backends, sandbox engine, storage and transports are adapters |
| P4 | **Fail closed.** An error in validation, policy, confinement or persistence means no execution, never execution with less protection |
| P5 | **Bounded everything.** Every step, execution, request, output and context has a limit from the limits register |
| P6 | **Recoverable by construction.** Every transition is durable; every write action has a snapshot before it; an unknown outcome is a state, not a guess |
| P7 | **Data is never an instruction.** Observations, tool definitions and user answers carry provenance into the prompt and never merge with instructions |
| P8 | **One name, one meaning** (glossary); numbers in one place (limits register); threats in one place (threat register) |
| P9 | **Users are never confused.** Interface clarity rules §2.1 apply to the model-facing schema, the control API, the CLI and the configuration |
| P10 | **Honest scope.** What a platform or milestone cannot make safe is declared unavailable, with the reason, not weakened |

### 2.1 Interface clarity rules

| # | Rule |
|---|---|
| CLR-1 | A tool or command does what was asked, or it refuses and says why. It never does something else silently |
| CLR-2 | No silent fallback: an invalid value is an error that lists the valid ones; model output is never repaired |
| CLR-3 | No silent reduction: a value above a ceiling is an error that states the ceiling |
| CLR-4 | Every decision, action, task and request ends in exactly one outcome from a small, fixed vocabulary |
| CLR-5 | Every rejection and error says what the caller may change |
| CLR-6 | One name for each thing, one meaning for each name; one casing rule: snake_case fields, lowercase enum values |
| CLR-7 | A name says what is counted: `max_chars`, `deadline_ms`, `memory_mib` |
| CLR-8 | Few tools, few parameters, flat schemas, enumerations instead of free text |
| CLR-9 | Defaults are safe, come from the limits register, and are the same everywhere |
| CLR-10 | External text is marked in the structure, never inside a string, and never written by the runtime into an instruction |
| CLR-11 | What the runtime changes in external text, it reports through a notice |
| CLR-12 | Clarity is measured: rejections are counted per code, and a rising count is a defect of the interface |

---

## 3. Actors and trust boundaries

| Actor | Trust |
|---|---|
| User | Authority for goals, approvals, answers and grants within the ceiling |
| Administrator | Authority for the ceiling, backends, tool servers and host tools, through configuration |
| Client | Trusted to relay the user's requests; never a policy authority |
| Model and backend | Untrusted output; the backend's locality is a policy attribute |
| Built-in tools | Trusted code operating only through capability handles |
| External tool servers (MCP) | Untrusted content and definitions; effects declared by the administrator |
| Host tools (M6) | Untrusted executions confined by the operating system |
| Sandbox and worker | Trusted enforcement components, treated as one layer among several |
| Persistence store and snapshots | Trusted after integrity checks |
| Operating system | Boundary outside mimlys |

Boundaries, cited as `B1` to `B9`:

| ID | Boundary | Crossed by |
|---|---|---|
| B1 | Client → runtime | Control-API requests: goals, approvals, answers, grants |
| B2 | Runtime → backend | Prompts; back: model text |
| B3 | Runtime → built-in tools | Authorized actions; back: results |
| B4 | Runtime → worker | Execution requests; back: execution records |
| B5 | Worker → operating system | Sandbox and confined processes |
| B6 | Runtime → MCP servers | Tool calls; back: results and definitions |
| B7 | Runtime → persistence | Transitions, events, blobs, snapshots |
| B8 | Runtime → project scope | Materialization in; `promote` out |
| B9 | Administrator → runtime | Configuration and secrets by reference |

---

## 4. Architecture

### REQ-ARCH-001 Layer separation — M1 · I
The workspace shall consist of the crates `mimlys-core` (domain model, state machines, decision schema, validation, grants and policy, effects, context assembly, recovery logic), `mimlys-persist` (SQLite store and snapshot store), `mimlys-exec` (built-in tools over capability handles, worker protocol, sandbox, confinement backends, host tools), `mimlys-model` (backend adapters), `mimlys-mcp` (MCP client adapter) and `mimlys` (CLI and runtime host). Dependencies point toward the core; the core depends on none of the others. (AO §26 of the draft, D-14)

### REQ-ARCH-002 Core without I/O or protocol dependencies — M0 · T
`mimlys-core` shall not depend on Wasmtime, SQLite, an HTTP client, an MCP SDK or a CLI framework. CI shall fail when such a dependency appears (REQ-TEST-014). Tokio is permitted (D-14).

### REQ-ARCH-003 Asynchronous runtime — All · I
Tokio is the executor. No abstraction over the executor shall exist for the sake of replaceability. (C-14, D-14)

### REQ-ARCH-004 Three API tiers — M1 · I
The runtime shall expose exactly three interfaces: the *control API* for clients (tasks, approvals, answers, grants, events, status), the *adapter SPI* (`ModelBackend`, `ToolProvider`, `Executor`, `Persistence`, `WorkspaceProvider` (the baseline's `SnapshotStore`, widened by RFC 006), `ApprovalChannel`), and the *loop API* for embedding. The control API shall contain no operation that submits a decision, validates an action or executes anything directly. (D-01, C-10)

### REQ-ARCH-005 Runtime authority by construction — All · T
The executor interface shall accept only authorized-action values that the validation pipeline alone can construct, so that no code path, including built-in tools, promotion and recovery, can execute without passing validation. (INV-01)

### REQ-ARCH-006 Deterministic control logic — M1 · T
Given the same persisted inputs, the same backend outputs (fake backend) and the same tool results, with an injected clock and identifier source, the runtime shall produce the same sequence of transitions and events. (REQ-TEST-016)

### REQ-ARCH-007 Pinned engines — M2, M5 · I
Wasmtime and the MCP SDK shall be pinned to exact versions and updated only through a reviewed change that names the advisories fixed and the features gained. (M-14)

### REQ-ARCH-008 Worker process — M2 · T
Every sandbox execution and every host-tool execution shall happen in the worker process, never in the runtime process. The worker's death shall be an execution failure, never a runtime failure. (D-10)

### REQ-ARCH-009 Two deployment modes — M9 · T
The runtime shall run embedded in the client or served by a daemon of the user; every rejection code, event, state, `--json` field and message means the same in both, and `status` says which mode is in force. (RFC 014)

---

## 5. Task lifecycle

### REQ-TASK-001 Creation — M1 · T
`create_task` shall accept a goal of 1 to LIM-D4 characters, a project scope (an existing directory outside the runtime data directory, with optional include patterns), a profile (default from configuration), optional grants within the ceiling, optional budgets within the ceiling, and an optional completion check. It shall create the task in state `created` with a UUID (LIM-U5), record the policy hash, and materialize the workspace (REQ-WS-002). (D-09)

### REQ-TASK-002 States and transitions — M1 · T
A task shall be in exactly one of `created`, `running`, `waiting_user`, `paused`, `halted`, `recovering`, `completed`, `failed`, `cancelled`. Only the transitions of external design §5 shall be possible; any other request shall be refused with the error kind `state`. (D-02, C-01)

### REQ-TASK-003 Terminal immutability — M1 · T
`completed`, `failed` and `cancelled` are terminal. No field of a terminal task, its steps, actions or observations shall change afterwards, except retention-driven deletion of blobs. (C-02)

### REQ-TASK-004 Fork — M3 · T
`create_task` with `from_task` and an optional `from_snapshot` shall create a new task whose workspace is materialized from that snapshot and whose history is attached read-only as context. The source task's active grants are copied and shown to the user at creation; nothing is broadened. A halted source is not terminal and still holds its project scope (REQ-TASK-012): its fork is refused on an overlapping scope, and the refusal names both ways on, another `--scope` or `task cancel` of the source first. (C-02, NFR-014 of the draft)

### REQ-TASK-005 Pause and resume — M1 · T
`pause_task` from `running` shall take effect at the next step boundary: a model request in flight is cancelled, an execution in flight finishes within its budget. `resume_task` shall be valid from `created`, `paused` and `halted`; from `halted` with reason `budget_steps` or `budget_deadline` it may extend those two budgets within the ceiling and nothing else. Resumption shall add a runtime notice to the next context. (A-10)

### REQ-TASK-006 Cancel — M1 · T
`cancel_task` shall be valid from every non-terminal state, shall abort a model request in flight, terminate an execution in flight and its process tree within LIM-O2, apply the workspace rule of REQ-WS-010, persist, and end in `cancelled`. An interrupted action that cannot be undone (as in REQ-CLI-005) is recorded with its outcome unknown, and the command's result names it. Cancellation shall never be an advisory flag. (ED §26 of the draft)

### REQ-TASK-007 Halt reasons — M1 · T
A halt shall carry one reason from: `budget_steps`, `budget_rejections`, `budget_failures`, `budget_deadline`, `convergence`, `approval_rejected`, `approval_expired`, `question_expired`, `recovered`, `unknown_outcome`, `workspace_quota`, `backend_unavailable`, `tool_unavailable`, `policy`. The reason is persisted and shown to the user. (A-10)

### REQ-TASK-008 Budgets — M1 · T
Each task shall carry the budgets LIM-T1 to LIM-T6 as durable counters and limits. Counters survive restarts; rolled-back executions count; only REQ-TASK-005 may raise a limit. (M-20)

### REQ-TASK-009 Deadline — M1 · T
The deadline is an absolute instant (LIM-U4) set from LIM-T5 at creation. It shall be checked at every step boundary, and every model request and execution shall receive a timeout no longer than the time remaining. Downtime does not stop the clock. (M-20)

### REQ-TASK-010 Completion — M1 · T
On a `complete` decision: if the task has a completion check, the runtime shall run it as an action under the same validation and budgets; success ends the task `completed` with `completion = verified`; failure becomes an observation and counts toward LIM-T6. Without a check the task ends `completed` with `completion = asserted`. The result shall show which. (D-19, M-02)

### REQ-TASK-011 Give up — M1 · T
A `give_up` decision shall end the task `failed` with `failure_reason = model_gave_up` and the model's bounded reason (LIM-D2). (D-02)

### REQ-TASK-012 One active task per project scope — M1 · T
A task whose project scope overlaps a non-terminal task's scope shall be refused with the error kind `conflict`. (A-12)

### REQ-TASK-013 Effort levels — M8 · T
A task shall be created at an effort level (`quick`, `standard`, `deep`; default `standard`), each a configured envelope of budgets and context handling within the ceiling and naming its backend; the level and who chose it are recorded and shown, and never change during the task. A level grants nothing. (RFC 015)

### REQ-TASK-015 Plan first — M8 · T
A task created with `--plan-first` shall reject any first decision but a `plan` with `state{reason: plan_required}`, show the plan to the user and act on nothing until the user resumes, with an optional note as a labelled `user` segment; the goal is never rewritten. (RFC 015)

---

## 6. Control loop and decisions

### REQ-DEC-001 Step structure — M1 · T
A step shall pass through the phases assemble → decide → validate → (wait) → execute → observe → evaluate, each phase change persisted (REQ-PERS-004). A step holds exactly one decision and at most one action. Steps of one task are sequential. (D-11)

### REQ-DEC-002 Decision schema — M1 · T
The model shall return one JSON object with `decision` from `act`, `plan`, `ask_user`, `complete`, `give_up` and the fields the external design §6 defines for each. Unknown fields shall be refused. Strings are bounded by LIM-D2, LIM-D3 and LIM-D4; the object by LIM-D1. The schema is versioned (REQ-MAIN-002). (M-01)

### REQ-DEC-003 Strict parsing, no repair — M1 · T
Exactly one JSON object shall be accepted; leading or trailing text, duplicate keys, or a size above LIM-D1 shall be rejected with `malformed`. The runtime shall never alter the model's text to make it parse. A format rejection consumes the rejection counter. In the `tool_calls` transport (REQ-MDL-009) the decision is the reply's structured `tool_calls`: exactly one entry naming an offered function, its `arguments` exactly one JSON object within LIM-D1; zero calls, several, an unknown name or unparseable arguments are `malformed`; a call written into the reply's text is never read, and text beside a structured call is neither parsed nor placed in the context. The call is mapped onto the decision envelope before the first validation stage. (CLR-2, A-09; amended by RFC 012 for 0.9)

### REQ-DEC-004 Validation order — M1 · T
Validation shall follow the order of external design §6.3: state, size and parse, schema, tool visibility, argument schema, canonicalization, authorization, semantic bounds, effect and approval derivation, budget reservation, approval, execution. No stage shall compensate for a skipped stage. (C-04)

### REQ-DEC-005 Tool visibility — M1 · T
The model may name only tools visible to the task through grants. Any other name shall be rejected with `unknown_tool`. The definitions shown to the model shall be generated from the visible set (REQ-CTX-005). (K-04)

### REQ-DEC-006 Argument validation — M1 · T
Arguments shall be validated against the tool's input schema (unknown properties refused, types, enumerations, bounds), then canonicalized: paths normalized lexically relative to the workspace root, Unicode NFC, case handled per REQ-WS-011.

### REQ-DEC-007 Authorization before existence — M1 · T
Path arguments shall be authorized against the workspace root and the task's grants by lexical canonicalization alone, before any filesystem access. An out-of-scope path shall yield `denied` with identical content whether or not the path exists. Existence is established at execution through the capability handle. (R-02, THR-03)

### REQ-DEC-008 Rejection vocabulary — M1 · T
Rejection codes shall be exactly: `malformed`, `schema`, `unknown_tool`, `invalid_argument`, `denied`, `state`, `budget`, `approval_rejected`. Each carries a hint of at most LIM-I5 characters from a fixed table and a `detail` object with fixed keys. No rejection shall contain filesystem contents, policy internals or data of another task. (M-11, CLR-5)

### REQ-DEC-009 Feedback — M1 · T
A rejection, a failed observation or a user's note shall enter the next step's context labelled with provenance `runtime` or `user`, never as an instruction. (REQ-CTX-002)

### REQ-DEC-010 Counters and convergence — M1 · T
The runtime shall maintain `steps`, `consecutive_rejections`, `consecutive_failures` and `identical_decisions`. A decision is identical when its hash (REQ-DEC-012) equals the previous step's and the workspace snapshot has not changed. Reaching LIM-T1, LIM-T2, LIM-T6 or LIM-T4 shall halt the task with the matching reason. (M-10, A-07)

### REQ-DEC-011 Plan — M1 · T
A `plan` decision shall replace the task's plan with a new immutable version of at most LIM-D3 items. The plan is shown to the user and to the model and has no execution semantics in v1. (D-11)

### REQ-DEC-012 Canonical hashes — M1 · T
Decisions, actions and grant sets shall be hashed per LIM-U6. The hashes are recorded on steps, actions and approvals.

---

## 7. Capabilities and policy

### REQ-CAP-001 No ambient authority — All · T
No operation shall happen without a grant or the task's implicit workspace rights (read and write inside its own workspace). Model text shall never create, widen or restore a grant. (INV-02, THR-09)

### REQ-CAP-002 Grant kinds — M1 · T
Grant kinds shall be: `tool{name}` (visibility of a tool), `host_read{root, patterns}`, `host_write{root, patterns}`, `network{domains, ports}` (M5), `host_tool{name}` (M6; it alone makes `run_host_tool` visible for that tool). `host_read` and `host_write` are reserved until a tool carries their effect (DR-016, DR-017): the interface refuses to issue them, with the reason, rather than issue a grant that changes nothing (amended 2026-10-31).

### REQ-CAP-003 Ceiling — M1 · T
The administrator's policy shall bound, per profile, the grant kinds, roots, domains, tools and budgets a task may have. A grant beyond the ceiling shall be refused with `denied`, whoever requests it. (C-06, D-05)

### REQ-CAP-004 Issuance — M1 · T
Grants are issued at creation (profile defaults plus the user's additions) or later by the user through `grant_capability`. Every issuance is an audit event naming the issuer.

### REQ-CAP-005 Revocation — M1 · T
`revoke_capability` shall prevent any new action under the grant at once, invalidate pending approvals that used it, and either let a running action finish within its budget or cancel it when the request says so.

### REQ-CAP-006 No broadening — All · T
A grant shall never become broader during its life; a change is a new grant. (ED §45 of the draft)

### REQ-CAP-007 Grant-set hash — M1 · T
The active grant set shall have a canonical hash (LIM-U6) recorded on every action and approval.

### REQ-CAP-008 Capability text generated — M1 · T
The description of the task's capabilities shown to the model shall be generated from the grant set, never hand-written. (M-19)

### REQ-CAP-009 Profiles — M1 · T
The shipped policy shall define at least `read_only` (read, list, search), `workspace` (default: workspace writes, `run_script`, `run_wasm`; no host, no network) and `project` (`workspace` plus promotion with confirmation). Administrators may define further profiles within the ceiling.

### REQ-CAP-010 Policy versioning — M1 · T
The policy carries `config_version` and a content hash recorded on every task at creation. A changed policy applies to new tasks; a reduced ceiling applies to running tasks at their next step boundary by revoking the excess, audited. (ED §37 of the draft)

---

## 8. Approvals and questions

### REQ-APR-001 When approval is required — M1 · T
Approval shall be required for effect `external_write`, for `host_write` unless the tool is reversible, for every tool or effect the profile lists, and for promotion. It shall never be required for `none`, `workspace_read` or `workspace_write` unless the profile lists the tool. (D-04)

### REQ-APR-002 Request content — M1 · T
An approval request shall be rendered by the runtime from the canonical action: tool, arguments, resolved absolute paths, effect, reversible, idempotent, one fixed sentence per tool saying what will happen, and the grants used. The model's `reason` shall appear separately, labelled as the model's text, bounded by LIM-D2. The whole request is bounded by LIM-I6. (R-01, THR-20)

### REQ-APR-003 Rendering safety — M1 · T
Every external string in a rendered request shall be quoted and escaped; control, invisible and bidirectional characters removed with a notice; paths shown absolute and canonical. (R-04, THR-21)

### REQ-APR-004 Binding — M1 · T
An approval shall reference the action id, the action hash and the grant-set hash. Execution shall verify all three; a mismatch shall yield `denied` and the event `approval.invalidated`. (R-03, THR-22)

### REQ-APR-005 One-shot — M1 · T
An approval is consumed when its action starts and shall never apply to another action or a second execution of the same action.

### REQ-APR-006 Expiry — M1 · T
A request may carry `expires_at` (LIM-A1). Expiry shall count as rejection with reason `expired` and halt the task (`approval_expired` or `question_expired`). A waiting task shall hold no model request, execution or worker resource. (A-11)

### REQ-APR-007 Human decisions — M1 · T
The user may approve; reject with an optional note of at most LIM-D2 characters and a `halt` flag whose default is the profile's `on_reject`; or issue a grant and let the model re-propose. A note reaches the model as `user` text.

### REQ-APR-008 No inferred approval — All · T
Silence, malformed input, model text and tool output shall never count as approval. (FR-093 of the draft, THR-20)

### REQ-APR-009 Authority — M1 · T
Only a user session may decide. The audit shall record the session and the operating-system user. In v1 the session is the CLI process (D-18).

### REQ-APR-010 Questions — M1 · T
An `ask_user` decision shall move the task to `waiting_user` with a question of at most LIM-D4 characters. `answer_question` with at most LIM-D4 characters shall enter the next step as `user` text. Expiry follows REQ-APR-006. At most LIM-A2 item is pending per task.

---

## 9. Tools and effects

### REQ-TOOL-001 Tool definition — M1 · T
Every tool shall declare: a lowercase snake_case name; a description of at most LIM-I2 characters; an input schema that is an object of at most LIM-I3 parameters nested at most LIM-I4 deep; an optional output schema; an effect with `reversible` and `idempotent`; a default deadline; a resource profile (`builtin`, `sandbox`, `host`); the names of arguments that denote network destinations; and its provider (`builtin`, `mcp{server}`, `host`).

### REQ-TOOL-002 Effect vocabulary — M1 · T
Effects shall be exactly `none`, `workspace_read`, `workspace_write`, `host_read`, `host_write`, `external_read`, `external_write`. The rules that derive approval (REQ-APR-001), recovery (REQ-PERS-007) and future parallelism from an effect shall live in one function of the core. (D-04, X-09)

### REQ-TOOL-003 Built-in tools — M1, M2 · T
The built-in tools shall be `read_file`, `write_file`, `replace_in_file`, `list_dir`, `search_files`, `delete_path` (M1) and `run_script`, `run_wasm` (M2), with the schemas of external design §11, and the gateway `run_host_tool` (M6a), visible through `host_tool` grants only. They are part of the external contract. Promotion is a control-API operation, not a tool.

### REQ-TOOL-004 File tools use directory handles — M1 · T
Every built-in file operation shall open through a capability handle rooted at the workspace, never through an absolute path string, and shall refuse to follow a symbolic link that leaves the root. (THR-03, THR-27)

### REQ-TOOL-005 Result validation — M1 · T
When a tool declares an output schema, its result shall be validated against it; a violation shall be the failure cause `output_invalid`, never passed through.

### REQ-TOOL-006 Deadlines — M1 · T
Every execution shall have a deadline equal to the minimum of the tool default (LIM-O4, LIM-O5, LIM-H1), the task's remaining time and the profile's cap. (A-14)

### REQ-TOOL-007 Cancellation — M1 · T
Built-in tools shall check cancellation between I/O operations; worker and MCP executions shall be terminated or cancelled within LIM-O2.

### REQ-TOOL-008 Output bounds — M1 · T
Every result, stdout and stderr shall be bounded (LIM-S4, LIM-H4, LIM-I7, LIM-C2) and every cut reported through a notice. (CLR-11)

### REQ-TOOL-009 Runtime retry — M1 · T
The runtime shall re-execute an action without consulting the model only when the failure cause is `tool_unavailable` or `timeout`, the tool is idempotent, executions are below LIM-T3, and, for a write effect, the previous execution was rolled back first.

### REQ-TOOL-010 Transaction equals action — M3 · T
A failed `workspace_write` execution shall be rolled back to its pre-execution snapshot before its observation is produced, and the observation shall state `rolled_back = true`. The rollback covers what snapshots cover: paths excluded by LIM-W4 and LIM-W5 are outside it, and the observation's documentation says so. Successful write actions persist. (FR-098 of the draft, THR-07)

---

## 10. Sandbox

### REQ-SBX-001 Wasmtime in the worker — M2 · T
Generated code shall run only inside Wasmtime in the worker: `run_script` through pinned WASM-hosted interpreters (JavaScript via QuickJS at least; Python when a maintained wasip1 build is pinned) and `run_wasm` for modules from the workspace of at most LIM-S5 bytes. (D-16)

### REQ-SBX-002 Triple guard — M2 · T
Fuel (LIM-S1), the epoch deadline (LIM-S2, tick LIM-S7) and the memory limiter (LIM-S3) shall each terminate an execution on its own. (REQ-TEST-008)

### REQ-SBX-003 Termination reporting — M2 · T
Every execution shall end with one `termination` from `exited`, `trapped`, `fuel_exhausted`, `deadline`, `memory_limit`, `output_limit`, `process_limit`, `cancelled`, `host_fault`, with `exit_code` present only for `exited`. Fuel exhaustion and deadline shall never be conflated, and a tree ended by a watchdog is never reported as a tool that exited by itself. (`process_limit` added 2026-11-07.) (C-12, X-07)

### REQ-SBX-004 WASI configuration — M2 · T
The sandbox shall receive exactly: a preopened `/workspace` (read-write or read-only per the tool's mode), a preopened ephemeral `/tmp` discarded after the execution, declared `args` and `stdin`, an empty environment, stdout and stderr to bounded buffers (LIM-S4), clocks of granularity LIM-S6, and no socket or other host function. (THR-11, THR-19)

### REQ-SBX-005 Output capture — M2 · T
When a stream exceeds its bound the execution ends with `output_limit`; the observation keeps head and tail per LIM-C2, the bounded stream is stored as a blob, and the notice `output_truncated` carries the total size. Invalid UTF-8 is replaced with a notice.

### REQ-SBX-006 Module validation — M2 · T
Modules shall be validated before instantiation; a malformed module ends the execution with `host_fault` and detail `invalid_module`, never with a process failure.

### REQ-SBX-007 Instance isolation — M2 · T
Each execution shall use a fresh store. Only read-only compiled interpreter modules, cached by content hash under the data directory, are shared.

### REQ-SBX-008 Replay mode — M2 · T
The sandbox shall support a fixed random seed and a fixed clock for replay (REQ-TEST-016).

### REQ-SBX-009 Overhead measurement — M2 · A
Instantiation and WASI setup time shall be measured per execution and reported; the M2 report fixes LIM-S8.

### REQ-SBX-010 No host compilers — M2 · I
The runtime shall not invoke a host compiler on generated code. Compiled-language adapters are deferred to the confined worker (DR-007). (R-06)

### REQ-SBX-011 Resource accounting — M2 · T
Fuel consumed, peak memory, wall time and output bytes shall be recorded per execution. Fuel is recorded where the engine counts it exactly (a normal end, an exit, fuel exhaustion, `unreachable`, a host call's refusal) and as unknown (`null`) otherwise, never as a figure the engine did not count (handoff 002d).

### REQ-SBX-012 Fail closed — M2 · T
If the worker or the sandbox configuration cannot be established as required, the execution shall fail with `host_fault` and detail `unavailable`; nothing runs with fewer guards. (SEC-012 of the draft)

---

## 11. Worker and confinement

### REQ-CONF-001 Confinement contract — M2 · I
The contract shall consist of the clauses F (explicit read and write sets), N (no network, loopback included), E (explicit environment), K (whole-tree termination), P (no privilege gain), M (memory ceiling) and C (process-count and time ceilings), as external design §13. F, N, E, K and P are mandatory; M and C may be best-effort where the operating system has no kernel limit, and the status shall say so. (D-06 v2)

### REQ-CONF-002 Worker outer layer — M2 · T
The worker shall run under the platform's confinement backend where available: read set limited to the data directory, temp and read-only toolchain and system paths; no network; explicit environment; tree termination. The sandbox's WASI boundary remains the primary boundary.

### REQ-CONF-003 Probe and tier — M2 · T
At startup the runtime shall probe which clauses the platform backend can enforce, persist the result, and expose it with the tier (`core` or `full`) through `get_runtime_status`. (D-07 v2)

### REQ-CONF-004 Fail closed for host tools — M6a · T
`run_host_tool` shall be invisible and unavailable unless every mandatory clause is enforced on this machine, or is best-effort and named by the administrator in `policy.accept_best_effort`, which the status and every host-tool approval then show (RFC 011 §5). (P10)

### REQ-CONF-005 Backends — M6a, M6b, M6c · T
Linux: Landlock and seccomp (the filter also denies `io_uring` and cross-process memory access, REQ-CONF-012), a delegated cgroup where available (else the worker as child subreaper and per-tool rlimits), each clause reporting its primitive. Windows: AppContainer without network capability, a Job Object with kill-on-close and breakaway forbidden, a restricted token. macOS: a Seatbelt profile, rlimits and a resident-size watchdog. Each backend shall pass the clause tests of REQ-TEST-013 on its platform in CI.

### REQ-CONF-006 Declared host tools — M6a · T
Host tools exist only as administrator declarations: name, command, argument template, effect, deadline, additional read paths, environment allow-list. `run_host_tool{tool, args}` validates `args` against the template; no shell is involved. The effect is `workspace_write`, or `external_read` exactly for the dependency step (`network = true`, REQ-CONF-007); a declaration that pairs them otherwise is refused at startup, naming the valid value (CLR-1, CLR-2; amended 2026-11-03, 0.7.1). The effect of a tool without network is `workspace_write`: a host tool writes only inside the workspace, because the worker's write set holds nothing else; `host_write` host tools need declared write paths (DR-017).

### REQ-CONF-007 Offline rule — M6a · T
Host tools run with network denied. Dependency fetching is a separately declared host tool with effect `external_read`, approval per profile, outside clause N and inside F, E, K and P; the documentation states this plainly. (assessment §6)

### REQ-CONF-008 Toolchain visibility — M6a · T
Read access shall be granted only to declared toolchain paths. Grants on user-profile locations are explicit, reversible and listed by `get_runtime_status`. Before an installation is removed, one confirmed command shall remove every permission entry mimlys added to the machine and the AppContainer profile it created (`mimlys toolchain revoke --all`), including entries whose record was lost, which it finds on mimlys's own directories and the declared read paths; it reports each entry removed and each it could not remove. An entry on a file (the worker binary, RFC 017) is recorded, listed, revoked and found like an entry on a directory. (THR-36; amended 2026-11-07, 0.8, and 2026-11-12)

### REQ-CONF-009 Tree termination — M2 · T
All processes of an execution shall be terminated on cancel, deadline or runtime exit through the platform's tree primitive. Orphans of a previous runtime shall be terminated at startup where identifiable. (THR-32)

### REQ-CONF-010 Unsafe code boundary — M2 · I
Foreign-function code for backends shall live only in the confinement modules of `mimlys-exec`, each `unsafe` block justified in a comment and reviewed. Every other crate forbids `unsafe`. (REQ-ENG-001)

### REQ-CONF-011 Read sets hold no credentials — M6a, M6b · T
A declared read path (`read_paths`, `toolchain grant`) shall be refused, with the entry named and the narrower path suggested, when it is a filesystem root, the user's home directory or an ancestor of it, or when it directly contains an entry named by LIM-H5. The worker binary and each configured interpreter module shall be admitted as files, never by their directories, and a `command` is judged against the declarations, never against where mimlys is installed (RFC 017). A host tool's Linux read set shall contain nothing of `/proc`: the command lines, status and network tables of the user's other processes are not for project code. The worker alone keeps `/proc` (to find the descendants of a cancelled tool, clause K on the `subreaper` rung), and each host tool's process is restricted to the worker's sets without it by a second Landlock layer. A clause test shall show a host tool failing to read a credentials file that lies beside its declared toolchain directory, and failing to read another process's `/proc/<pid>/cmdline`. (THR-23, THR-24; amended 2026-11-03, 0.7.1, and 2026-11-12, RFC 017)

### REQ-CONF-012 Process memory and descriptors are not reachable — M6a · T
The Linux seccomp filter shall always deny `io_uring_setup`, `io_uring_enter`, `io_uring_register`, `process_vm_readv` and `process_vm_writev`, in every worker including the dependency step's. Where Landlock ABI 4 or later exists and network is denied, TCP `bind` and `connect` shall be handled with an empty allow-set as a second layer. The runtime and the worker shall mark themselves non-dumpable at start, so that a same-user process that is not itself inside a Landlock domain cannot read their memory, environment or descriptors through `/proc` or `process_vm_readv` (Landlock already refuses this to a confined process that is outside the target's domain; the flag covers the rest). A clause test per denied route shall show the attempt failing from inside a host tool. (THR-11, THR-23, THR-24; added 2026-11-03, 0.7.1)

### REQ-CONF-013 CPU part of clause C — M7b · T
Clause C shall gain a CPU part, best effort allowed on every platform (it never lowers the tier): `cpu.max` of the `tool` cgroup where one is delegated (else `unavailable` with the reason), the Job Object's CPU rate on Windows, a lowered priority on macOS reported `best_effort (priority)`; the share is LIM-H7, a budget a task may raise within the ceiling. (RFC 013)

---

## 12. Workspace

### REQ-WS-001 Location and permissions — M1 · T
Workspaces shall live under `<data_dir>/workspaces/<task_id>/`, the data directory created with mode 0700 or an owner-only DACL. The default data directory is never under a temporary directory; a configured one inside it is named by `check` and `status` with the risk (a cleaner of temporary files can delete tasks). (M-17, THR-31; amended 2026-11-07)

### REQ-WS-002 Materialization — M1 · T
A workspace shall be a copy of the project scope (reflink where available; never a hard link into a writable tree). Symbolic links whose target stays inside the scope are recreated; others are omitted with a notice. A scope above LIM-W1 or LIM-W2 shall be refused at creation with the measured size. (D-09, THR-27)

### REQ-WS-003 Exclusions — M1 · T
LIM-W4 paths and LIM-W5 patterns shall be excluded from materialization, from reading and from promotion unless the profile grants them explicitly. (D-22, THR-16)

### REQ-WS-004 Snapshots — M3 · T
A snapshot (content-addressed manifest and objects, LIM-U6) shall be taken at materialization, before every action with a write effect and after every successful one. Objects and manifests shall be fsynced before any record references them. (D-08, THR-29)

### REQ-WS-005 Symlink safety — M3 · T
Snapshot and restore shall never follow symbolic links; links are stored as entries; restore shall refuse any target that is absolute or escapes the root. (R-09, THR-27)

### REQ-WS-006 Rollback — M3 · T
`rollback_workspace{snapshot}` shall be valid when the task is not `running` or `recovering`. Restoration shall delete files absent from the manifest and verify every object by hash. Automatic rollback follows REQ-TOOL-010 and REQ-PERS-007.

### REQ-WS-007 Promote — M3 · T
`promote_workspace` shall compute the difference between the base snapshot and the current workspace, refuse when a target file differs from the base (listing the conflicts), back up the target files into a snapshot (making the operation reversible), show the diff, require the user's confirmation, apply, and record the file list in the audit. Promotion shall be valid when the task is not `running` or `recovering`. The model cannot propose it in v1. (D-09, R-10, THR-28)

### REQ-WS-008 Quotas — M1 · T
LIM-W1, LIM-W2 and LIM-W3 shall be enforced at write time by built-in tools and at step boundaries for executions; a breach halts the task with `workspace_quota`. The size limit is best-effort and documented as such. (R-18)

### REQ-WS-009 Lifecycle — M3 · T
Workspaces move `allocated → active → archived → deleted`: `archived` when the task is terminal (derived, nothing is removed), `deleted` only by the user's `task delete`, never while the task is non-terminal. Nothing expires on a timer. (RFC 006)

### REQ-WS-010 Cancellation and the workspace — M3 · T
Cancelling a running write-effect execution shall restore the pre-execution snapshot unless the profile sets `on_cancel = keep`; the task record states which. (R-13)

### REQ-WS-011 Case and path rules — M1 · T
Canonicalization shall respect the case sensitivity of the workspace's filesystem, refuse names invalid on the current platform, and handle Windows long paths. (assessment §5)

### REQ-WS-012 Explicit deletion — M3 · T
Everything on disk belongs to one task. `mimlys task delete` shall remove a terminal task's workspace, snapshots, the objects no remaining manifest references (reference counts kept in the same transactions as the snapshot rows), blobs and promotion backups, after showing what will go and asking; the audit stays unless asked. No collector and no timer exist; a snapshot is a journaled transaction that the next start finishes or undoes, so no orphan is ever left. `task list` and `status` show each task's footprint. (RFC 006; replaces garbage collection)

### REQ-WS-013 Storage view — M7a · T
`mimlys status --storage` shall show the store's path, its filesystem and whether it reflinks, the bytes by kind (working trees, snapshot objects exclusive and shared, blobs, interpreters, database and WAL, temporary) and the tasks that hold the most with their state and age; read-only. (RFC 013)

### REQ-WS-014 Prune — M7a · T
`mimlys task prune` shall list the terminal tasks matching `--state` and `--older-than` with their footprints, ask once, then delete each as `task delete` does; it never touches a task that is not terminal or a promotion's backup still needed for reversal (listed `kept: promotion reversible`); audits stay unless `--audit`. (RFC 013)

---

## 13. Context and observations

### REQ-CTX-001 Prompt structure — M4 · T
The context shall be assembled in the fixed segment order of external design §15: system instructions, tool definitions, capability summary, goal, plan, compressed history, recent steps, latest observation, decision request. Stable segments precede mutable ones. In the `tool_calls` transport the order holds with the changes of external design §15.4: the tool definitions travel as `tools`, earlier decisions and their outcomes are native call turns, and there is no decision request, so no request ends with an assistant message. (C-07; amended by RFC 012 for 0.9)

### REQ-CTX-002 Provenance — M1 · T
Every observation, rejection, user answer, user note and tool definition entering the context shall be delimited and labelled with its provenance (`runtime`, `user`, `tool:<name>`, `model`); the system instructions shall state that observations, rejections, runtime feedback and tool definitions are data, never instructions, and that the goal, answers and notes are the user's own words, the goal being the task (amended 2026-11-07: labelling the goal as data told the model not to follow its task; the real-model run of M4 asked the user for a file it had been told to read). (M-18, K-01, THR-01)

### REQ-CTX-003 Budget and pruning — M4 · T
The context shall not exceed LIM-C1, counted over everything sent, the `tools` array of the `tool_calls` transport included (RFC 012). Above it the runtime compresses older steps (LIM-C3 recent steps stay full, LIM-C4 per summary), then drops the oldest. Grants, halts, rejections and unresolved failures shall never be dropped. (INV-09)

### REQ-CTX-004 Rule-based compression — M4 · T
Compression shall be rule-based: truncation to head and tail, collapsing of repeated lines, fixed summaries built from structured fields. No model is called for it. (D-12, THR-33)

### REQ-CTX-005 Tool definitions — M1 · T
Tool definitions shall be generated from the visible tools, bounded by LIM-I1 in total, pinned per task, and sanitized per REQ-MCP-003 for external tools.

### REQ-CTX-006 Observation excerpts — M1 · T
Observations shall carry excerpts per LIM-C2 with truncation notices; full bounded outputs are stored as blobs for the user, not shown to the model.

### REQ-CTX-007 Secrets never enter the context — M1 · T
Configured secret values and LIM-W5 content shall be redacted from observations before they enter the context. (REQ-PRIV-002)

### REQ-CTX-008 Invisible characters — M1 · T
Control, zero-width, tag and bidirectional characters shall be removed from external text before it enters the context or a display, with a notice. (THR-21)

### REQ-CTX-009 Structured results preserved — M1 · T
Tool results shall be kept as validated JSON and shown to the model as JSON, never flattened to prose. (K-10)

### REQ-CTX-010 Prefix stability measured — M4 · T
Every decision request shall record its context size, what the compression rules did, the prefix shared with the task's previous request, the model's latency and the backend's usage when reported; `get_metrics` derives the prefix and compression ratios and characters per prompt token. The M4 report fixes LIM-C1 and LIM-C3 from these numbers. (Amended by RFC 007: measured by the runtime, not only when the backend reports it.) Both decision transports are measured (RFC 012).

---

## 14. Persistence and recovery

### REQ-PERS-001 Store — M1 · T
One SQLite database `<data_dir>/mimlys.db` in WAL mode with synchronous FULL (LIM-P1), owner-only permissions, a lock file, and a single writer; a second runtime on the same data directory shall refuse to start within LIM-O3. (D-15, THR-31)

### REQ-PERS-002 Schema version — M1 · T
The database shall carry a schema version; migrations are forward-only; a newer schema shall be refused with a message naming both versions.

### REQ-PERS-003 Durable transitions — M1 · T
Every task, step, action, approval, question and grant transition and every event shall be written in its own transaction and be durable when the call returns. (INV-05)

### REQ-PERS-004 Write ordering — M1 · T
For every action the runtime shall persist, in order: the proposal with its hash; the validation result; the approval request and decision when required; the authorization with the pre-execution snapshot id (M3); the running intent; the execution record and observation; the terminal status; the step outcome and counters. Execution shall not start before the running intent is durable. (AO §17.1 of the draft)

### REQ-PERS-005 Blobs — M1 · T
Outputs beyond LIM-C2 shall be stored as blobs referenced by id and bounded by LIM-S4 or LIM-H4.

### REQ-PERS-006 Integrity at startup — M1 · T
At every start (LIM-P5) the runtime shall run the database integrity check and verify the consistency rules CONS-01 to CONS-08 of the external design; on failure it shall refuse to run tasks and name the failing rule. (M-09)

### REQ-PERS-007 Recovery procedure — M1 (basic), M3 (with snapshots) · T
At startup every task in `running` shall enter `recovering`. An action in `running` becomes `unknown`. Its disposition follows its effect: reversible → the pre-execution snapshot is restored and the task halts with `recovered`; non-reversible → the task halts with `unknown_outcome` and lists the action. With `recovery.auto_resume = true` a cleanly rolled-back task returns to `running` instead of halting. Tasks in `waiting_user`, `paused`, `halted` and `created` keep their state; expired approvals are processed. (D-03, R-11)

### REQ-PERS-008 No automatic replay — All · T
An action with effect `external_write`, `host_write` (the runtime undoes no host write, even one the tool backed up), or a non-idempotent `external_read`, whose outcome is `unknown`, shall never be re-executed automatically. Such an action whose execution started and was ended by the runtime rather than by the tool (a deadline, a resource limit, a lost worker or connection, an interruption) shall be recorded `unknown`, never `failed`, and its task halted `unknown_outcome` with the action listed, as after a crash (external design §7.5; amended 2026-11-14, review of handoff 010e). (INV-07, THR-08)

### REQ-PERS-009 Snapshot references — M3 · T
The database shall reference only snapshots whose objects and manifest were fully written and fsynced; objects no remaining manifest references are removed with their task by REQ-WS-012; recovery shall verify a manifest before trusting a checkpoint. (R-12, THR-29)

### REQ-PERS-010 Retention — M3 · T
Nothing is deleted without the user's command (REQ-WS-012); the only recovery point of a non-terminal task can never be deleted; the audit stays unless `task delete --audit` asks for its removal. LIM-P2 … LIM-P4 carry no timer. (RFC 006)

### REQ-PERS-011 Backup — M3 · I
The data directory shall be self-contained: copying it while the runtime is stopped is a complete backup, and the documentation says so.

### REQ-PERS-012 Versions recorded — M1 · T
Every task shall record the policy hash, the decision-schema version, the tool-definition hashes and the runtime version at creation. (ED §83 of the draft)

### REQ-PERS-013 Read-only open — M7a · T
Commands that only read (`status`, `events` and `events --follow`, `task list`, `task show`, `metrics`, `toolchain list`, `promote --patch`; `check` opens no store) shall open the store as WAL readers that hold no lock while they read (a momentary try-lock, released at once, may tell whether a writer holds the store), and shall run no recovery, integrity check, reaping or migration; they record no event, `runtime.started` included, because a read command is not a runtime. A store that needs recovery is read as committed and `status` says `recovery pending`; while another process holds the lock, `status` names it. A read command that writes is a defect, refused by the type of its persistence handle. (RFC 013)

### REQ-PERS-014 One owner of a store under the daemon — M9 · T
A daemon shall hold the store's exclusive lock for its life; a daemon and an embedded runtime never own one store at once, and whichever comes second names the holder and exits. (RFC 014)

### REQ-PERS-015 The backend recorded per step — M8 · T
Each step shall record the backend's name, the model and the decision transport it was decided with, and the task the set of backends it used; the persistence schema version rises with a tested migration. (RFC 015)

---

## 15. Scheduling and concurrency

### REQ-SCHED-001 Concurrency limits — M1 · T
At most LIM-X1 tasks shall be `running` and at most LIM-X2 executions in flight; further tasks wait in `created` with a visible queued flag; nothing is dropped. (Not met through 0.9: a second task is refused with `conflict` instead of queued; the queued flag, with its `reason`, is delivered by RFC 013 part two, M7b. Found in handoff 013's review, 2026-11-11.)

### REQ-SCHED-002 Sequential steps — M1 · T
One step at a time per task; one execution at a time per task. (D-11)

### REQ-SCHED-003 Memory budget — M2 · A
The runtime's worst-case memory shall be derivable as LIM-X2 × LIM-S3 plus a measured base, and the M2 report states it.

### REQ-SCHED-004 Cancellation propagation — M1 · T
Cancellation of a task shall reach its model request, its execution and its worker tree (REQ-TASK-006).

### REQ-SCHED-005 Resource release — M1 · T
A terminal task shall hold no worker, backend, lock or scheduler resource.

### REQ-SCHED-006 Admission per user — M7b · T
At most LIM-X5 executions of one operating-system user shall run at once across all runtimes, counted in an owner-only registry under the user's runtime directory; a runtime without a free slot waits with its task queued (`reason: admission`), says so in `status` and records `task.admission_changed`; a stale entry is reaped at every open; a registry that cannot be used within LIM-O11 lets the runtime run unadmitted, shown in `status` and the task's events. (RFC 013)

### REQ-SCHED-007 Engine gate — M7b · T
At most `max_in_flight` model requests (default 1) of one user shall be in flight per backend endpoint across runtimes; the wait is a metric and a log line and is bounded by the request's own deadline. (RFC 013)

### REQ-SCHED-008 One scheduler under the daemon — M9 · T
Under a daemon, LIM-X1, LIM-X2, admission and the engine gate shall be the daemon's, over every client's tasks; clients take no registry slots. (RFC 014)

---

## 16. Observability and audit

### REQ-OBS-001 Events — M1 · T
The runtime shall persist the events of external design §17, each with timestamp (LIM-U4), actor, task, step and action references where applicable, and the policy hash.

### REQ-OBS-002 Event stream — M1 · T
`get_events{task_id?, since}` shall return events in order; clients subscribe to events rather than reconstructing state from logs. (API-007 of the draft)

### REQ-OBS-003 Metrics — M1, M4 · T
`get_metrics` shall derive from events and step records: tasks by state, events by name, rejections by code, halts by reason, approvals by decision, actions by status, and over measured steps the context size, compression ratio, prefix ratio, model latency and token usage (external design §17.2).

### REQ-OBS-004 Logs — M1 · T
Logs shall be structured lines on standard error with timestamp, level and event, and, for a line written while a task's step runs, the task and step ids and the action id when there is an action; never prompt content, file content or secrets at the default level; the switch that logs prompts and model output (`logging.debug_prompts`) shall do so, redacted like every line, and shall warn at start. (REQ-PRIV-004)

### REQ-OBS-005 Audit retention — M3 · T
Events are retained until the user removes a task's audit with `task delete --audit`; no timer (REQ-PERS-010).

### REQ-OBS-006 Runtime status — M2 · T
`get_runtime_status` shall show version, data directory, backend kind and locality, tier with probe results, running tasks and pending user items.

### REQ-OBS-007 Replay — M4 · T
A task's persisted decisions and tool results shall be replayable through the fake backend and fake tools to reproduce its transitions. (REQ-ARCH-006)

### REQ-OBS-008 Refusals counted — M1 · T
Rejections by code, denials by grant kind and halts by reason shall be counted; the documentation names a rising rejection rate as an interface defect. (CLR-12)

### REQ-OBS-009 Machine view — M7b · T
`status` shall show the user's runtimes executing or waiting (pid, data directory, task, since when), the engine gates held and the slots taken against LIM-X5; `metrics` gains `admission_wait_ms` and `engine_wait_ms` per task. (RFC 013)

### REQ-OBS-010 Cost visibility — M8 · T
`task show` shall answer what a task cost: its effort level, the steps used of its envelope, model time, prompt and completion tokens where reported, and the backend; `metrics` aggregates per level. (RFC 015)

---

## 17. Security

### REQ-SEC-001 Threat register — All · A
The threat register shall be reviewed at every milestone and whenever a data flow, an external integration or an authority mechanism changes.

### REQ-SEC-002 Model text is never authority — All · T
No model output shall change grants, budgets, policy, approvals or the visibility of tools. (INV-02)

### REQ-SEC-003 Fail closed — All · T
Any error in validation, policy, confinement, sandbox setup or persistence shall result in no execution. (P4)

### REQ-SEC-004 Invariants tested — All · T
Each invariant INV-01 to INV-14 of the external design shall have at least one test naming it.

### REQ-SEC-005 Path safety — M1 · T
Tests shall cover traversal, symbolic links, case folding, Windows path forms and the existence leak (REQ-TEST-005). (THR-03)

### REQ-SEC-006 Network denial — M2 · T
The sandbox and the worker shall have no network; external network exists only through tools whose `network` grant covers the destination, and through the declared dependency step of REQ-CONF-007, which runs in its own worker under a `host_tool` grant, has no bounded destination, and is named as such by the declaration, the status and the approval (THR-35). (D-13, THR-11; amended 2026-10-31)

### REQ-SEC-007 Approval channel — All · I
The approval and answer operations shall exist only on the control API and never on any surface a model can reach; mimlys exposes no MCP server in v1 (DR-004). (R-17, THR-20)

### REQ-SEC-008 Local files — M1 · T
Database, blobs, snapshots and workspaces shall be owner-only; the lock prevents concurrent runtimes. (THR-31)

### REQ-SEC-009 Model output size — M1 · T
Model output shall be bounded at LIM-D1 while being read; overflow is `malformed`. (R-19)

### REQ-SEC-010 Non-local backends — M4 · T
A backend with `locality = remote` shall be usable only when the administrator sets `remote_backend_allowed`, shall be shown in task status and audit, and shall be refused for profiles that forbid external communication. (D-21, THR-30)

### REQ-SEC-011 Process stability — All · T
No model output, tool result, module or workspace content shall end the runtime process; the worker's death is an execution failure (REQ-ARCH-008). A panic in a model backend or a tool provider is contained at every call the runtime makes into it (a request is `backend_unavailable`, an execution `host_fault{unavailable}`; at a step, a provider panic outside an execution, in its definitions, pins or events, halts the task `tool_unavailable` with its `task.state_changed` event, so the task ends in one stated outcome; before a task exists it is the caller's error), and its message reaches only the log, bounded and redacted, with a crate-relative location (`src/…:line`, never an absolute path, which carries the build machine's home directory), never standard error unredacted. A panic in the persistence or the workspace adapter is not contained: their state on disk is then unknown, so the process ends and the next start recovers (REQ-PERS-007), which is the fail-closed outcome (P4). (Amended 2026-11-07.)

### REQ-SEC-012 Dependency hygiene — M0 · T
CI shall run a dependency vulnerability audit and a license check on every change (REQ-TEST-014). (THR-12)

### REQ-SEC-013 Security regression corpus — M1 · T
A corpus of hostile decisions, paths, tool results, descriptions and modules shall run in CI on every change (REQ-TEST-017).

### REQ-SEC-014 Approval fatigue is a defect — All · I
Any design or default that would ask the user to approve routine, reversible work is rejected at review; approvals are reserved for the effects of REQ-APR-001. (THR-34)

### REQ-SEC-015 Promotion is the only exit — All · T
Nothing but `promote_workspace` shall write to the project scope. (INV-10, THR-28)

### REQ-SEC-016 The daemon's transport — M9 · T
The daemon shall be reached only through a Unix domain socket of mode 0600 in a 0700 directory with a peer-credential check refusing every uid but its own, or on Windows a named pipe whose DACL admits the user's SID alone; no TCP, no port; a client and a daemon of different runtime versions refuse each other. The daemon resolves configuration and `env:` references at its start and shows their hash in `status`. (RFC 014)

---

## 18. Privacy and secrets

### REQ-PRIV-001 Minimization — M1 · T
Only the workspace and granted host paths can enter observations; the goal and the user's answers are the user's own text.

### REQ-PRIV-002 Secret patterns and redaction — M1 · T
LIM-W5 patterns shall be excluded from materialization and reading by default; configured secret values shall be redacted as `[redacted:<name>]` in observations and logs. (THR-16)

### REQ-PRIV-003 Environment — M1 · T
Child processes shall start with an explicit allow-listed environment; the sandbox environment is empty; secrets for backends and servers are given by reference (`env:` or `file:`) and never inline; a test asserts that no configured secret is visible inside a tool. (R-07, THR-24)

### REQ-PRIV-004 Logs — M1 · T
See REQ-OBS-004.

### REQ-PRIV-005 Disclosure — M4, M5 · I
The documentation shall state what leaves the host: prompts to the backend (and whether it is local), tool arguments to external servers, and nothing else.

### REQ-PRIV-006 Retention — M3 · T
See REQ-PERS-010.

---

## 19. Model backend

### REQ-MDL-001 Backend SPI — M1 · T
`ModelBackend` shall offer `generate(request) → text | error` where the request carries messages, the decision schema and limits; `cancel`; and optional usage and cache information.

### REQ-MDL-002 Fake backend — M1 · T
A fake backend that returns scripted responses from a file shall ship with the runtime and be used by tests and replay. (M-15)

### REQ-MDL-003 OpenAI-compatible adapter — M4 · T, D
The adapter shall use chat completions with `response_format` of type JSON schema when enabled, `max_tokens` LIM-M2, deadline LIM-M1, cancellation by closing the connection, and at most LIM-M3 transport retries of the identical request. Backend failure shall yield `backend_unavailable` and, after retries, a halt; never a fabricated decision. Demonstrated against llama-server and Ollama.

### REQ-MDL-004 Structured output modes — M4 · T
`structured_output` shall be `auto`, `schema` or `none`; in every mode the runtime parses the text strictly (REQ-DEC-003). It applies to the `envelope` decision transport only (REQ-MDL-009).

### REQ-MDL-005 Locality — M4 · T
See REQ-SEC-010.

### REQ-MDL-006 Usage metrics — M4 · T
Prompt and completion tokens and cache hits shall be recorded when reported.

### REQ-MDL-007 Output is untrusted text — All · T
Backend output is text until validated; it is bounded while being read (REQ-SEC-009).

### REQ-MDL-008 No model inside the core — All · I
The core shall call a model only for the step decision. (D-12)

### REQ-MDL-009 Decision transport — M4 · T
`decision_transport` shall be `auto`, `tool_calls` or `envelope` (default `auto`). With `tool_calls`, every pinned tool is offered as one function whose parameters are a JSON schema generated deterministically from its pinned descriptor, and the four other decision kinds as the reserved functions `plan`, `ask_user`, `complete` and `give_up`; a tool whose function name is invalid or collides (every tool of a collision) is unavailable in this transport with the reason, computed from the pinned list at each step once the transport is known: the model is told through the `tool_unavailable` feedback and `status` lists it with the same reason. `auto` resolves at the probe: `tool_calls` when the engine accepts `tools`, else `envelope` with the reason; a configured `tool_calls` that the engine refuses is `backend_unavailable` with that reason, never a silent fallback. `status` shows the transport in force. (RFC 012, 0.9.)

### REQ-MDL-010 Several backends — M8 · T
`status` shall list every configured backend with its probe result and, where the engine reports it, the model resident; a level whose backend is unreachable makes `run --effort <level>` refuse with the reason. (RFC 015)

### REQ-MDL-011 Fitness probe — M8 · T
`mimlys model probe [<backend>]` shall run one scripted mini-task against the real model in a runtime-created scope under the data directory, with the built-in read and write tools only, within LIM-E2 steps and LIM-E3, and record `acts`, `plans_only`, `no_structured_call` or `unreachable` with the date; it never touches a user's project, deletes its task and scope, and is refused while another process holds the store. (RFC 015)

---

## 20. MCP client

### REQ-MCP-001 Servers as children — M5 · T
MCP servers are configured by the administrator and spawned over stdio with an allow-listed environment and secrets by reference; they are terminated at runtime exit by closing their standard input and, after LIM-O16 (per server `shutdown_ms`), killing their process group; restarts are bounded by LIM-X3; startup by LIM-O6. (K-05; LIM-O16 from rypsok's letter of 2026-11-11)

### REQ-MCP-002 Discovery and pinning — M5 · T
Tool definitions shall be listed at task start, hashed and pinned on the task. A changed definition shall make the tool unavailable for that task with the event `tool.unpinned` until a new task pins it; the change is detected at the next step boundary and named to the model (RFC 009 §7). (R-08, THR-25)

### REQ-MCP-003 Definition import — M5 · T
Descriptions shall be sanitized (REQ-CTX-008) and bounded by LIM-I2; schemas above LIM-I3 parameters or LIM-I4 depth make the tool unavailable with a reason; annotations are recorded and never trusted. (THR-25, THR-26)

### REQ-MCP-004 Effect declaration — M5 · T
The administrator declares, per external tool, the effect, `idempotent`, destination arguments and deadline (`reversible` is always false for external effects, RFC 003). An undeclared tool is `external_write`, non-idempotent. A declaration admits the tool to the ceiling (RFC 009 §4). (K-06)

### REQ-MCP-005 Destination validation — M5 · T
Destination arguments shall be validated against the task's `network` grants (scheme, domain, port) before the call and again on a `redirected` result. Only `https` destinations are admitted. An `http` destination is refused with the reason `scheme_not_https`, whose hint says to propose its `https` form (rypsok reads an `http` URL over `https` in any case). (K-02; amended 2026-11-11. An earlier wording of the same day assumed that mimlys admitted `http`; it never did, as handoff 009d found.)

### REQ-MCP-006 Results — M5 · T
Structured content is validated against the declared output schema; `isError` becomes the cause `tool_error`; content is bounded and labelled external. (K-10)

### REQ-MCP-007 Cancellation — M5 · T
`notifications/cancelled` shall be sent and the call abandoned within LIM-O2.

### REQ-MCP-008 Deadlines — M5 · T
Per-tool deadlines default to LIM-O5 and are never below a server-declared maximum when one is configured. (K-03)

### REQ-MCP-009 Visibility — M5 · T
Only tools with a `tool` grant are visible; `engine_status` of rypsok is not granted by default. (K-04)

### REQ-MCP-010 No MCP server surface — All · I
mimlys exposes no MCP server in v1 (DR-004). (REQ-SEC-007)

---

## 21. Operator interface: configuration and CLI

### REQ-CFG-001 Configuration file — M1 · T
All settings come from one TOML file located by `--config`, `MIMLYS_CONFIG` or the platform default, carrying `config_version`. Unknown keys and invalid values shall stop the process at startup with exit code 1 naming the key. `mimlys check` validates and exits.

### REQ-CFG-002 Defaults — M1 · T
Every default is the value of the limits register; a process with no file runs with the fake backend and the `workspace` profile.

### REQ-CFG-003 Secrets by reference — M1 · T
Credentials are given as `env:NAME` or `file:PATH`; an inline secret shall be refused at startup.

### REQ-CFG-004 Policy — M1 · T
The policy section defines profiles, the ceiling, approval rules, exclusions and recovery options; it is validated at startup and hashed (REQ-CAP-010).

### REQ-CFG-005 No hot reload — M1 · I
Configuration changes apply at restart.

### REQ-CFG-006 Message catalog — M1 · I
All user-facing messages live in one catalog so that other languages can be added without redesign. (D-17)

### REQ-CFG-007 Effort and backends configuration — M8 · T
`[effort.<level>]` and `[backends.<name>]` shall be configuration sections; each backend passes REQ-SEC-010 on its own; until 1.0 a level's backend must be local; `config_version` 2, with a version-1 `[backend]` read as `backends.default` for one release. (RFC 015)

### REQ-CLI-001 Commands — M1 · T
The `mimlys` binary shall offer the commands of external design §19.2 with exit codes 0 (success), 1 (runtime or configuration error), 2 (usage error).

### REQ-CLI-002 Approval prompt — M1 · T
The approval prompt shall use a fixed template, show the rendered request of REQ-APR-002, and offer approve, reject, reject and halt, and grant.

### REQ-CLI-003 Follow mode — M1 · T
`mimlys run` shall create a task, show events as they happen and prompt when the task waits for the user.

### REQ-CLI-004 Output — M1 · T
Human-readable output by default; `--json` for every command with stable field names that are part of the contract.

### REQ-CLI-005 Shutdown — M1 · T
A stop signal shall pause running tasks: the step in flight starts no new action, and the task pauses when what is in flight has finished; a second signal cancels them; the process exits within LIM-O1 with the worker terminated. One exception, said to the user: while an action that cannot be undone is in flight (its outcome after an interruption would be unknown: an external write, a host write, an external read that is not idempotent), the first signal does not interrupt it; the process says that it waits for that action and why, in words for its effect, and the wait is bounded by the action's own deadline. A second signal then interrupts it anyway: the action's outcome is recorded as unknown and the task halts `unknown_outcome` with the action listed, never `cancelled` as if nothing had happened. (Owner's decision, 2026-11-14; "starts no new action" and the words for each effect from the review of handoff 010e, the same day.)

### REQ-CLI-006 Tier display — M2 · T
`mimlys status` shall show the tier and every clause the platform cannot enforce.

### REQ-CLI-007 Storage and operation pages — M7a · I
The book shall have a page on storage (what lives where, reflinks per filesystem, what grows, how to reclaim) and a section on several projects and jobs (one store per user; how several runtimes behave; the two reasons a task is queued), with tested examples. (RFC 013)

### REQ-CLI-008 Daemon lifecycle — M9 · T
`mimlys daemon start | stop | status` shall exist; without a daemon a client runs embedded; `stop` finishes or checkpoints the running execution within LIM-O2; an idle daemon exits after LIM-O13; a client finding a dead daemon's socket reaps it and says `daemon not running` within LIM-O14. (RFC 014)

---

## 22. Platforms and engineering

### REQ-PLAT-001 Rust — All · I
Rust, edition 2024, minimum version 1.97 (raised from 1.90 at the design review of 0.6: Wasmtime 50 requires it); the workspace is published as a whole.

### REQ-PLAT-002 Platforms — M1 · T
Releases shall build and pass tests on Linux (x86_64, aarch64), Windows (x86_64) and macOS (aarch64). macOS x86_64 shall build and pass the unit and integration tests under Rosetta 2 on the arm64 runner; the confinement behaviour of Intel hardware is not tested and is stated as such. (D-07 v2; amended 2026-11-03 after the Rosetta spike: the Intel runner images end in 2027)

### REQ-PLAT-003 Tier per platform — M2, M6 · T
The core tier is available on every supported platform; the full tier as each backend lands (M6a, M6b, M6c).

### REQ-PLAT-004 Filesystem semantics — M1 · T
Case-insensitive filesystems, Windows path rules, symbolic-link privileges, reflink absence and file locking shall be handled per the external design and tested on each platform. (assessment §5)

### REQ-ENG-001 Unsafe code — M0 · T
`unsafe` is forbidden in every crate except the confinement modules of `mimlys-exec` (REQ-CONF-010), where it is denied by default and allowed per justified module.

### REQ-ENG-002 CI gate — M0 · T
See REQ-TEST-014.

### REQ-ENG-003 Test layout — All · I
Tests live beside their module as `src/<mod>/tests.rs` (project rules).

### REQ-ENG-004 Panic policy — M1 · T
Release builds unwind; adapters and tool handlers run under panic containment; the worker is a separate process.

### REQ-ENG-005 Typed boundaries — All · I
Every external structure (decision, configuration, MCP message, IPC message) is an explicit type validated at deserialization.

### REQ-ENG-006 Documentation — All · I
Documentation is an mdbook under `docs/`; the README follows the project rules; every contract change appears in `CHANGELOG.md`.

---

## 23. Testing

### REQ-TEST-001 Specification-derived tests — All · I
Every requirement marked `T` shall have at least one test that names it.

### REQ-TEST-002 Test doubles — M1 · T
A fake backend (REQ-MDL-002), a fake tool provider, a fake approval channel, an injectable clock and identifier source, and crash injection at every persistence point shall exist.

### REQ-TEST-003 State machines — M1 · T
Every legal transition of tasks, actions and approvals shall be tested, and every illegal one shall be refused.

### REQ-TEST-004 Decision parsing — M1 · T
Tests shall feed oversized, nested, duplicated-key, trailing-text, invisible-character and wrong-type decisions and verify the rejection code and that no execution occurred.

### REQ-TEST-005 Authorization and paths — M1 · T
Tests shall cover traversal, symbolic links, case folding, Windows path forms, reserved names, and shall verify that an out-of-scope path yields the same `denied` whether or not it exists.

### REQ-TEST-006 Approvals — M1 · T
Tests shall verify hash binding, grant-set binding, one-shot consumption, expiry, invalidation on revocation, and that rendered requests escape hostile strings.

### REQ-TEST-007 Crash recovery — M1, M3 · T
Tests shall kill the runtime at every persistence point and verify resumption, the `unknown` disposition per effect, and that no non-reversible action is replayed.

### REQ-TEST-008 Sandbox — M2 · T
Tests shall verify each guard independently, filesystem and network denial, output limits, clock granularity, malformed modules, and that the worker's death does not end the runtime.

### REQ-TEST-009 Workspace — M3 · T
Tests shall verify snapshot and rollback by hash, symbolic links in both directions, promotion conflicts and backups, quotas, and exclusions.

### REQ-TEST-010 Context — M4 · T
Tests shall verify provenance delimiting, prefix stability across steps, and that compression keeps grants, halts, rejections and unresolved failures.

### REQ-TEST-011 Budgets and convergence — M1 · T
Tests shall verify every budget halt and the identical-decision rule.

### REQ-TEST-012 MCP client — M5 · T
Tests shall verify pinning, environment scrubbing, output-schema validation, cancellation, destination validation, hostile descriptions and undeclared-effect defaults against a local test server.

### REQ-TEST-013 Confinement — M2, M6a, M6b, M6c · T
Per platform, tests shall verify each clause the probe reports as enforced, that an unenforceable clause makes host tools unavailable, and that the probe is honest (an injected failure is reported).

### REQ-TEST-014 CI gate — M0 · T
On every change, on Linux, Windows and macOS: formatting, lints as errors, tests, build; and on Linux: the build and tests on the minimum Rust version (REQ-PLAT-001), the packaging dry-run of the workspace, dependency audit and license check, the `unsafe` rule, the core-dependency rule (REQ-ARCH-002), the RFC-directory invariants, documentation build and documentation cross-reference check.

### REQ-TEST-015 Privacy — M1 · T
Tests shall verify that no configured secret appears in prompts, observations, logs or a tool's environment.

### REQ-TEST-016 Determinism and replay — M1 · T
Tests shall replay recorded tasks through the fakes and verify identical transitions and events.

### REQ-TEST-017 Security regression corpus — M1 · T
The hostile corpus of REQ-SEC-013 shall run on every change.

### REQ-TEST-018 Formal proofs, kept narrow — All · T
A Verus proof shall be written only for a pure function whose property is a stated requirement or threat control. Its specifications sit in the production file under `cfg_attr(verus_keep_ghost, …)` or `#[cfg(verus_keep_ghost)]` only, the checker reads that file itself (never a copy), and no published crate gains a dependency. Each proof records the mutations that fail it, those that survive with what holds them instead, its run time and what it does not prove (`repository-guards.md`). A required CI job checks the proof with a pinned, hash-checked Verus and its named toolchain, and runs at least one recorded mutation that must fail. (RFC 016)

---

## 24. Maintainability and versioning

### REQ-MAIN-001 Contract versioning — M1 · I
Crates follow semantic versioning. A change to a task state, decision field, tool name or schema, effect value, rejection code, event name, CLI command or JSON output field is a breaking change of the external contract: announced in `CHANGELOG.md` before 1.0.0 and carried by a major version after it.

### REQ-MAIN-002 Independent schema versions — M1 · T
The decision schema, the action and observation schemas, the event schema, the configuration schema and the persistence schema shall carry independent versions, recorded per REQ-PERS-012.

### REQ-MAIN-003 Adding a built-in tool — All · I
A new built-in tool requires its schema, effect, tests, documentation and a changelog entry, and nothing else changes.

### REQ-MAIN-004 Adding a confinement backend — All · I
A new backend requires passing REQ-TEST-013 on its platform and a probe entry; the contract does not change.

### REQ-MAIN-005 Documents in step — All · I
A release verifies that requirements, external design, limits, threat register, glossary and roadmap match the shipped behaviour (project rules, release deliverables).

---

## 25. Deferred requirements

| ID | Deferred capability | Condition |
|---|---|---|
| DR-001 | Plan nodes with dependencies and parallel read-only steps | Own RFC after M5 |
| DR-002 | Daemon mode and authenticated client sessions | Own RFC, planned after 0.10 (owner's decision of 2026-11-08: in this repository; first scope one daemon per operating-system user, a local socket with a peer-credential check, no network); authentication beyond the operating system's user boundary stays deferred |
| DR-003 | Multi-user isolation | After DR-002 |
| DR-004 | An MCP server surface for mimlys | Own RFC; approval must stay unreachable by models |
| DR-005 | Model-based summarizer adapter, output untrusted | Own RFC after M4 measurements |
| DR-006 | Built-in network adapter for the sandbox | Only if external tools prove insufficient |
| DR-007 | Compiled-language adapters inside the confined worker | After M6a |
| DR-008 | GUI and additional languages | After the message catalog exists |
| DR-009 | Vector memory as a context adapter | Own RFC |
| DR-010 | Cryptographic audit integrity | Own RFC |
| DR-011 | Policy simulation before execution | Own RFC |
| DR-012 | Model-invocable rollback tool | After M3 |
| DR-013 | Metrics export (OpenTelemetry) | Own RFC |
| DR-014 | Repair tooling for a damaged database | Own RFC |
| DR-015 | Windows aarch64 | When CI runners allow |
| DR-016 | Host-path reads by built-in tools under `host_read` grants (absolute paths under a granted root, canonicalized and authorized before existence like workspace paths) | Own RFC; the grant kind and the ceiling root exist, the interface refuses them until then |
| DR-017 | Host tools with declared write paths (`host_write` host tools, `host_write` grants) | Own RFC after DR-016; the worker's write set must name the paths before a declaration may promise them |

---

## 26. Acceptance per milestone

| Milestone | Accepted when |
|---|---|
| M0 | REQ-ARCH-002, REQ-ENG-001, REQ-SEC-012, REQ-TEST-014 hold on an empty workspace on all three platforms; this document, the external design, glossary, limits and threat register are baselined; RFCs 001–004 are accepted |
| M1 | Every requirement marked M1 passes; a scripted task runs create → decide → validate → approve → execute → observe → complete with the fakes; killing the process at every persistence point resumes correctly; an out-of-scope path yields `denied` without an existence leak |
| M2 | Every M2 requirement passes on all three platforms; an infinite loop, an allocation bomb and an output flood each end with the correct termination; the overhead report exists |
| M3 | Snapshots, rollback, promotion and unknown-outcome recovery verified; symbolic-link tests pass in both directions |
| M4 | A local model completes a scripted repair of a WASM-targetable workload within budgets; token and prefix measurements reported |
| M5 | rypsok tools run under capability and destination policy; `engine_status` invisible to the model |
| M6a / M6b / M6c | The confinement clause tests pass on the platform; a declared host tool runs offline: the demonstration of external design §23 |

---

## Appendix A — Disposition of the draft requirements

Every identifier of the draft requirements specification (owner's archive) is accounted for. "→" names the surviving requirement; "merged" means the content lives in the named requirement without its own identifier; "informative" means the content moved to the architecture overview or the roadmap; "dropped" names the reason.

| Draft | Disposition |
|---|---|
| G-01 … G-10 | Principles P1–P10 and §1; G-01 "state machine and/or DAG" resolved by D-11 (linear plan, DR-001) |
| NG-01 … NG-07 | §1 non-goals |
| P-01 … P-10 | Principles P1–P10 |
| FR-001 … FR-004 | → REQ-TASK-001, REQ-TASK-002, REQ-TASK-002, REQ-PERS-003 |
| FR-005, FR-006 | → REQ-PERS-004, REQ-PERS-007 (checkpoint = step boundary + snapshot, glossary) |
| FR-007, FR-038, FR-105 | → REQ-TASK-006, REQ-TOOL-007, REQ-SCHED-004 (duplicates merged) |
| FR-008, FR-009 | → REQ-TASK-009, REQ-TASK-008 |
| FR-010, FR-025, FR-099 | → REQ-TASK-007, REQ-DEC-010 (duplicates merged) |
| FR-011 … FR-017 | → REQ-DEC-011 (advisory plan); DAG, dependencies and parallelism deferred as DR-001 (D-11) |
| FR-018 … FR-022 | → REQ-DEC-001, REQ-ARCH-004, REQ-DEC-002, REQ-ARCH-005, REQ-CTX-009 |
| FR-023, FR-024, FR-095, FR-096 | → REQ-TOOL-009, REQ-TASK-008 |
| FR-026 … FR-032 | → REQ-MDL-004, REQ-DEC-003, REQ-DEC-006, REQ-DEC-007, REQ-CAP-001, REQ-SEC-003, REQ-DEC-008/009 |
| FR-033 … FR-040 | → REQ-TOOL-001, REQ-TOOL-001, REQ-ARCH-005, REQ-TOOL-005, REQ-TOOL-006, REQ-TOOL-007, REQ-CONF-001/REQ-SBX-001, REQ-ARCH-004 |
| FR-041 … FR-051 | → REQ-SBX-001, REQ-SBX-001, REQ-SBX-010 (adapters), REQ-SBX-004, REQ-SBX-004, REQ-SBX-004, REQ-SBX-004, REQ-SEC-006, D-13 (no built-in network; DR-006), REQ-SBX-005, REQ-SBX-005 |
| FR-052 … FR-056 | → REQ-SBX-002, REQ-SBX-002, REQ-SBX-002, REQ-SBX-002, REQ-SBX-003 |
| FR-057 … FR-059 | → REQ-SBX-003, REQ-SBX-011, REQ-TOOL-002 (causes), REQ-DEC-009 |
| FR-060 … FR-067 | → REQ-WS-001, REQ-WS-002, REQ-WS-004, REQ-WS-006, REQ-TOOL-010, REQ-TOOL-010, REQ-TOOL-001 (idempotent flag), REQ-PERS-009 |
| FR-068 … FR-074 | → REQ-CTX-001, REQ-CTX-001, REQ-CTX-004, REQ-CTX-003, REQ-CTX-003, REQ-CTX-003, REQ-CTX-006 |
| FR-075 … FR-080 | → REQ-PERS-001, REQ-PERS-001, REQ-PERS-001, REQ-PERS-003, DR-009, DR-009 |
| FR-081 … FR-087 | → REQ-CAP-001, REQ-CAP-001, REQ-CAP-001, REQ-CAP-002, REQ-CAP-009, REQ-CAP-004, REQ-CAP-005 |
| FR-088 … FR-093 | → REQ-APR-001, REQ-TOOL-002 (effects replace destructive classification), REQ-DEC-004, REQ-APR-006, REQ-OBS-001, REQ-APR-008 |
| FR-094, FR-097, FR-098, FR-100 | → REQ-TOOL-002, REQ-DEC-009, REQ-TOOL-010, REQ-PERS-007 |
| FR-101 … FR-106 | → REQ-ARCH-003 (Tokio decided), DR-001, REQ-SCHED-003, REQ-SCHED-001, REQ-SCHED-004, REQ-SCHED-005 |
| FR-107 … FR-122 | → REQ-OBS-003 and REQ-SBX-009/REQ-SBX-011/REQ-CTX-010; FR-116 token density is a derived metric of REQ-OBS-003 |
| NFR-001 … NFR-016 | → P1, P4, §1, P1, REQ-PERS-007, P5, REQ-ARCH-001, REQ-ARCH-004, REQ-SBX-009, REQ-OBS-001, REQ-OBS-001, REQ-TOOL-008, REQ-SBX-004, REQ-CAP-006, REQ-PLAT-002, REQ-ARCH-002 |
| SEC-001 … SEC-012 | → REQ-SEC-001, REQ-SEC-002, REQ-CAP-001, REQ-SBX-004, REQ-SEC-006, REQ-SBX-002, REQ-SEC-005, REQ-SBX-004, REQ-PRIV-001, REQ-TOOL-008, REQ-OBS-001, REQ-SBX-012 |
| §22 threat table | → threat register THR-01 … THR-34 |
| §23 data model, §24 lifecycles | → external design §5–§8, §14, §16; `priority`, `parent_task_id`, `cpu_time_ms` dropped (A-13); `suggested_fix` dropped: the runtime does not author advice to the model, rejections carry fixed hints (T-05) |
| §25 external interfaces | → REQ-ARCH-004 and external design §4; `submit_decision`, `validate_action`, `execute_action`, `observe_result` removed from the control API (C-10) |
| LLM-001 … LLM-004 | → REQ-MDL-001, REQ-MDL-001, REQ-MDL-004, REQ-CTX-001 |
| §27 hardware figures | Informative; not requirements |
| §28 configurable limits | → limits register |
| REL-001 … REL-006 | → REQ-PERS-003, REQ-PERS-007, REQ-PERS-008 (rewritten as an absolute rule, R-11), REQ-TOOL-009, REQ-SCHED-005, REQ-OBS-001 |
| §30 configuration | → REQ-CFG-001 … REQ-CFG-004 |
| LOG-001 … LOG-007 | → REQ-OBS-004, REQ-OBS-001, REQ-OBS-001, REQ-OBS-004, REQ-OBS-001, REQ-OBS-001, REQ-PRIV-002 |
| §32 testing | → REQ-TEST-001 … REQ-TEST-017 |
| AC-001 … AC-012 | → §26 acceptance (M1: AC-001, 002, 003, 006, 010, 011, 012; M2: AC-004, 005; M3: AC-007, 008; M4: AC-009) |
| Q-001 … Q-006 | → REQ-OBS-003 metrics and the measurement reports of REQ-SBX-009, REQ-CTX-010 |
| §35 phases | → `ROADMAP.md` milestones (D-23) |
| ACST-001 … ACST-007 | → REQ-ARCH-001, REQ-ARCH-005, REQ-SBX-001, REQ-PERS-003, REQ-TOOL-010, REQ-CTX-002, REQ-ARCH-004 |
| TBD-001 … TBD-014 | → decisions D-15, D-01, D-02, D-15, D-08, D-16, REQ-SBX-001, D-13, DR-009, DR-013, REQ-PERS-010, DR-002, DR-003, REQ-SEC-001 |
| Draft external design EXT-TBD-001 … 010 | → DR-002, DR-003, D-18, REQ-CFG-004, D-08, REQ-PERS-002, D-13, REQ-PRIV-003, DR-010, out of scope |
