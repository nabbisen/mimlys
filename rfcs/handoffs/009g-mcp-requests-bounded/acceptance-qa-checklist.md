# Handoff 009g — Acceptance checklist (0.10)

- [ ] A listing, each page and a call have deadlines and observe the interruption; pages and tools are bounded; no lock is held across an unbounded wait; a hung server never holds a step, another call or the exit (A-08)
- [ ] A non-string destination makes the tool unavailable at import, and a non-string destination value is denied at stage 7 (A-11; REQ-MCP-005)
- [ ] Output validation has a visit budget and runs within the deadline; costly schemas are refused at import (A-09)
- [ ] Parameter names and enum values are cleaned or make the tool unavailable (A-10)
- [ ] `cause`, keys and text before the cut are redacted (A-13 a to c; REQ-CTX-007)
- [ ] Server strings are cleaned before display (A-28)
- [ ] An unreachable server at a step boundary keeps its tools pinned (A-31)
- [ ] The proposed bounds stated, for registration as limits; tests by state with mutations; gate; CI per platform with skips; book; CHANGELOG citing the audit ids; linear history
