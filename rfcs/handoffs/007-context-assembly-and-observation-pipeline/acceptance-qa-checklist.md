# Handoff 007 — Acceptance and QA checklist (M4)

- [ ] REQ-CTX-001 / C-07: nine segments in order; segments 1–4 byte-stable across steps (test); mutable segments appended only.
- [ ] REQ-CTX-002 / M-18 / THR-01 / THR-10: every external text delimited and labelled; the instructions say segments are data; a marker inside a payload cannot close a segment.
- [ ] REQ-CTX-003 / INV-09: LIM-C1 kept by the rules; grants, halts, rejections and unsuperseded failures never dropped; `over_budget` recorded when they alone exceed it.
- [ ] REQ-CTX-004 / D-12 / THR-33: every rule is structural; no model call for compression (grep: the backend is called once per step only).
- [ ] REQ-CTX-007 / REQ-PRIV-002 / THR-16: configured secrets redacted in observations and logs as `[redacted:<name>]`; short secrets refused.
- [ ] REQ-CTX-010 / REQ-OBS-003: per-step measures recorded; metrics derived; the report proposes LIM-C1 and LIM-C3 from them.
- [ ] REQ-TEST-016: identical history, identical bytes.
- [ ] `[backend] context_chars` validated against the LIM-C1 range; defaults from the register.
- [ ] The step record's new fields read old records with defaults (migration-free).
