# Handoff 002c — Acceptance checklist (0.10)

- [ ] The model reads the rejection's own hint (by reason for `denied`) and the fixed detail keys; every hint names only keys the segment carries; nothing of the filesystem or the policy; the out-of-scope denial identical whether or not the path exists
- [ ] The detail survives a restart (persisted; migration tested against a 0.9.0 store; older steps read with the code's hint); `decision.rejected` carries `reason` and `grant_kind`
- [ ] Denials counted by reason and grant kind; the `metrics` documentation names a rising rejection rate as an interface defect
- [ ] `BudgetKind` holds only what stage 10 builds; convergence uses the tree's content identity, tested through the loop; the fork refusal names its remedies
- [ ] Each change has a test naming its requirement and a mutation where a rule is guarded; gate; CI per platform with skips; CHANGELOG; linear history
