# External tools: rypsok

mimlys has no network code of its own. Searching and reading the web go
through an external tool server over the Model Context Protocol, and the
first one is [rypsok](https://github.com/nabbisen/rypsok): one tool call,
several engines, one clean answer. This chapter sets it up.

## Install and declare

```sh
cargo install rypsok        # 0.7.0 or later
rypsok serve                # answers on stdin and stdout; Ctrl-C to stop
```

```toml
[servers.rypsok]
command = "rypsok"
args = ["serve"]
env_allow = ["PATH"]

[servers.rypsok.tools.web_search]
effect = "external_read"
idempotent = true

[servers.rypsok.tools.web_fetch]
effect = "external_read"
idempotent = true
destination_arguments = ["url"]

[policy.profiles.research]
tools = ["read_file", "list_dir", "search_files", "rypsok.web_search", "rypsok.web_fetch"]
```

mimlys starts `rypsok serve` as a child process the first time a task needs
it, with only the environment you listed. A keyed engine takes its key by
reference: `env_secrets = { BRAVE_API_KEY = "env:BRAVE_API_KEY" }` puts the
variable in the child's environment and redacts its value from every
observation and log line.

The two declarations are what admit the tools; without them a tool the
server lists is still imported, but as `external_write`, not idempotent,
and approval is required for every call. `engine_status` is listed by
rypsok and imported, but never shown to the model unless you grant it: it
is for operators.

## A first search

```sh
mimlys run "what does the Rust 2024 edition change about `impl Trait`? cite two pages" \
  --profile research --grant network:doc.rust-lang.org --grant network:blog.rust-lang.org
```

`web_search` needs no grant beyond the tool: the query leaves the host to
the engines rypsok configured. `web_fetch` names a destination, and the
runtime checks it against the task's `network` grants before anything
leaves the host; a `network` grant can only name a domain the ceiling
allows (`policy.ceiling.network_domains`). A page on another host is refused as `denied` without a
call; the model is told so and may ask you for the grant.

What the model receives is rypsok's response as data: `outcome` (`ok`,
`partial`, `empty`), `notices`, and `results` whose `title`, `url` and
`snippet` are external text, bounded and cleaned twice (once by rypsok,
once by mimlys), labelled as an observation and never as instructions.

## Reading a response that failed

rypsok answers an error with one object: `kind`, `message`, `detail`.
mimlys records it as a failed action with the cause `tool_error` and the
kind in the detail, and the model sees the whole object:

| Kind | What it means | What happens next |
|---|---|---|
| `denied` | the query carried a credential, or the fetch policy refused the URL | the model should not retry |
| `redirected` | the page moved to another host; `detail.location` says where | the model may propose that URL; the runtime checks it against the grants like any other |
| `unavailable` | engines down, rate limit, deadline; `retry_after_ms` when known | a retry later is reasonable |
| `invalid_request` | a parameter was out of range; the message names it | the model corrects the call |
| `unsupported_content`, `too_large` | not a page it can read | choose another page |

## What `status` shows

```text
server          rypsok (running); rypsok 0.7.0; revision 2026-07-28; tools: rypsok.web_fetch, rypsok.web_search
  unavailable   rypsok.odd: schema_unsupported:x:anyOf
```

`status` starts each configured server (within its startup deadline) and
lists its tools the way a task would import them. Every server appears
with its state, what it reported about itself, the tools imported, the
tools refused with the reason, declared tools it does not list, and
whether its tool list changed since the last task (a new task pins the new
definitions; running tasks keep the old ones and are told when a tool went
away). A server that could not be started shows `unavailable` with the
reason.

## What is pinned, and why

At task creation mimlys lists the server's tools, cleans and bounds every
description, converts every input schema into its own flat parameter
model, keeps the output schema for checking results, and records a hash of
each definition on the task (`tool.pinned`). The server's annotations
(`readOnlyHint` and friends) are recorded and never trusted: the effect of
a tool is what you declared. If the server changes a definition while a
task runs, the tool is unpinned once (`tool.unpinned`), hidden from the
model, and named to it as unavailable; nothing changes meaning silently.
