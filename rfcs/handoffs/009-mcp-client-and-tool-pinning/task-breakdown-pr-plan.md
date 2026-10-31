# Handoff 009 — PR plan (M5)

| PR | Content | Depends on | Size |
|---|---|---|---|
| 009-1 | `mimlys-core::schema` (bounded JSON-Schema validator), `ImportRecord`, `TaskVersions.tool_imports`, the two server events, `ResourceProfile::External` | — | M |
| 009-2 | `mimlys-mcp::server` + `client` (rmcp child process, explicit environment, initialize within LIM-O6, list, call, cancel, shutdown, restarts); the test server binary | — | L |
| 009-3 | `mimlys-mcp::import` + `tools` (import under bounds, declarations, pin check, result mapping) | 009-1, 009-2 | L |
| 009-4 | `mimlys`: `[servers]` configuration, composition per task, `status`, shutdown at close | 009-3 | M |
| 009-5 | Tests of REQ-TEST-012/015 against the test server; the demonstration behind `MIMLYS_RYPSOK_BIN`; the book chapter; CHANGELOG 0.5 | 009-4 | M |
