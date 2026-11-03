# RFC 008 — The model backend adapter

**Status.** Implemented (0.7.0; accepted 2026-10-29)
**Tracks.** Decision D-21 of the startup review; findings A-09, M-01, M-15, R-15, R-19; threats THR-02, THR-16, THR-30.
**Touches.** `mimlys-model` (the OpenAI-compatible adapter), `mimlys` (configuration, the backend probe in `status`, the demonstration), `mimlys-core` (the profile's `remote_backend` key, the SPI's usage report), external design §3, §19.1, §20; requirements REQ-MDL-003 … 006, REQ-SEC-010.

## Summary

Ship one real backend: an adapter for OpenAI-compatible chat completions, which covers llama-server and Ollama. It sends the assembled messages with a deadline (LIM-M1), a completion bound (LIM-M2), at most LIM-M3 retries of the identical request on transport failures, a structured-output mode resolved once per process, and a bounded read of the answer (LIM-D1). It reports usage and cache hits when the server does. It never fabricates a decision: failure is `backend_unavailable`. A remote backend needs the administrator's flag and the profile's consent, and is shown in status and audit.

## Motivation

- The fake backend proved the loop; a model has to drive it now (M-15, the roadmap's M4).
- Constrained decoding is not available everywhere; the runtime must behave the same with or without it (A-09): strict parsing, no repair, a fixed rejection code.
- A backend is the one place where the task's context leaves the process; locality must be explicit and auditable (D-21, R-15, THR-30).
- Model output is text until validated and must be bounded while read (R-19, THR-02).

## Decision

The normative text is external design §3 (adapter SPI), §19.1 (`[backend]`), §20 (backend unreachable), requirements REQ-MDL-001 … REQ-MDL-008, REQ-SEC-009, REQ-SEC-010, and limits LIM-M1 … LIM-M4, LIM-D1. In brief:

1. **Protocol.** `POST {url}/chat/completions` with `model`, `messages` (the assembler's roles and contents, unchanged), `max_tokens` = LIM-M2, `temperature` (`[backend] temperature`, default 0), `stream = false`, and `response_format` by mode. The answer's `choices[0].message.content` is the decision text; `usage` is recorded when present (`prompt_tokens`, `completion_tokens`, `prompt_tokens_details.cached_tokens` or llama-server's `timings` cache count). `Authorization: Bearer <key>` when `api_key` is configured, resolved at start from its reference and never logged.
2. **Structured-output modes (REQ-MDL-004).** `schema` sends `response_format = {type: "json_schema", json_schema: {name, strict: true, schema}}` with the decision envelope's schema (the five decision kinds and their fields; tool arguments stay an open object, validated by the runtime's twelve stages, REQ-DEC-004); `none` sends no `response_format`; `auto` resolves to one of them **at the probe, before any task runs**: a one-token completion with `json_schema`, on a refusal the same with `json_object`, on a refusal none. The resolved mode is recorded in `runtime.started{backend}`, shown by `status`, and announced once by `run` when it is `none` after a fallback, so nobody believes a schema is enforced when it is not. `schema` on a server that refuses it is a startup error naming the fix (`structured_output = "none"` or `"auto"`), not a silent change. In every mode the text is parsed strictly (REQ-DEC-003): a failure is the rejection `malformed`, never a repair; the mode changes how often the model succeeds, never what the runtime accepts.
3. **Failure, retries, deadline, cancellation (REQ-MDL-003).** A transport failure, a 5xx or a 429 is retried with the identical request at most LIM-M3 times with a short back-off; any other 4xx, a body that is not the expected JSON, or an exhausted retry budget is `BackendError::Unavailable` with a fixed detail; the loop then halts `backend_unavailable` (handoff 010). The request as a whole observes LIM-M1; cancellation drops the connection within one poll interval. The body is read through a bound of LIM-D1 plus one byte; a longer body is `malformed` (REQ-SEC-009).
4. **Probe (LIM-M4).** At start, for commands that run steps and for `status`, `GET {url}/models` within LIM-M4 says whether the server answers and whether `model` is listed, and the one-token completion of point 2 resolves `auto`; the whole probe observes LIM-M4. `status` shows reachable or not with the reason, whether the model is listed, and the resolved mode. The probe never blocks a command that needs no backend, and a backend that fails the probe halts the first step `backend_unavailable` rather than refusing the command (the server may come up).
5. **Locality (REQ-SEC-010, D-21).** `locality = remote` needs the administrator's `policy.remote_backend_allowed` (exists) and a profile that permits external communication: the profile key `external` (new; false in every shipped profile; one meaning: *this task may communicate outside the host*). The same key governs tools with external effects from M5 on, so there is one concept, not a switch per channel. A task created under a profile with `external = false` while the backend is remote is refused `denied{reason: "profile <name> forbids external communication (external = false)"}`. The locality is shown by `status` and recorded in `runtime.started` and on the task's versions (`backend` descriptor: kind, model, locality).
6. **Secrets (REQ-PRIV-002).** The key's resolved value joins the redaction set of RFC 007; the configuration refuses a key shorter than eight characters.
7. **The fake backend stays** the backend of tests, replay and CI; the adapter's own tests run against a local mock server speaking the protocol (reachable, refusing `response_format`, slow, failing, oversized, with and without usage). The demonstration against a real local engine is a documented, reproducible run (`tests/m4_demo.rs` behind `MIMLYS_M4_BACKEND_URL`; the exact engine command and the pinned model in the report) whose transcript and numbers go into the M4 report when an engine is available on the reference machine; the implementation does not wait for it.

## Alternatives considered

- Streaming: earlier cancellation and progress, but a chunked body complicates the bound and the strict parse for no decision-time gain in v1. Deferred; the SPI does not preclude it.
- Native engine bindings (llama.cpp in process): the fastest path, but it would put inference into the runtime process and tie the build to a GPU stack. Rejected (D-21: adapters over HTTP).
- Tool-calling APIs (`tools`, `tool_calls`): a second decision grammar to keep in step with the schema; the decision object already is the call. Rejected for v1.
- Repairing almost-JSON: hides model drift and makes rejections invisible (A-09). Rejected.

## Consequences

- `mimlys-model` gains `openai.rs` with `reqwest` (rustls); the fake stays.
- Profiles gain `external`; `[backend]` gains `temperature` and (RFC 007) `context_chars`; `status` gains the probe result and the resolved structured-output mode; `runtime.started` gains `backend`.
- The task's versions record the backend descriptor; the changelog announces the new keys.
- M4's demonstration (external design §23): a local model repairs a scripted JavaScript defect within LIM-T1 steps; the prefix of segments 1–4 byte-stable across the run; the report with context, compression and cache numbers.

## Open questions

- Whether to expose `seed` and `top_p` as configuration; decided from the report's variance.

## References

Requirements §19 (REQ-MDL), REQ-SEC-009, REQ-SEC-010; external design §3, §19.1, §20, §23; limits LIM-M1 … M4, LIM-D1; startup review D-21, A-09, M-15.
