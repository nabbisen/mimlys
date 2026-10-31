# Handoff 011a — Acceptance and QA checklist (M6a)

- [ ] REQ-CONF-001 / D-06 v2: the seven clauses implemented on Linux with the ladder of RFC 011 §2; each state names its primitive.
- [ ] REQ-CONF-002 / REQ-CONF-010: the worker confines itself before its first frame; `unsafe` only in `mimlys-exec/confine` (`check-unsafe.sh`), none for Linux.
- [ ] REQ-CONF-003: the probe is a real worker's report; `status` shows every clause with its primitive or reason.
- [ ] REQ-CONF-004 / THR-23: `run_host_tool` invisible and unavailable below tier `full`; accepted best-effort explicit, visible in `status` and in approvals; never for an unavailable clause.
- [ ] REQ-CONF-006: declarations with fixed command and arguments, `allowed_args`, `arg_pattern`, `max_args`; no shell; validation at stage 5; `host_tool` grant at stage 7.
- [ ] REQ-CONF-007: host tools cannot connect; the dependency step is a `network = true` tool, `external_read`, approved, in a one-shot worker.
- [ ] REQ-CONF-008: only declared read paths visible; `toolchain list` shows them; the union is in `status`.
- [ ] REQ-CONF-009 / THR-32: `setsid` cannot escape the kill; orphans of a previous runtime killed at startup with the event.
- [ ] REQ-TEST-013: every clause test, the honest probe, the argument tests, the demonstration (`cargo test --offline` → `completed{verified}`) in CI on Linux.
- [ ] The report with the clause tables of CI and the reference machine; book chapter; CHANGELOG 0.6.
