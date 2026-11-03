# Configuration

One TOML file, found in this order: `--config FILE`, the `MIMLYS_CONFIG`
environment variable, then `config.toml` in the platform's configuration
directory (`~/.config/mimlys`, `%APPDATA%\mimlys`, `~/Library/Application
Support/mimlys`). Without a file, mimlys runs with the built-in defaults:
the fake backend and the `workspace` profile.

Rules (REQ-CFG-001 … 005):

- `config_version = 1` is required.
- An unknown key anywhere stops the process at startup with exit code 1 and
  names the key.
- Secrets are references, `env:NAME` or `file:PATH`; an inline value is refused.
- Every default is the value of the [limits register](../maintainers/limits-and-defaults.md);
  a value outside its range is refused with the range in the message.
- Changes apply at the next start. `mimlys check` validates the file and
  prints it resolved.

## Reference

```toml
config_version = 1

[runtime]
data_dir = "/home/me/.local/share/mimlys"   # platform default when absent

[backend]
kind = "fake"                   # "fake" (scripted) or "openai_compatible" (a local model)
script = "decisions.json"       # fake backend: the decisions to return, in order
locality = "local"              # "remote" needs policy.remote_backend_allowed and a profile with external = true
structured_output = "auto"      # auto | schema | none; auto is resolved at the probe and shown by `status`
timeout_ms = 600000             # LIM-M1, 10000–3600000
max_tokens = 4096               # LIM-M2, 256–65536
context_chars = 32000           # LIM-C1, 4000–1000000; `mimlys metrics` shows chars per prompt token
temperature = 0.0               # 0.0–2.0; 0 for the most repeatable decisions the engine allows
# url = "http://127.0.0.1:8080/v1"     openai_compatible only
# model = "…"
# api_key = "env:MIMLYS_BACKEND_KEY"   by reference only; at least 8 characters, redacted everywhere

[policy]
default_profile = "workspace"
remote_backend_allowed = false
# accept_best_effort = ["K"]    accept a best-effort mandatory clause (macOS's tree termination); shown in status and approvals

[policy.ceiling]                # what any task may be granted at most
tools = ["read_file", "write_file", "replace_in_file", "list_dir", "search_files", "delete_path", "run_script", "run_wasm"]
host_tools = []                 # a [host_tools.<name>] declaration admits its name here by itself
network_domains = []
# host_read_roots = []          reserved: no tool reads host paths through host_read grants yet; refused when non-empty
# host_write_roots = []         reserved likewise
max_steps = 200                 # LIM-T1 range

[policy.profiles.workspace]     # overlays on a shipped profile; a new profile needs `tools`
tools = ["read_file", "write_file", "replace_in_file", "list_dir", "search_files", "delete_path", "run_script", "run_wasm"]
host_tools = []                 # declared host tools granted at creation (`host_tool` grants); never `run_host_tool` in `tools`
steps = 40                      # LIM-T1
consecutive_rejections = 3      # LIM-T2
consecutive_failures = 5        # LIM-T6
identical_decisions = 3         # LIM-T4
executions_per_action = 2       # LIM-T3
approval_tools = []             # tools that always need approval in this profile; `run_host_tool` covers every host tool
on_reject = "continue"          # continue | halt
on_cancel = "rollback"          # rollback | keep (M3)
promote = false                 # the `project` profile sets true (M3)
external = false                # may the task communicate outside the host (a remote backend); false in every shipped profile

[policy.exclusions]
materialize = [".git/", ".hg/", ".svn/", "target/", "node_modules/"]            # LIM-W4
secrets = [".env", ".env.*", "*.pem", "*.key", "id_*", ".ssh/", ".aws/", ".gnupg/"]  # LIM-W5

[policy.recovery]
auto_resume = false             # return a cleanly recovered task to running

[host_tools.cargo_test]         # a declared host tool; see Host tools
command = "~/.cargo/bin/cargo"  # absolute, or a name resolved in read_paths/*/bin, never PATH
args = ["test", "--offline"]    # fixed; host tools run offline
allowed_args = ["--lib"]        # literals the model may add
arg_pattern = "[A-Za-z0-9_:]+"  # what any other added argument must match (a test name)
max_args = 2
effect = "workspace_write"      # the tool writes only inside the workspace (reversible); `external_read` exactly when network = true; host writes are a later release
timeout_ms = 600000             # LIM-H1, 10000–3600000
read_paths = ["~/.cargo/bin", "~/.rustup"]   # the toolchain; the only paths outside the system's the tool can read; a directory holding credentials (`~/.cargo` holds `credentials.toml`) is refused
env = { RUSTUP_HOME = "~/.rustup", RUSTUP_TOOLCHAIN = "stable", PATH = "~/.cargo/bin:/usr/bin:/bin" }   # PATH: the built one adds <read path>/bin, so name ~/.cargo/bin here
# env_allow = ["RUSTFLAGS"]     copied from mimlys's environment
# network = true                the dependency step: the effect must then be `external_read` (and only then); approval where the profile asks; destinations unbounded

[servers.rypsok]                # an MCP server as a child process; see External tools
command = "rypsok"
args = ["serve"]
env_allow = ["PATH"]            # copied from mimlys's environment; nothing else is inherited
env_secrets = { BRAVE_API_KEY = "env:BRAVE_API_KEY" }   # by reference, at least 8 characters, redacted everywhere
startup_ms = 15000              # LIM-O6, 1000–120000
# max_call_ms = 11000           # the server's declared maximum call time; no tool deadline falls below it
[servers.rypsok.tools.web_search]   # a declaration admits the tool to the ceiling
effect = "external_read"        # external_read | external_write; an undeclared tool is external_write
idempotent = true
timeout_ms = 10000              # LIM-O5, 1000–600000
[servers.rypsok.tools.web_fetch]
effect = "external_read"
idempotent = true
destination_arguments = ["url"] # checked against the task's network grants before every call
timeout_ms = 15000

[interpreters.javascript]       # optional: the shipped build applies without this section
module = "/path/to/qjs-wasi.wasm"   # default: <data_dir>/interpreters/<sha256>.wasm
sha256 = "42a732a6…"            # must match the file; a mismatch is never loaded

[sandbox]                       # the guards of run_script and run_wasm
fuel = 10000000000              # LIM-S1
deadline_ms = 30000             # LIM-S2
memory_mib = 256                # LIM-S3
output_bytes = 1048576          # LIM-S4

[workspace]
max_bytes = 1073741824          # LIM-W1, 64 MiB – 64 GiB
max_files = 200000              # LIM-W2
# retention_days does not exist: nothing expires; `mimlys task delete` removes a task

[logging]
level = "info"                  # error | warn | info | debug | trace
debug_prompts = false           # true logs prompts and model output, and warns at start
```

