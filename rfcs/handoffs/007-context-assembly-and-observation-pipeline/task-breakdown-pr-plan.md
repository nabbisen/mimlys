# Handoff 007 — PR plan (M4)

| PR | Content | Depends on | Size |
|---|---|---|---|
| 007-1 | `context.rs`: repeated-line collapse, `StepSummary`, the retained rule, `Budget::fit`, `Redactor`; tests | — | M |
| 007-2 | `runtime/context.rs` replacing `context_m1.rs`: the nine segments, the stable prefix, batches; `Step` measures; the loop records them; the fakes' tests of REQ-TEST-010 | 007-1 | L |
| 007-3 | Redaction wired: `RuntimeDeps.secrets` from the configuration; observations and the log layer | 007-1 | S |
| 007-4 | Metrics: persistence derives the new measures; `mimlys metrics` prints them; `[backend] context_chars` | 007-2 | S |
| 007-5 | The report's context section (with 008-5) | 007-4, 008-5 | S |
