# Handoff 002e — Acceptance checklist (0.10)

- [ ] A failed restore after a cancelled execution is logged and halts `unknown_outcome` (A-17; REQ-WS-010)
- [ ] A task found in `recovering` is recovered again; every recovery step is idempotent (A-18; §16.3)
- [ ] No built-in blocks on a FIFO, socket or device; reads are bounded (X-04)
- [ ] An unreadable tree halts the task with a reason and a remedy, never an internal error leaving it `running` (X-05)
- [ ] `effect_not_allowed` is gone from the code, the book and the specification (A-35)
- [ ] Tests by state with mutations; gate; CI per platform with skips; CHANGELOG citing the audit ids; linear history
