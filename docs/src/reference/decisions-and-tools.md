# Decisions, tools and observations

This chapter describes what the model sees and returns. The
normative text is the external design: [§6 Steps and
decisions](../maintainers/external-design.md#6-steps-and-decisions), [§11
Built-in tools](../maintainers/external-design.md#11-built-in-tools-req-tool-003)
and [§8 Observations](../maintainers/external-design.md#8-observations-req-ctx-006-req-ctx-009).

## One decision per step

Every step sends the model one request and expects exactly one JSON object
back. Nothing before or after it, no repair, no second chance within the
step: anything else is a `malformed` rejection that reaches the model as
feedback in the next step.

| `decision` | Fields | Effect |
|---|---|---|
| `act` | `tool`, `arguments`, `reason?` | Proposes one action; the runtime validates and may ask the user |
| `plan` | `steps` (strings) | Records a plan version; nothing runs |
| `ask_user` | `question` | The task waits for the user's answer |
| `complete` | `summary` | Ends the task (`asserted`), or runs the completion check first (`verified`) |
| `give_up` | `reason` | Ends the task `failed` |

`reason` is shown to the user on the approval screen, marked untrusted. It
never changes what the runtime does.

## Validation

An `act` goes through twelve stages in a fixed order: task state, parse,
schema, tool visibility, argument shape, path canonicalization,
authorization, bounds, effect derivation, budget reservation, approval,
execution. A failure is one of eight codes with a fixed hint that says what
the model may change:

`malformed`, `schema`, `unknown_tool`, `invalid_argument`, `denied`,
`state`, `budget`, `approval_rejected`.

A `denied` rejection names its reason (`path_outside_workspace`,
`path_excluded`, `grant_missing`, `destination_not_granted`,
`effect_not_allowed`, `promote_not_allowed`, `argument_not_allowed`) and
its hint follows the reason. Authorization comes before any filesystem
access: a path outside the workspace is `denied` with the same bytes
whether it exists or not.

## Tools

All paths are relative to the workspace root.

| Tool | Arguments | Result | Effect |
|---|---|---|---|
| `read_file` | `path`, `start_line?`, `max_chars?` | `content`, `truncated`, `total_chars`, `line_range` | `workspace_read` |
| `write_file` | `path`, `content` | `bytes_written`, `created`, `parents_created` | `workspace_write` |
| `replace_in_file` | `path`, `find`, `replace`, `occurrences?` | `replaced` | `workspace_write` |
| `list_dir` | `path`, `depth?` | `entries[]{path, type, size}`, `truncated` | `workspace_read` |
| `search_files` | `pattern`, `glob?`, `max_results?` | `matches[]{path, line, text}`, `truncated` | `workspace_read` |
| `delete_path` | `path`, `recursive?` | `deleted` | `workspace_write` |
| `run_script` | `language`, `source`, `stdin?`, `args?`, `mode?` | `termination`, `exit_code?`, stdout and stderr excerpts | by `mode` |
| `run_wasm` | `module`, `args?`, `stdin?`, `mode?` | `termination`, `exit_code?`, stdout and stderr excerpts | by `mode` |
| `run_host_tool` | `tool` (a declared host tool the task was granted), `args?` | `termination`, `exit_code?`, stdout and stderr excerpts | as declared (`workspace_write`; `external_read` for the dependency step) |
| `<server>.<tool>` | as imported from the MCP server | as the server's output schema says | as the administrator declared |

A tool is visible to the model only under a grant: a `tool` grant for a
built-in or external tool, a `host_tool` grant for a declared host tool
(which shows `run_host_tool` for that tool, at tier `full` only). The
shipped `read_only` profile grants the three readers, `workspace` and
`project` grant the eight built-in tools; external and host tools need the
administrator's declaration and a grant. See [External
tools](./external-tools.md) and [Host tools](./host-tools.md).

## The sandbox

`run_script` and `run_wasm` run in `mimlys-worker`, a separate process, inside
Wasmtime with three guards that each end an execution on their own:

| Guard | Limit | Termination |
|---|---|---|
| Fuel | `[sandbox] fuel` (LIM-S1) | `fuel_exhausted` |
| Deadline | `[sandbox] deadline_ms` (LIM-S2) | `deadline` |
| Memory | `[sandbox] memory_mib` (LIM-S3) | `memory_limit` |

A stream over `[sandbox] output_bytes` (LIM-S4) ends the execution
`output_limit`; a program's own end is `exited` with its `exit_code`; a trap
is `trapped` with its name; a cancel is `cancelled`; a sandbox or worker
failure is `host_fault` with `invalid_module`, `unavailable` or `worker_died`.
A non-zero exit is a `tool_error` the model sees as feedback.

The program sees exactly `/workspace` (the task's tree, read-only when
`mode = read_only`), an empty `/tmp` discarded afterwards, its `args` and
`stdin`, no environment, no network, clocks rounded to a millisecond. The
`language` of `run_script` is one of the configured interpreters; today that
is `javascript` (QuickJS-NG, which reads stdin and args and prints; it has no
file modules, the file tools are for files). `run_wasm` runs a wasip1 module
from the workspace of at most LIM-S5 bytes.

## What the model sees

The request is built in a fixed order: a short system text, the tool
definitions, a capability summary generated from the grants, the goal, the
current plan, a compressed history of older steps, the recent steps in full
(each decision, then its observation or its rejection as a labelled
segment), and the decision request. The first four parts are byte-identical
from step to step; the whole request stays within the context budget by
rules alone (excerpts, repeated lines collapsed, old steps summarized, then
dropped in batches), and grants, halts, rejections and unresolved failures
are never dropped. Every piece of data has a provenance label:

```text
<<observation tool="read_file" status="succeeded">>
{"status":"succeeded","payload":{"content":"…"},"notices":[]}
<</observation>>
```

The segment body is JSON, so a tool result or a user answer can never
forge a label. Control characters, zero-width and bidirectional marks are
removed from external text with a `characters_removed` notice.

## Observations

An observation is the runtime's envelope around a tool's result:
`status` (`succeeded`, `failed`, `cancelled`, `unknown`), `cause` when
failed (`tool_error`, `timeout`, `resource_exhausted`, `sandbox_fault`,
`tool_unavailable`, `output_invalid`), `rolled_back`, `payload`, excerpts
of `stdout` and `stderr`, `notices` and `metrics`. A failed action is
retried by the runtime only when the cause and the effect allow it
(a timed-out idempotent read, for instance); otherwise the failure is fed
back to the model and counts toward the failure budget.

## Snapshots and rollback

Every action with a write effect (`write_file`, `replace_in_file`,
`delete_path`, `run_script` and `run_wasm` in `read_write` mode) is a
transaction over the workspace: a snapshot is taken before it and, when it
succeeds, after it. When it fails, the workspace is restored to the
pre-execution snapshot before the model sees the observation, which carries
`rolled_back: true` and the notice `rolled_back`; when it is cancelled, the
same happens unless the profile sets `on_cancel = "keep"`. A write found
running after a crash is restored at the next start and the task halts
`recovered`.

A snapshot is a manifest (every file hashed, every time) plus the files'
contents stored once by hash under the data directory. Two consecutive
snapshots of an unchanged tree share one record. Symbolic links are stored
as links and never followed; a link whose target leaves the workspace is
never recreated.

## Budgets and halts

A task halts, with a reason, when it reaches its step budget, three
rejections in a row, five failures in a row, the same decision three
times, or its deadline. `mimlys task resume` continues it; a step or
deadline halt takes an extension within the administrator's ceiling.
