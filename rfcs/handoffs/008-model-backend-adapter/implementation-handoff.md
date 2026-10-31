# Handoff 008 — The OpenAI-compatible backend adapter (M4)

**RFC.** [RFC 008](../../accepted/008-model-backend-adapter.md) (Accepted)
**Specification.** REQ-MDL-001 … 008; REQ-SEC-009; REQ-SEC-010; REQ-CFG-003; REQ-OBS-006; external design §3, §19.1, §20, §23 (M4); limits LIM-M1 … M4, LIM-D1, LIM-T1.
**Crates.** `mimlys-model` (`openai.rs`, `openai/schema.rs`, `openai/tests.rs` with a mock server), `mimlys-core` (`Profile.external`, `TaskVersions.backend`), `mimlys` (configuration keys, the probe in `status`, the demonstration script and the report).
**Milestone.** M4, with handoff 007. Depends on 001–006, 010.

## 1. Scope

In: the adapter with its modes, retries, deadline, cancellation, bound and usage; the probe; locality enforcement per profile; the backend descriptor on the task; the mock-server tests; the demonstration run against a local engine and the report.

Out: streaming; tool-calling APIs; native bindings; `seed`/`top_p` (open question).

## 2. Module layout

```text
crates/mimlys-model/src/
  openai.rs        OpenAiBackend::new(OpenAiConfig{url, model, api_key: Option<String>, timeout_ms, max_tokens, temperature, structured_output, locality}) ; ModelBackend impl ; probe(deadline) -> Probe{reachable, model_listed, structured_output: Resolved, detail} resolving `auto` with one-token completions ; resolved_mode()
  openai/schema.rs the decision envelope's JSON schema (draft 2020-12), generated from the decision kinds; `strict: true` with additionalProperties false on the envelope, `arguments` an open object
  openai/tests.rs  a mock server on 127.0.0.1 (tokio + hyper or a hand-written HTTP/1.1 responder): happy path, refuses response_format (400 naming it), json_object fallback, 500 then 200 (retry), always 500 (unavailable after LIM-M3), 429 with Retry-After, slow (deadline), oversized body (LIM-D1), no usage, llama-server timings, Authorization header seen, cancellation drops the connection
crates/mimlys-core/src/policy.rs   Profile.external: bool (default false; serde default): may the task communicate outside the host
crates/mimlys-core/src/task.rs     TaskVersions.backend: BackendDescriptor{kind, model, locality}
crates/mimlys/src/config.rs        [backend] temperature (default 0.0, 0.0–2.0), context_chars (RFC 007); api_key resolved at App::open for the adapter and the redaction set
crates/mimlys/src/app.rs           builds OpenAiBackend when kind = openai_compatible; refuses a remote backend without the flag (exists) ; runs the probe for step-running commands and status; `run` prints one notice when auto resolved to none
crates/mimlys/src/commands/status.rs  backend: kind, model, locality, reachable, model_listed, structured_output mode
crates/mimlys/tests/m4_demo.rs     the demonstration (ignored unless MIMLYS_M4_BACKEND_URL is set) and the mock-server loop test
docs/src/maintainers/m4-report.md  the report
```

## 3. Behaviours

### 3.1 Request and response (REQ-MDL-003)

