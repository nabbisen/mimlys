# Handoff 007 — Context assembly, compression, redaction, measurement (M4)

**RFC.** [RFC 007](../../accepted/007-context-assembly-and-observation-pipeline.md) (Accepted)
**Specification.** REQ-CTX-001 … 010; REQ-OBS-003; REQ-PRIV-002; REQ-TEST-010; REQ-TEST-016; external design §15, §17.2, §19.1; limits LIM-C1 … C4, LIM-D4, LIM-I1.
**Crates.** `mimlys-core` (`context.rs` grown: segment writer, repeated-line collapse, step summaries, the budget algorithm, redaction; `runtime/context.rs` replacing `context_m1.rs`; `Step` measures; metrics), `mimlys-persist` (metrics derived from step records), `mimlys` (secret values to the runtime; `metrics` output; `[backend] context_chars`).
**Milestone.** M4, with handoff 008. Depends on 001–006, 010.

## 1. Scope

In: the nine-segment assembler with the stable prefix; the compression rules and the retained set; redaction of configured secrets; the per-step measures and the derived metrics; the tests of REQ-TEST-010 over the fakes; the M4 report's context section.

Out: any model-written summary (D-12); tokenizers; near-identical-line collapsing (open question).

## 2. Module layout

```text
crates/mimlys-core/src/
  context.rs        context/tests.rs     write_segment (M1), collapse_repeated_lines, StepSummary{step, decision, tool, status, cause, notices, first_line} ≤ LIM-C4, Retained rule, Budget{limit} fit(…) -> Assembled{messages, measures}, Redactor{names, values} redact(text)
  runtime/context.rs  (replaces context_m1.rs)  assemble(inputs) -> Assembled: segments 1–9, history (RFC 006) after the goal, the retained set, batches of LIM-C3
  step.rs           (+) Step{context_chars, prefix_chars, compressed_steps, dropped_steps, over_budget, model_ms, usage: Option<Usage>}
  runtime/loop_.rs  measures recorded on the step at line 2; redaction applied in Observation::from_outcome's caller before line 6; the previous request's bytes kept per task in memory for prefix_chars
  spi.rs            (+) RuntimeDeps.secrets: Vec<(String, String)> (name, value); Metrics{context_chars_avg, prefix_ratio, compression_ratio, model_ms_avg, prompt_tokens, completion_tokens, cached_tokens, chars_per_prompt_token: Option<f64>}
crates/mimlys-persist/src/metrics.rs   the new measures from step records
crates/mimlys/src/app.rs               resolves [backend] api_key and (M5) server secrets into RuntimeDeps.secrets; config.rs (+) [backend] context_chars (LIM-C1 default, range)
```

## 3. Behaviours

### 3.1 Assembly (REQ-CTX-001, REQ-CTX-002)

`assemble` takes the task, the pinned definitions text, the grants, the plan, steps, actions, observations, questions, approvals, the fork history and `now`, and returns the messages plus the measures. Order: system (instructions + `tool_definitions` segment + capability summary), user `goal`, user `history` (fork), assistant plan (the `plan` segment), user `feedback kind="history"` (compressed steps, one JSON array), then for each recent step: the assistant's decision text, the `observation`/`feedback`/`user` segments of M1, then the decision request. The capability summary is rebuilt from the grants on every call (it only changes when they change). The instructions text is a constant of the runtime version.

### 3.2 Budget (REQ-CTX-003, REQ-CTX-004)

`Budget::fit`: measure the characters of all contents; while above the limit apply, in order: collapse repeated lines inside every observation excerpt (consecutive identical lines → one line + `[… repeated N times …]`); summarize steps older than the last LIM-C3 (each `StepSummary` serialized ≤ LIM-C4 characters, truncated at a character boundary); drop the oldest summaries LIM-C3 at a time, skipping retained ones. Retained: a step whose action was rejected (its code), a step with a halt/resume feedback, a step whose action failed and no later step succeeded with the same tool and the same `path` argument. If the limit is still exceeded after everything droppable is gone, `over_budget = true` and the request goes out as is.

### 3.3 Redaction (REQ-CTX-007)

