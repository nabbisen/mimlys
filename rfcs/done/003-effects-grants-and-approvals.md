# RFC 003 — Effects, grants and approvals

**Status.** Implemented (0.7.0; accepted 2026-10-27)
**Tracks.** Decisions D-04, D-05 and D-19 of the startup review; findings C-06, X-03, X-09, R-01, R-03, R-04, A-11, M-02, M-08.
**Touches.** `mimlys-core` (effect, grant, policy, approval modules), the CLI approval prompt, the configuration's policy section, external design §7.2, §9, §10.

## Summary

Replace four overlapping classifications with one declared effect per tool from which approval, rollback, recovery and future parallelism derive; separate the administrator's ceiling from per-task grants; and make approvals one-shot, content-bound and runtime-rendered so that no model text can reach the approval screen.

## Motivation

- The drafts carried `side_effect_class`, `risk_class`, `approval_requirement` and idempotency metadata, plus three numbered permission schemes (X-03, X-09).
- Grants were both inside an immutable policy and attached to tasks (C-06).
- The approval request's description could have been model text, so prompt injection could write the approval screen (R-01); nothing bound an approval to the action's content (R-03); nothing sanitized the display (R-04); expiry had no consequence (A-11).
- Nothing defined when a task is complete (M-02).

## Decision

The normative text is external design §7.2 (effects), §9 (grants, ceiling, profiles), §10 (approvals and questions), §5.3 (completion) and requirements REQ-TOOL-001, REQ-TOOL-002, REQ-CAP-001 to REQ-CAP-010, REQ-APR-001 to REQ-APR-010, REQ-TASK-010. In brief:

1. **Effects:** `none`, `workspace_read`, `workspace_write`, `host_read`, `host_write`, `external_read`, `external_write`, with `reversible` and `idempotent`. One function of the core derives: approval required for `external_write`, for non-reversible `host_write`, and wherever a profile lists a tool; recovery after `unknown` by reversibility and idempotency; parallelism (deferred) for reads only. The D-04 vocabulary of five values is refined to seven so that `external_read` (a web search) and `external_write` (a push) are not the same class, and so that reads inside and outside the workspace are distinct.
2. **Ceiling and grants:** the administrator's policy is the ceiling (what may be granted, per profile); grants are per task, mutable, audited, issued by the user through the control API, always within the ceiling, never widened. The task's own workspace is readable and writable implicitly. Grant kinds: `tool`, `host_read`, `host_write`, `network`, `host_tool`.
3. **Approvals:** one-shot, for one exact action, bound to the action hash and the grant-set hash, consumed at execution start, invalidated on mismatch or revocation. The description is rendered by the runtime from the canonical action; the model's `reason` appears separately, labelled untrusted; every external string is escaped and stripped of invisible characters. Expiry is a rejection that halts the task. The user may approve, reject (with a note that reaches the model), reject and halt, or grant instead.
4. **Questions** (`ask_user`) use the same waiting mechanism and expiry.
5. **Completion:** a `complete` decision ends the task `completed` with `completion = verified` when a configured check passed, or `asserted` when none exists; the CLI shows which.
6. **Approval fatigue is a defect:** no default asks for approval of routine, reversible work (REQ-SEC-014).

## Alternatives considered

- Action-class approvals ("approve all deletes under X"): blurs approval and grant; modelled instead as a grant the user issues. Rejected.
- Trust MCP annotations for effect classification: server-asserted; rejected (RFC 009 declares effects administratively).
- Numbered risk levels L0–L5: numbers carry no meaning to a user; named effects do. Rejected.
- Approval per host-tool invocation instead of confinement: fatigue; rejected (D-06 option C).

## Consequences

- Tool declarations gain `effect`, `reversible`, `idempotent`, destination arguments (REQ-TOOL-001).
- The CLI prompt has a fixed, escaped template (REQ-CLI-002).
- Tests cover hash binding, one-shot consumption, expiry, invalidation and hostile strings on the screen (REQ-TEST-006).

## Open questions

Whether `external_read` should require approval by default in a future "confidential" profile; the shipped profiles do not require it. Decided when such a profile is requested.

## References

Requirements §7, §8, §9; external design §7.2, §9, §10; startup review D-04, D-05, D-19.
