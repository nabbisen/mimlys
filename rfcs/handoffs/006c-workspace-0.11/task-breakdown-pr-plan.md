# Handoff 006c — PR plan (0.11, phase 1)

| PR | Content | Depends on | Size |
|---|---|---|---|
| 006c-1 | Items 1 and 2: a backup is not a rollback target; a named pipe never blocks `promote` | — | S |
| 006c-2 | Items 3, 4 (but the link chains), 5 and 7: verify then replace, the one walk, the Windows directory link, the temporary files after a crash | 006c-1 | M |
| 006c-3 | Item 6: modes kept | 006c-2 | M |
| 006c-4 | Items 8 and 9: `prune` goes on; the halt lines | — | S |
| 006c-5 | Item 4's link chains | 006c-2 | M |

006c-1 first: two quick, safety-relevant fixes.
