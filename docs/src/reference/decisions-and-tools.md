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
`scheme_not_https` (only `https` addresses are admitted), `destination_not_canonical`
(the address is not written the way it parses, or holds user information; the
rejection's `value` is the form to write instead), `promote_not_allowed`,
`argument_not_allowed`, `arguments_do_not_fit`) and
its hint follows the reason. `arguments_do_not_fit` means that the names of a
tool's arguments are too long to put to you for approval, so the action is
refused before it is recorded; the hint tells the model to use another tool or
ask you. Authorization comes before any filesystem
access: a path outside the workspace is `denied` with the same bytes
whether it exists or not.

What the model reads of a rejection is its code, its own hint and its detail,
`{"code", "hint", "detail"}`, where the detail has only the fixed keys of the
code (`problem` and `limit` for `malformed`; `path`, `expected` and `allowed`
for `schema`; `tool` and `visible_tools` for `unknown_tool`; `path`, `min`,
`max` and `allowed` for `invalid_argument`; `reason`, `grant_kind` and, for
`argument_not_allowed`, `argument` with `value`, or `max`, and for
`destination_not_canonical` `value`, for `denied`;
`reason` for `state`; `budget` for `budget`), every string cut to 200
characters. A hint names only keys that its segment carries. The rejected
decision is not repeated, and the detail holds nothing from the file system
or the policy. The budgets of a `budget` rejection are the deadline and the
workspace's size limit; the step budget halts at the step boundary and the
retries of one action are bounded by `executions_per_action`. The detail is
stored with the step (since version 3 of the store's schema, which is at 4 in
0.10), so the same decision
gets the same segment after a restart; a step stored by an older release has
the code and its hint only. `mimlys events` shows the `reason` (and
`grant_kind` of a `denied`) in `decision.rejected`, and `mimlys metrics`
counts denials by reason and by grant kind.

## Tools

All paths are relative to the workspace root.

| Tool | Arguments | Result | Effect |
|---|---|---|---|
| `read_file` | `path`, `start_line?`, `max_chars?` (at most 8 000) | `content`, `truncated`, `total_chars`, `line_range` | `workspace_read` |
| `write_file` | `path`, `content` | `bytes_written`, `created`, `parents_created` | `workspace_write` |
| `replace_in_file` | `path`, `find`, `replace`, `occurrences?` | `replaced` | `workspace_write` |
| `list_dir` | `path`, `depth?` | `entries[]{path, type, size}`, `truncated` | `workspace_read` |
| `search_files` | `pattern`, `glob?`, `max_results?` | `matches[]{path, line, text}`, `truncated` | `workspace_read` |
| `delete_path` | `path`, `recursive?` | `deleted` | `workspace_write` |
| `run_script` | `language`, `source`, `stdin?`, `args?`, `mode?` | `termination`, `exit_code?`, stdout and stderr excerpts | by `mode` |
| `run_wasm` | `module`, `args?`, `stdin?`, `mode?` | `termination`, `exit_code?`, stdout and stderr excerpts | by `mode` |
| `run_host_tool` | `tool` (a declared host tool the task was granted), `args?` | `termination`, `exit_code?`, stdout and stderr excerpts | as declared (`workspace_write`; `external_read` for the dependency step) |
| `<server>.<tool>` | as imported from the MCP server | as the server's output schema says | as the administrator declared |

The built-in file tools look at a path before they use it, open it without
blocking and look at what they opened, so a named pipe, a socket or a device
never holds a tool, or the process, for good. Anything that is not a regular
file is refused with `not_a_file` (and a hint); a directory given to
`read_file`, `write_file` or `replace_in_file` is `is_directory`; `read_file`
reads at most 8 MiB (LIM-W3) and refuses a larger file with `too_large`, since it
does not truncate before it checks. `search_files` skips an entry that is not a
regular file and counts it with the entries it could not read.

What the model reads of any tool's result, the built-in tools' included, is at
most 8 000 characters of JSON (LIM-I19). `read_file` takes a `max_chars` of at
most 8 000 (4 000 when it is not given) and returns a prefix of the text that
fits as the model reads it: a quote or a newline is two characters in JSON, so
a text full of them gives less than `max_chars`. `truncated` is then `true`,
`line_range` is what was returned and `total_chars` is the whole, so the next
read starts at the line after. `list_dir` and `search_files` drop whole entries
from the end of the list until it fits and say so with `truncated`; a path is
never cut, so none names a file that does not exist. The runtime applies the
same bound last to every result, whatever made it, and says so once, as
`content_truncated`.

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

`[sandbox] output_bytes` (LIM-S4) is 1 MiB by default and at most 16 MiB (at
least 64 KiB; the maximum was 64 MiB before 0.12, and a configuration above 16 MiB no longer
loads): the bytes of a stream are held several times over while an execution is recorded, and at
16 MiB, with both streams at the bound, one execution peaks at about 170 MB in the runtime
(about 310 MB for bytes that are not text) and about 115 MB in the worker (Linux, measured).
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

The `mode` also sets the action's effect: `read_only` is `workspace_read` (the
workspace is opened read-only, so nothing in it can change: the action takes no
snapshot and has nothing to roll back), `read_write`, the default, is
`workspace_write` (a snapshot before, one after when it succeeds, and a
rollback when it fails or is cancelled). Whether the profile lists the tool in
`approval_tools` decides approval in both modes.

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
forge a label. Control characters, zero-width and bidirectional marks, and the
other characters that draw nothing (the soft hyphen, variation selectors, the
fillers of some scripts, tag characters, invisible format characters) are
removed from external text with a `characters_removed` notice. Symbols that are
drawn stay, even those whose name sounds blank (the braille blank, the object
replacement character).

