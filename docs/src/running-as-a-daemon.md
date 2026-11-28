# Running mimlys as a daemon

From 0.14 mimlys can run in two ways, and you choose by whether a daemon is running.

- **Embedded**, the way it always ran. Each command is a runtime of its own. A command that runs
  a task owns the store while it lives, runs the task in its own process, and ends when the task
  ends or you stop it.
- **As a daemon.** One background process per user and data directory owns the store and runs the
  tasks of **every** command of yours. A command is then a client: it sends the daemon its
  request, follows the task if you asked it to, and ends. You can start a task in one terminal,
  approve it from a second one, and close the first.

A client that finds no daemon runs embedded. **Nothing changes until you start a daemon or ask for
one** (`serve = "on_demand"`, below).

## The two modes, side by side

| | Embedded | As a daemon |
|---|---|---|
| What it is | Each command is a runtime. | One process holds the store and runs every client's tasks; a command is a client. |
| How it starts | By running a command. | `mimlys daemon start`, or, with `serve = "on_demand"`, by the first command that finds none. |
| How it stops | When the command ends. | `mimlys daemon stop`; or by itself after `daemon_idle` (30 minutes by default) with nothing to do. |
| Closing the terminal | **Stops the task** with the process. | **The task goes on.** |
| Ctrl-C | Pauses the task at its next boundary. | Pauses it, in the client that follows it. |
| `--detach` | Refused: it needs a daemon. | The command returns when the task is running. |
| Your configuration and your secrets | Read by each command, so a change is the next command's. | **Read when the daemon starts.** |
| Two tasks at once | The second `running` task is refused (`conflict`). | The second waits in line, and says what it waits behind. |
| Another terminal | A second command that writes is refused while the first holds the store. | It approves, answers, cancels, follows. |
| Where its log goes | Standard error of the command. | `logs/daemon.log` under the data directory (10 MiB, five rotated copies). |

`mimlys status` says which mode is in force, and, when a daemon serves, what `daemon status` says
first.

## Starting, stopping, restarting

<!-- tested -->
```sh
mimlys daemon start                              # starts it, detached, and returns when it answers
mimlys daemon status                             # its process, version, uptime, clients, tasks
mimlys run "edit the project" --detach --scope <scratch>  # the daemon runs it; this returns
mimlys daemon restart                            # stop, start, and resume what the stop paused
mimlys daemon stop                               # pause what runs, list it, end
```

- **`daemon start`** returns once the daemon answers. `--foreground` keeps it attached, for a
  service manager or a test. If a daemon already runs for this data directory, it says so.
- **`daemon stop`** pauses the running tasks at their next boundary (an execution within two
  seconds; an action that cannot be undone, such as a call to an external tool that writes, is
  waited for, and the command says which one and until when at the latest). It lists every task it
  paused and every task that waited in line, with how to resume each, and ends the daemon.
  **Nothing resumes by itself.**
- **`daemon restart`** is a stop, a start, and the resumption of exactly the tasks that the stop
  paused, then the tasks that waited in line, put back in it in their order. Each is said by name.
- **`daemon status`** shows the daemon's process, version, uptime and where it listens (a socket
  on Linux and macOS, a named pipe on Windows), whether your configuration file differs from the
  one it read, its clients (counted as processes, with their connections beside them), the tasks
  it runs and those in line. `--log` adds the tail of its log.

### Starting one without asking: `serve = "on_demand"`

```toml
[runtime]
serve = "on_demand"      # "manual" is the default
```

With `on_demand`, the first command that finds no daemon starts one (detached, with no terminal),
waits for it to answer, **says once, on standard error, that it started one, with its process id**,
and sends its request through it. The daemon outlives that command, and ends by itself after
`daemon_idle`. The next commands find it and say nothing.

- **Two commands at once** may each start one. The store's lock decides: the second daemon ends,
  naming the first, and its command goes through the first.
- **A start that fails says why, and the command does not run embedded instead.** You asked for a
  daemon; mimlys does not give you another mode without telling you. The reason is the
  daemon's own (a configuration it cannot read, a store that something else holds, and so on).
- **The daemon is started with the environment of the command that started it**, so its secrets
  are that command's. See [what a daemon reads when it starts](#what-a-daemon-reads-when-it-starts).
- Commands that only read (`status`, `events`, `task list`, `task show`, `metrics`) and
  `daemon stop` never start one.

## What is different, plainly

