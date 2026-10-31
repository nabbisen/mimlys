# Handoff 011b — PR plan (M6b)

| PR | Content | Depends on | Size |
|---|---|---|---|
| 011b-1 | AppContainer profile, spawn with security capabilities, restricted token, job limits; the worker's report | 011a | L |
| 011b-2 | ACL management with records; `toolchain grant|revoke|list`; the write ACEs at data-directory creation | 011b-1 | M |
| 011b-3 | Redirected environment; file-lock retry; the fetch worker's capability | 011b-1 | S |
| 011b-4 | The clause suite and the demonstration on `windows-latest`; report; book; CHANGELOG 0.7 | 011b-2, 011b-3 | M |