## Observations

An observation is the runtime's envelope around a tool's result:
`status` (`succeeded`, `failed`, `cancelled`, `unknown`), `cause` when
failed (`tool_error`, `timeout`, `resource_exhausted`, `sandbox_fault`,
`tool_unavailable`, `output_invalid`), `rolled_back`, `payload`, excerpts
of `stdout` and `stderr`, `notices` and `metrics`. A failed action is
retried by the runtime only when the cause and the effect allow it
(a timed-out idempotent read, for instance); otherwise the failure is fed
back to the model and counts toward the failure budget.

The model reads a cut of a long result and of a long output (an excerpt of
4 000 characters, 8 000 of a result); mimlys keeps the whole of each for you, and
`mimlys task output` shows it (see [How a task runs](../how-a-task-runs.md#what-persists-and-how-to-continue)).

## Snapshots and rollback

Every action with a write effect (`write_file`, `replace_in_file`,
`delete_path`, `run_script` and `run_wasm` in `read_write` mode) is a
transaction over the workspace: a snapshot is taken before it and, when it
succeeds, after it. When it fails, the workspace is restored to the
pre-execution snapshot before the model sees the observation, which carries
`rolled_back: true` and the notice `rolled_back`; when it is cancelled, the
same happens unless the profile sets `on_cancel = "keep"`. A write found
running after a crash is restored at the next start and the task halts
`recovered`. A task found `recovering` (the process ended during a long
restore) is recovered again from the start, and every step of the recovery can
be repeated. If the restore after a cancelled write fails, the failure is
logged and the task halts `unknown_outcome`, not silently paused on a
half-restored tree. Every halt that follows a rollback that failed says so: the
line ends `the workspace was not rolled back`, the halt's event
(`task.state_changed`) carries `rollback_failed: true` and `task show --json`
has `rollback_failed: true`. (`unknown_outcome` without that clause is the halt
of an action that cannot be undone.)

If the definitions of the tools granted to a task do not fit the 8 000
characters the model is shown (LIM-I1), which a grant is refused for when it
would make them so, the task halts once as `definitions_too_large` and says to revoke a tool grant, then resume;
it is never left `running` with every step failing.

If a part of the task's workspace cannot be read (an action made a directory
unreadable, or the tree could not be walked), the task halts
`workspace_unreadable`: the halt names the path and, when the cause is a
refused permission, says to restore its permissions, then resume (`task show`,
`resume`, `run`). A rollback of such a tree names the path and changes nothing.
When the cause is not a refused permission (an entry that vanished during the
walk, a failing device), the error names the path and the kind of the failure,
`part of the workspace could not be read: <path> (<kind>)`, and the task halts
the same way; the event `workspace.unreadable` then carries the `kind`, and the
lines of the halt give no advice about permissions, which would not be the
remedy.

A snapshot is a manifest (every file hashed, every time) plus the files'
contents stored once by hash under the data directory. Two consecutive
snapshots of an unchanged tree share one record. Symbolic links are stored
as links and never followed; a link whose target leaves the workspace is
never recreated. The target is judged as the system would resolve it, through
the workspace's own links: `x -> ..` in a directory and `y -> dir/x/..` leave
it, though `dir/x/..` is lexically inside; a chain is followed through at most
40 links (a loop is refused at that bound). A link's target is kept exactly as the system holds it (a
backslash in it stays a backslash on Linux and macOS).

The manifest records each file's permission bits (masked to `0777`; none on
Windows), and a restore gives them back exactly: a `0600` file comes back
`0600`. A manifest made before 0.11 records none and keeps its hash; its files
are restored as before, `0755` when executable and `0644` when not. A mode that
cannot be set is an error, not ignored.

A restore verifies, then replaces. Every file it needs to write is checked
against its hash and written into a staging directory in the root of the tree
(`.mimlys-stage-<pid>-<n>`) before anything is removed; a missing or damaged
object, or a full disk, stops the restore with the tree as it was. Then what
the snapshot does not hold is removed, the staged files are renamed into place,
and links are written last. The same holds for the reversal of a promotion
(it stages in the root of your project). A staging directory that a crash left
is removed by the next restore, or the next reversal. What the snapshot does
not hold includes a named pipe, a socket or a device that an action made in the
tree: a rollback removes it, and no walk of the tree ever opens one. On Windows
a link to a directory is restored as a directory link.

The walk that reads a tree (the snapshot, the restore, the size check, the copy
at the start of a task) is one and does not nest: a tree of any depth is read in
the same stack. A name that is not valid UTF-8 cannot be recorded, so it
refuses the creation of the workspace (and stops a snapshot), naming the path
with the name written out byte by byte (`src/caf\xE9.txt`); a part of your
project that cannot be read refuses it too, naming the path and, unless it is a
permission, the kind of the failure.

## Budgets and halts

A task halts, with a reason, when it reaches its step budget, three
rejections in a row, five failures in a row, the same decision three
times while the workspace's content is unchanged (the manifest of the task's
latest snapshot), or its deadline. `mimlys task resume` continues it; a step or
deadline halt takes an extension within the administrator's ceiling.
