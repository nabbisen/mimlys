# Handoff 013 — Acceptance checklist (M7a, 0.10)

- [ ] REQ-PERS-013: read commands open without the lock and run no recovery, check, reap or migration; the handle type has no writer; `store: in use`, `recovery pending` and the schema cases shown as stated; never "locked" for a read.
- [ ] REQ-WS-013: the storage view from records, its sum against `du` within the stated tolerance; reflink shown.
- [ ] REQ-WS-014: prune lists first, asks once, deletes only terminal tasks, keeps reversible promotions, keeps audits unless `--audit`, stops cleanly on a failure.
- [ ] REQ-CLI-007: the Storage page and the Getting started section, examples tested.
- [ ] The mutations of section 5 are caught; every new number registered.
- [ ] Gate; CI per platform with skips; CHANGELOG; linear history.
