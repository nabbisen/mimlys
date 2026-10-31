# Handoff 008 — Acceptance and QA checklist (M4)

- [ ] REQ-MDL-003: chat completions; `max_tokens` = LIM-M2; deadline LIM-M1; cancellation drops the connection; LIM-M3 retries of the identical body on transport, 5xx and 429 only; every failure is `unavailable`, never a fabricated decision; demonstrated against llama-server or Ollama (transcript in the package).
- [ ] REQ-MDL-004 / A-09: `auto`, `schema`, `none`; `auto` resolved at the probe and visible in `status`, `runtime.started` and (when none) a `run` notice; `schema` refused by the server fails at open with the fix; strict parsing in every mode; `malformed` on anything else.
- [ ] REQ-MDL-006: prompt, completion and cached tokens recorded when reported (both OpenAI's and llama-server's field names).
- [ ] REQ-MDL-007 / REQ-SEC-009 / THR-02: the body read through LIM-D1 + 1; an oversized answer is `malformed`.
- [ ] REQ-SEC-010 / D-21 / THR-30: `remote` needs the flag and a profile with `external = true`; refused `denied` with the message otherwise; every shipped profile `external = false`; shown in `status`, `runtime.started` and the task's versions.
- [ ] REQ-CFG-003: the key by reference only; resolved once; never in a log, a prompt or an observation (redaction, handoff 007).
- [ ] LIM-M4: the probe (listing plus mode resolution) within its deadline; `status` reports reachable, model listed, mode; an unreachable backend halts the first step, it does not refuse the command.
- [ ] REQ-MDL-008 / INV-02: the backend is called once per step and for nothing else (grep).
- [ ] The M4 demonstration (ED §23): a local model fixes the JavaScript defect within LIM-T1 steps with `completed{verified}`; segments 1–4 byte-stable (`prefix_chars`); the report exists with LIM-C1 and LIM-C3 proposals.
- [ ] Book: the backend chapter (llama-server and Ollama examples, locality); CHANGELOG 0.4 with the new keys.
