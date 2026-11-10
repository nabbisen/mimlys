# Handoff 009b — Acceptance checklist (0.9)

- [ ] `cargo deny check` green; only `rusqlite`, `libsqlite3-sys`, `rmcp` and what they require moved in `Cargo.lock`.
- [ ] `SCHEMA_VERSION` unchanged; a 0.8.0 store opens, checks and lists (REQ-PERS-006 test).
- [ ] The M5 suite green; `rmcp` still exactly pinned, same features.
- [ ] CHANGELOG under Security; CI 9 of 9 with skips.
