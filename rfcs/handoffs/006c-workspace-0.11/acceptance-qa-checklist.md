# Handoff 006c — Acceptance checklist (0.11, phase 1)

- [ ] `rollback --snapshot` refuses a backup and names `rollback --promotion` (X-06)
- [ ] A special file of the project is refused in the plan; no open of a project file blocks
- [ ] A restore and a reversal verify every object before they remove anything; a missing or corrupt object changes nothing (A-14)
- [ ] One iterative walk sees every kind of entry; non-UTF-8 names refuse creation; link targets are kept byte for byte; the scope's read errors are stated; link chains are resolved and refused when they leave the root (A-38)
- [ ] A Windows directory link is restored as a directory link
- [ ] Modes are kept by the manifest, restore, promotion (the project's own mode; no group or other write) and the reversal; old manifests keep their hash (A-20)
- [ ] A crash's temporary files are removed by the restore (A-24's residual)
- [ ] `task prune` goes on past a task deleted with files left, and reports `left`
- [ ] The halt lines say a rollback did not happen, and give permission advice only for a permission problem
- [ ] Tests by state with mutations; gate; CI per platform with skips; book; CHANGELOG; linear history
