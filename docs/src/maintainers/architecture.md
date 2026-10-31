# Architecture Overview

**Status:** Baseline 1.0 (2026-10-02). **Informative.** The normative sources are the [requirements](./requirements.md) and the [external design](./external-design.md); where this chapter and those disagree, they win.

## 1. Purpose

mimlys sits between an uncertain model and the environment the agent works in. The model proposes; the runtime decides whether a proposal is well formed, permitted, bounded and recoverable, and only then executes it.

```text
      model                       mimlys runtime                         environment
  "what should     ──decision──▶  valid? granted? bounded? approved?  ──action──▶  workspace,
   I do next?"    ◀─observation─  recoverable? ── execute safely ──  ◀─result──  sandbox, tools
```

## 2. Components

```text
┌───────────────────────────── mimlys (CLI, runtime host) ─────────────────────────────┐
│  control API · approval prompt · follow mode · configuration                          │
│  ┌──────────────────────────── mimlys-core ───────────────────────────────────────┐   │
│  │ task state machine · step loop · decision schema · validator · grants/policy    │   │
│  │ effects · approvals · context assembly · recovery logic · events                │   │
│  └───────┬───────────────┬──────────────────┬───────────────────┬──────────────────┘   │
│   mimlys-model     mimlys-mcp         mimlys-persist         mimlys-exec (library)     │
│   backends         MCP client         SQLite · snapshots     worker protocol           │
└──────────┬────────────────┬────────────────────┬────────────────────┬──────────────────┘
      model server     MCP servers          data directory      mimlys-worker (process)
      (llama-server,   (rypsok, …)          db · blobs ·        wasmtime sandbox ·
       Ollama, fake)                        snapshots · trees   OS confinement · host tools
```

| Crate | Responsibility | May depend on |
|---|---|---|
| `mimlys-core` | Every semantic the external design freezes: states, schemas, validation, effects, grants, approvals, context, recovery, events; the adapter SPI traits | serde, Tokio; nothing with I/O (REQ-ARCH-002) |
| `mimlys-persist` | `Persistence` and `SnapshotStore` on SQLite and the content-addressed store | core |
| `mimlys-exec` | Built-in tools over capability handles; `Executor`: the worker protocol, the sandbox (Wasmtime), confinement backends per OS, host tools; the `mimlys-worker` binary | core |
| `mimlys-model` | `ModelBackend` adapters: OpenAI-compatible HTTP, fake | core |
| `mimlys-mcp` | `ToolProvider` for MCP servers over stdio: discovery, pinning, calls | core |
| `mimlys` | The CLI: control API in process, approval prompt, configuration loading, logging | all of the above |

The dependency rule is one-directional toward the core and is checked in CI.

## 3. Process model

Two processes: the runtime (inside the CLI in v1) and the worker. The worker performs every execution and is confined by the operating system where the platform allows; its death is an execution failure, never a runtime failure. MCP servers are further child processes with scrubbed environments. The model backend is a separate service the user runs.

## 4. One step

```text
assemble context ─▶ backend.generate ─▶ parse strictly ─▶ validate (12 stages)
      ▲                                                        │
      │                                   rejected ◀───────────┼──────────▶ authorized
      │                                       │                                 │
      │                                   feedback                      approval needed? ─▶ waiting_user
      │                                                                         │
      │                                                                 snapshot_before (write effects)
      │                                                                         │
      │                                                                   executor.run (worker)
      │                                                                         │
      └──────────── observation ◀── normalize ◀── execution record ◀── rollback on failure (write effects)
```

Every arrow that changes state is a durable transaction before the next arrow starts (external design §16.2).

## 5. Recovery

At startup the runtime checks the database, finds tasks that were running, marks their in-flight actions `unknown`, and disposes of each by effect: reversible effects are rolled back and the task halts as `recovered`; non-reversible effects halt the task as `unknown_outcome` with the action listed for the user. Nothing is re-executed automatically.

## 6. Security layers

```text
user authority ─ control API session ─ policy ceiling ─ grants ─ schema ─ authorization before existence
 ─ effects and approvals ─ budgets ─ worker process ─ OS confinement ─ WASI sandbox ─ triple guard ─ audit
```

Each layer assumes the others may fail. The approval screen is rendered by the runtime, never by the model; the only path into the user's tree is promotion.

## 7. What remains stable

Model output is untrusted; execution is validated and effect-classified; grants are explicit and never widen; every bound comes from one register; every transition is durable; unknown outcomes are states; data carries provenance into the prompt; backends, engines, storage and transports are adapters. These anchors outlive any technology choice.

## 8. Later directions

The deferred requirements (requirements §25) name them: plan nodes with dependencies, daemon mode and sessions, an MCP server surface, a model-based summarizer adapter, compiled-language adapters in the confined worker, a vector-memory adapter, audit integrity, policy simulation, metrics export. None of them changes the anchors above.
