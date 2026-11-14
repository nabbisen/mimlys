# Handoff 010e — Acceptance checklist (0.10)

- [ ] The first signal interrupts only actions that can be undone, by `after_unknown`; for any other it says why it waits, and the action's own deadline bounds the wait
- [ ] A second signal during such an action records its outcome unknown and halts `unknown_outcome` with the action listed; `task cancel` records and names it
- [ ] The interruption API says that it interrupts a step; a request is cleared at every task transition and never reaches a later step
- [ ] The messages are in the user's words; tests by state with mutations; gate; CI per platform with skips; CHANGELOG; linear history
