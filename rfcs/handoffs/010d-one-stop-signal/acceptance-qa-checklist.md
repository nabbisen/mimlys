# Handoff 010d — Acceptance checklist (0.10)

- [ ] On a timeout the test prints the run's streams, its process tree and the task's events
- [ ] The cause is named, with evidence; one stop signal ends the run within LIM-O1 after the boundary, or cancels at LIM-O1, at every signal point tested
- [ ] Mutations of the pause and of the watchdog fail the tests
- [ ] Gate; CI per platform with skips; a stress loop of the test; CHANGELOG if the product changed; linear history
