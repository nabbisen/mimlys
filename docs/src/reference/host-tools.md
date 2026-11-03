# Host tools: fixing a failing test

A model that fixes a failing test has to run the test. The test runner is
the project's own toolchain, native code that executes project code with
your privileges, so mimlys runs it only under the confinement contract of
this machine, and never otherwise. This chapter sets up `cargo test`.

## The tier line

```sh
mimlys status
```

```text
tier            full
clause F filesystem   enforced (landlock)
clause N network      enforced (seccomp+landlock_net)
clause E environment  enforced (explicit_environment)
clause K termination  enforced (cgroup_kill)
clause P privilege    enforced (no_new_privs+seccomp)
clause M memory       best effort (rlimit_as (memory.max: permission denied))
clause C count        best effort (wall_clock)
```

`full` means every mandatory clause (F, N, E, K, P) is enforced by the
kernel: host tools are available. `core` means one is not, and the line
says which and why; host tools are then invisible to the model, not merely
refused. The table above is a desktop session: the kernel lets the worker
create its own cgroup, so every process a host tool starts is in it and
dies with one `cgroup.kill`, while the session refuses the memory and
process-count controllers, so those two are best effort (M and C may be;
F, N, E, K and P may not). Where the kernel offers less the primitives
change (`subreaper` for K) and the line still says so. N names both of its
layers when the kernel has them: the seccomp filter (no socket outside
`AF_UNIX`, and no `io_uring`, which would create sockets the filter never
sees) and, from Landlock ABI 4 (Linux 6.7), a rule set that denies TCP
`bind` and `connect`; on an older kernel it reads `enforced (seccomp)` and
is complete all the same. The probe is a real worker's report of its own
confinement, not a guess.

## Declare the tool

```toml
[host_tools.cargo_test]
command = "~/.cargo/bin/cargo"
args = ["test", "--offline"]
allowed_args = ["--lib"]
arg_pattern = "[A-Za-z0-9_:]+"
max_args = 2
effect = "workspace_write"
read_paths = ["~/.cargo/bin", "~/.rustup"]
env = { RUSTUP_HOME = "~/.rustup", RUSTUP_TOOLCHAIN = "stable" }

```

The tool's `PATH` is built from the read paths that are themselves a `bin`
directory (`~/.cargo/bin`) or hold one, then `/usr/bin:/bin`, so `rustc` is
found with nothing more; a `PATH` in the declaration's `env` replaces the
built one. A `command` that is a name is looked up in the same directories.

The command and its fixed arguments are yours. The model may add a literal
from `allowed_args` or a test name matching `arg_pattern`, and nothing
else: no shell, no other flag, no path. The tool runs in the task's private
workspace with only `read_paths` and the system directories visible, with
no network, with the environment you declared and nothing inherited. It
writes `target/` inside the workspace, which snapshots and promotion skip;
a host tool never writes anywhere else in this release.

The declaration admits the name; one grant makes it usable. A task with
`--grant host_tool:cargo_test` sees `run_host_tool` for that tool and
nothing more, and `approval_tools = ["run_host_tool"]` in the profile asks
you before any host tool runs. To grant it to every task of a profile,
write `host_tools = ["cargo_test"]` under `[policy.profiles.<name>]`;
`run_host_tool` itself is never written in `tools`.

```sh
mimlys run "the test in src/lib.rs fails: fix it" --scope . --grant host_tool:cargo_test \
  --completion-check '{"tool": "run_host_tool", "arguments": {"tool": "cargo_test"}}'
```

The completion check runs the test once more when the model declares the
task done, so the task ends `completed (verified)` only if the test passes.

## Offline, and the dependency step

Host tools cannot reach the network, loopback included. `cargo test
--offline` therefore needs the dependencies on disk already. Fetching them
is a separate declared tool with `network = true`:

```toml
[host_tools.cargo_fetch]
command = "~/.cargo/bin/cargo"
args = ["fetch"]
effect = "external_read"
network = true
read_paths = ["~/.cargo/bin", "~/.rustup"]
env = { RUSTUP_HOME = "~/.rustup" }
```

