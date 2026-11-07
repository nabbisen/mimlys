# Handoff 011f — Acceptance and QA checklist (0.8)

- [ ] REQ-CONF-008 / THR-36: no write is ever made after the wait ends; the refusal is `acl_not_applied` with the stated reason; the mutation (LIM-O7 → 1 ms) is caught.
- [ ] LIM-O7 is a named constant with its id; no other wait or threshold number in the code.
- [ ] One `info` line per wait, naming the directory; nothing at `warn` on the normal path.
- [ ] The start fails with the reason when mimlys's own entries are refused; a grant and a tool report it as `acl_not_applied` today does.
- [ ] Abandoned mutex: acquired, the base re-read under the lock; stated in the header.
- [ ] Two installations granting one directory at once keep both entries (Windows CI); the reproduction shows 0 of 10 failures.
- [ ] The worker-file measurement is reported, nothing changed by it.
- [ ] REQ-CONF-010: any new `unsafe` justified; `check-unsafe.sh` green. Book, CHANGELOG; gate; CI per platform with skips.
