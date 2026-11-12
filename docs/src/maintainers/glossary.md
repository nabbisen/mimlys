# Glossary

**Status:** Baseline 1.0 (2026-10-02).

One name for each thing, one meaning for each name. Every mimlys document uses
these words only in the sense given here. Where MCP or rypsok uses a word
differently, the difference is stated at the end.

| Term | Meaning in mimlys |
|---|---|
| **mimlys** | This product: a local AI-agent execution and safety runtime. It turns uncertain model decisions into validated, bounded, recoverable state transitions. It is not a model, a search engine or a user interface |
| **Runtime** | The mimlys process that owns tasks, policy, validation, persistence and recovery. In v1 it runs inside the `mimlys` CLI process |
| **Worker** | The child process of the runtime that performs executions: the WASM sandbox, and from M6 host tools. Confined by the operating system where the platform allows |
| **Client** | A program that drives the runtime through the control API; in v1 the `mimlys` CLI. A client is never a policy authority |
| **User** | The human who gives goals, answers questions, approves actions and issues grants within the administrator's ceiling, through a client |
| **Administrator** | The human who sets the policy ceiling, tool servers and model backends, through configuration files. One person may hold both roles; the channels differ |
| **Session** | One client's connection to the runtime under one user identity. In v1 the CLI process is the session and the operating-system user is the identity |
| **Model** | The language model that proposes decisions. Its output is untrusted |
| **Backend** | The adapter and service that produce model output for the runtime: an OpenAI-compatible HTTP server, or the fake backend used in tests |
| **Task** | A persistent unit of work with a goal, a workspace, grants, budgets and a state. Created by a user, driven by the runtime |
| **Task state** | One of `created`, `running`, `waiting_user`, `paused`, `halted`, `recovering`, `completed`, `failed`, `cancelled` |
| **Step** | One iteration of the control loop: assemble context, obtain a decision, validate, possibly wait for the user, execute, observe, evaluate. A step holds exactly one decision and at most one action |
| **Decision** | The one structured object the model returns per step: `act`, `plan`, `ask_user`, `complete` or `give_up` |
| **Rejection** | The runtime's refusal of a decision before any execution, with a code from a fixed vocabulary and a hint of what may change |
| **Action** | A validated request to run one tool with canonical arguments. It carries an effect, a hash of its content and a lifecycle from `proposed` to a terminal status |
| **Execution** | One attempt to run an action. An action has at most LIM-T3 executions; each produces an execution record |
| **Observation** | The normalized, bounded, provenance-labelled result of an action that the model sees next. Observations are data, never instructions |
| **Tool** | A named operation the runtime can perform for a task: a built-in tool (file operations, `run_script`, `run_wasm`), an external tool served over MCP, or, from M6, a declared host tool |
| **Effect** | The declared side-effect class of a tool and of every action: `none`, `workspace_read`, `workspace_write`, `host_read`, `host_write`, `external_read`, `external_write`, with the flags `reversible` and `idempotent` |
| **Capability grant** | An explicit, per-task, revocable authorization: a tool's visibility, a host path for reading or writing, network destinations, a host tool. No grant, no operation |
| **Policy ceiling** | The administrator's configuration that bounds what grants and budgets a task may have. A grant never exceeds the ceiling |
| **Profile** | A named set of default grants, budgets and approval rules within the ceiling, chosen at task creation |
| **Budget** | A durable counter with a limit on a task: steps, consecutive rejections, consecutive failures, identical decisions, executions per action, and the absolute deadline |
| **Approval** | A one-shot human authorization for one exact action, bound to the action's content hash and to the grants in force. Consumed when the action starts |
| **Question** | A model's request for information from the user (`ask_user`). The answer enters the next step as user text |
| **Workspace** | The task's private, materialized copy of the authorized project scope, owned by the runtime. The model reads and writes only there |
| **Project scope** | The directory and patterns of the user's own tree that a task is materialized from and may be promoted to |
| **Snapshot** | An immutable, content-addressed record of a workspace at a point in time: a manifest plus objects. Its reason names when it was taken: `base` (materialization), `before` and `after` (a write action), `backup` (a promotion's targets) |
| **Rollback** | Restoring a workspace to a snapshot |
| **Promote** | The explicit, user-initiated, confirmed operation that applies the workspace's reviewed changes to the user's project scope |
| **Checkpoint** | A step boundary at which the persisted task state and the current snapshot together allow resumption. Nothing is stored beyond those two |
| **Recovery** | What the runtime does at startup with tasks that were running: establish each in-flight action's outcome, roll back or halt according to its effect, and never replay a non-reversible effect |
| **Sandbox** | The WASM execution environment (Wasmtime with WASI) with fuel, deadline and memory guards, a preopened workspace, no network and an empty environment |
| **Confinement** | The operating-system restriction of the worker, and from M6 of host tools, under the confinement contract: filesystem sets, no network, explicit environment, tree termination, no privilege gain, resource ceilings |
| **Clause** | One of the seven guarantees of the confinement contract, named by a letter: F (filesystem sets), N (no network), E (explicit environment), K (tree termination), P (no privilege gain), M (memory ceiling), C (process count and wall clock). F, N, E, K and P are mandatory |
| **Primitive** | The operating-system mechanism that holds a clause on this machine, named in the status beside the clause's state (`landlock`, `seccomp`, `cgroup_kill`, `subreaper`, …) |
| **Tier** | What this installation can enforce, detected at startup: `core` (sandbox only) or `full` (confined host tools available). A best-effort mandatory clause gives `full` only when the administrator accepted it by name, and the tier then says so |
| **Host tool** | A native program the administrator declared (`[host_tools.<name>]`): a fixed command and arguments, what the model may add, the toolchain paths it reads, its environment. Run only through `run_host_tool`, only under a `host_tool` grant, only at tier `full`, inside the workspace |
| **Declaration** | The administrator's description of an external or host tool in the configuration: its effect and bounds for an MCP tool, its command and arguments for a host tool. A declaration admits the tool to the ceiling; nothing else does |
| **Dependency step** | A declared host tool with `network = true`, such as `cargo fetch`: the one host-tool execution that may reach the network, run by a worker of its own with every other clause in force, effect `external_read` |
| **Read set** | The paths the worker and every host tool may read: the interpreters directory, the worker binary and the interpreter modules (as files, RFC 017), the declared toolchain paths and the system directories; `mimlys toolchain list` shows it. The write set is the workspaces and the temporary directory |
| **Import** | Taking an MCP server's tool definition into mimlys's flat parameter model under bounds, with a named reason when it does not fit; the result is pinned on the task |
| **Pinning** | Recording the hash of every tool definition on the task at creation; a definition that changes afterwards makes the tool unavailable for that task, never different |
| **Footprint** | What a task holds on disk: its workspace, snapshots, objects (exclusive and shared), blobs and promotion backups; shown by `task list` and `task show`, released by `task delete` |
| **Fork** | A new task created from another task's snapshot, with its grants copied and a short history attached; the source is untouched |
| **Redaction** | Replacing every occurrence of a configured secret's value by `[redacted:<name>]` in observations and logs before anything is stored or shown |
| **Halt** | A stop by the runtime for a reason from a fixed vocabulary; the user resumes or cancels |
| **Pause** | A stop by the user; the user resumes |
| **Notice** | A statement the runtime attaches to an observation or a response about something it changed: a truncation, removed characters, a replaced encoding |
| **Event** | A structured, persisted record of a transition or decision; the source of audit and metrics |
| **Audit** | The persisted events with their actors, kept under the retention policy |
| **Limits register** | `limits-and-defaults.md`: every number the specifications cite as a `LIM-` identifier |
| **Threat register** | `threat-register.md`: every threat with its controls and verifying tests |
| **Milestone** | M0 to M6c as defined in `ROADMAP.md`. Every requirement names the milestone in which it is verified |

## Words with another meaning elsewhere

| Word | mimlys | MCP | rypsok |
|---|---|---|---|
| Capability | An explicit grant to a task | A protocol feature a client or server declares | Not used |
| Session | A client's connection under one user | Not present in the current revision | Not used |
| Operator | Not used; mimlys says Administrator and User | — | Whoever configures and starts rypsok |
| Host | The machine and operating system mimlys runs on, as in host tool and `host_write` | The program that connects a model to a server | The program that connects a model to rypsok |
| Tool | A built-in, external or host operation the runtime performs | A function a server exposes | One of its three operations |
| Policy | The administrator's ceiling and the runtime's fixed rules | — | Its egress policy |
| Observation | The normalized result of an action | — | Not used |
| Provider | Not used as a noun; adapters implement traits | — | The organization behind an engine |
