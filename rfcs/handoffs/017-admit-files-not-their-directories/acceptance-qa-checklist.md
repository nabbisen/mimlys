# Handoff 017 — Acceptance checklist (0.10)

- [ ] REQ-CONF-011: the worker and each module are files in every read set; their directories are not; a command beside the worker needs a declaration, refused at load with the directory named.
- [ ] Each backend admits a file exactly (Landlock, Seatbelt, a non-inheritable Windows entry).
- [ ] REQ-CONF-008: the Windows file entry recorded, listed, revoked, found by `revoke --all`.
- [ ] Section 2 measured: nothing else needed from the worker's directory on five platforms; a private Windows install; the start-time difference.
- [ ] The mutations caught; the book and CHANGELOG with the remedy; gate; CI per platform with skips plus `verus`; linear history.
