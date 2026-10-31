# RFC 007 — Context assembly and the observation pipeline

**Status.** Accepted (2026-10-29)
**Tracks.** Decision D-12 of the startup review; findings C-07, M-18, M-19, K-01, R-14; threats THR-01, THR-10, THR-16, THR-21, THR-33.
**Touches.** `mimlys-core` (`context.rs`, the runtime's assembler replacing the M1 form, the step record's context measures, metrics), `mimlys` (the resolved secret values given to the runtime for redaction, `metrics`), external design §15, §17.2, §19.1 (`[backend] context_chars`); requirements REQ-CTX-001, 003, 004, 007, 010, REQ-OBS-003, REQ-TEST-010.

## Summary

Fix the model's context as a pure function of the persisted history: nine segments in a fixed order whose first four are byte-stable for the task's life, every external text delimited and labelled as data, and a budget (LIM-C1) kept by rules alone: excerpts, repeated lines collapsed, old steps reduced to fixed summaries, then the oldest summaries dropped in batches, with grants, halts, rejections and unresolved failures never dropped. Redact configured secrets before anything reaches an observation or a log. Measure the prefix the backend can reuse and the compression ratio on every request so that the M4 report can fix LIM-C1 and LIM-C3 from numbers.

## Motivation

- The M1 assembler sends every step in full; a long task will exceed any local model's window and the drafts never said what goes first (C-07, R-14).
- The prefix cache of a local engine is the difference between seconds and milliseconds of time to first token; a layout that changes early segments every step defeats it (C-07, the study §2).
- Observations are untrusted; the delimiting of M1 must stay structural under compression, and no summary may be written by a model (M-18, D-12, THR-33).
- Configured secrets exist from M4 (the backend's key); they must never appear in an observation, a prompt or a log (REQ-CTX-007, THR-16).

## Decision

The normative text is external design §15 (segments, delimiting, compression), §17.2 (metrics), §19.1 (`[backend] context_chars`), requirements REQ-CTX-001 … REQ-CTX-010, REQ-OBS-003, REQ-PRIV-002, REQ-TEST-010, and limits LIM-C1 … LIM-C4. In brief:

1. **Nine segments, one order, a stable prefix.** The request is built as messages in the order of §15.1: one `system` message holding segments 1–3 (instructions fixed per runtime version, the tool definitions pinned at creation, the capability summary generated from the grants), one `user` message for the goal (4), then the plan (5) as an `assistant` message, the compressed history (6) as one `feedback kind="history"` segment, the recent steps (7) as labelled segments in their original roles, the latest observation or feedback (8), and the decision request (9). Segments 1–4 are byte-identical across every request of a task until a grant changes (then once); segment 5 changes only on a `plan` decision; segment 6 changes only when compression runs; segments 7–9 change every step. Everything mutable is appended; nothing mutable precedes anything stable (C-07).
2. **Delimiting under every rule.** A segment is `<<label attrs>>`, one JSON body, `<</label>>`; labels are `observation`, `feedback`, `user`, `tool_definitions`, `plan`, `history` (the fork history of RFC 006). A compressed step is a `feedback kind="history"` entry built by the runtime from structured fields; the model's own messages keep the `assistant` role and are never relabelled. The system instructions say that labelled segments are data (M1 wording kept).
3. **The budget and the rules (REQ-CTX-003, 004).** The budget is LIM-C1 characters over all message contents (`[backend] context_chars` overrides it for one backend; the M4 report maps characters to tokens). Applied in order until the request fits: excerpts to LIM-C2 head and tail (already at observation time); repeated consecutive lines in an excerpt collapsed to one line plus `[… repeated N times …]`; every step older than the last LIM-C3 reduced to a summary of at most LIM-C4 characters built from `step`, `decision` kind, `tool`, `status`, `cause`, notice codes and the first line of output; then the oldest summaries dropped, LIM-C3 at a time, so that the history segment changes in batches (an event, not a constant). Never dropped and never summarized away: the capability summary (segment 3), every halt and resumption of the task, every rejection code, and every failed action no later successful action on the same tool and path superseded. When even the retained set exceeds the budget the request is sent anyway and the step records `over_budget`; no model is ever asked to summarize (D-12).
4. **Redaction (REQ-CTX-007, REQ-PRIV-002).** The runtime receives the resolved values of every configured secret with their names; every observation payload, excerpt, notice text and log line is scanned for each value (and its JSON-escaped form) and the match replaced by `[redacted:<name>]` before persistence. Values shorter than eight characters are not scanned (they would redact noise); the configuration refuses such a secret with a message. LIM-W5 content already never enters the workspace (RFC 006).
5. **Measurement (REQ-CTX-010, REQ-OBS-003).** Each step records `context_chars`, `prefix_chars` (the byte length of the longest common prefix with the task's previous request, serialized identically), `compressed_steps`, `dropped_steps`, `over_budget`, the model's latency, and the backend's reported usage (prompt, completion and cached tokens) when any. `get_metrics` derives the averages, the prefix ratio, and characters per prompt token when the backend reports usage, so a user can set `[backend] context_chars` from a model's window without guessing. The M4 report fixes LIM-C1 and LIM-C3 from these numbers on the reference model.
6. **Purity.** The assembler is a function of the persisted task, steps, actions, observations, questions, approvals, grants and plan, plus the fixed instructions: the same history yields the same bytes (REQ-TEST-016), and the fakes test it without a model.

## Alternatives considered

- Token counting with a tokenizer per model: exact but ties the core to the backend's vocabulary; characters with a per-backend override and a measured ratio keep the core model-free. Rejected.
- A rolling summary rewritten by the model: smaller contexts, but injected text would be laundered into runtime text (THR-33). Rejected (D-12).
- Putting the task's state summary first, as the drafts did: readable, but it re-encodes the whole prompt every step (C-07). Rejected.
- Dropping old steps one at a time: smallest context, but the history segment changes every step and the cache never settles. Rejected; batches of LIM-C3.

## Consequences

- The M1 assembler (`context_m1`) is replaced; its tests become the assembler's tests plus the compression suite of REQ-TEST-010.
- The step record gains the measures of point 5 (schema version of the step record bumps; old records read with defaults).
- `[backend] context_chars` is a new configuration key; `[backend] api_key` and server secrets (M5) feed the redaction set.
- The M4 report is a deliverable: context size, compression ratio and prefix reuse on the reference model, and the resulting LIM-C1 and LIM-C3.

## Open questions

- Whether the repeated-line rule should also collapse near-identical lines (timestamps, counters); decided from the report.

## References

Requirements §15 (REQ-CTX), REQ-OBS-003, REQ-TEST-010; external design §15, §17.2; limits LIM-C1 … C4; startup review D-12, C-07, M-18; the study §2.
