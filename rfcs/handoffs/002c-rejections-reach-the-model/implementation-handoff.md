# Handoff 002c — A rejection reaches the model with its reason and detail (release 0.10)

**RFC.** [RFC 002](../../done/002-decision-schema-and-validation-order.md) (the validator and its rejections), [RFC 007](../../done/007-context-assembly-and-observation-pipeline.md) (the feedback segment) · **Specification.** REQ-DEC-008, REQ-DEC-009, external design §6.3 and §6.4 (CLR-5: "a `denied` hint is chosen by its `reason`, so the model is told what to change"); REQ-OBS-008; REQ-DEC-010; REQ-TASK-004 and external design §5.4 (amended 2026-11-12) · **Found.** Handoff 010b's request, discrepancies 1, 2 and 7; handoff 002b item 1, slice 2 finding 1 and slice 3 finding F2.

**What is wrong today.** The validator builds a `Rejection` with the right hint (`DeniedReason::hint` for `denied`) and a typed `detail`. Then the loop stores only `rejection.code` (`runtime/loop_.rs`, `step.rejection = Some(rejection.code)`), and the feedback segment is built from the code (`runtime/context.rs`, `feedback_segment_in(code, transport)` → `code.hint_for(transport)`). So:

- every `denied` gets the missing-grant sentence: a model that wrote `../x` is told to ask the user for a grant, and the `scheme_not_https` sentence of 0.10 never reaches it;
- `schema` says "the field at `path`", `unknown_tool` says "listed in `visible_tools`", and neither key is in the segment.

1. **The feedback carries the rejection, not its code.**
   - The segment's body is `{"code", "hint", "detail"}`:
     - the hint is the `Rejection`'s own: by reason for `denied`, and adjusted for the transport where `hint_for` adjusts it today (`malformed` and `unknown_tool` in `tool_calls`);
     - the detail has exactly the fixed keys of external design §6.4.
   - The rejected decision is still not repeated. Nothing from the filesystem, no policy internals and no other task's data reaches the model (REQ-DEC-008). The denial of a path outside the workspace stays identical whether or not the path exists (REQ-DEC-007, INV-03).
   - Every hint names only keys that the segment carries. `unknown_tool`'s `visible_tools` is the list of tools the model can already see.
   - Bound every string of the detail; write the bound as a LIM id.
2. **The detail is persisted with the step,** so that the context after a restart, the next request and a fork's history all see what the live run saw.
   - This is `SCHEMA_VERSION` 3: one nullable column, and a migration from 2 tested against a store written by 0.9.0, with handoff 009b's fixture as the model.
   - A step written before the migration has the code only; its feedback is the code's hint, as today.
   - RFC 015's backend per step then takes the next version; its decision 4 is noted.
3. **The event.** `decision.rejected` gains `reason` (for `denied`, `state` and `budget`) and `grant_kind` (for `denied`). These are additive fields: under external design §22 no version is raised.
4. **Counted (REQ-OBS-008).**
   - `metrics` counts denials by reason and by grant kind, from the events, in text and JSON.
   - `cli.md`'s `metrics` entry says what the requirement says: a rising rejection rate is a defect of the interface the model is given (its tool definitions, the instructions, the transport). It is to be investigated, not tolerated as noise.
5. **The budget vocabulary.** Stage 10 rejects for the deadline and the workspace quota only (external design §6.3 and §6.4, amended). `BudgetKind::Steps` and `BudgetKind::Executions` are never built: remove them. The step budget halts at the step boundary, and LIM-T3 bounds an action's retries.
6. **Convergence (REQ-DEC-010).**
   - Both production call sites build the step result with `snapshot: None` (`runtime/loop_.rs`, `runtime/control.rs`), so a decision repeated LIM-T4 times halts even when the tree changed.
   - Pass the workspace's content identity in force when the decision was made: the manifest hash of the task's latest snapshot. The same decision with an unchanged tree halts `convergence`; with a changed tree it does not.
   - First list which actions produce a snapshot, so that the request says which repeated decisions can now continue.
   - Test through the loop, not only on `Counters::apply`.
7. **A fork of a halted source onto its own scope.**
   - The refusal stays: a halted task still holds its scope (REQ-TASK-012).
   - When the overlapping task is the fork's source, the `conflict` detail names the source and both ways on: another `--scope`, or `mimlys task cancel <source>` first (REQ-TASK-004, external design §5.4, amended).
8. **The book.** In `how-a-task-runs.md` (handoff 010b), the "Today" note and the sentences about `denied`, `schema` and `unknown_tool` follow the code. Check `decisions-and-tools.md` too.

**Tests,** each naming its requirement. Through a real step:
- a path outside the workspace gets the relative-path hint;
- an `http` URL gets the `https` hint;
- a schema error's segment carries its `path`;
- the same decision after a restart gets the same segment.

Also: the migration against the 0.9.0 fixture; the counters; the convergence pair; the fork message.

**Mutations:**
- the feedback built from the code again;
- the detail not persisted;
- the snapshot not passed;
- a denial not counted.

Return: `.git-exclude/review-requests/002c-rejections/README.md`.
