# External tools: rypsok

mimlys has no network code of its own. Searching and reading the web go
through an external tool server over the Model Context Protocol, and the
first one is [rypsok](https://github.com/nabbisen/rypsok): one tool call,
several engines, one clean answer. This chapter sets it up.

## Install and declare

```sh
cargo install rypsok --locked --version 0.10.0   # the version mimlys is tested with
rypsok serve --help                              # serve answers on stdin and stdout
```

Start rypsok with a configuration file that you own, and keep it next to
mimlys's own. Without one, rypsok runs on its defaults, which are fine to try
but are not yours to rely on. `~/.config/mimlys/rypsok.toml`:

```toml
config_version = 1

[fetch]
# Keep this false (rypsok's default, written down so that it is yours). When it
# is true, rypsok follows a redirect to another host by itself, past mimlys's
# check of the destination against the task's network grants.
follow_cross_host_redirects = false
```

```sh
rypsok check --config ~/.config/mimlys/rypsok.toml
```

`rypsok check` prints the limits the file gives. Two lines matter to mimlys:
`search: ... deadline_ms=2500` and `fetch: ... deadline_ms=10000`. rypsok
documents a search call as lasting at most its deadline plus 0.3 s, and a
fetch call its deadline plus one second (11 s with these values; if you raise
`fetch.deadline_ms`, raise the number below by the same amount). In rypsok
0.9.0 these maxima held by the way each pipeline is built; from 0.10.0 they
are also enforced over the whole call, tested, and printed by `check` as
`tool call maxima in ms: web_search=2800 web_fetch=11000` (mimlys does not read
that text: it is for people; rypsok's letters of 2026-11-11). Either way, mimlys ends every call at its own per-tool
deadline (`timeout_ms`), and never gives a call less time than the server's
`max_call_ms`.

```toml
[servers.rypsok]
command = "rypsok"
args = ["serve", "--config", "/home/you/.config/mimlys/rypsok.toml"]
env_allow = ["PATH"]
max_call_ms = 11000        # the fetch deadline (10 s) plus one second
shutdown_ms = 5000         # rypsok ends within 3 s of its input closing; mimlys waits longer

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
it, with only the environment you listed, and ends it when the runtime exits:
its input is closed, it is given `shutdown_ms` to finish by itself
(rypsok's own graceful shutdown takes up to 3 s, `service.shutdown_ms`), and
what is still running after that is killed with everything it started.
Keep `shutdown_ms` longer than rypsok's.

rypsok checks pages over TLS with the operating system's certificate store
when its environment is only `PATH`. On Linux, `SSL_CERT_FILE` and
`SSL_CERT_DIR`, if you list them in `env_allow`, replace that store; if no
root certificate can be loaded, rypsok does not start and says why (see
"Which certificates the fetches trust" on rypsok's deployment page). A keyed engine takes its key by
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

Only `https` addresses are admitted. rypsok reads an `http` address over
`https` on port 443 by default, so an `http` address would reach a connection
that your grant for port 80 never named; mimlys refuses it as `denied` with the reason `scheme_not_https`, and
the hint tells the model to propose the `https` form of the page.

**A destination is checked as the server will read it.** The grants judge the
address after it is parsed, and the server receives the text the model wrote,
so the two must be one address. An address is admitted only when it is written
the way the parse writes it: a lower-case host, no user information
(`user@host`), no default port written out (`:443`), no spaces, control
characters or backslashes, no short forms of a number (`0x7f.1`, `127.1`,
`2130706433`), and no `.` or `..` in the path. An empty path may be left out
(`https://example.org` and `https://example.org/` are one address). Any other
spelling is refused as `denied` with the reason `destination_not_canonical`,
before the grants are asked, and the rejection's `value` gives the form to
write instead, which the model may propose at once. mimlys never rewrites the
text: the call, its hash and your approval are of what the model wrote. The
same check applies to the address a server gives in a `redirected` result when
the model proposes it.

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
server          rypsok (running); rypsok 0.10.0; revision 2025-11-25; tools: rypsok.web_fetch, rypsok.web_search
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
model, and named to it as unavailable; nothing changes meaning silently. The
hash is compared once more on the very connection the call is sent on, so a
server that was restarted between the check and the call, and now lists the
tool otherwise, is not called: the model is told `definition_changed`. A
change the server announces while it is being listed is kept for the next
check, not lost.

## What mimlys bounds in a server's answers

A server is not trusted to be small, quick or well behaved. These bounds are
fixed (limits LIM-I10 to LIM-I16 and LIM-O6), and a tool that exceeds one is
unavailable, with the reason in `status`, rather than half imported.

- **Every request has a deadline and ends at the stop signal.** The listing of
  a server's tools, each page of it, and the start of the server are bounded
  by the server's `startup_ms` (LIM-O6, 15 000 ms unless you set it); a stop
  signal ends them sooner. The check of a tool's definition just before a call,
  and the wait for a server that died since that check to be started again,
  end at the **first** Ctrl-C, whatever the tool does: nothing has been sent,
  so the action is closed as not begun (`cancelled`, with the notice
  `stopped_before_start`), nothing is unknown and the task pauses, never
  halts as `tool_unavailable`. Nothing waits behind a listing: a call, another
  task's pin check and the shutdown never queue behind one. A listing stops at
  16 pages (LIM-I11) and 256 tools (LIM-I12). When a server cannot be reached
  at a step boundary its tools **stay pinned**, and a call to one is
  `tool_unavailable` with the reason (`list:timeout`, `list:cancelled`,
  `list:too_many_pages`, `list:too_many_tools`), where an older release told the
  model that the definition had changed.
- **A destination is a string.** A parameter you declare in
  `destination_arguments` must be a string in the tool's schema; otherwise the
  tool is unavailable (`destination_not_string:<parameter>`). A call that gives
  an array or an object for a destination is `denied` with
  `destination_not_granted`, so a destination is always checked against the
  `network` grants.
- **Names and values are cleaned and bounded.** A parameter's name (and a
  nested key) is ASCII letters, digits, `_`, `.` or `-`, at most 64 characters
  (LIM-I10); a tool with another name is unavailable (`definition_invalid`,
  naming the parameter by its position), so **a parameter name longer than 64
  characters makes its tool unavailable**. An enumeration's values are cleaned
  and cut like a description, and so are a server's name, version, revision, a
  tool's name and the reason of an unavailable tool before `status` or an
  event shows them.
- **One tool's definition, and an enumeration, are bounded.** The model is
  shown every tool you grant, and all of them together may take 8 000
  characters (LIM-I1). A tool of a server is rendered alone when it is imported,
  and **one that renders to more than 2 000 characters is unavailable**
  (`definition_too_large:<size>><bound>`; eight parameters of eight nested fields
  is far over it), and so is one with an enumeration of **more than 128 values**
  (`too_many_enum_values:<parameter>:<count>><bound>`); these are LIM-I20 and
  LIM-I21. Rypsok's `web_search` renders to
  about 650 characters, and the built-in tools take about 4 200 of the 8 000.
- **A grant cannot make the definitions too large.** `mimlys grant <task> tool
  <name>`, and the tools of a profile when a task is made (with the grants of a
  fork and of the request, judged as one set), are refused as `denied` when the
  definitions of the tools they make visible would pass 8 000 characters; the
  message names the tool that crosses the limit and gives its size, the size of
  the tools granted before it and the total. Nothing is recorded: the task is not
  made, the grant is not issued. Revoke a grant, or give the task a smaller
  profile, and try again. A task made before this check existed and found over
  the limit at a step is halted once as `definitions_too_large`, where an earlier release failed that step, and every
  step after it, with the task left `running`; revoke a tool grant, then resume.
- **An output schema is checked for cost.** A schema over 1 024 nodes
  (LIM-I13), a list of more than 128 entries (LIM-I14), or alternatives that
  cost more than 64 visits per value (LIM-I15) make the tool unavailable. A
  validation of a result has a budget of 100 000 visits (LIM-I16); a result
  that would cost more is `output_invalid` ("the schema was too costly to
  check"), and the validation runs off the executor within the call's deadline.
- **Redaction comes before any cut.** A secret (the value of `env_secrets`) is
  removed from a tool's failure detail, from the keys of an object and from a
  tool's output streams before a long text is cut, so that a secret that
  straddles the cut leaves nothing of itself in the excerpt. A secret's case
  and percent-encoded variants are not covered.
- **What a server writes on standard error is bounded.** The runtime logs it
  line by line at the level `info`, where you read a server's errors. A line is
  kept up to 1 024 bytes: a longer one is logged at once with the cut marked
  (`[... line cut at 1024 bytes]`) and the rest of it, up to its newline, is
  read and thrown away, so a server that writes one line that never ends costs
  one log line and no memory. At most 50 lines are logged in each second; the
  lines past that are counted, and when the second ends one line says how many
  (`N lines dropped`). These are LIM-I17, LIM-I18 and LIM-O18.
- **One message from a server is bounded before it is read.** A server's
  answer is one line of JSON on its standard output, and the runtime counts the
  bytes of a line as they arrive: past 8 MiB (LIM-I22) it does not read the
  message, whole or in part, and ends the connection as it ends when a server
  exits. The call in flight is `host_fault` with `worker_died`, the cause
  `sandbox_fault`, as for a server that died during a call (an effect that
  cannot be undone is then `unknown`, as there); the process is killed with its
  group, and the log says at `warn` which server and the bound. The next call
  starts the server again, and that counts against the restart limit of 3 per
  task like any start after a death. A server that returns more than 8 MiB at
  once, as a file or a page, is better made to return it in parts.
- **A result is bounded as a whole.** What the model reads of a tool's result
  is at most 8 000 characters of JSON in total (LIM-I19), however many strings, keys and levels the server sends
  (the built-in tools' results are bounded the same way, and the runtime bounds every result last, whatever provider made it). The
  result is first cleaned (invisible characters removed, secrets redacted), then
  checked against the tool's output schema, and only then bounded, so a cut is
  never the reason for `output_invalid`: a string that must be 20 000 characters
  long by the schema is cut after the check. The longest strings are cut first,
  all to one length, with the usual `[… N characters omitted …]` marker, so the
  result stays valid JSON and a short string is kept whole; a key longer than 200
  characters is cut. A result that is nothing but structure past the bound (a
  hundred thousand short keys) is replaced by a small object that says how long
  it was. The observation says each kind of change **once**: one
  `characters_removed` with the number of characters removed, one
  `content_truncated` with the number of characters of everything cut, however
  many strings it came from. The whole result, cleaned and within 1 MiB (LIM-S4),
  is kept in the task's record with the observation, as `full_payload`, and is
  never shown to the model. `mimlys task output <task> --result` prints it (and
  without `--result` says whether the model read a cut of the result). An error
  from a tool and a result that is not valid are bounded the same way.
- **Content that is not passed on is said.** When a tool answers without
  `structuredContent`, only its first text block is the result; any other
  content block (an image, a second text) is not passed on, and the
  observation carries a `content_omitted` notice with the number of blocks.
- **A server that does not answer costs one wait in a step.** When the
  listing at a step boundary does not finish within `startup_ms`, a call to that
  server in the same step is `tool_unavailable` with the same reason at once,
  without waiting again; the next boundary asks again.
- **A server's start counts.** The first start of a server is free; every start
  after it made for a task counts against the restart limit of 3 per task, a
  start that fails (the program cannot be run, or does not initialize or list)
  as well as a server that died. Past it the tools are `tool_unavailable` with
  `restart_limit_reached`, and when the last start failed its reason follows
  (`restart_limit_reached (last: list:protocol)`).
- **A server has a session of its own.** On Linux and macOS the server is the
  leader of a new session (`setsid`) and of its own process group, so it has
  no controlling terminal; at the end, what it started is killed with it.
