# Handoff 011k — Acceptance checklist (spike S3)

- [ ] No published crate depends on Verus; build, test, publish dry run and `cargo deny` unchanged (before and after shown).
- [ ] The proof checks the production source (or the extracted core), not a copy; or the spike stopped and said why.
- [ ] P1 to P4 each proved, partly proved or not provable, with the reason.
- [ ] The three mutations fail the proof, with Verus's output.
- [ ] Effort, run time, CI needs and re-proof cost measured; the recommendation given.
- [ ] Within the time box; scratch removed; nothing merged.
