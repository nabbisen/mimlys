# How a task runs

You give mimlys a goal; a model proposes what to do, one decision at a time;
mimlys checks every decision before anything happens. This page follows one
task from the goal to the end, in the order things happen, so that you can
predict what mimlys will do and see why it did something else. Each section
ends with where the exact rules are written.

A **step** is one round: mimlys builds a request, the model answers with one
decision, mimlys checks it, and, if it is an action, runs it and records what
came of it. The next step's request carries the result.

## What your goal becomes

The goal is the text you typed, of 1 to 4000 characters (mimlys refuses an
empty or longer one when it creates the task). It is stored with the task and
sent with every request, in the same place and as the same text. mimlys does
not rewrite it, shorten it, add to it or replace it with its own version. It
is labelled as yours (`<<user kind="goal">>`), and the fixed instructions
at the top of every request tell the model that this is the task it is to do.
The text travels as a JSON string, so nothing inside it can pass for one of
mimlys's own labels.

What the model decides for itself goes elsewhere: its plan (below), kept apart
from your goal.

The only way the model can ask you something is the `ask_user` decision. The
task then waits (`waiting for your answer`; the question does not expire), and your answer
reaches the model in the next request as your own words
(`<<user kind="answer">>`). The instructions tell the model to ask only for
what no tool it has been given can supply: a choice, a missing grant, an
ambiguity in the goal. It is to read a file with `read_file`, not to ask you
for it. If you answer from another terminal, the command is
`mimlys answer <task> <text>`.

