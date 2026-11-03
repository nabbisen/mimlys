# Limits and Defaults

**Status:** Baseline 1.0 (2026-10-02), revised 2026-11-03. Fixed by measurement: LIM-S8 (the M2 report: setup under a millisecond per execution, steady state; the worker's start is the separate LIM-X4), and from the [M4 report](./m4-report.md) LIM-C3, LIM-C4, LIM-M2 and LIM-M4 (kept). **Still provisional: LIM-C1**, which waits for the M4 run against a real local model (the report's run B has not been made). The other values are design choices, not measurements, and change only here.
**Governs:** every number that the [requirements](./requirements.md) and the [external design](./external-design.md) cite by a `LIM-` identifier. The documents never repeat a value; a value changes here only.

"Range" is what an administrator may configure. "fixed" means no configuration changes it. Budgets a user may set per task are bounded by the administrator's ceiling, which is itself within the range.

## Units

| ID | Quantity | Unit |
|---|---|---|
| LIM-U1 | Text budgets | Unicode scalar values (characters), cut only at a character boundary; tokens are used instead where the backend reports them |
| LIM-U2 | Byte budgets | Bytes |
| LIM-U3 | Time | Milliseconds on a monotonic clock for durations; absolute instants as RFC 3339 UTC |
| LIM-U4 | Timestamps in records and output | RFC 3339, UTC, millisecond precision |
| LIM-U5 | Identifiers | UUID version 7, lowercase hyphenated; the CLI accepts any unique prefix |
| LIM-U6 | Hashes | SHA-256 over canonical JSON (keys sorted, no insignificant whitespace, NFC) or over file content; rendered as lowercase hex |

## Task budgets

| ID | Parameter | Default | Range | Basis |
|---|---|---:|---|---|
| LIM-T1 | Steps per task | 40 | 1–1000 | Study: tasks beyond a few attempts signal a defect; local inference is slow |
| LIM-T2 | Consecutive rejections before halt | 3 | 1–20 | A-07 |
| LIM-T3 | Executions per action (1 + runtime retries) | 2 | 1–5 | REQ-TOOL-009 |
| LIM-T4 | Identical consecutive decisions before halt | 3 | 2–10 | M-10; identical means same decision hash with an unchanged workspace snapshot |
| LIM-T5 | Task deadline from creation | 2 h | 1 min – 24 h | Slow local models (study §4) |
| LIM-T6 | Consecutive failed actions before halt | 5 | 1–50 | FR-099 of the draft |

## Decisions and interface sizes

| ID | Parameter | Default | Range | Basis |
|---|---|---:|---|---|
| LIM-D1 | Size of one decision object | 256 KiB | 16 KiB – 4 MiB | R-19; a written file travels inside a decision |
| LIM-D2 | `reason` of a decision | 500 characters | fixed | Shown separately on approval screens |
| LIM-D3 | Plan: items × characters per item | 20 × 200 | fixed | Advisory plan (D-11) |
| LIM-D4 | Goal, question, answer, summary text | 4000 characters | 500–32000 | Context budget |
| LIM-I1 | Tool definitions shown to the model, all tools together | 8000 characters | 2000–32000 | Study: minimize tool prompt text |
| LIM-I2 | Description of one tool | 400 characters | fixed | Clarity rule CLR-8 |
| LIM-I3 | Parameters of one tool | 8 | fixed | CLR-8 |
| LIM-I4 | Nesting depth of a tool's input schema | 2 | fixed | Built-in tools are flat; imported tools up to one nested object |
| LIM-I5 | Length of a notice, hint or error message | 200 characters | fixed | Context budget |
| LIM-I6 | Rendered approval request | 4000 characters | fixed | Readable in a terminal |
| LIM-I7 | Entries returned by `list_dir` or `search_files` | 500 | 50–5000 | Context budget |

## Context

| ID | Parameter | Default | Range | Basis |
|---|---|---:|---|---|
| LIM-C1 | Context budget per model request | 32000 characters | 4000 – 1000000 | About 8k tokens of English; per-backend override |
| LIM-C2 | Observation excerpt (head + tail) | 4000 characters (3000 + 1000) | 500–64000 | Compiler errors cluster at the tail |
| LIM-C3 | Recent steps kept in full | 6 | 1–50 | Prefix stability |
| LIM-C4 | Summary of a compressed step | 300 characters | fixed | Rule-based summaries (D-12) |
| LIM-C5 | Steps of a forked task's history shown as context | 20 | 1–200 | REQ-TASK-004; the decision and outcome of each, no file content |

## Sandbox

| ID | Parameter | Default | Range | Basis |
|---|---|---:|---|---|
| LIM-S1 | Fuel per execution | 10^10 | 10^6 – 10^12 | Deadline usually fires first; fuel gives determinism |
| LIM-S2 | Deadline of one execution | 30000 ms | 1000 – 600000 | Study |
| LIM-S3 | Memory of one instance | 256 MiB | 16 – 4096 MiB | Study |
| LIM-S4 | Captured stdout and stderr, each | 1 MiB | 64 KiB – 64 MiB | Study |
| LIM-S5 | Size of a module or interpreter | 64 MiB | operator | Interpreter builds are tens of MiB |
| LIM-S6 | Clock granularity visible to the sandbox | 1 ms | fixed | SEC-008 of the draft, R-21 |
| LIM-S7 | Epoch tick | 10 ms | fixed | Study |
| LIM-S8 | Sandbox overhead ratio target | below 5 % | measured in M2 | Study Q-003 |

