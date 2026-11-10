# Command line

`mimlys` embeds the runtime: one process per command, the data directory
locked while it runs, your operating-system user as the identity recorded
on every decision. Steps run only inside the process that drives the task,
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
| `task delete <task> [--audit] [--yes]` | Remove a terminal task's workspace, snapshots and outputs after showing what goes; the audit stays unless `--audit` |
| `task new <goal> --from-task T [--from-snapshot S]` | Fork: a new task from another task's snapshot, with its grants and history |
| `approve <task>` | Approve the pending action; it runs at once |
| `reject <task> [--note TEXT] [--halt]` | Reject the pending action; the task continues or halts |
| `answer <task> <text>` | Answer the pending question |
| `grant <task> tool NAME` · `host_tool NAME` · `network DOMAIN[:PORT]` | Issue a grant within the ceiling (`KIND:SCOPE` works too, as on `run --grant`); `host_read` and `host_write` are refused as reserved, and `tool run_host_tool` points at `host_tool` |
| `revoke <task> <grant>` | Revoke a grant at once |
| `events [<task>] [--since N] [--follow]` | Events in order |
| `metrics [--task T]` | Derived counts |
| `status` | Version, data directory, backend (kind, model, locality, reachable, decision transport with the engine's reason when `auto` chose the envelope, structured-output mode of the envelope transport), tier, each clause by letter and word with its state and primitive, declared host tools, configured servers with their tools, running and pending tasks |
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
| `run`, `task new|show|resume|pause|cancel`, `approve`, `reject`, `answer` | `{task, pending_approval, pending_question, plan, grants}`: the task record with its `state` (`created`, `running`, `waiting_user` + `wait_kind`, `paused`, `halted` + `halt_reason`, `recovering`, `completed` + `completion`, `failed` + `reason`, `cancelled`), `counters`, `budgets`, `versions`, `scope`, `profile` |
| `task list` | `[{task_id, goal, state, created_at, updated_at}]` |
| `grant`, `revoke` | the grant: `{grant_id, task_id, kind, issued_by, issued_at, expires_at, status}` |
| `events` | `[{position, record: {event_id, at, actor, …, task_id, step_id, action_id, policy_hash, event, …}}]` |
| `metrics` | `{tasks_by_state, events_by_name, rejections_by_code, halts_by_reason, approvals_by_decision, actions_by_status, context: {measured_steps, context_chars_avg, prefix_ratio, compression_ratio, over_budget_steps, compressed_steps, dropped_steps, model_ms_avg, prompt_tokens, completion_tokens, cached_tokens, chars_per_prompt_token}}` |
| `status` | `{version, backend: {kind, model?, locality, reachable, model_listed?, decision_transport (tool_calls \| envelope; null while unresolved), transport_reason?, parallel_tool_calls?, structured_output? (envelope transport only), fell_back, detail?}, tier, clauses, unavailable_clauses, running, pending, data_dir, data_dir_in_temporary_directory, config, engine_version, interpreters, servers: [{name, state: running | unavailable, detail?, revision?, server_name?, server_version?, instructions?, tools, unavailable: [[tool, reason]], undeclared, list_changed}], host_tools: [{name, command, network, unavailable?}], read_set, recovered}`; `clauses` values carry `primitive` when enforced or best effort; `clauses` maps `filesystem`, `network`, `environment`, `termination`, `privilege`, `memory`, `count` to `{state: enforced \| best_effort \| unavailable, reason?}` |
| `interpreters list` | `[{language, sha256, module, installed, build}]` |
| `interpreters install` | `{language, module, sha256}` |
| `task show` (and `run`, `task new`, …) also carry | `snapshots: [{snapshot_id, reason, manifest_hash, bytes, files, created_at, action_id?, project_root?}]`, `promotions: [{promotion_id, backup_snapshot, project_root, files, applied_at, reversed_at?}]`, `footprint: {workspace_bytes, snapshots, object_bytes_exclusive, object_bytes_shared, blob_bytes, promotions}` |
| `task list` | `[{task_id, goal, state, created_at, updated_at, footprint}]` |
| `rollback` | the snapshot record, or the promotion record when reversing |
| `promote` | `{plan: {task_id, base_snapshot, current_hash, changes: [{path, kind, bytes}], excluded, text_diffs}, applied, promotion?}`; a conflict is the error `conflict` with `detail.paths` |
| `task delete` | `{footprint, deleted, audit?}` |
| `toolchain revoke --all` | `{removed: [{path, source: runtime \| user, found_by: record \| scan}], failed: [{path, reason}], profile_deleted, record_deleted}`; on a partial failure the same object is printed and the exit code is 1 |
| `check` | `{ok, config_path, data_dir, data_dir_in_temporary_directory, config, external_tools, undeclared_external_tools, host_tools}` |
| any error | `{"error": {"kind", "message", "detail"}}` with kind from `invalid_request`, `not_found`, `state`, `denied`, `conflict`, `unavailable`, `internal` |
