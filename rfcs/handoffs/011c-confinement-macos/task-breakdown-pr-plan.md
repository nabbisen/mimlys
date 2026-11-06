# Handoff 011c — PR plan (M6c, 0.8)

| Step | Content | Depends on | Size |
|---|---|---|---|
| S1 | Spike: the nine questions of §3 on `macos-latest`, branch only, four CI runs at most; report | — | M |
| — | Architect: RFC 011 Amendment 2 from S1 (K's primitive, P, the mach allow-list, the read set, LIM-H6); ED §13.3 macOS row | S1 | — |
| 011c-1 | Profile generation (pure, unit-tested) and self-application with the self-check; F, N, P | Amendment 2 | M |
| 011c-2 | Tree tracking, the K primitive, the RSS and count watchdogs; M, C | 011c-1 | M |
| 011c-3 | The macOS read set in `app.rs`; the clause suite with mutation checks; the demonstration | 011c-2 | M |
| 011c-4 | M6c report, book's macOS section, CHANGELOG; review request | 011c-3 | S |
