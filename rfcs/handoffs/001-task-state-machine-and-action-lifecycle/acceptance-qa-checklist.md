# Handoff 001 — Acceptance and QA checklist

- [ ] REQ-TASK-002: nine states; the transition table of ED §5.1 and nothing else; illegal pairs refused with `StateError`.
- [ ] REQ-TASK-003: terminal states refuse every input; no field of a terminal task is mutable through the public API.
- [ ] REQ-TASK-005: extension only from `Halted{BudgetSteps|BudgetDeadline}`, only `steps` and `deadline`, within the given ceiling; `pause_requested` is a flag.
- [ ] REQ-TASK-007: fourteen halt reasons, snake_case names, no others.
- [ ] REQ-TASK-008 / REQ-DEC-010: counters and the four halt rules; identical-decision rule uses the snapshot id; consecutive counters reset as specified.
- [ ] REQ-TASK-009: deadline absolute, set from LIM-T5, `remaining()` correct with a fake clock.
- [ ] REQ-DEC-001: step phases and outcomes exactly as ED §6.1.
- [ ] REQ-SBX-003: `exit_code` only with `Exited`; eight termination values.
- [ ] REQ-TOOL-009: disposition rules; retry conditions exact.
- [ ] REQ-OBS-001 / ED §17.1: every event name present and serialized with the dotted form; `Actor` vocabulary.
- [ ] REQ-DEC-012 / LIM-U6: canonical hash tests.
- [ ] REQ-ARCH-006: clock and ids injected everywhere; determinism test; guard against direct time/uuid calls.
- [ ] REQ-ARCH-002: `scripts/check-core-deps.sh` passes; no I/O crate in `mimlys-core`.
- [ ] REQ-ENG-003: tests in `src/<mod>/tests.rs`; REQ-TEST-001: test names cite requirements.
- [ ] Every number is a `limits.rs` constant with its LIM id.
- [ ] Review-request package has an entry point citing RFC 001, this handoff, and the REQ ids.
