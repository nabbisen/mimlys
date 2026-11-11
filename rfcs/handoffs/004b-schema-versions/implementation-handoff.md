# Handoff 004b — The two schema versions REQ-MAIN-002 requires (release 0.10)

**RFC.** [RFC 004](../../done/004-persistence-checkpoints-and-recovery.md) (persistence; versions recorded) · **Specification.** REQ-MAIN-002 (independent schema versions), REQ-PERS-012 (versions recorded at creation) · **Found.** Handoff 012's review: the code has a decision-schema version (`DECISION_SCHEMA_VERSION`), a persistence-schema version (`SCHEMA_VERSION`) and a configuration version (`config_version`), but **no event-schema version and no action/observation-schema version**.

**Status (2026-11-11): merged.** `EVENT_SCHEMA_VERSION` 2 and `ACTION_SCHEMA_VERSION` 2, each in the module it versions, recorded on every task, with older tasks read with their era's values. Question 1 is accepted: the history is numbered by release, and the forward rule is additive raises nothing. Question 2 is accepted: the era is shown on a copy of the fixture. External design §22 carries the proposed text.

- Add `EVENT_SCHEMA_VERSION` and `ACTION_SCHEMA_VERSION` (the action, execution and observation records) beside `DECISION_SCHEMA_VERSION`, each with a doc comment saying what a change of it means. Their first values describe 0.9.0's shapes: the event schema with `decision.received{transport, text_chars?}` is a version after 0.8.0's, so give 0.8.0's shape its own number, the next one to 0.9.0's, and say how they were chosen.
- Record both on every task at creation (`task.versions`), as REQ-PERS-012 records the others. A task created before this change reads with the values that describe its era.
- Rule for later changes, in the doc comments and in external design §22: an additive field raises nothing; a renamed or removed field, or a changed meaning, raises the version. The architect writes §22's text from your proposal.
- Tests: the versions recorded on a new task; a 0.9.0 store's tasks read with their era's values (009b's fixture approach); REQ-MAIN-002 named.

Return: `.git-exclude/review-requests/004b-schema-versions/README.md`.