`effect` and `network` must agree: `external_read` exactly when `network =
true`, and any other pairing is a startup error that names the declaration
and the value to write (a tool that reaches the network is one that reads
from outside, and the file says so). The step runs with every other clause in
force; a profile that lists `run_host_tool` in `approval_tools` asks you before
any host tool runs. Its destinations are not bounded: `cargo fetch` reaches
whatever the manifests name, so grant it to a task knowingly, and `status`
marks it `network allowed`. Nothing else a host tool does ever leaves the
host.

## What you may accept

On macOS the kernel cannot promise that every process of a tree is
terminated (a re-parented orphan can outlive a cancel). mimlys reports
clause K as best effort there and keeps host tools unavailable, unless you
write `accept_best_effort = ["K"]` under `[policy]`. The tier line then
reads `full (K best-effort, accepted by policy.accept_best_effort)`, and so
does the `Tier` line of every approval of a host tool. Nothing is accepted
by default, and a clause that is unavailable can never be accepted.

## What a host tool can read

Only the paths you declare in `read_paths` and the system's directories
(`/usr`, `/lib`, `/lib64`, `/bin`, `/sbin`, `/etc`, `/opt`, a few devices).
The home directory is not among them, and neither is any file of another
process: `/proc` is not readable by a host tool, which keeps the command
lines and network tables of your other programs from project code.

**Declare the toolchain's own directories, never a directory that holds
credentials.** `~/.cargo` holds `credentials.toml` (your crates.io token)
beside `bin`, so declaring `~/.cargo` would let project code read it. mimlys
refuses such a declaration when it loads the configuration (`mimlys check`
and every command), naming the entry it found and the narrower path:

```text
configuration key `host_tools.cargo_test.read_paths`: "~/.cargo" directly
holds "credentials.toml" (REQ-CONF-011, LIM-H5: a read path must not expose
credentials); declare the subdirectory the tool needs, for example
"~/.cargo/bin"
```

It refuses a filesystem root, the home directory and its ancestors the same
way, and any directory that directly holds a name from the credential list
(`credentials`, `credentials.*`, `.netrc`, `.npmrc`, `.pypirc`, `.docker/`,
and the secret patterns of the workspace exclusions: `.env`, `*.pem`, `*.key`,
`id_*`, `.ssh/`, `.aws/`, `.gnupg/`). Landlock has no deny rule, so the check
is on the declaration, before anything is granted. It is a guard on what the
administrator wrote, looking one level down: it is not a proof that a
declared directory holds no credentials further in, so declare the narrowest
directory the tool needs. `/etc` and `/opt` stay readable as a whole.

**When the toolchain needs a file outside the set**, it fails with the
file's name; the remedy is to declare the directory that holds it. With
`~/.rustup` left out, for instance, the observation's `stderr` reads:

```text
error: could not read settings file: '/home/me/.rustup/settings.toml': Permission denied (os error 13)
```

and adding `"~/.rustup"` to `read_paths` is the fix. Declare the narrowest
directory that holds what the tool names, and nothing that sits beside
credentials.

**Dependencies offline.** `cargo test --offline` needs the crates on disk,
and a host tool cannot write the user's `~/.cargo`. The way that works for
any dependency is to vendor them into the project: `cargo vendor` writes a
`vendor/` directory and prints the `[source]` section for the project's own
`.cargo/config.toml`; the task copies both like any file, and the tool reads
them from its workspace. The other way is to declare `~/.cargo/registry` and
set `CARGO_HOME = "~/.cargo"` in `env`; it works only for crates your own
builds have already unpacked under `registry/src`, because the registry is
read-only to the tool, and a crate that was downloaded but never unpacked
fails with `failed to create directory …/registry/src/…: Permission denied`.
(The dependency step above runs in a worker of its own; do not count on what
it fetched being on disk for the offline tool.)

