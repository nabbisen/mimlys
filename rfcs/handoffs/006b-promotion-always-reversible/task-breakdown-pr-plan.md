# Handoff 006b — PR plan (0.10)

| PR | Content | Depends on | Size |
|---|---|---|---|
| 006b-1 | Item 5: a unique temporary name (it removes X-03's failures from every later test) | — | S |
| 006b-2 | Items 1, 3, 4: nothing unlisted removed; the apply-time re-check; never through a linked parent | 006b-1 | M |
| 006b-3 | Item 2: the backup committed first, the `applying` state, the restore on error and at start (`SCHEMA_VERSION` 4) | 006b-2 | M |
| 006b-4 | Items 6 and 7: `task delete` leaves no orphan; the A-39 promotion items | 006b-1 | S |
