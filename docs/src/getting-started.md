# Getting started

mimlys 0.7.1, the first release on crates.io (milestones M1 to M6b), is the deterministic core with the sandbox,
the transactional workspace, a local model, external tools and confined host
tools (on Linux with `cargo test` as the demonstration; on Windows the
confinement is verified, but `cargo` itself cannot run yet): a task runs one validated step at a time
against a private copy of your project, with the built-in file tools,
`run_script` (JavaScript) and `run_wasm` in a Wasmtime sandbox inside a
separate worker process, approvals, questions, budgets and durable records.
Every write action is a transaction over the workspace: a snapshot before, a
snapshot after, and a rollback when it fails. Nothing reaches your project
until you promote it. The decisions come from a local model through any
OpenAI-compatible server (llama-server, Ollama), or from the scripted fake
backend that the examples below use. On Linux a declared toolchain command
such as `cargo test` runs under kernel confinement (see
[Host tools](./reference/host-tools.md)).

## Install

```sh
cargo install mimlys
mimlys interpreters install javascript
```

The second command downloads the pinned QuickJS-NG build, verifies its
SHA-256 and stores it under the data directory; `mimlys interpreters list`
shows what is installed. Offline, give the file with `--from FILE`.

The `mimlys` binary embeds the runtime and starts `mimlys-worker` (installed
beside it) for sandbox executions. Data lives in the platform's user data
directory (`~/.local/share/mimlys`, `%LOCALAPPDATA%\mimlys`,
`~/Library/Application Support/mimlys`), or where `runtime.data_dir` points.

## First run

With a local model, point the configuration at it (see
[Configuration](./reference/configuration.md#a-local-model)) and run:

```sh
mimlys run "fix the failing test in this project" --scope .
```

Without one, write a decision script for the fake backend, one decision per
step; everything below works the same way with a model:

```json
[
  {"decision": "plan", "steps": ["read the file", "fix it"]},
  {"decision": "act", "tool": "read_file", "arguments": {"path": "src/lib.rs"}},
  {"decision": "act", "tool": "write_file", "arguments": {"path": "src/lib.rs", "content": "fn a() -> i32 { 1 }\n"}, "reason": "apply the fix"},
  {"decision": "complete", "summary": "done"}
]
```

Point the configuration at it and ask for approval before every write:

```toml
config_version = 1

[backend]
kind = "fake"
script = "/path/to/script.json"

[policy.profiles.workspace]
approval_tools = ["write_file"]
```

Run a task on your project:

```sh
mimlys --config mimlys.toml run "fix the failing test" --scope .
```

mimlys copies the project (without `.git/`, `target/`, `node_modules/`,
secrets and the like) into a private workspace, prints each event as it
happens, and stops at the write with the approval prompt:

```text
Approval required · task 01a1… · step 2
Tool         write_file
Effect       workspace_write · reversible: yes · idempotent: yes
Will         create or overwrite the file "src/lib.rs" in the workspace
Arguments    content = "fn a() -> i32 { 1 }\n" · path = "src/lib.rs" (resolved: "…/workspaces/01a1…/tree/src/lib.rs")
Grants used  tool:write_file
Expires      never
Model's stated reason (untrusted): "apply the fix"
[a]pprove   [r]eject   [h]alt (reject and halt)   [g]rant …
```

Press `a`. The action runs, the task completes, and your project is
untouched: the change is in the workspace under the data directory.

## Bringing the result into your project

```sh
mimlys promote <task>
```

shows the diff between what the task started from and what it left (a
unified diff per text file), refuses when you changed one of the target
files in the meantime (it lists them), and otherwise asks before writing.
The targets are backed up first, so `mimlys rollback <task> --promotion <id>`
puts them back. `mimlys promote <task> --patch` prints a patch and writes
nothing. Nothing but `promote` ever writes to your project.

`mimlys rollback <task>` restores the workspace to its latest snapshot, or
to any snapshot `task show` lists. `mimlys task new "goal" --from-task <id>`
starts a new task from a finished one's latest snapshot, with its grants and
a short history.

## What stays on disk, and how it leaves

Nothing expires. A task owns its workspace, its snapshots and its captured
outputs; `mimlys task list` and `mimlys task show` show each task's
footprint. When you are done:

```sh
mimlys task delete <task>
```

shows what will go (and that any promotion of that task can no longer be
reversed), asks, and removes it. The task's events stay as audit unless you
add `--audit`. Copying the data directory while mimlys is not running is a
complete backup.

## What to look at next

- `mimlys task show <id>` shows the task, its plan, its grants and any pending item.
- `mimlys events <id>` lists every recorded event; `mimlys metrics` counts them and reports the context size, the prefix reuse and the tokens per task.
- `mimlys status` shows the data directory, the backend with what the probe found, the engine, the tier
  and, clause by clause, what this machine's confinement enforces.
- `mimlys check` validates the configuration and prints it resolved.

Without a terminal (a pipe, `--json`), a task that needs you stops with a
message naming the command to run: `mimlys approve <id>`, `mimlys reject
<id>`, `mimlys answer <id> <text>`. The command continues the task in its
own process.

Press Ctrl-C once to pause at the next step boundary; twice to cancel.
`mimlys task resume <id>` continues a paused or halted task; a budget halt
takes `--extend-steps` or `--extend-deadline`.
