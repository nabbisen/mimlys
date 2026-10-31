# Handoff 011c — The macOS backend (M6c)

**RFC.** [RFC 011](../../accepted/011-confinement-backends.md) §4, §5 · **Specification.** REQ-CONF-005 (macOS), REQ-TEST-013; external design §13.3 · **Milestone.** M6c (release 0.8). Depends on 011a.

## 1. Scope

The Seatbelt backend (F, N, P), the explicit environment (E), the process-group kill with a descendant walk (K, best-effort), the resident-size watchdog (M, best-effort), rlimits (C, best-effort), the accepted-best-effort path of RFC 011 §5 exercised for K, the clause tests and the demonstration on `macos-latest` in CI.

## 2. Module layout

```text
crates/mimlys-exec/src/confine/macos.rs   seatbelt profile generation from the spec ; sandbox_init through one justified unsafe call (libc) with a startup self-check (a denied read must fail) ; kill_descendants via proc_listchildpids (libproc) repeated until none within LIM-O2 ; the RSS watchdog thread (proc_pidinfo) killing the tree above LIM-H2 ; rlimits
```

## 3. Behaviours

- The worker confines itself like on Linux (self-applied Seatbelt); the report: F, N, P `enforced (seatbelt)`, E `enforced`, K `best_effort (process_group_walk)`, M `best_effort (rss_watchdog)`, C `best_effort (rlimit)`.
- Tier `core` unless `accept_best_effort = ["K"]`: the book's macOS section explains exactly what is best-effort (a re-parented orphan can outlive a cancel) and what the administrator accepts by the line.
- The deprecated `sandbox_init` is checked at startup by a self-test; a macOS release that removes it makes F, N, P `unavailable{seatbelt_missing}` and the tier `core`, never a crash.

## 4. Tests

The clause suite of 011a on macOS; K's walk kills a `setsid` child in the common case and the test documents the escape it cannot cover; the demonstration with the runner's toolchain under `accept_best_effort = ["K"]`, the approval line showing it.

## 5. Return

Package under `.git-exclude/reviewed/m6c-011c/`, the report `docs/src/maintainers/m6c-report.md`, the book's macOS section, CHANGELOG 0.8.
