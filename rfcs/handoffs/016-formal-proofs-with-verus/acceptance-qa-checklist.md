# Handoff 016 — Acceptance checklist (0.10)

- [ ] The production edits keep behaviour and the seven tests; the specifications are cfg-only; comments say what is and is not proved.
- [ ] `verify/` is outside the workspace and the packages; `Cargo.lock` unchanged; the publish dry run unchanged.
- [ ] The CI job: the pinned Verus, its hash compared (failing on a mismatch), cached; the toolchain Verus names, pinned; the proof passes; mutation m1 required to fail.
- [ ] `repository-guards.md` records the property, the failing and surviving mutations with what holds the survivors, the run time and the pin.
- [ ] Gate; CI 9 of 9 with skips plus the new job; linear history.
