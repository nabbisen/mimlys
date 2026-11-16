# Handoff 009h — PR plan (0.11, phase 1)

| PR | Content | Depends on | Size |
|---|---|---|---|
| 009h-1 | Items 1 and 2: a destination judged as sent; the other invisible characters | — | S |
| 009h-2 | Items 3, 4, 5, 6 and 7: the server's lifecycle (standard error bounded, the pin on the call's connection, `dirty` cleared first, a failed start counted, the omitted content, `setsid`, one wait) | — | M |
| 009h-3 | Item 8: the stack-margin guard | — | S |

009h-3 first if it is ready first: it protects the rest of 0.11 against stack growth.