Body: `{model, messages: [{role, content}], max_tokens, temperature, stream: false, response_format?}`. Headers: `Content-Type: application/json`, `Authorization: Bearer …` when configured. Response: `choices[0].message.content` (string) is returned as bytes to the loop; missing or non-string → `Unavailable{detail: "response_shape"}`; `usage` parsed into `Usage{prompt_tokens, completion_tokens, cached_tokens}` from `usage.prompt_tokens_details.cached_tokens` or `timings.cache_n`, kept for `last_usage`. The reader stops at LIM-D1 + 1 bytes: a longer body is returned as those bytes so the strict parser rejects it `malformed` (consistent with the fake backend's cut).

### 3.2 Modes (REQ-MDL-004)

`schema`: `response_format = {type: "json_schema", json_schema: {name: "mimlys_decision", strict: true, schema}}`; when the probe's one-token completion with it is refused, `App::open` fails `backend unusable: the server refuses json_schema; set structured_output = "none" or "auto"`. `auto`: resolved by the probe: a one-token completion (`messages: [{role: user, content: "ok"}]`, `max_tokens: 1`) with `json_schema`; a 400 whose body mentions `response_format`, `json_schema` or `schema` → the same with `{type: "json_object"}`; another such 400 → none. The resolved mode is kept for the process (`resolved_mode`), recorded in `runtime.started{backend.structured_output}`, shown by `status`, and `run` prints `structured output: none (the server refused json_schema and json_object)` once when that is the outcome. `none`: never sends the parameter. Probe completions count against nothing. A process whose probe did not run (a command that needs no backend) has no resolved mode and sends no step.

### 3.3 Retries, deadline, cancellation

A transport error, a 5xx or a 429 → wait 250 ms × attempt (429 honours `Retry-After` up to 5 s), resend the identical body, at most LIM-M3 times; then `Unavailable{detail: "transport" | "status:NNN"}`. The whole call observes LIM-M1 through `tokio::time::timeout`; the request future is dropped on `cancel.is_cancelled()` polled every 100 ms → `BackendError::Cancelled`. Other 4xx → `Unavailable{detail: "status:NNN"}` without retry.

### 3.4 Probe (LIM-M4)

`GET {url}/models` with the auth header, then the resolving completions of §3.2, all within LIM-M4; `reachable` when the listing answers 2xx; `model_listed` when `data[].id` contains `model` (Ollama and llama-server both list). Run by `App::open` only for commands that run steps or `status`; its result goes to `status` (`backend: {kind, model, locality, reachable, model_listed, structured_output, detail}`) and to `runtime.started{backend}`. An unreachable backend does not refuse the command: the first step halts `backend_unavailable`.

### 3.5 Locality (REQ-SEC-010)

`create_task`: when the backend's locality is `remote` and the profile's `external` is false → `denied{reason: "profile <name> forbids external communication (external = false)"}`. Every shipped profile has `external = false`; a user who wants a remote backend writes a profile with `external = true` (and the administrator sets the flag). `TaskVersions.backend` records kind, model and locality; `status` and `runtime.started` show the locality (the event gains `backend: BackendDescriptor`).

### 3.6 The demonstration (ED §23, M4)

A scope with `tools/sum.js` (a function with an off-by-one) and `tests/sum.test.js` (a script that prints `ok` or exits 1); goal: "make tests/sum.test.js pass by fixing tools/sum.js"; profile `workspace`; completion check `run_script{language: javascript, source: <the test file's content>}` … or simply the test file through `run_script`. Run with `MIMLYS_M4_BACKEND_URL` (llama-server or Ollama; the report pins the engine command and the model file; a 7B-class coder instruct model in 4-bit fits the reference machine's 16 GB GPU) and `MIMLYS_M4_MODEL`; assert `completed{verified}` within LIM-T1 steps; print the steps, the context measures and the usage; the prefix test asserts `prefix_chars` ≥ the stable prefix's length on every step after the first. Without the variable the test is skipped; the mock-server loop test runs the same scope with scripted answers.

## 4. Tests

- The mock-server suite of §2 (`openai/tests.rs`), each case asserting the request body seen by the server (identical bodies across retries; `response_format` per mode; the auth header; `max_tokens` = LIM-M2) and the adapter's result.
- Configuration: `temperature` range; `api_key` shorter than eight characters refused; `remote` without the flag refused (exists); profile `external` parsed, default false; `schema` with a refusing server fails at open with the message.
- Locality: a remote backend with a profile whose `external` is false → `denied` at creation with the message (runtime test with a fake remote backend); `external = true` admits it.
- Status JSON gains the backend object; the fake backend reports `reachable: true, structured_output: "none"`; `auto` resolution: all three outcomes against the mock, each visible in status and `runtime.started`.
- `m4_demo.rs`: skipped without the URL; the mock loop test always.

## 5. Recommended crates

`reqwest` (default features off; `rustls-tls`, `json`), `schemars` is not needed (the schema is small and hand-written), `tokio` (exists). For the mock server: `hyper` 1 with `hyper-util` and `http-body-util` as dev-dependencies, or a minimal blocking `std::net::TcpListener` responder (fewer dependencies; chosen unless the suite needs HTTP/1.1 keep-alive).

## 6. Return

Review-request package under `.git-exclude/reviewed/m4-007-008/` with the mock-suite results, the demonstration transcript against a local engine (model, engine, version, numbers), the report `docs/src/maintainers/m4-report.md` with the proposed LIM-C1, LIM-C3 (and LIM-M2 if the completion bound needed changing), and the `status` JSON shape.

## 7. Amendments made during implementation (2026-10-29)

- **One retry mechanism.** The adapter does not retry (the loop's LIM-M3 identical retries of M1 remain the only retries, keeping INV-02's one call site). After a 429 or a 5xx the adapter pauses 250 ms, or `Retry-After` up to five seconds, before returning `unavailable{status:NNN}`, so the loop's retry is spaced. Other 4xx, a dropped connection (`transport`) and a body that is not the expected shape (`response_shape`) return at once.
- **The bound.** The HTTP body is read through `max_bytes + 64 KiB` of envelope; an oversized body or an oversized `content` comes back cut at `max_bytes + 1` bytes, which the strict parser rejects `malformed`, exactly as the fake backend does.
- **`auto` resolution.** Lives in the adapter (`resolve`) and runs at the probe; should `generate` be called on an unresolved adapter (a harness without the probe), it resolves first by the same chain, so no request ever goes out with an undecided format. `schema` is verified only at the probe; a refusing server is the startup error of RFC 008 §2.
- **Probe.** `App::probe_backend` is an explicit async step after `App::open`, run by `main` for `run`, `task new`, `task resume`, `approve`, `reject`, `answer` and `status`; the fake backend reports `reachable: true, structured_output: none`.
- **Status.** The runtime's status carries the backend descriptor; the CLI merges it with the probe into one `backend` object and drops `backend_kind`/`backend_locality` (0.4 contract change, in the changelog).
- **`ModelBackend::describe`** is the trait's new required method; `locality()` is derived from it.
- **The demonstration workload.** `run_script` takes source text, not a path, so the test file imports the module as `/workspace/tools/sum.js` and the completion check carries the test file's content; a failing test throws (the sandboxed QuickJS build has no `std`), which ends the execution `failed{tool_error}`; `replace_in_file` takes `find`/`replace`.
- **Dependencies.** `reqwest 0.12` (rustls, no default features) in `mimlys-model`; the mock server is `std::net` only, behind the `mock-server` feature, used by the adapter's tests and the CLI's M4 tests.
- **The M1 follow-mode guard** that refused `openai_compatible` is removed with its `AvailableFrom` error.