**No core dump, no `ptrace`.** The runtime and the worker mark themselves
non-dumpable at start, so a same-user program that is not itself confined
cannot read their memory, environment or descriptors through `/proc` or
`process_vm_readv`, and the kernel writes no core dump of them. (A confined
program, such as a host tool, is already refused that by Landlock when the
target is outside its domain.) The price is the same for a debugger: you
cannot attach to `mimlys` or `mimlys-worker` as the same user unless you run
it as root. `env:`-referenced secrets sit in the runtime's environment, and
removing a variable after reading it would not hide it: `/proc/<pid>/environ`
shows the environment a process started with. A host tool or an MCP server
child is a new program and dumpable again; what protects their environments
is the read set above.

## What `toolchain list` shows

```text
read            /home/me/.cargo/bin
read            /home/me/.rustup
read            /usr
…
host tool       cargo_test: /home/me/.cargo/bin/cargo (offline)
  /home/me/.cargo/bin
  /home/me/.rustup
```

The read set is the union of the declared paths, the interpreters, the
worker's own directory and the system directories; a host tool sees
nothing else, your home directory included.

## Windows

On Windows the worker runs in an AppContainer named after the data
directory, inside a Job Object with the memory and process limits, under a
restricted low-integrity token where the system accepts one with the
container. A container reaches only what its SID is allowed to: mimlys
grants it the workspaces, its temporary directory, the interpreters and the
worker's own directory at startup, and you grant the toolchain:

```text
mimlys toolchain grant %USERPROFILE%\.cargo\bin
mimlys toolchain grant %USERPROFILE%\.rustup
```

Each asks before adding one inheritable read entry for the container to the
directory, records it under the data directory, and refuses the directories
that [the read set refuses](#what-a-host-tool-can-read) (a root, the home
directory, one that holds credentials such as `%USERPROFILE%\.cargo` with its
`credentials.toml`), and `mimlys toolchain
revoke <path>` removes exactly that entry. Until a declared path carries its
entry the tool is unavailable: `status` and `toolchain list` say
`acl_not_applied: <path>` with the command to run, and a `host_tool` grant
for it is refused with the same reason, so nothing is granted that changes
nothing.

```text
tier            full
clause F filesystem   enforced (appcontainer)
clause N network      enforced (appcontainer)
clause E environment  enforced (explicit_environment)
clause K termination  enforced (job_object)
clause P privilege    enforced (appcontainer)
clause M memory       enforced (job_object)
clause C count        enforced (job_object)
```

The profile paths a toolchain writes to (`USERPROFILE`, `LOCALAPPDATA`,
`APPDATA`, `TEMP`) point into the workspace's temporary directory, so
`cargo` keeps its caches there unless you declare `CARGO_HOME`. The
dependency step's worker carries the internet client capability; a container
never reaches loopback, so a `network = true` tool talks to the network, not
to a server on this machine.

Where the system refuses the restricted token together with the container,
the worker runs under the container's own lowbox token, which has no
privilege and cannot elevate, and `status` says `enforced (appcontainer)`
for clause P.

**`cargo` on current Windows.** A process inside an AppContainer cannot open
the `NUL` device (`Access is denied`), and `cargo` opens it for the standard
input of every `rustc` it starts; so `cargo test` declared as a host tool
fails at its first `rustc -vV` on Windows Server 2025, the observation's
`stderr` says so with the notice `host_tool_not_started` or cargo's own
message, and the task halts. The clause suite verifies the confinement with
its probe tool; a tool that does not need `NUL` runs today, and no way to lend the device to the container that keeps the
confinement was found (Windows Server 2022 and 2025 behave alike). A tool
that does not ask for a null standard input runs today.

After a `grant` or a `revoke`, `icacls` prints the directory's inherited
entries with `(I)` where it may not have before: the write marks the
security descriptor auto-inherited, as every write through the Windows
security interface does, and the shell then shows the inherited flag the
entries carried all along. The entries themselves are what they were.
