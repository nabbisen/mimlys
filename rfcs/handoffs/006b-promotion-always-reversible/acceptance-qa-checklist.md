# Handoff 006b — Acceptance checklist (0.10)

- [ ] Promotion never removes a project entry the plan does not list; such a directory is refused and its entries named (A-03)
- [ ] Nothing in the project is written before the backup is committed and the promotion recorded `applying`; an error or a crash restores what was applied and records `failed` (A-04; §14.5, §16.3)
- [ ] Each entry is re-checked against the base at apply (A-15)
- [ ] No write goes through a linked parent; exclusion is judged on the resolved path (A-16)
- [ ] The temporary name is unique, short, `create_new`, and cleaned on every error; a 255-byte name restores and promotes (A-24, X-03)
- [ ] `task delete` and `task prune` leave no orphan and report each failure (A-19)
- [ ] An out-of-root link is refused at plan time; `rollback --promotion` lists and asks (A-39)
- [ ] `SCHEMA_VERSION` 4 by a new migration; tests by state with mutations; gate; CI per platform with skips; book; CHANGELOG citing the audit ids; linear history
