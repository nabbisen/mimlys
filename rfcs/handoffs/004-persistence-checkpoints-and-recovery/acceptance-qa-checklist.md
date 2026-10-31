# Handoff 004 — Acceptance and QA checklist

- [ ] REQ-PERS-001 / LIM-P1 / REQ-SEC-008: WAL, synchronous FULL, owner-only file and directory, lock refuses a second runtime within LIM-O3.
- [ ] REQ-PERS-002: schema version; forward-only migrations; newer version refused naming both versions.
- [ ] REQ-PERS-003 / INV-05: every transition its own transaction; durable when the call returns (reopen tests).
- [ ] REQ-PERS-004: the eight methods exist, are called in order by the loop (verified in handoff 010), and no method batches two lines; execution cannot start before the running intent (API shape).
- [ ] REQ-PERS-005: blobs written and fsynced before their rows; bounded by LIM-S4.
- [ ] REQ-PERS-006 / LIM-P5: integrity check and CONS-01 … CONS-08 at every start; first violation named.
- [ ] REQ-PERS-007: recovery plan table correct for M1 (no restore yet; documented as M3).
- [ ] REQ-PERS-008 / INV-07 / INV-12: no path from `Unknown` to `Running`; an unfinished action never becomes `Succeeded`.
- [ ] REQ-PERS-012 / REQ-MAIN-002: version columns and task version fields recorded.
- [ ] REQ-OBS-001 / REQ-OBS-002: events appended in the transition's transaction; `since` ordered and complete.
- [ ] REQ-OBS-003 / REQ-OBS-008: metrics and refusal counts derived from events; fixtures match.
- [ ] REQ-WS-001: data directory 0700 / owner-only.
- [ ] REQ-TEST-007: crash injection at every point, before and after commit, recovers as planned on all three platforms.
- [ ] Tests cite requirements; `rusqlite` and `fd-lock` live in `mimlys-persist`, not in core.
