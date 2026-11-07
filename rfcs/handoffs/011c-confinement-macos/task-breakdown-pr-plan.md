# Handoff 011c — PR plan (M6c, 0.8)

| Step | Content | Depends on | Size |
|---|---|---|---|
| S1 | Spike: the nine questions of §3 on `macos-latest`, branch only, four CI runs at most; report | — | M |
| — | Architect: RFC 011 Amendment 2 from S1 (K's primitive, P, the mach allow-list, the read set, LIM-H6); ED §13.3 macOS row | S1 | — |
| 011c-1 | Profile generation (pure, unit-tested) and self-application with the self-check; F, N, P | Amendment 2 | M |
| S1b | Spike: the open points of §3b, three CI runs; report | S1 | S |
| — | Architect: Amendment 2 settled (points 6 to 9) | S1b, 011c-1 | — |
| 011c-2 | The membership kill loop (K enforced), the RSS and count watchdogs (M, C), the `kern.proc.` deny, the dependency step's three rules | 011c-1 merged | M |
| 011c-3 | The settled read set in `app.rs` (the per-user temporary directory by `confstr`); the load-time refusal of a command leaving the read paths (all platforms); the clause suite with mutation checks; the demonstration without `accept_best_effort` | 011c-2 | M |
| 011c-4 | M6c report, book's macOS section, CHANGELOG; review request | 011c-3 | S |
