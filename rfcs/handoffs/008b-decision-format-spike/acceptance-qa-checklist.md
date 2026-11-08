# Handoff 008b — Acceptance checklist (spike S2)

- [ ] Every cell of the matrix run three times, or the reason it could not be run.
- [ ] Each model's SHA-256 equals the repository's listing; no executable downloaded.
- [ ] Engines started by hand on `127.0.0.1` and stopped; no service enabled.
- [ ] Per cell: outcome, steps, first decision, valid-decision count, tool-call errors, tokens, cached tokens, wall time.
- [ ] For F3/F4: where the engine puts the call, and what it needs to return `tool_calls`.
- [ ] No product change; scratch removed; models kept.
