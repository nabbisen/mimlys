# Handoff 011c — Acceptance and QA checklist (M6c)

- [ ] REQ-CONF-005 (macOS): Seatbelt (F, N, P), explicit environment (E), process group with the descendant walk (K best-effort), RSS watchdog (M best-effort), rlimits (C best-effort); states name their primitives; a missing `sandbox_init` is `unavailable`, never a crash.
- [ ] RFC 011 §5: tier `core` by default; `accept_best_effort = ["K"]` gives `full (K best-effort, accepted …)` shown in `status` and in approvals.
- [ ] REQ-CONF-010: the one `unsafe` call justified; `check-unsafe.sh` passes.
- [ ] REQ-TEST-013: the clause suite and the demonstration pass on `macos-latest`.
- [ ] Report, book section, CHANGELOG 0.8.
