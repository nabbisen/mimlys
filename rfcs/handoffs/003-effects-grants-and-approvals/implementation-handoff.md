# Handoff 003 — Effects, grants, policy and approvals

**RFC.** [RFC 003](../../accepted/003-effects-grants-and-approvals.md) (Accepted 2026-10-27)
**Specification.** Requirements REQ-TOOL-002; REQ-CAP-001 … 010; REQ-APR-001 … 010; REQ-TASK-010; REQ-SEC-002, 014; REQ-CFG-004; external design §5.3, §7.2, §9, §10, §19.1 (policy section); limits LIM-A1, LIM-A2, LIM-D2, LIM-D4, LIM-I5, LIM-I6, LIM-W4, LIM-W5.
**Crate.** `mimlys-core` (effects, grants, policy, approvals, questions, rendering). The policy *file* parsing belongs to handoff 010; this handoff defines the typed `Policy` the file deserializes into.
**Milestone.** M1. Depends on handoffs 001 and 002.

## 1. Scope

In: the effect type and the one derivation function; grant kinds, grant records and the grant-set hash; the policy ceiling and profiles as types with validation; the validator hooks of handoff 002 (`GrantCheck`, `EffectDerivation`, `BudgetReservation`); approval requests with runtime rendering, binding, one-shot consumption, expiry and invalidation; questions; the completion rule.

Out: the terminal prompt itself (handoff 010 renders the string this handoff produces); persistence (004); the policy TOML schema (010).

## 2. Module layout

```text
crates/mimlys-core/src/
  effect.rs    effect/tests.rs    Effect, EffectDecl{effect, reversible, idempotent}, `derive(&EffectDecl, &Profile) -> Derived{approval_required, after_unknown, parallel_ok}`
  grant.rs     grant/tests.rs     GrantKind, Grant, GrantSet (hash, lookup), matching of paths and destinations
  policy.rs    policy/tests.rs    Ceiling, Profile, Policy (hash), `Policy::validate()`, `Ceiling::permits(&GrantKind)`
  approval.rs  approval/tests.rs  ApprovalRequest, ApprovalStatus, `render(&AuthorizedAction, &GrantSet, model_reason) -> Rendered`, binding checks, expiry
  question.rs  question/tests.rs  Question, answer handling
  hooks.rs                        impls of handoff 002's hook traits over GrantSet/Policy/Budgets
```

## 3. Behaviours

### 3.1 Effects (ED §7.2; REQ-TOOL-002)

- `Effect`: the seven values; `EffectDecl { effect, reversible, idempotent }` with constructor rules: `workspace_write` is always reversible; `external_write` is never reversible (refuse other combinations at construction so a tool author cannot declare nonsense).
- `derive` is the single function that yields `approval_required` (REQ-APR-001: `external_write`; `host_write && !reversible`; any tool the profile lists in `approval_tools`; promotion), `after_unknown` (`Redo | Rollback | Halt(HaltReason)` per ED §7.2), and `parallel_ok` (reads only; unused until DR-001 but tested now so the rule is frozen). Handoff 001's `EffectView` is produced from `EffectDecl` by a `From` impl.

### 3.2 Grants (ED §9.1, §9.3; REQ-CAP-001 … 007)

- `GrantKind`: `Tool{name}`, `HostRead{root, patterns}`, `HostWrite{root, patterns}`, `Network{domains, ports}`, `HostTool{name}`. `Grant { grant_id, task_id, kind, issued_by: Actor, issued_at, status, valid_until }`.
- `GrantSet`: the active grants; `hash()` canonical (REQ-CAP-007); `visible_tools()`; `permits_path(&WorkspacePath | HostPath, Access)`; `permits_destination(url)` (scheme `https`, host in `domains`, port in `ports`; M5 uses it, implement and test now); implicit workspace read/write is a method, not a stored grant.
- Revocation sets `status = Revoked` and returns the ids of pending approvals that used the grant so the loop invalidates them (REQ-CAP-005); a grant never mutates its `kind` (REQ-CAP-006: no setter).

### 3.3 Policy and ceiling (ED §9.2, §19.1; REQ-CAP-003, 009, 010; REQ-CFG-004)

