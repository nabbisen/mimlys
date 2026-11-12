# Command line

`mimlys` embeds the runtime: one process per command, your operating-system
user as the identity recorded on every decision. A command that writes locks
the data directory while it runs (a second one waits one second and then says
`store locked by another runtime`). The commands that only read (`status`,
`events`, `metrics`, `task list`, `task show`, `toolchain list`, `check` and
`promote --patch`) do not take the lock: they open the store as a reader, so
they answer while another process runs a task, say so and never say "locked",
and they change nothing, not even a file: no recovery, no reaping of a leftover
worker, no migration, no `runtime.started` event. They show what is committed.
If the last runtime ended in the middle of a task, `status` says `recovery
pending: the next command that writes recovers it`; if another process holds
the store, `status` says `store: in use by runtime <pid> since <time>`. A store
written by a newer mimlys is refused with the reason; one from an older mimlys
says `migration pending`, and the next command that writes migrates it. Steps run only inside the process that drives the task,
so a command that leaves a task `running` follows it until the task waits,
pauses, halts or ends.

Global options: `--config FILE`, `--json`. Exit codes: 0 success, 1 runtime
or configuration error, 2 usage error.

| Command | What it does |
|---|---|
| `run <goal> [--scope DIR] [--profile P] [--grant KIND:SCOPE]… [--steps N] [--deadline D] [--completion-check JSON]` | Create and start a task, print events, prompt when it waits |
| `task new <goal> [--hold] …` | Create (and start unless `--hold`) |
| `task show <task>` | The task, its plan, grants and pending item |
| `task list` | Every task |
| `task resume <task> [--extend-steps N] [--extend-deadline D]` | Continue a created, paused or halted task |
| `task pause <task>` | Pause at the next step boundary |
| `task cancel <task>` | Cancel |
| `task prune [--state STATES] [--older-than D] [--audit] [--yes]` | List the finished tasks that match (`--state` takes `completed`, `failed`, `cancelled`; `--older-than` counts from the task's last change) with what each frees and the total, ask once, then delete each as `task delete` does; never a task that has not finished; a task whose promotion can still be reversed is listed as `kept: promotion reversible` and skipped; the audit stays unless `--audit`; on a failure it stops, names the failed task and the tasks it did not touch (exit 1). Without a terminal and without `--yes` it deletes nothing |
| `task delete <task> [--audit] [--yes]` | Remove a terminal task's workspace, snapshots and outputs after showing what goes; the audit stays unless `--audit` |
| `task new <goal> --from-task T [--from-snapshot S]` | Fork: a new task from another task's snapshot, with its grants and history |
| `approve <task>` | Approve the pending action; it runs at once |
| `reject <task> [--note TEXT] [--halt]` | Reject the pending action; the task continues or halts |
| `answer <task> <text>` | Answer the pending question |
| `grant <task> tool NAME` · `host_tool NAME` · `network DOMAIN[:PORT]` | Issue a grant within the ceiling (`KIND:SCOPE` works too, as on `run --grant`); `host_read` and `host_write` are refused as reserved, and `tool run_host_tool` points at `host_tool` |
| `revoke <task> <grant>` | Revoke a grant at once |
| `events [<task>] [--since N] [--follow]` | Events in order; `--follow` of a task another process runs polls the store every half second (it takes no lock) and ends when the task ends, or says `recovery pending` and ends when the runtime that ran it is gone |
| `metrics [--task T]` | Derived counts, from the events and the records: tasks by state, events by name, rejections by code, denials by reason and by grant kind, halts by reason, approvals by decision, actions by status, and the context and model measures. A rising rejection rate is a defect of the interface the model is given (its tool definitions, the instructions, the transport): investigate it, do not tolerate it as noise |
| `status --storage` | The store's path, its filesystem and whether it can reflink, the bytes by kind (working trees, snapshot objects exclusive and shared, blobs, interpreters, database and WAL, temporary) and the tasks that hold the most (ten in text, all in JSON) with state, age and what deleting each frees. Read-only; see [Storage](./storage.md) |
| `status` | The store (free, in use by another runtime and which, recovery pending, or not created yet), version, data directory, backend (kind, model, locality, reachable, decision transport with the engine's reason when `auto` chose the envelope, structured-output mode of the envelope transport), tier, each clause by letter and word with its state and primitive, declared host tools, configured servers with their tools (with no data directory yet no server is started: `servers: not started (no data directory yet)`, and the tools each declares are listed as declared), running and pending tasks |
| `toolchain list` | The read set host tools run under and each declared tool's paths; on Windows whether each carries the container's permission |
| `toolchain grant <path> [--yes]` · `revoke <path> [--yes]` | Windows: add or remove, after asking, the worker container's read permission on a toolchain directory, recorded for exact reversal; elsewhere nothing is needed and the command says so |
| `toolchain revoke --all [--yes]` | Windows, before an uninstall: remove every permission the container holds (mimlys's own directories and the declared read paths included, found by a bounded scan if the record lost them), delete the container profile and the record; elsewhere nothing was changed outside the data directory and the command says so. A path with `--all` is a usage error (exit 2) |
| `check` | Validate the configuration and print it |
| `interpreters list` | The configured interpreters and whether they are installed |
| `interpreters install <language> [--from FILE]` | Verify by hash and install an interpreter build (download of the shipped build without `--from`, within two minutes and retried three times on a transient failure; if it still fails, the error is `unavailable`, component `download`, and `--from FILE` is the remedy) |
| `rollback <task> [--snapshot S]` | Restore the workspace to a snapshot (the latest by default) |
| `rollback <task> --promotion P` | Reverse a promotion from its backup |
| `promote <task> [--yes] [--summary] [--patch]` | Diff against the base, refuse conflicts, back up, ask, write; `--patch` prints a patch and writes nothing |

A `<task>` is an id or a unique prefix of at least four characters.
Durations are `30s`, `15m`, `2h`, `1d`.

## The approval prompt

The prompt is rendered by the runtime from the canonical action, never by
the model or a tool. Every external string is quoted; invisible characters
are removed with a notice; the model's `reason` is shown separately and
marked untrusted.

- `a` approves. The action runs under the grants in force at that moment.
- `r` rejects with an optional note the model sees as user text.
- `h` rejects and halts the task.
- `g` issues a grant. The pending request is withdrawn and shown again under
  the new grant set, because an approval binds to one action and one grant
  set.

An approval of a host tool carries one more line, `Tier`, with the tier
in force and any best-effort clause the administrator accepted, so the
confinement the tool runs under is on the screen with the command.

Without a terminal the prompt is not shown: the command prints the pending
item and exits 0; `approve`, `reject` and `answer` continue the task from
another invocation.

## Signals

Ctrl-C (and SIGTERM on Unix) once asks for a pause at the next step
boundary; a step that does not reach its boundary within LIM-O1 has its
execution cancelled first. A second signal cancels the task. The process
exits after the state is persisted.

## JSON output

`--json` prints one value on standard output. Field names are part of the
contract (REQ-CLI-004):

| Command | Value |
|---|---|
| `run`, `task new|show|resume|pause|cancel`, `approve`, `reject`, `answer` | `{task, pending_approval, pending_question, plan, grants}`: the task record with its `state` (`created`, `running`, `waiting_user` + `wait_kind`, `paused`, `halted` + `halt_reason`, `recovering`, `completed` + `completion`, `failed` + `reason`, `cancelled`), `counters`, `budgets`, `versions` (the policy hash, `decision_schema_version`, `event_schema_version`, `action_schema_version`, the tool-definition hashes, `runtime_version`, the backend, the imported tools), `scope`, `profile` |
| `task list` | `[{task_id, goal, state, created_at, updated_at}]` |
| `grant`, `revoke` | the grant: `{grant_id, task_id, kind, issued_by, issued_at, expires_at, status}` |
| `events` | `[{position, record: {event_id, at, actor, …, task_id, step_id, action_id, policy_hash, event, …}}]` |
| `metrics` | `{tasks_by_state, events_by_name, rejections_by_code, denials_by_reason, denials_by_grant_kind, halts_by_reason, approvals_by_decision, actions_by_status, context: {measured_steps, context_chars_avg, prefix_ratio, compression_ratio, over_budget_steps, compressed_steps, dropped_steps, model_ms_avg, prompt_tokens, completion_tokens, cached_tokens, chars_per_prompt_token}}` |
| `status` | `{store: {state: free \| in_use \| recovery_pending \| absent \| lock_untested, holder?: {pid, since}}, version, backend: {kind, model?, locality, reachable, model_listed?, decision_transport (tool_calls \| envelope; null while unresolved), transport_reason?, parallel_tool_calls?, structured_output? (envelope transport only), fell_back, detail?}, tier, clauses, unavailable_clauses, running, pending, data_dir, data_dir_in_temporary_directory, config, engine_version, interpreters, servers: [{name, state: running | unavailable | not_started, detail?, revision?, server_name?, server_version?, instructions?, tools, unavailable: [[tool, reason]], undeclared, declared? (only when `not_started`), list_changed}], host_tools: [{name, command, network, unavailable?}], read_set, dependency_step_read_set_extra, recovered (always empty: `status` recovers nothing)}`; under the `tool_calls` transport an MCP tool the model cannot see for its function name (invalid, or equal to another tool's) is listed in its server's `unavailable` with the reason `function_name_invalid` / `function_name_collision`, not in `tools`; `clauses` values carry `primitive` when enforced or best effort; `clauses` maps `filesystem`, `network`, `environment`, `termination`, `privilege`, `memory`, `count` to `{state: enforced \| best_effort \| unavailable, reason?}` |
| `interpreters list` | `[{language, sha256, module, installed, build}]` |
| `interpreters install` | `{language, module, sha256}` |
| `task show` (and `run`, `task new`, …) also carry | `snapshots: [{snapshot_id, reason, manifest_hash, bytes, files, created_at, action_id?, project_root?}]`, `promotions: [{promotion_id, backup_snapshot, project_root, files, applied_at, reversed_at?}]`, `footprint: {workspace_bytes, snapshots, object_bytes_exclusive, object_bytes_shared, blob_bytes, promotions}` |
| `task list` | `[{task_id, goal, state, created_at, updated_at, footprint}]` |
| `status --storage` | `{path, filesystem: {name?, reflink: yes \| no \| unknown}, bytes: {working_trees, snapshot_objects_exclusive, snapshot_objects_shared, blobs, interpreters, database, temporary, total}, tasks: [{task_id, state, goal, created_at, updated_at, age_seconds, working_tree, snapshot_objects_exclusive, blobs, total}], unreadable_manifests}`; `tasks` is every task, the one that frees most first; `age_seconds` counts from `updated_at` |
| `task prune` | `{tasks: [{task_id, state, age_seconds, footprint, kept?}], total_bytes, confirmed, audit, deleted: [{task_id, footprint}], freed_bytes, failed?: {task_id, error}, untouched}`; `kept` is `promotion reversible`; when it did not finish the same object is printed and the exit code is 1 |
| `rollback` | the snapshot record, or the promotion record when reversing |
| `promote` | `{plan: {task_id, base_snapshot, current_hash, changes: [{path, kind, bytes}], excluded, text_diffs}, applied, promotion?}`; a conflict is the error `conflict` with `detail.paths` |
| `task delete` | `{footprint, deleted, audit?}` |
| `toolchain revoke --all` | `{removed: [{path, source: runtime \| user, found_by: record \| scan}], failed: [{path, reason}], profile_deleted, record_deleted}`; on a partial failure the same object is printed and the exit code is 1 |
| `check` | `{ok, config_path, data_dir, data_dir_in_temporary_directory, config, external_tools, undeclared_external_tools, host_tools}` |
| any error | `{"error": {"kind", "message", "detail"}}` with kind from `invalid_request`, `not_found`, `state`, `denied`, `conflict`, `unavailable`, `internal` |