### Closing the terminal

Embedded, closing the terminal ends the process, and the task stops with it. As a daemon, the task
**goes on**: it belongs to the daemon, not to the terminal that started it. Ctrl-C in a client that
follows a task asks for a pause at the next boundary, in both modes; closing the client leaves the
task running.

### `--detach`

`run`, `task new`, `task resume`, `approve`, `reject` and `answer` follow the task until it needs
you. **`--detach` returns as soon as the task is running** and leaves it to the daemon, so it needs
one: without a daemon it is refused, before anything is created.

### What a daemon reads when it starts

A daemon reads your configuration file and the value of each secret (`env:NAME`) **once, when it
starts**. A command that finds a different file, or a different value for a secret, is refused
for what changes state, naming the key (never the value) and the remedy:

```
mimlys daemon restart
```

What only reads is not affected. After you edit the configuration, or change a secret in your
environment, restart the daemon. A version of mimlys that differs from the daemon's is refused
the same way; see [Upgrading](#upgrading).

## The line, and every reason a task waits

A task is always in one of these states when it waits for something other than you. In both modes
it says what, and what it waits behind.

| What the task waits for | Where you see it | What it says |
|---|---|---|
| **Another of your tasks, in a daemon** (one task runs at a time in a runtime) | the command that follows it, `daemon status` | `queued behind task <id>, running since <time>`; it starts when that task leaves `running` |
| **Another task, embedded** | the command that tried | There is no line: a second `running` task is refused (`conflict`), naming the one that runs |
| **A slot, across your runtimes** (`admission_slots`, two by default) | `task show`, `status` | `waiting: admission (… slots taken by runtimes …)`; the task stays `running` |
| **The model server** (`max_in_flight`, one request at a time by default) | `task show`, `status` | `waiting: engine (max_in_flight = …, … in flight, held by runtimes …)`; the task stays `running` |
| **The store, held by another process** | the command that tried | Refused, naming the holder: `the store is in use by runtime <pid> since <time>` (a daemon is called one) |
| **You** (an approval, an answer) | `task show`, `status` | `waiting for your approval` or `waiting for your answer`; it does not expire |

**The line lives in the daemon, not in the store.** If a daemon ends in any way but `stop`, the
tasks that waited in line are left `created`, and `status` lists them as `not started`, with the
command that starts each. `daemon stop` lists them, and `daemon restart` puts them back in line.

## Repeats

A request can reach the daemon and its answer be lost: the connection breaks. The command makes
the connection again and sends the same request once more, under the same request id, and answers
with **what the store recorded**; it never says that it does not know whether the request took
effect. A request that was applied is applied once.

One thing follows from that. A repeated creation finds its task as long as the task's record
stays, deleted or not. After `mimlys task delete --audit` nothing of the task is left, so a repeat
of the creation that comes after it makes **a new task**. A repeat comes right after a
reconnection, so you meet this only if you delete a task, with its audit, in that moment.

## Who may reach a daemon

- **Your own processes, and only yours.** The daemon's socket (on Linux and macOS, a file of mode
  0600 in a directory of mode 0700) or pipe (on Windows, which admits your account alone) is
  reachable by the user that started the daemon. Both ends check the other: the daemon refuses a
  process of another user, and a command refuses to send anything to a daemon that is not yours.
- **Never the daemon's own tree.** The worker that runs a task, the tool servers it starts and
  what they start can ask the daemon nothing. A request from them is refused, with the reason, and
  recorded (`daemon.refused`). This is what stops a tool from approving its own action.
- **What is not covered.** A process of yours outside the daemon's tree has your authority
  already, as it has over your files. A process that the daemon's tree starts and that leaves it
  (on Linux and macOS, one that begins a session of its own; on Windows, one that the tool server
  starts in the instant before the daemon marks the server) is not caught. This is the residual
  of the daemon's threat (THR-39 in the [threat register](./maintainers/threat-register.md)).

## Upgrading

A daemon runs the version of mimlys that started it, and **a client and a daemon of different
versions do not talk to each other**: the command says so, naming both versions. After you install
a new release, restart the daemon:

```sh
mimlys daemon restart
```

Any client can stop any daemon, whatever their versions: `daemon stop` is the one request that
never changes, so an old daemon is never stuck for want of a client that can end it.

Where this is written: [Command line](./reference/cli.md), [Configuration](./reference/configuration.md),
[How a task runs](./how-a-task-runs.md).