## Workspace

| ID | Parameter | Default | Range | Basis |
|---|---|---:|---|---|
| LIM-W1 | Workspace size (best effort, measured at write time and step boundaries) | 1 GiB | 64 MiB – 64 GiB | R-18 |
| LIM-W2 | Files in a workspace | 200000 | operator | Manifest size |
| LIM-W3 | Single file written by a tool | 8 MiB | 64 KiB – 1 GiB | Context and snapshot cost |
| LIM-W4 | Materialization exclusions (default) | `.git/`, `.hg/`, `.svn/`, `target/`, `node_modules/` | operator | VCS metadata and build outputs |
| LIM-W5 | Secret patterns excluded from materialization and reading (default) | `.env`, `.env.*`, `*.pem`, `*.key`, `id_*`, `.ssh/`, `.aws/`, `.gnupg/` | operator | R-16, D-22 |

## Concurrency

| ID | Parameter | Default | Range | Basis |
|---|---|---:|---|---|
| LIM-X1 | Tasks in `running` at once | 1 | 1–16 | v1 is sequential (D-11); one task per project scope |
| LIM-X2 | Executions in flight, whole runtime | 1 | 1–8 | Memory: LIM-X2 × LIM-S3 |
| LIM-X3 | Worker or MCP server restarts per task | 3 | 0–10 | Fault containment |
| LIM-X4 | Worker start, including interpreter compilation | 60000 ms | 5000 – 600000 | Cranelift compiles a multi-megabyte interpreter once per worker |

## Approvals and questions

| ID | Parameter | Default | Range | Basis |
|---|---|---:|---|---|
| LIM-A1 | Expiry of an approval request or question | none | none, 1 min – 7 d | A-11; a waiting task holds no resources |
| LIM-A2 | Pending user items per task | 1 | fixed | One action at a time |

## Persistence and retention

| ID | Parameter | Default | Range | Basis |
|---|---|---:|---|---|
| LIM-P1 | SQLite journal and synchronous mode | WAL, FULL | fixed | D-15 |
| LIM-P2 | Operational blobs (raw outputs) after a task is terminal | no timer: deleted with the task (`task delete`) | — | RFC 006: ownership instead of collection |
| LIM-P3 | Snapshots and workspaces after a task is terminal | no timer: deleted with the task (`task delete`) | — | RFC 006: the user sees the footprint and decides |
| LIM-P4 | Audit events | no timer: kept until `task delete --audit` | — | RFC 006: audit stays longest |
| LIM-P5 | Integrity check at startup | every start | fixed | REQ-PERS-006 |

## Operation and timeouts

| ID | Parameter | Default | Range | Basis |
|---|---|---:|---|---|
| LIM-O1 | Time to exit after a stop signal | 5000 ms | 1000 – 30000 | Persist, then kill the worker |
| LIM-O2 | Time until a cancelled execution's tree is terminated | 2000 ms | 500 – 10000 | FR-007 of the draft |
| LIM-O3 | Wait for the runtime lock before refusing to start | 1000 ms | fixed | Single writer |
| LIM-O4 | Deadline of a built-in tool invocation | 30000 ms | 1000 – 300000 | File operations |
| LIM-O5 | Deadline of an external (MCP) tool invocation | 60000 ms | 1000 – 600000; never below the server's declared maximum | rypsok expectation 5 |
| LIM-O6 | Startup of an MCP server | 15000 ms | 1000 – 120000 | Child process |

## Model backend

| ID | Parameter | Default | Range | Basis |
|---|---|---:|---|---|
| LIM-M1 | Deadline of one model request | 600000 ms | 10000 – 3600000 | 3–8 tok/s local inference (study §4) |
| LIM-M2 | Maximum completion tokens | 4096 | 256 – 65536 | Decision plus a written file |
| LIM-M3 | Transport retries of a model request | 2 | 0–5 | Identical request; never a different one |
| LIM-M4 | Backend health probe timeout | 5000 ms | fixed | Startup status |

## Host tools (M6)

| ID | Parameter | Default | Range | Basis |
|---|---|---:|---|---|
| LIM-H1 | Deadline of a host tool execution | 600000 ms | 10000 – 3600000 | Builds and test suites |
| LIM-H2 | Memory of a host tool tree | 2 GiB | 256 MiB – 32 GiB | Compilers |
| LIM-H3 | Processes in a host tool tree | 64 | 1–1024 | Fork-bomb bound |
| LIM-H4 | Captured output of a host tool, each stream | 4 MiB | 64 KiB – 64 MiB | Build logs |
| LIM-H5 | Names refused directly under a declared read path (REQ-CONF-011) | LIM-W5's patterns, `credentials`, `credentials.*`, `.netrc`, `.npmrc`, `.pypirc`, `.docker/` | fixed | Registry and cloud tokens live beside toolchains (`~/.cargo/credentials.toml`) |