- `Ceiling { host_read_roots, host_write_roots, network_domains, tools, max_steps, … }`; `Profile { name, tools, budgets, approval_tools, on_reject, on_cancel }`; `Policy { config_version, default_profile, remote_backend_allowed, ceiling, profiles, exclusions{materialize, secrets}, recovery{auto_resume} }` with `hash()`.
- `Policy::validate()` refuses a profile that exceeds the ceiling, an unknown tool name, an exclusion pattern that fails to compile, and a budget above the limits register's range (CLR-3: the error states the ceiling or range and the key path).
- `Ceiling::permits(&GrantKind) -> Result<(), Denied{reason, ceiling}>`; the shipped profiles `read_only`, `workspace`, `project` are constructed in `Policy::shipped()` and tested against ED §9.2.
- Ceiling reduction for running tasks (REQ-CAP-010): `GrantSet::reconcile(&Ceiling) -> Vec<GrantId>` returns the grants to revoke; the loop (010) calls it at each step boundary and emits `grant.revoked` with `issued_by = Runtime`.

### 3.4 Validator hooks (handoff 002 §3.4)

Implement `GrantCheck` (stage 7: tool visibility already checked at stage 4; here path and destination permissions and exclusions, `denied{reason}`), `EffectDerivation` (stage 9), `BudgetReservation` (stage 10: steps, deadline, executions per action, workspace quota from a counter the loop passes) over the types above.

### 3.5 Approvals (ED §10; REQ-APR-001 … 009; REQ-SEC-002)

- `ApprovalRequest { approval_id, task_id, action_id, action_hash, grant_set_hash, rendered: Rendered, model_reason: Option<String>, requested_at, expires_at, status, decided_at, decided_by, note, halt }`.
- `render` builds `Rendered { lines: Vec<RenderedLine> }` from the canonical `AuthorizedAction` only: tool, effect flags, one fixed "Will …" sentence per tool from a table (`approval/sentences.rs`), each argument as `name = quoted value` with resolved absolute paths supplied by the caller (handoff 010 knows the workspace root), the grants used, the expiry. `model_reason` is sanitized (handoff 002's `sanitize`), truncated to LIM-D2, and kept in its own field labelled untrusted; the total rendering is bounded by LIM-I6 (truncate argument values, never the fixed lines). Quoting: JSON string quoting for every external value, so newlines and quotes cannot start a new line or a fake label (THR-21).
- `decide(&mut self, Decision::Approve | Reject{note, halt}, by, now)`; `consume(&mut self, action_hash, grant_set_hash, now) -> Result<(), Invalidated>` compares both hashes (REQ-APR-004) and moves `Approved → Consumed` once (REQ-APR-005); `expire(now)` when `expires_at` passed → `Expired`, which the loop maps to `HaltReason::ApprovalExpired` (REQ-APR-006). `invalidate(reason)` on hash mismatch or grant revocation.
- Nothing in this module reads model output except `model_reason`, which is display-only (REQ-SEC-002, REQ-APR-008).

### 3.6 Questions (REQ-APR-010)

`Question { question_id, task_id, step_id, text (≤ LIM-D4, sanitized), asked_at, expires_at, status, answer, answered_by }`; `answer(text ≤ LIM-D4)`; expiry → `HaltReason::QuestionExpired`. At most LIM-A2 pending item per task: the loop enforces it; provide `Task::pending_item()` in handoff 001's types if missing (coordinate; add to the review request).

### 3.7 Completion (ED §5.3; REQ-TASK-010)

`CompletionCheck { tool, arguments }` on the task; on a `Complete` control decision the loop runs the check as an ordinary action (through the validator); `Completion::Verified` when it succeeds, `Asserted` when no check exists; a failed check is an observation and counts toward LIM-T6. This handoff supplies the types and the decision table; the loop (010) wires it.

## 4. Tests (REQ-TEST-006, 003)

- `derive` table: every effect × reversible × idempotent × profile listing → expected approval, after-unknown, parallel (`req_tool_002_*`, `req_apr_001_*`).
- Grants: path matching with patterns and case modes; destination matching (scheme, domain, port); revocation returns pending approvals; no widening API exists (`req_cap_001_*` … `req_cap_007_*`).
- Policy: shipped profiles equal ED §9.2; validation refusals name the key and the ceiling; `reconcile` finds excess grants (`req_cap_003_*`, `req_cap_009_*`, `req_cap_010_*`).
- Approvals: hash binding (changed arguments or revoked grant → invalidated), one-shot (second `consume` fails), expiry, rendering with hostile strings (newlines, ANSI, bidi, a fake `[a]pprove` line, a 10 kB reason) stays within LIM-I6 and keeps every external value quoted (`req_apr_002_*` … `req_apr_006_*`).
- Questions: bounds, expiry (`req_apr_010_*`).
- Completion decision table (`req_task_010_*`).

## 5. Recommended crates

`globset` (patterns), `url` (destination parsing), plus those of handoffs 001–002. No I/O.

## 6. Return

Review-request package under `.git-exclude/reviewed/m1-003/`, including the table of "Will …" sentences and the hint strings added, which are model- and user-facing contract text.
