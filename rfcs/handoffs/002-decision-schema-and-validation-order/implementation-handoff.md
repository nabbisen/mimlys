# Handoff 002 — Decision schema, validation pipeline and built-in tools

**RFC.** [RFC 002](../../done/002-decision-schema-and-validation-order.md) (Accepted 2026-10-27)
**Specification.** Requirements REQ-ARCH-004, 005; REQ-DEC-002 … 009, 011, 012; REQ-TOOL-001, 003 … 008; REQ-WS-011; REQ-CTX-005, 006, 008, 009; REQ-SEC-009; REQ-MDL-001, 002; external design §3, §6.2–6.4, §8, §11, §15.2; limits LIM-D1 … D4, LIM-I1 … I7, LIM-C2, LIM-W3, LIM-O4.
**Crates.** `mimlys-core` (schema, parser, validator, tool model, SPI, observation); `mimlys-exec` (built-in file tools over capability handles); `mimlys-model` (fake backend).
**Milestone.** M1. Depends on handoff 001.

## 1. Scope

In: the decision schema v1 and its strict parser; the tool definition model and input-schema model; the twelve-stage validator producing `AuthorizedAction` or `Rejection`; path canonicalization; the adapter SPI traits; the observation type with notices; the six M1 built-in file tools; the fake model backend and fake tool provider.

Out: effects, grants, approvals (handoff 003: the validator calls into them through traits defined here); persistence (004); the CLI (010); `run_script`, `run_wasm` (M2).

## 2. Module layout

```text
crates/mimlys-core/src/
  decision.rs    decision/tests.rs   Decision enum (ED §6.2), `parse_strict(&[u8]) -> Result<Decision, Rejection>`
  tool.rs        tool/tests.rs       ToolDefinition, ParamSchema, ResourceProfile, Provider, destination arguments (REQ-TOOL-001)
  path.rs        path/tests.rs       `canonicalize(workspace_root, input) -> Result<WorkspacePath, PathError>` (lexical only)
  validate.rs    validate/tests.rs   the pipeline; `Rejection {code, detail, hint}`; `AuthorizedAction` (constructible only here)
  observation.rs observation/tests.rs Observation, Notice, excerpt(head, tail) per LIM-C2
  spi.rs                             ModelBackend, ToolProvider (ED §3); Persistence, SnapshotStore, Executor, ApprovalChannel arrive with handoffs 004, 006, 005, 003
  context.rs     context/tests.rs    M1 minimum: the delimited segment writer of ED §15.2 and tool-definition rendering (LIM-I1); full assembly is M4
crates/mimlys-exec/src/
  builtin.rs     builtin/            read_file.rs, write_file.rs, replace_in_file.rs, list_dir.rs, search_files.rs, delete_path.rs, tests.rs
crates/mimlys-model/src/
  fake.rs        fake/tests.rs       scripted backend
```

## 3. Behaviours

### 3.1 Decision and parsing (ED §6.2, §6.3 stages 2–3; REQ-DEC-002, 003; REQ-SEC-009)

- `Decision`: `Act{tool, arguments: serde_json::Map, reason: Option<String>}`, `Plan{steps}`, `AskUser{question}`, `Complete{summary}`, `GiveUp{reason}`; `#[serde(deny_unknown_fields, tag = "decision", rename_all = "snake_case")]`.
- `parse_strict`: refuse before parsing when the input exceeds LIM-D1 (`malformed{too_large}`); use a streaming deserializer so that a second value or trailing non-whitespace yields `malformed{multiple_values | trailing_text}`; duplicate keys yield `malformed{duplicate_key}` (implement a `Visitor` that tracks keys; `serde_json` accepts duplicates by default, so this must be explicit); string bounds LIM-D2/D3/D4 yield `schema` with `path`. Never transform the bytes before parsing (CLR-2).
- `Decision::hash()` uses handoff 001's canonical hash.

### 3.2 Tool model (ED §11; REQ-TOOL-001)

