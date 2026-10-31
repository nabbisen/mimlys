# Handoff 009 — Acceptance and QA checklist (M5)

- [ ] REQ-MCP-001 / REQ-PRIV-003 / THR-24: child started from an explicit environment (`env_allow` + `env_secrets` only); secrets by reference; ended at exit; LIM-O6 startup; LIM-X3 restarts; a test shows no operator secret inside a tool.
- [ ] REQ-MCP-002 / THR-25: definitions hashed and pinned at creation (`tool.pinned`); a change → `tool.unpinned` once, the tool unavailable for that task, available to a new task.
- [ ] REQ-MCP-003: descriptions sanitized and cut to LIM-I2; LIM-I3/LIM-I4 enforced; unsupported constructs make the tool unavailable with a named reason; annotations recorded, never used.
- [ ] REQ-MCP-004 / THR-26: effect, idempotent, destinations, deadline from the administrator; undeclared = `external_write`, non-idempotent; `reversible = false`.
- [ ] REQ-MCP-005 / D-13: destinations validated against `network` grants before the call; redirects never followed; the re-proposed URL validated again.
- [ ] REQ-MCP-006 / REQ-TOOL-005: `structuredContent` validated against the pinned output schema (`output_invalid` otherwise); `isError` → `tool_error{kind}`; content bounded and sanitized; labelled external.
- [ ] REQ-MCP-007 / REQ-TOOL-007: `notifications/cancelled` sent; the call abandoned and the server killed if needed within LIM-O2.
- [ ] REQ-MCP-008 / REQ-TOOL-006: deadlines default LIM-O5, never below `max_call_ms`, within the minimum rule.
- [ ] REQ-MCP-009 / K-04: only granted tools visible; `rypsok.engine_status` not granted by default; the ceiling must list an external tool before a profile or a grant may.
- [ ] REQ-MCP-010: no server surface (grep: no `transport-io`/server features).
- [ ] REQ-ARCH-002: `rmcp` only in `mimlys-mcp` (`check-core-deps`).
- [ ] `status` lists servers, tools, unavailable reasons; configuration validation names every key.
- [ ] The M5 demonstration (external design §23) against a real rypsok when present, else the test server; the transcript in the package.
- [ ] Book chapter "External tools: rypsok"; CHANGELOG 0.5 with the new keys and events.
