# Handoff 011n — Acceptance checklist (0.11, phase 1)

- [ ] The Windows container has read, write, execute and delete on the workspaces and `<data>/tmp`, never `WRITE_DAC`, `WRITE_OWNER` or `GENERIC_ALL`; an old entry is replaced at start
- [ ] The ACL lock is keyed on the directory's identity; `has_entry` checks the mask and refuses an inherited entry; the data directory is created protected and its owner checked (A-23 a, c, d)
- [ ] A corrupt or unreadable `toolchain-acls.json` stops the start with the file and the remedy and is never rewritten; saves are atomic (A-23 b)
- [ ] The permission read retries on an insufficient buffer and is made under the lock (the os error 122 flake)
- [ ] Exit code 259 is a final exit code; a Windows tool's stdin is closed; no handle leaks on an error path; the attribute list is aligned
- [ ] `RLIMIT_AS` is set in the child, not in the worker
- [ ] A-34: no argument part names a path outside the workspace unless the declaration says `allow_paths = true`
- [ ] A task's paths are opened without following links; a non-plain component is refused
- [ ] The runtime's budget covers the worker's deadline and tree kill; a late answer after the deadline is recorded `deadline`
- [ ] Tests by state with mutations; gate; CI per platform with skips; book; CHANGELOG; linear history
