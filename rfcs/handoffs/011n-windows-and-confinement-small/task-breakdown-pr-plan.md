# Handoff 011n — PR plan (0.11, phase 1)

| PR | Content | Depends on | Size |
|---|---|---|---|
| 011n-1 | Items 1, 2 and 3: the Windows mask and its upgrade, the ACL bookkeeping, the read retry under the lock | — | M |
| 011n-2 | Items 4, 5 and 6: exit code 259, a closed stdin, no leaked handle, the aligned attribute list | — | S |
| 011n-3 | Items 7, 8 and 9: `RLIMIT_AS` in the child, A-34 with `allow_paths`, the task's paths opened | — | S |
| 011n-4 | Item 10: the runtime's budget for a host tool, the record of a late answer, the frame timings | — | S |

011n-1 first: it removes a right that lets a tool change another task's files' permissions.
