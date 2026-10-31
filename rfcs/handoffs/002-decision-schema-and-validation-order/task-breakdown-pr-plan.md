# Handoff 002 — PR plan

| PR | Content | Depends on | Size |
|---|---|---|---|
| 002-1 | `decision`: schema v1, strict parser with duplicate-key and trailing-text detection, hostile-input tests | 001-5 | M |
| 002-2 | `tool`: `ParamSchema`, `ToolDefinition`, `render_definitions` with LIM-I1; tests | 001-5 | M |
| 002-3 | `path`: lexical canonicalization, platform name rules, case modes; tests on all three platforms (CI matrix) | 001-5 | M |
| 002-4 | `validate`: stages 1–12 with hook traits and no-op test hooks, `AuthorizedAction`, `Rejection` and the hint table; stage-ordering and existence tests | 002-1 … 002-3 | L |
| 002-5 | `observation` and `context::write_segment`: excerpts, sanitization, notices, marker unforgeability | 001-5 | M |
| 002-6 | `spi`: the six traits; `mimlys-model::fake`; `FakeToolProvider` | 002-1, 002-2 | M |
| 002-7 | `mimlys-exec::builtin`: the six file tools over `cap-std`, deadlines, cancellation, quotas; tests including symlink escape | 002-3, 002-5, 002-6 | L |

CI on all three platforms must be green for 002-3 and 002-7 before review.