## A local model

`kind = "openai_compatible"` talks to any server with the chat-completions
API. Two that run on your own machine:

```sh
# llama.cpp
llama-server -m Qwen2.5-Coder-7B-Instruct-Q4_K_M.gguf --port 8080 --jinja
# Ollama
ollama serve   # then: ollama pull qwen2.5-coder:7b
```

```toml
[backend]
kind = "openai_compatible"
url = "http://127.0.0.1:8080/v1"      # Ollama: http://127.0.0.1:11434/v1
model = "Qwen2.5-Coder-7B-Instruct-Q4_K_M.gguf"   # Ollama: qwen2.5-coder:7b
```

Before a task runs, and for `mimlys status`, mimlys probes the server
within five seconds: whether it answers, whether the model is listed, and
which structured-output mode it accepts. `structured_output = "auto"` tries
the decision schema, then plain JSON mode, then nothing, and `status` shows
the result; `run` says so once when it ended at plain text. In every mode
the runtime parses the model's text strictly: a mode changes how often the
model succeeds, never what the runtime accepts. `schema` on a server that
refuses it is a startup error naming the fix.

One request is sent per step, with the whole context (`context_chars`
characters at most; `mimlys metrics` reports characters per prompt token so
you can set it from the model's window), the completion bound and the
deadline. A transport failure or a server error is retried with the
identical request, then the task halts `backend_unavailable`; resume it
when the server is back. The first four segments of the context are
byte-identical from step to step, so an engine with a prompt cache reuses
them; `metrics` shows the measured prefix ratio and the cached tokens the
server reports.

A backend on another machine is `locality = "remote"`: it needs
`policy.remote_backend_allowed = true` and a profile with `external = true`,
and `status`, the task's versions and the `runtime.started` event all say
so. The key is a reference, never a value; its resolved value is redacted
from observations and logs.

## Host tools

Every `[host_tools.<name>]` section declares a program the model may run
through `run_host_tool`, under the confinement of this machine and never
otherwise; the [Host tools](./host-tools.md) chapter walks through
`cargo test`. The declaration is what admits the name to the ceiling. One
grant does the rest: a task with a `host_tool:<name>` grant sees
`run_host_tool` for that tool (`--grant host_tool:cargo_test`, or
`host_tools = ["cargo_test"]` in a profile for every task created under
it). `run_host_tool` is never written in `tools`; `approval_tools` may name
it to ask you before any host tool runs. A host tool writes only inside the
workspace in this release. `mimlys status` shows the tier and every clause
with the primitive that holds it; `mimlys toolchain list` shows the read set
the worker runs under.

## External tools

Every `[servers.<name>]` section is an MCP server mimlys starts as a child
process when a task first needs it. Its tools appear as `<name>.<tool>`;
the [External tools](./external-tools.md) chapter walks through rypsok.
Declaring a tool under `[servers.<name>.tools.<tool>]` is what admits it to
the ceiling: a profile may then list it, or a task may be granted it.
`mimlys check` prints every admitted external tool with the declaration
that admits it, and warns about a tool named in `policy.ceiling.tools`
without a declaration, because such a tool runs as `external_write`, not
idempotent, with approval required.

## Profiles

The three shipped profiles are `read_only` (the three reading tools),
`workspace` (the eight built-in tools) and `project` (the same, with
promotion); none of them permits external communication (`external =
false`) and none grants a host tool. A profile section with the name of a
shipped profile changes only the keys it names. A new profile must list its
`tools`; every tool and every `approval_tools` entry must be in
`policy.ceiling.tools` (`run_host_tool` in `approval_tools` is the one
exception), and every `host_tools` entry must be a declared host tool.

Grants of the kinds `host_read` and `host_write` are reserved: no tool reads
or writes a host path through them yet, so `mimlys grant` refuses them and
the ceiling roots are refused when non-empty, each with the reason. They
return when a tool carries that effect.

## Interpreters

`run_script` runs a language only when an interpreter module for it is
configured and installed. mimlys ships one pinned build per language it
knows (`javascript`: QuickJS-NG v0.17.0 as a wasip1 module); `mimlys
interpreters install <language>` downloads it, verifies the SHA-256 the
binary carries and stores it under `<data_dir>/interpreters/`. A section
`[interpreters.<language>]` overrides the path or the hash; a language mimlys
does not ship needs both `module` and `sha256` (and `script_name` for the
file the source is written to under `/tmp`). The worker refuses a module
whose hash is not the configured one, and `status` says so.

## Logs

Logs are JSON lines on standard error with timestamp, level and event.
Prompt content, file content and secrets never appear at `info`.
