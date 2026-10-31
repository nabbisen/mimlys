# Handoff 008 — PR plan (M4)

| PR | Content | Depends on | Size |
|---|---|---|---|
| 008-1 | `openai/schema.rs` (the envelope schema) and `openai.rs` (request, response, usage, bound, modes, retries, deadline, cancellation); the mock server and its suite | — | L |
| 008-2 | Configuration: `temperature`, the key's length rule; `Profile.remote_backend`; `TaskVersions.backend`; locality refusal at creation; `runtime.started{backend}` | — | M |
| 008-3 | `App::open` builds the adapter; the probe; `status` with the backend object; redaction set from the key (with 007-3) | 008-1, 008-2 | S |
| 008-4 | `tests/m4_demo.rs`: the scope, the mock loop test, the real-engine run behind `MIMLYS_M4_BACKEND_URL` | 008-3, 007-2 | M |
| 008-5 | The demonstration run on the reference machine; `docs/src/maintainers/m4-report.md`; book chapters (a real backend, locality); CHANGELOG 0.4 | 008-4 | M |
