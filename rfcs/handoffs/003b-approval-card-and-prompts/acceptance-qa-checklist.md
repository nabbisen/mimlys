# Handoff 003b — Acceptance checklist (0.10)

- [ ] The approval card names every argument, destinations and paths first; each value bounded on its own and marked when cut; the whole within LIM-I6, the reason included (A-02; REQ-APR-002)
- [ ] `[v]iew`, `task show --arguments` and `pending_approval.arguments` give the full, sanitized arguments before a decision
- [ ] The limits make every key fit (shown), or an unrenderable action is rejected with a stated reason
- [ ] A stop signal at an approval or question prompt ends the process within LIM-O1, leaves the task `waiting_user`, and prints one true line (A-07; REQ-CLI-005)
- [ ] Tests by state with mutations; gate; CI per platform with skips; book; CHANGELOG citing the audit ids; linear history
