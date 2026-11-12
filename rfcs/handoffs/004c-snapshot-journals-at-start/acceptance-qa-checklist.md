# Handoff 004c — Acceptance checklist (0.10)

- [ ] A writing start finishes a journal whose row exists and undoes one without a row; no object a row references is ever removed; the integrity check passes after
- [ ] The event records what was done; the read-only open removes nothing and says `recovery pending`
- [ ] REQ-WS-012 named by the tests of every clause; mutations of the step and of the reference check fail them
- [ ] Gate; CI per platform with skips; CHANGELOG; linear history
