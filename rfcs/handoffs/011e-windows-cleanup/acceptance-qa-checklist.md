# Handoff 011e — Acceptance and QA checklist (0.8)

- [ ] REQ-CONF-008: `toolchain revoke --all` removes every recorded and found entry and the profile; directories equal their state before mimlys (without the `(I)` marker); the record holds exactly what failed.
- [ ] The scan is bounded to mimlys's directories and the declared read paths; the mutation check fails without it.
- [ ] Confirmation names that mimlys re-applies its own entries at its next start; `--yes`; usage error with a path; JSON shape as documented.
- [ ] Linux and macOS: the message, exit 0.
- [ ] REQ-CONF-010: the new `unsafe` block justified; `check-unsafe.sh` green.
- [ ] Book (Removing mimlys), `cli.md`, CHANGELOG; gate; CI per platform with skips.
