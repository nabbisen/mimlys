# mimlys

[![CI](https://github.com/nabbisen/mimlys/actions/workflows/ci.yml/badge.svg)](https://github.com/nabbisen/mimlys/actions/workflows/ci.yml)
[![License: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](./LICENSE)

**A local AI-agent execution and safety runtime: the model proposes, mimlys decides.**

## Overview

mimlys lets a local language model work on a task one validated step at a
time. Every decision is checked against a fixed schema, the task's explicit
grants and its budgets before anything runs. Generated code runs in a WASM
sandbox with no network; files change only in a private workspace with a
snapshot before every write; nothing reaches your own tree except an explicit,
reviewed promotion. Every transition is durable, every stop has a reason, and
a crash never replays an irreversible action.

## Why / when

- You want an agent on your own machine, with your own model, and you want to
  stay in control of what it may touch.
- You want failed attempts to be undone, not accumulated.
- You want to see why something was refused, approved or stopped.

mimlys is not a model, a search engine or a user interface. Search is served by
[rypsok](https://github.com/nabbisen/rypsok), an independent project, through the
Model Context Protocol.

## Quick start

mimlys is pre-release. Release 0.13.0 (milestones M1 to M8: 0.12.0, which is 0.11.1 plus the user's copies of what a tool printed and operation on one machine, plus adaptive effort, part one: effort levels, several backends, the fitness probe and plan first, RFC 015; upgrade from 0.12.0) is the deterministic core, the sandbox, the transactional workspace, a local model behind any
OpenAI-compatible server (llama-server, Ollama), external tools over the
Model Context Protocol with [rypsok](https://github.com/nabbisen/rypsok) as
the first server, and declared host tools under the platform's confinement:
on Linux Landlock, seccomp and tree termination, with `cargo test` as the
demonstration; on Windows an AppContainer and a Job Object, verified by the
clause suite, where `cargo` itself cannot run yet (current Windows refuses
an AppContainer the `NUL` device cargo's child processes open); on macOS a Seatbelt profile the worker applies to itself, with every mandatory clause enforced and no clause to accept, and `cargo test` as the demonstration.

**Upgrading from 0.12.0.** 0.13.0 adds adaptive effort, part one. **Read first:** the configuration is `config_version = 2`, with `[backends.<name>]` for `[backend]`; a version-1 file still loads and runs as before (`mimlys check` says how to write version 2), and is read through 0.14. A backend whose URL is not on this machine must now be declared `locality = "remote"` (with `policy.remote_backend_allowed` and a profile with `external = true` for a task to use it): a 0.12.0 file that declared a LAN address `local` stops loading, and says which backend and which host. A backend's redirect is no longer followed, and a task's backend is checked against its profile at every start, not only at its creation. `run` and `task new` no longer create a task whose backend does not answer (0.12.0 created it and halted it). New: `--effort quick|standard|deep` (a budget and a backend per level; `deep` is the larger model you name, refused until `[effort.deep]` is configured), several backends, `mimlys model probe` (whether a model acts on this machine, never whether it finishes), `--plan-first` (a plan you approve before anything acts) with `approve --note`, and what a task cost in `task show`. Commands of 0.13 show a word that a later release wrote by its name and go on, instead of failing. No vulnerability of 0.12.0 is fixed in 0.13.0; the checks that REQ-SEC-010 asks for are hardening (see the Security section of the [CHANGELOG](./CHANGELOG.md)). The store's schema is unchanged (version 4), and 0.12.0 can open a store 0.13.0 has written as long as no task waits for its plan: a task that waits for its plan (`wait_kind: plan`) makes 0.12.0's `status`, `task show`, `events` and every writer fail, and `events` of a task that went through a plan wait fails in 0.12.0 (decide or cancel plan waits with 0.13 before you go back); 0.12.0 also refuses a version-2 configuration. **Upgrading from 0.11.1.** 0.12.0 keeps the whole of what a tool returned or printed, and `mimlys task output` shows it; it also bounds several runtimes of yours on one machine: all of them together run at most two executions at once (`admission_slots`) and send one request at a time to a model engine (`max_in_flight`), a task that waits says `waiting` and why, `mimlys status` lists who runs and who waits, and a host tool's tree is held to half the cores where the system allows (`cpu_share`; on Linux in practice it is not, and the book shows how to bound the runtime with `systemd-run`). It fixes a defect of 0.11.1 and earlier: a tool that printed more than about 600 KB ended the worker and its execution. No vulnerability of 0.11.1 is fixed in 0.12.0 (see the Security section of the [CHANGELOG](./CHANGELOG.md)). The store's schema is unchanged (version 4), and 0.11.1 can open a store 0.12.0 has written, except that its `events` command cannot read a task that waited for a slot; 0.11.1 refuses a configuration that sets the new keys. A configuration with `[sandbox] output_bytes` above 16 MiB no longer loads. **Upgrading from 0.10.0:** 0.11.0 closes the two residuals 0.10.0 stated (on Windows a host tool of one task could read and change another task's tree, and on macOS a host tool could outlive its worker) and the audit's smaller findings; see the Security section of the [CHANGELOG](./CHANGELOG.md). Until you upgrade, do not grant host tools to tasks running code you do not trust (on Windows, not beside tasks you do). **On Windows the first start of 0.11.0 moves the installation's permissions once and can take a while on a large data directory**; `status` says so meanwhile. The store's schema is unchanged (version 4). A configuration whose host tool reads the data directory, or a path that covers it, no longer loads, and `read_file`'s `max_chars` is at most 8 000. **Upgrading from 0.9.0:** 0.10.0 changed the store's schema to 4, so 0.9.0 refuses a store that 0.10.0 or later has opened, and `mimlys rollback <task> --promotion` asks (a script passes `--yes`).

**The model, plainly.** 0.9 asks the model for its decision through the engine's native function calling. With it, one model on one task was measured to work: Qwen2.5-7B-Instruct (Q4_K_M) repaired the M4 task (a failing test in a small JavaScript project, checked by the completion check) in 9 of 10 runs, 4 of 5 on llama.cpp and 5 of 5 on Ollama; the one run that did not finish ended by asking the user a question. A model that does not return structured calls does not act (the Qwen2.5-Coder models in the same measurements returned none), and under 0.8's decision format, still available as `decision_transport = "envelope"`, no measured model acted ([M4 report](docs/src/maintainers/m4-report.md), §6). That is one model, one task and ten runs on one machine: a result to repeat on your own model, not a promise. The scripted fake backend, described in the book's Getting started, runs a task end to end without any model.

```sh
cargo install mimlys --locked
mimlys interpreters install javascript   # the pinned QuickJS build, verified by hash
cat > ~/.config/mimlys/config.toml <<'EOF'
config_version = 2
[backends.default]
kind = "openai_compatible"
url = "http://127.0.0.1:8080/v1"       # a llama-server or Ollama endpoint on this machine
model = "Qwen2.5-7B-Instruct-Q4_K_M.gguf"   # as the engine lists it; it needs a 16 384-token context (the book's configuration chapter)
EOF
mimlys run "fix the failing test in this project" --scope .
```

The task works on a private copy of the project, prints every event, and
asks before a write when the profile says so:

```text
Approval required · task 01a1… · step 2
Tool         write_file
Will         create or overwrite the file "src/lib.rs" in the workspace
[a]pprove   [r]eject   [h]alt (reject and halt)   [g]rant …   [v]iew (every argument in full)
```

Every write is a transaction over the private copy (snapshot before, snapshot
after, rollback on failure). When the task is done, `mimlys promote <task>`
shows the diff, refuses conflicts, backs the targets up and asks before
writing to your project; `mimlys task delete <task>` is the only way anything
leaves the disk.

See [Getting started](./docs/src/getting-started.md) and
[ROADMAP.md](./ROADMAP.md) for what each milestone delivers.

## Design notes

- Model output is an untrusted proposal; the runtime is the authority.
- No grant, no operation; grants never exceed the administrator's ceiling and never widen.
- Authorization comes before any filesystem access, so a refusal never reveals what exists.
- Effects are declared once (`workspace_write`, `host_write`, `external_read`, …) and approval, rollback and recovery derive from them.
- The approval screen is rendered by the runtime, never written by the model.
- Unknown outcomes after a crash are a state, not a guess.

## More detail

- [Documentation](./docs/src/introduction.md) (mdbook): for new users, integrators and maintainers
- [Requirements](./docs/src/maintainers/requirements.md), [External design](./docs/src/maintainers/external-design.md), [Threat register](./docs/src/maintainers/threat-register.md)
- [RFCs](./rfcs/README.md) · [Roadmap](./ROADMAP.md) · [Changelog](./CHANGELOG.md)

## License

Apache-2.0. See [LICENSE](./LICENSE) and [NOTICE](./NOTICE).
