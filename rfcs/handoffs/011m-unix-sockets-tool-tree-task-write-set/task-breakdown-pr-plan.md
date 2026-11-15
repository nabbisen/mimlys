# Handoff 011m — PR plan (0.10)

| PR | Content | Depends on | Size |
|---|---|---|---|
| 011m-1 | Items 1 and 3: no Unix socket outside the write set, on both rungs; no cgroup right in the tool layer | — | M |
| 011m-2 | Item 2 and the `HOME` item: the tree ends with the execution; survivors reported; a fresh temporary directory per execution | — | M |
| 011m-3 | Items 4 and 5: the per-task write set (Linux; macOS as measured); every worker checked before use | 011m-1 | M |
| 011m-4 | Item 6: no `.cmd` or `.bat` on Windows | — | S |

011m-1 closes the critical finding: open it first, and say so in the request if it is ready before the others.