- `ParamSchema`: an object of at most LIM-I3 named parameters, each `String{min, max, pattern?} | Integer{min, max} | Boolean | Enum{values} | Array{of: primitive, max_len} | Object{…, depth ≤ LIM-I4}`; `validate(&self, &Map) -> Result<(), SchemaError>` with unknown properties refused. This is deliberately not full JSON Schema; MCP import (M5) converts a subset into it.
- `ToolDefinition { name, description (≤ LIM-I2), input: ParamSchema, output: Option<ParamSchema>, effect: EffectDecl (from handoff 003's types; until 003 lands, use the `EffectView` of handoff 001), deadline_ms, profile: Builtin|Sandbox|Host, destination_arguments: Vec<String>, provider }`.
- `render_definitions(&[ToolDefinition]) -> String` bounded by LIM-I1; refuse to render above the budget (the task cannot start with an oversized tool set; error kind `denied` with detail `tool_definitions_too_large`).

### 3.3 Path canonicalization (ED §6.3 stage 6; REQ-DEC-006, 007; REQ-WS-011)

- Lexical only: no filesystem access. Normalize separators, collapse `.` and `..`, refuse absolute paths and any `..` that would leave the root, refuse NUL, refuse names invalid on the current platform (Windows reserved names, trailing dot/space, forbidden characters) with `invalid_argument`, apply NFC, and produce `WorkspacePath` (relative, forward-slash form). Case folding: a `CaseMode { Sensitive, Insensitive }` passed by the caller (determined once per workspace by handoff 004/010 by probing the filesystem); comparisons for exclusions use it.
- Exclusion check (LIM-W4, LIM-W5 patterns, from the policy of handoff 003) happens at stage 7 and yields `denied{path_excluded}`.

### 3.4 Validator (ED §6.3; REQ-DEC-004 … 008; REQ-ARCH-005)

- `validate(ctx: &ValidationContext, raw: &[u8]) -> Outcome` where `Outcome = Authorized(AuthorizedAction) | Rejected(Rejection) | Control(ControlDecision)` (`plan`, `ask_user`, `complete`, `give_up` are control decisions handed to the loop).
- Stages 1–12 as separate functions called in order; a stage never runs when an earlier one failed; stage 7 (authorization) before stage 8 (semantic bounds) and before any filesystem access, which only the executor performs (REQ-DEC-007, INV-03). The `ValidationContext` carries the task state, the visible tools, the workspace root and case mode, the exclusion patterns, and the hooks of handoff 003 (`GrantCheck`, `EffectDerivation`, `BudgetReservation`) as trait objects so this crate compiles before 003 lands (provide no-op test impls).
- `AuthorizedAction` has a private constructor; the `Executor` and `ToolProvider` traits take `&AuthorizedAction` (REQ-ARCH-005, INV-01). Add a compile-fail test (`trybuild`) or a doc test showing it cannot be built outside `validate`.
- `Rejection { code: RejectionCode, detail: Detail (typed per code, ED §6.4), hint: &'static str }`; hints are a table in `validate/hints.rs`, each ≤ LIM-I5; `detail` never carries filesystem contents.

### 3.5 Observations (ED §8; REQ-CTX-006, 008, 009; REQ-TOOL-005, 008)

- `Observation` exactly as ED §8; `payload` is the tool's JSON validated against its output schema when declared (violation → `FailureCause::OutputInvalid`, never passed through); `excerpt(bytes, head, tail)` cuts at character boundaries (LIM-U1) and adds `output_truncated{stream,total_bytes}`; `sanitize(text) -> (String, removed_count)` strips control, zero-width, tag and bidirectional characters (REQ-CTX-008) and adds `characters_removed`; invalid UTF-8 replaced with `invalid_utf8_replaced`.
- `context::write_segment(label, attrs, json)` emits the marker form of ED §15.2; a test proves that a payload containing the closing marker on its own "line" cannot produce a marker line (the newline is escaped by JSON encoding).

### 3.6 Adapter SPI (ED §3; REQ-ARCH-004; REQ-MDL-001)