`Redactor::new(secrets)` keeps `(name, value, json_escaped_value)` for values of eight characters or more; `redact(text)` replaces every occurrence of any value with `[redacted:<name>]`, longest value first. Applied to the observation's payload (as serialized JSON, then re-parsed), stdout and stderr excerpts and notice texts before `record_execution`; to every log line through a `tracing` layer in `mimlys::logging`. The configuration refuses a resolved secret under eight characters (`backend.api_key: too short to redact safely`).

### 3.4 Measures (REQ-CTX-010, REQ-OBS-003)

At line 2 the step records `context_chars` (sum of contents), `prefix_chars` (common prefix with the previous request of the task, both serialized as `role\n content\n` per message; 0 for the first), `compressed_steps`, `dropped_steps`, `over_budget`, `model_ms` (the backend call's wall time), `usage` (from `ModelBackend::last_usage`). `metrics` derives: average `context_chars`, `prefix_ratio` = Σ prefix / Σ context over steps after the first, `compression_ratio` = Σ chars before fit / Σ after, average `model_ms`, token sums, and `chars_per_prompt_token` = Σ context_chars / Σ prompt_tokens over steps with usage (absent otherwise). The report (§5) prints them for the reference run.

## 4. Tests (REQ-TEST-010, REQ-TEST-016)

- Segment order and roles; segments 1–4 byte-identical across ten steps with the fakes; a grant change changes segment 3 once; a `plan` changes segment 5 only.
- Delimiting: an observation payload containing `<</observation>>` and newlines cannot end the segment (JSON escaping); a user answer containing a marker line is quoted.
- Compression: with LIM-C1 forced small, repeated lines collapse; steps older than LIM-C3 become summaries ≤ LIM-C4; summaries drop in batches of LIM-C3; a rejection, a halt notice and an unsuperseded failure survive every rule; a superseded failure is dropped; `over_budget` set when only retained steps remain.
- Redaction: a secret value in stdout, in a payload string and in a notice becomes `[redacted:<name>]`; the JSON-escaped form too; nothing shorter than eight characters is scanned.
- Measures: `prefix_chars` grows monotonically with a stable prefix; `compressed_steps`/`dropped_steps` match the rules; metrics derive the ratios.
- Replay: the same persisted history assembles the same bytes twice (REQ-TEST-016).

## 5. Return

The context section of the M4 report (with handoff 008's run): context size per step, compression ratio, prefix ratio measured by the runtime and cache hits reported by the engine, and the proposed LIM-C1 and LIM-C3.

## 7. Amendments made during implementation (2026-10-29)

- **The step being decided is not an entry.** `assemble` skips a step record without outcome, action or rejection (the one started for this request), so the LIM-C3 window and the summaries count only finished steps.
- **Repeated lines.** Runs of three or more identical consecutive lines collapse to one line plus `[… repeated N times …]`; a run of two stays (a marker would be longer than the saving). The rule applies only when the request is over budget, as §15.3 orders the rules.
- **Summaries.** `StepSummary` carries `step`, `decision`, `tool`, `path`, `status`, `cause`, `rejection`, `outcome` (when not a plain continuation), notice codes and `first_line`; `render` drops `first_line` first, then cuts at a character boundary. The history segment is `feedback kind="history" summarized=N dropped=M` with one JSON array of summaries.
- **Retained set.** A step is retained when it was rejected, when it halted the task, or when its action did not succeed and no later step succeeded on the same tool and `path` argument. Retained steps are summarized like the others but never dropped.
- **`prefix_chars` is a process measure.** The previous request is kept in memory per task; the first request after a restart measures 0. The persisted measure is honest about this; the M4 report reads the ratio over whole runs.
- **Model timing uses the runtime clock** (REQ-ARCH-006), not `std::time`.
- **Redaction** also covers the `symlink_omitted` notice's path; the notice `secret_redacted{name}` is appended once per secret found. Log lines pass through a redacting writer installed before the application opens, so startup lines are covered too.
- **Metrics** gain `chars_per_prompt_token` (Σ context chars / Σ prompt tokens over steps that reported usage) so a user can set `context_chars` from a model's window. `Metrics` is no longer `Eq` (ratios are floating point).
- **Retries stay in the loop.** The adapter of handoff 008 does not retry; the runtime's LIM-M3 identical retries are the one retry mechanism (INV-02 keeps one backend call site), and the adapter only spaces them after a 429 or a 5xx.
