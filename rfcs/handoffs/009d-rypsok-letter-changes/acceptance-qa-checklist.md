# Handoff 009d — Acceptance checklist (0.10)

- [ ] REQ-MCP-005: an `http` destination needs its `https` form granted too; the rejection names the form; a redirect is judged the same way; the mutation is caught.
- [ ] REQ-MCP-001 / LIM-O16: a server that ends within its grace is not killed; per-server `shutdown_ms`; `check` warns below 1 000 ms; call cancellation unchanged.
- [ ] The book: rypsok with its own configuration file, `follow_cross_host_redirects = false` and why, `max_call_ms` from `rypsok check`.
- [ ] Gate; CI per platform with skips; CHANGELOG; linear history.
