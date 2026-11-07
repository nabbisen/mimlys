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

Where this machine can enforce one of the mandatory clauses (F, N, E, K,
P) only as best effort, mimlys says so and keeps host tools unavailable,
unless you write that clause in `accept_best_effort` under `[policy]`, for
example `accept_best_effort = ["K"]`. None of the backends of this release
needs it on a supported platform, macOS included (see "On macOS"). The tier line then
reads `full (K best-effort, accepted by policy.accept_best_effort)`, and so
does the `Tier` line of every approval of a host tool. Nothing is accepted
by default, and a clause that is unavailable can never be accepted.

## On macOS

Host tools run on macOS (Apple silicon; 0.8). **No clause needs to be
accepted**: the tier is `full` with nothing in `accept_best_effort`, because
every mandatory clause is enforced by the system.

```text
tier            full
clause F filesystem   enforced (seatbelt)
clause N network      enforced (seatbelt)
clause E environment  enforced (explicit_environment)
clause K termination  enforced (seatbelt_membership)
clause P privilege    enforced (seatbelt)
clause M memory       best effort (rss_watchdog)
clause C count        best effort (process_count_watchdog)
```

mimlys confines the worker with one Seatbelt profile, and every host tool
inherits it (a sandboxed process cannot apply a second one). K is enforced
because the sandbox itself tells the worker which processes are its own: after
a cancel, a deadline or the end of a tool, the worker ends every one of them,
a process that detached itself and was adopted by `launchd` included. M and C
are watchdogs that look at the tool's processes every 100 ms and kill them
above 2 GiB or 64 processes, so they are best effort (they may miss something
shorter than a poll), as on Linux without a delegated cgroup. If the worker
cannot verify the mechanism on a given macOS, K, M and C read `unavailable:
membership_unverified: …` and host tools are not offered.

**What to declare.** The system's pieces a build needs are in the set already:
`/usr`, `/bin`, `/sbin`, two small files, the active developer directory (the
Xcode or Command Line Tools that `xcode-select -p` names; mimlys reads the link
`xcode-select` keeps, it does not run it), and the one cache file `xcrun`
uses. Declare the toolchain itself, and declare **the real `cargo`, not a
shim**: Homebrew's `rustup` puts `cargo` on the `PATH` as a link into
`/opt/homebrew/Cellar/…`, and a confinement matches the resolved path, so
mimlys refuses such a command when it loads the configuration, on every
platform, and names the directory to declare:

```toml
[host_tools.cargo_test]
command = "~/.rustup/toolchains/stable-aarch64-apple-darwin/bin/cargo"
args = ["test", "--offline"]
allowed_args = ["--lib"]
max_args = 1
effect = "workspace_write"
read_paths = ["~/.rustup"]
env = { PATH = "~/.rustup/toolchains/stable-aarch64-apple-darwin/bin:/usr/bin:/bin" }
```

(`~/.rustup` holds no credentials; do not declare `~/.cargo`.) The
toolchain's name (`stable-aarch64-apple-darwin`) is the one `ls ~/.rustup/toolchains`
shows. When a tool still fails with `Operation not permitted` on a file,
the observation's `stderr` names it: declare the narrowest directory that
holds it.

**What a host tool can read on a Mac** is that and nothing else: not
`/System` as a whole, not `/etc` (only the OpenSSL configuration file), not
`/opt`, not any file of your home directory, not the per-user temporary
directory (only the one cache file by name), and not another program's
environment or arguments. A tool can learn that a path exists (the loader needs
that) and the **numbers** of the processes on the machine (the price of the
membership loop); it cannot learn their names, owners or arguments.

**The dependency step** (`network = true`) runs in a worker of its own whose
profile adds three things to the offline one: outbound TCP to port 443,
name lookup through the system's one lookup socket, and the system's
certificate file. Registries over HTTPS work; a git dependency over SSH is not
reachable, and the offline tool cannot resolve a name at all.

**One thing the system cannot give.** A program of yours that is *not*
confined can read the runtime's environment, `env:`-referenced secrets
included, with `sysctl KERN_PROCARGS2`; macOS has no refusal for an unconfined
reader, and removing a variable afterwards would not help. A host tool is
refused it by the profile.

## What a host tool can read

Only the paths you declare in `read_paths` and the system's directories
(on Linux `/usr`, `/lib`, `/lib64`, `/bin`, `/sbin`, `/etc`, `/opt`, a few
devices; on macOS the shorter list above).
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

Two more reasons carry the same name. They appear when mimlys cannot take
the lock that orders the writers of one directory's permissions (two
runtimes, two installations or two commands of one, would otherwise lose one
another's entry), for example on the directory your worker is installed in:

```text
acl_not_applied: the permissions of <dir> are being written by another runtime
acl_not_applied: the permissions lock of <dir> cannot be taken: <system error>
```

The first: another runtime is writing the directory's permissions. A
runtime that finds the directory busy writes one line to its log (`waiting
for another runtime's write of the permissions of <dir>`) and waits up to a
minute (LIM-O7), for a write that has to reach every file below the
directory can take several seconds; if the other runtime has not finished by
then, nothing is written. The second: the system would not create the lock
or wait on it, and nothing is written either, since a write without the lock
could remove the other runtime's entry. The command fails with the error
kind `unavailable` and `detail.component` `"permissions"` (`{"error":
{"kind": "unavailable", "message": "acl_not_applied: ...", "detail":
{"component": "permissions"}}}` with `--json`). `toolchain grant` and
`revoke` print it and exit with an error, and nothing was changed; at startup
it stops the start, and the next start applies mimlys's own directories.
What to do: retry. Wait for the other runtime's command or start to finish
(it is usually another `mimlys` running) and run the command again, or start
mimlys again. If the second reason repeats, the system error in it says why
the lock is refused.

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

**Removing mimlys.** On Windows mimlys changes the machine outside its data
directory: it creates the container profile and adds permissions for the
container to its own directories and to the toolchain directories you grant.
Deleting mimlys without undoing them leaves those behind. Before an uninstall:

1. `mimlys toolchain revoke --all` lists every permission the container holds
   (the ones in the record, and any the scan finds in mimlys's own directories
   and your declared `read_paths` although the record lost them), asks once,
   removes them, deletes the container profile and the record. mimlys applies
   the entries of its own directories again at its next start, so this is the
   last mimlys command you run. `--yes` skips the question; the command
   reports each entry it removed and each it could not, and exits 1 if any
   failed (run it again to retry exactly those).
2. `cargo uninstall mimlys`.
3. Delete the data directory.

After a `grant` or a `revoke`, `icacls` prints the directory's inherited
entries with `(I)` where it may not have before: the write marks the
security descriptor auto-inherited, as every write through the Windows
security interface does, and the shell then shows the inherited flag the
entries carried all along. The entries themselves are what they were.
