# Handoff 011b — Acceptance and QA checklist (M6b)

- [ ] REQ-CONF-005 (Windows): AppContainer without network capability (F, N), restricted token (P), Job Object kill-on-close and limits (K, M, C), explicit redirected environment (E); every state names its primitive.
- [ ] REQ-CONF-008: `toolchain grant|revoke|list`, reversible to the byte, recorded; `acl_not_applied` names the command.
- [ ] REQ-CONF-010: every new `unsafe` block justified; `check-unsafe.sh` passes.
- [ ] REQ-TEST-013: the clause suite and the demonstration pass on `windows-latest`.
- [ ] Report, book section, CHANGELOG 0.7.
