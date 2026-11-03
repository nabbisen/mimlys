# RFC 002 — Decision schema and validation order

**Status.** Implemented (0.7.0; accepted 2026-10-27)
**Tracks.** Decision D-01 of the startup review; findings M-01, M-11, R-02, A-09, C-04, C-10.
**Touches.** `mimlys-core` (decision, validator, rejection modules), the adapter SPI, the control API, external design §3, §4, §6.

## Summary

Define the one contract between the model and the runtime: a flat decision object with five kinds, parsed strictly with no repair; a twelve-stage validation order in which authorization precedes any filesystem access; a fixed rejection vocabulary with hints; and three API tiers so that loop internals never appear on the client surface.

## Motivation

- No draft defined what the model returns (M-01); every other contract depended on it.
- The drafts checked resource existence before authorization, letting a model probe whether `/etc/shadow` exists through the error class (R-02).
- Rejections had no vocabulary, so error text could leak policy internals or filesystem contents (M-11).
- The draft "core interface" exposed `submit_decision` and `execute_action` to clients, bypass surfaces the same draft forbade (C-10).

## Decision

The normative text is external design §3 (API tiers), §4 (control API), §6.2 (decision schema), §6.3 (validation order), §6.4 (rejection vocabulary), §6.5 (counters) and requirements REQ-ARCH-004, REQ-ARCH-005, REQ-DEC-001 to REQ-DEC-012, REQ-SEC-009. In brief:

1. **Decision schema v1:** `act{tool, arguments, reason?}`, `plan{steps}`, `ask_user{question}`, `complete{summary}`, `give_up{reason}`. Flat, bounded (LIM-D1 to LIM-D4), unknown fields refused. `reason` is the model's text, shown separately on approval screens and never used by the runtime.
2. **Strict parsing:** exactly one JSON object, no trailing text, no duplicate keys, no repair of any kind; a format failure is the rejection `malformed` and consumes the rejection counter.
3. **Validation order:** state → size and parse → decision schema → tool visibility → argument schema → canonicalization → authorization → semantic bounds → effect and approval derivation → budget reservation → approval → execution. No stage compensates for a skipped one; no filesystem access before execution; an out-of-scope path is `denied` identically whether or not it exists.
4. **Rejection codes:** `malformed`, `schema`, `unknown_tool`, `invalid_argument`, `denied`, `state`, `budget`, `approval_rejected`, each with fixed `detail` keys and a hint of at most LIM-I5 characters that says what the model may change.
5. **API tiers:** control API (clients), adapter SPI, loop API. The executor accepts only authorized actions the validator constructs (type-level enforcement of INV-01).
6. **Counters:** `steps`, `consecutive_rejections`, `consecutive_failures`, `identical_decisions`, each halting the task at its limit; identical means the same decision hash with an unchanged snapshot.

## Alternatives considered

- Let the runtime repair near-valid JSON: silent reinterpretation of an untrusted output; rejected (CLR-2).
- Check existence during semantic validation for better feedback: leaks the filesystem; rejected (R-02).
- A nested, richer decision schema (plans as graphs, multi-action decisions): small local models fail on nested schemas, and one action per step keeps transactions simple; deferred with DR-001.
- Expose loop internals on the control API for external agent frameworks: a bypass surface; rejected. Embedding uses the loop API, which runs the same validation.

## Consequences

- The fake backend (REQ-MDL-002) scripts decisions in exactly this schema, so M1 tests the runtime without any model.
- Rejection counts per code become the first interface-quality metric (CLR-12).
- A change to any field or code is a breaking change of the external contract (REQ-MAIN-001).

## Open questions

Whether `structured_output = schema` should be the default once M4 measures how often small models emit invalid JSON without it. Decided in RFC 008.

## References

Requirements §4, §6; external design §3, §4, §6; startup review D-01, M-01, R-02.
