# Handoff 005b — Acceptance checklist (0.8)

- [ ] LIM-O9 and LIM-O10 as named constants; no other number.
- [ ] Transient failures retried, others not; the hash verified before storing; `unavailable`/`download` with the URL, status and `--from` remedy, typed.
- [ ] The stand-in tests (504 twice, 404, 504 always) and the mutation.
- [ ] CI installs QuickJS from the cache with `--from`; mdbook retried; cargo-deny's behaviour stated.
- [ ] Gate; CI per platform with skips; CHANGELOG; `cli.md`.