`ModelBackend` and `ToolProvider` as in ED §3, returning boxed sendable futures so they can be used through `dyn`. `ModelBackend::generate` returns bytes, bounded by LIM-D1 while reading (REQ-MDL-007) — the backend truncates and flags, the parser rejects. The four other traits are defined by the handoffs that own their method sets.

### 3.7 Built-in file tools (ED §11; REQ-TOOL-003, 004, 006, 007, 008; `mimlys-exec::builtin`)

- Every tool opens through a `cap_std::fs::Dir` handle rooted at the workspace (`Dir::open_ambient_dir` once at task start, by handoff 010); never through `std::fs` with an absolute path; `cap-std` refuses symbolic-link escapes at open time (REQ-TOOL-004). Pass the `WorkspacePath` from the validator; the tool never canonicalizes.
- Schemas and results exactly as ED §11: `read_file` (line-based `start_line`, `max_chars` ≤ LIM-C2 ceiling, `content_truncated` notice), `write_file` (≤ LIM-W3; missing parent directories are created and counted in `parents_created`, because there is no directory tool and a model must be able to add a module), `replace_in_file` (`no_match`/`ambiguous`), `list_dir` (depth 1–3, LIM-I7, `entries_truncated`; symbolic links listed as type `symlink` without their target, never descended), `search_files` (regex with a size bound; LIM-I7; `glob` via `globset`; links, files over LIM-W3 and unreadable files skipped and counted in `entries_skipped`), `delete_path` (`recursive` required for non-empty directories; refuse the root).
- Deadline: each invocation runs under `tokio::time::timeout(min(LIM-O4, task remaining))` and checks a cancellation token between I/O operations (REQ-TOOL-006, 007).
- Quota: `write_file` and `replace_in_file` refuse a write that would exceed LIM-W3, and report the bytes written so handoff 004 can account LIM-W1 at step boundaries (REQ-WS-008).

### 3.8 Fakes (REQ-MDL-002, REQ-TEST-002)

- `FakeBackend::from_script(path)`: a JSON array of responses; each entry is either raw bytes to return verbatim (to test malformed output) or a `Decision`; optional per-entry `delay_ms` and `fail: "unavailable"`; exhausted script → `BackendError::ScriptExhausted`. Records every request for assertions.
- `FakeToolProvider`: declares tools from a table and returns scripted `Execution` records; used by handoffs 003/004/010.

## 4. Tests (REQ-TEST-004, 005, 002)

- Hostile decisions: oversized, nested beyond the schema, duplicate keys, trailing text, two values, invisible characters in `tool`, wrong types, unknown fields, each with the expected code and `detail` (`req_dec_003_*`, `req_sec_009_*`).
- Paths: `../x`, `a/../../x`, `/etc/shadow`, `C:\x`, `NUL`, `a.`, `a `, NFC/NFD pairs, case pairs under both modes, symbolic-link escape at execution refused by `cap-std` (`req_dec_007_*`, `req_ws_011_*`, `req_tool_004_*`); and the existence test: `denied` for an out-of-scope existing path and a non-existing one are byte-identical.
- Stage ordering: a decision that fails stages 7 and 8 reports stage 7's code; no stage runs after a failure (`req_dec_004_*`).
- `AuthorizedAction` cannot be constructed outside the validator (`req_arch_005_*`).
- Each built-in tool against ED §11 including every notice; deadline and cancellation (`req_tool_003_*`, `req_tool_006_*`, `req_tool_007_*`, `req_tool_008_*`).
- Marker unforgeability (`req_ctx_002_*`); sanitization (`req_ctx_008_*`); excerpt boundaries (`lim_c2_*`).
- Fake backend script round-trip and exhaustion (`req_mdl_002_*`).

## 5. Recommended crates

`serde`, `serde_json` (features `raw_value`), `cap-std`, `regex`, `globset`, `tokio` (`time`, `sync`), `tokio-util` (cancellation token), `unicode-normalization`, `thiserror`; `trybuild` as a dev dependency if used.

## 6. Return

Review-request package under `.git-exclude/reviewed/m1-002/` as in handoff 001's §6, plus the list of every hint string (they are part of the model-facing contract).
