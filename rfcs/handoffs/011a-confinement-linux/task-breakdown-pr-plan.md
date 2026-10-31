# Handoff 011a — PR plan (M6a)

| PR | Content | Depends on | Size |
|---|---|---|---|
| 011a-1 | `mimlys-core`: host-tool declarations and argument validation; `ClauseState` with primitives; `Tier` with accepted best-effort; `accept_best_effort`; the orphans event | — | M |
| 011a-2 | `mimlys-exec`: `ConfinementSpec` and the report; `confine/linux.rs` (cgroup, namespaces, subreaper, seccomp, Landlock); the worker's self-confinement and `--probe` report | 011a-1 | L |
| 011a-3 | `mimlys-exec`: `run_host_tool` in the worker and the tool definition; the tree kill; output capture | 011a-2 | M |
| 011a-4 | `mimlys`: `[host_tools]`, the spec from the configuration, the real-worker probe, the fetch worker, `toolchain list`, `status`; the orphan record | 011a-3 | M |
| 011a-5 | The clause tests with the probe tool; the demonstration project and test; the book chapter; the M6a report; CHANGELOG 0.6 | 011a-4 | L |
