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

mimlys is pre-release. Release 0.7.1, the first on crates.io (milestones M1 to M6b),
is the deterministic core, the sandbox, the transactional workspace, a local model behind any
OpenAI-compatible server (llama-server, Ollama), external tools over the
Model Context Protocol with [rypsok](https://github.com/nabbisen/rypsok) as
the first server, and declared host tools under the platform's confinement:
on Linux Landlock, seccomp and tree termination, with `cargo test` as the
demonstration; on Windows an AppContainer and a Job Object, verified by the
clause suite, where `cargo` itself cannot run yet (current Windows refuses
an AppContainer the `NUL` device cargo's child processes open). macOS
follows in 0.8.

```sh
cargo install mimlys --locked
mimlys interpreters install javascript   # the pinned QuickJS build, verified by hash
cat > ~/.config/mimlys/config.toml <<'EOF'
config_version = 1
[backend]
kind = "openai_compatible"
url = "http://127.0.0.1:8080/v1"       # a llama-server or Ollama endpoint on this machine
model = "qwen2.5-coder:7b"
EOF
mimlys run "fix the failing test in this project" --scope .
```

The task works on a private copy of the project, prints every event, and
asks before a write when the profile says so:

```text
Approval required · task 01a1… · step 2
Tool         write_file
Will         create or overwrite the file "src/lib.rs" in the workspace
[a]pprove   [r]eject   [h]alt (reject and halt)   [g]rant …
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
