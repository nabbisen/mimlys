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
clause N network      enforced (seccomp)
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
change (`subreaper` for K) and the line still says so. The probe is a real
worker's report of its own confinement, not a guess.

## Declare the tool

```toml
[host_tools.cargo_test]
command = "~/.cargo/bin/cargo"
args = ["test", "--offline"]
allowed_args = ["--lib"]
arg_pattern = "[A-Za-z0-9_:]+"
max_args = 2
effect = "workspace_write"
read_paths = ["~/.cargo", "~/.rustup"]
env = { RUSTUP_HOME = "~/.rustup", RUSTUP_TOOLCHAIN = "stable" }

```

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
effect = "workspace_write"
network = true
read_paths = ["~/.cargo", "~/.rustup"]
env = { RUSTUP_HOME = "~/.rustup" }
```

It runs with every other clause in force and its effect is `external_read`;
a profile that lists `run_host_tool` in `approval_tools` asks you before
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

## What `toolchain list` shows

```text
read            /home/me/.cargo
read            /home/me/.rustup
read            /usr
…
host tool       cargo_test: /home/me/.cargo/bin/cargo (offline)
  /home/me/.cargo
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
mimlys toolchain grant %USERPROFILE%\.cargo
mimlys toolchain grant %USERPROFILE%\.rustup
```

Each asks before adding one inheritable read entry for the container to the
directory, records it under the data directory, and `mimlys toolchain
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
