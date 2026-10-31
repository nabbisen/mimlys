# Handoff 003 — PR plan

| PR | Content | Depends on | Size |
|---|---|---|---|
| 003-1 | `effect`: type, declaration rules, `derive`; exhaustive table test | 001-5 | S |
| 003-2 | `grant`: kinds, set, hash, path and destination matching, revocation; tests | 002-3 | M |
| 003-3 | `policy`: ceiling, profiles, shipped profiles, validation, `reconcile`; tests | 003-1, 003-2 | M |
| 003-4 | `hooks`: the three validator hooks over grants, policy and budgets; integration tests through `validate` | 002-4, 003-3 | M |
| 003-5 | `approval`: request, rendering table, binding, consumption, expiry, invalidation; hostile rendering tests | 002-5, 003-2 | L |
| 003-6 | `question` and `CompletionCheck` types with the decision table; tests | 001-5 | S |
