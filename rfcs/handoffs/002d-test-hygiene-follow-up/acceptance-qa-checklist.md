# Handoff 002d — Acceptance checklist (0.10)

- [ ] An interrupted run records the fuel it used, or `null` with the reason documented, never 0; tested with a mutation
- [ ] `events --follow` on a task about to run does not end before its runtime holds the store, or the defect is shown not to exist; the test decides by state
- [ ] No test of the full tier returns uncounted; the invariants line fails the `rules` job; the corpus has no optional field; every page with tested blocks is read
- [ ] REQ-SCHED-002 and REQ-SCHED-005 named by tests that verify them; REQ-PLAT-004's Windows clauses tested on CI or counted as skipped
- [ ] Gate; CI per platform with skips; CHANGELOG where a product behaviour changes; linear history