Where this is written: [Decisions, tools and observations](./reference/decisions-and-tools.md#one-decision-per-step),
[the segments of a request](./maintainers/external-design.md#151-segments-in-order),
[questions](./maintainers/external-design.md#103-questions-req-apr-010).

## What the model is offered

**Only the tools you granted.** A task starts with the tools its profile
lists (`read_only`: the three readers; `workspace` and `project`: the eight
built-in tools), plus any `--grant` you add and any grant you issue later with
`mimlys grant`. A tool that is not granted is not shown to the model, and a
decision that names it anyway is rejected as `unknown_tool` before anything
runs. A declared host tool such as `cargo test` appears as `run_host_tool`
only under a `host_tool` grant and only where the confinement tier is `full`.

**A short note on what is allowed.** Each request carries a capability note
generated from the task's grants: the names of the tools, one sentence saying
whether the model may write inside the workspace (a task with no granted tool
that writes is told it may read and may not write), and any grants that are
not tools, such as a network domain. When you grant or revoke something, the
note changes in the next request, and only then.

**One way to answer.** The model must answer with exactly one decision, in one
of two forms, the *decision transport*:

- `tool_calls`: the engine's native function calling. Every granted tool is
  offered as a function, beside four more: `plan`, `ask_user`, `complete`
  and `give_up`. The reply must be one structured call.
- `envelope`: one JSON object written as text.

With `decision_transport = "auto"` (the default) mimlys tries the first and
falls back to the second if the engine refuses `tools`; `mimlys status` shows
the one in force. In both, anything but one decision is rejected as
`malformed`, and mimlys changes nothing to make it pass: a call written into
the model's text is not read, and text beside a structured call is not kept.

Where this is written: [Configuration](./reference/configuration.md#the-decision-transport),
[Profiles](./reference/configuration.md#profiles),
[One decision per step](./reference/decisions-and-tools.md#one-decision-per-step),
[the `tool_calls` transport](./maintainers/external-design.md#154-the-tool_calls-transport-req-mdl-009-rfc-012-09).

## `plan`: a stored decision

`plan` is one of the five decisions (`act`, `plan`, `ask_user`, `complete`,
`give_up`). It carries `steps`, between 1 and 20 short strings, and nothing
runs: mimlys records it as the task's next numbered plan version and the step
ends. It still counts as a step.

The plan is the model's note to itself, and mimlys does not carry it out or
hold the model to it. It grants nothing and changes nothing about what is
allowed. What mimlys does with it is feed it back: from the next step on,
every request holds the latest version right after the goal, as the model's
own message (`<<plan version="2">>`; in the `tool_calls` transport, as an
earlier `plan` call with its acknowledgement), so the model sees it again at
every step. Earlier versions are not sent; a new `plan` replaces it. You see the same
plan in `mimlys task show <task>`.

Where this is written: [Decisions](./reference/decisions-and-tools.md#one-decision-per-step),
[decision schema](./maintainers/external-design.md#62-decision-schema-req-dec-002-version-1).

## What reaches the model at each step

A request is built in a fixed order. Everything that does not change comes
first, everything that does comes last:

| # | What | Changes |
|---|---|---|
| 1 | The fixed instructions, then (`envelope` only) the tool definitions, then the capability note | Never, except when a grant changes |
| 2 | Your goal (for a fork, the source task's history, below) | Never |
| 3 | The latest plan | When the model records a new one |
| 4 | A short history of older steps | Only when the request would otherwise be too big |
| 5 | The recent steps in full: each decision, then what came of it (an observation, or the rejection), your answers and your notes | Every step |
| 6 | `envelope` only: one line asking for the next decision | Never |

In the `tool_calls` transport the tool definitions travel as the request's
`tools` instead, an earlier decision is a call and its result the answer to
that call, and there is no last line; the latest observation is then the
last thing in the request. In the `envelope` transport the latest observation
is followed only by that one line.

Because the first rows are the same from step to step, the beginning of each
request is the same as the last one's; `mimlys metrics` reports how much of a
request repeated the previous one (`prefix_ratio`).

**Everything is labelled.** Each observation, rejection, answer and note is
wrapped in a marker that says where it came from (`<<observation …>>`,
`<<feedback …>>`, `<<user …>>`), and the instructions tell the model that
everything except your goal, answers and notes is data and never instructions.
The body of a marker is JSON, so a tool's output cannot forge a marker.

**History is summarised by rules, only when needed.** The request is kept
within a budget (32 000 characters by default; `context_chars` under
`[backend]` changes it). While it fits, every step is sent in full. If it does
not, mimlys applies these rules in order until it does: repeated lines in an
output are collapsed to one with a count; every step older than the last six
becomes a fixed summary of at most 300 characters (built from the step's
recorded fields, never by asking a model); then the oldest summaries are
dropped, six at a time. Never dropped: the capability note, a step that halted
the task, a rejected decision, and a failed action that no later success on
the same tool and path has superseded. If those alone exceed the budget the
request is sent anyway and the step records `over_budget`.

**A rejection reaches the model as its code and one fixed sentence.** When a
decision is rejected, the model's next request holds, in place of an
observation, a `<<feedback kind="rejection" code="…">>` segment with the code
and one sentence for that code, which says what it may change. For
`malformed`: return exactly one JSON object with a `decision` field (in the
`tool_calls` transport, call exactly one of the provided functions). For
`unknown_tool`: use one of the tools it was given. For `schema`: fix the field
so that it matches what is expected. For `invalid_argument`: change the
argument to a value within the stated bounds. For `denied`: no grant covers
this; use the tools and grants it has, or ask you for the grant with
`ask_user`. For `state`: wait for the pending item. For `budget`: finish with
`complete` or `give_up`. The rejected decision itself is not repeated. Three
rejections in a row halt the task. When you reject an action, the model gets
`approval_rejected` and the note you gave, as your own words
(`<<user kind="rejection_note">>`).

> **Today.** The sentence is chosen by the code alone. The reference lists
> finer reasons of a `denied` (`path_outside_workspace`, `path_excluded`,
> `grant_missing` …) and details such as `path`, `expected` and
> `visible_tools`; the validator produces them, but what is stored and sent to
> the model is the code and its sentence. So every `denied` gets the sentence
> for a missing grant, whatever the reason, and a sentence that speaks of a
> `path` or of `visible_tools` does not come with them (the tool names are in
> the capability note). `mimlys events` shows the code too, not the reason.

Where this is written: [What the model sees](./reference/decisions-and-tools.md#what-the-model-sees),
[Observations](./reference/decisions-and-tools.md#observations),
[context assembly](./maintainers/external-design.md#15-context-assembly-req-ctx-001-to-req-ctx-010),
[compression](./maintainers/external-design.md#153-compression-req-ctx-003-req-ctx-004),
[the rejection vocabulary](./maintainers/external-design.md#64-rejection-vocabulary-req-dec-008).

## How an action is checked

A decision to run a tool (`act`) passes twelve stages, in this order, and the
first that fails ends the check with its rejection code:

| # | Stage | If it fails |
|---|---|---|
| 1 | The task is running and nothing waits for you | `state` |
| 2 | Exactly one JSON object, not too large, no duplicate keys | `malformed` |
| 3 | The decision has the right fields and types | `schema` |
| 4 | The tool is one the task was granted | `unknown_tool` |
| 5 | The arguments fit the tool's description | `schema` |
| 6 | Paths are normalised against the workspace root; invalid names refused | `invalid_argument` |
| 7 | Authorisation: paths inside the workspace and not excluded, destinations covered by a `network` grant, a host tool granted and within its declaration, the effect allowed by the profile | `denied` |
| 8 | Numbers within range, patterns valid, a written file not too large | `invalid_argument` |
| 9 | The action's effect, and whether it needs your approval, is worked out | none |
| 10 | The task has time left and the write fits the workspace's size limit | `budget` |
| 11 | Your approval, when one is needed | `approval_rejected` |
| 12 | The action runs | a failed result, not a rejection |

The other four decisions (`plan`, `ask_user`, `complete`, `give_up`) pass
stages 1 to 3 and are then handled directly; the stages about tools do not
apply to them. A path is judged by its text alone, before any file is
touched: a path that leaves the workspace is `denied` in the same way whether
or not the file exists.

**Approvals.** mimlys asks you before a tool runs when your profile lists it
under `approval_tools`, or when its effect changes something outside this
machine, which mimlys cannot undo; otherwise it does not ask. The task then
waits (`waiting for your approval`; the request does not expire). The prompt
is written by mimlys from the action it will run, never by the model; the
model's own stated reason is shown separately and marked untrusted. An
approval is for that one action under the grants in force when you gave it: if
the grants changed before it ran, the approval is void and the action does not
run. Rejecting continues the task, or halts it with `--halt` (or with
`on_reject = "halt"` in the profile).

**Confinement.** The built-in file tools work through one handle rooted at the
task's private copy of your project; a symbolic link that leaves it does not
open. `run_script` and `run_wasm` run in a separate process, `mimlys-worker`,
inside a sandbox that sees `/workspace` and an empty `/tmp`, with no network
and no environment, and with limits on fuel, time, memory and output. A host
tool runs only where this machine enforces every mandatory clause of the
confinement (`mimlys status` lists them). An action that writes is a
transaction: a snapshot before, a snapshot after, and the workspace restored
if it fails. Your own project is never touched until you promote the result
(below).

Where this is written: [Validation](./reference/decisions-and-tools.md#validation),
[The sandbox](./reference/decisions-and-tools.md#the-sandbox),
[Snapshots and rollback](./reference/decisions-and-tools.md#snapshots-and-rollback),
[The approval prompt](./reference/cli.md#the-approval-prompt),
[Host tools](./reference/host-tools.md#the-tier-line),
[the validation order](./maintainers/external-design.md#63-validation-order-req-dec-004).

## How "done" is decided

When the model sends `complete` with a `summary`, it is saying that it thinks
the goal is reached. That is not what ends the task. What happens depends on
whether you gave the task a completion check:

- **No check.** The task ends `completed (asserted by the model, no check
  configured)`. You are told it is the model's word.
- **A check** (`--completion-check '{"tool": NAME, "arguments": {…}}'` on `run`
  or `task new`). mimlys runs that tool itself, through the same twelve
  stages, and the task ends `completed (verified by the completion check)`
  only if the run succeeds. The check never asks for your approval, and so
  must be a tool the task is granted and one the profile does not hold for
  approval: if it is not allowed to run, the task halts with the reason
  `policy`.
- **A check that fails** does not complete the task. Its failed result goes to
  the model like any observation and the task goes on (it counts as a failed
  action toward the limit of five in a row).

The summary decides nothing either way. A model that gives up with `give_up`
ends the task `failed`, with its reason shown. [Host tools](./reference/host-tools.md)
shows a check that runs a project's tests.

Where this is written: [Decisions](./reference/decisions-and-tools.md#one-decision-per-step),
[terminal fields](./maintainers/external-design.md#53-terminal-fields),
[the state machine](./maintainers/external-design.md#5-task-state-machine-req-task-002).

## What persists, and how to continue

Everything a task does is recorded in the data directory before the task goes
on: each step, the hash of each decision, each action and its result, every
plan version, grants, approvals, questions, snapshots and an event for each
transition ([Storage](./reference/storage.md) says where and how much). A
task that waits, pauses or halts keeps its state across a restart, and so do
its counters and its deadline. If a process ends in the middle of a task, the
next command that writes restores the workspace to the snapshot before an
interrupted write and the task halts with `recovered`.

You see all of it from any terminal, and none of these commands changes
anything:

<!-- tested -->
```sh
mimlys task show <task>
mimlys events <task>
mimlys metrics --task <task>
```

**Continue.** `mimlys task resume <task>` continues a task that is paused,
halted, or created with `--hold`. A task that halted for its step budget or
its deadline takes `--extend-steps N` or `--extend-deadline D`, within the
administrator's ceiling. `approve`, `reject` and `answer` continue a task that
waits. `completed`, `failed` and `cancelled` are final: such a task cannot be
resumed.

**Fork.** To go on from where a finished task left off, start a new task
from it:

<!-- tested -->
```sh
mimlys task new retry --from-task <task> --hold
```

(Give a goal of more than one word in quotes.) The new task gets a workspace
made from the source's latest snapshot (a promotion's backup does not count),
or from `--from-snapshot S`; the source is untouched. It starts with the
grants the source still holds (you may add `--grant`, within the same
ceiling), and its first request carries a read-only history of the source: its
goal, its latest plan, and its last 20 steps as the decision and the outcome
of each, never file content. It works on the same project directory unless you
give another `--scope`. Only one unfinished task may work on a project
directory (or one that contains or lies within it) at a time, so a task that
only halted, which is not finished, can be forked only into another `--scope`;
until then, `task resume` it. `--hold` leaves the new task created and not
started until you `task resume` it.

**Promote.** The only command that writes to your project is `mimlys promote
<task>`. It shows the difference, refuses if you changed one of the files in
the meantime, backs up what it will overwrite, asks, and writes.
`mimlys promote <task> --patch` prints the patch and writes nothing:

<!-- tested -->
```sh
mimlys promote <task> --patch
```

Where this is written: [Getting started](./getting-started.md#bringing-the-result-into-your-project),
[Storage](./reference/storage.md), [Command line](./reference/cli.md),
[fork](./maintainers/external-design.md#54-fork-req-task-004),
[recovery](./maintainers/external-design.md#163-recovery-at-startup-req-pers-006-req-pers-007).

## What is bounded today

Bounds are per task, with the defaults below. Reaching one halts the task
with a reason you can read in `mimlys task show`; it does not lose anything,
and `task resume` goes on (a step or deadline halt takes an extension).

| What | Default |
|---|---|
| Steps in a task | 40 |
| Time from creation to the deadline | 2 hours |
| Rejected decisions in a row | 3 |
| Failed actions in a row | 5 |
| The same decision in a row, with the workspace unchanged | 3 |
| Executions of one action (the first, and one retry the runtime may make) | 2 |
| One decision | 256 KiB |
| One request to the model | 32 000 characters |
| One sandbox execution | 10^10 units of fuel, 30 seconds, 256 MiB of memory, 1 MiB of output per stream |
| A task's workspace | 1 GiB, 200 000 files, 8 MiB per file a tool writes |

The profile in your configuration sets the limits on steps, rejections,
failures, identical decisions and executions (`steps`,
`consecutive_rejections`, `consecutive_failures`, `identical_decisions`,
`executions_per_action`), and `--steps` and `--deadline` set the first two for
one task; the rest are in
[Limits and defaults](./maintainers/limits-and-defaults.md).

Per runtime, one task runs at a time and one execution is in flight. **Across
runtimes there is no bound yet.** If you point two configurations at two data
directories, each keeps its own limits and neither knows about the other; a
cap on executions across all your runtimes, a bound on a tool's use of the
processor, and a gate of one request per model server are part two of RFC 013,
which is not in this release. [Getting started](./getting-started.md#several-projects-several-jobs)
says what to expect until then.

Where this is written: [Budgets and halts](./reference/decisions-and-tools.md#budgets-and-halts),
[counters and convergence](./maintainers/external-design.md#65-counters-and-convergence-req-dec-010),
[Limits and defaults](./maintainers/limits-and-defaults.md),
[Configuration](./reference/configuration.md#profiles).
