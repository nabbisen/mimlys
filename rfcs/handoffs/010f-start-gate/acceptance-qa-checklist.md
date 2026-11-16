# Handoff 010f — Acceptance checklist (0.11, phase 2)

- [ ] A first stop signal either finds an action not begun, which never begins, or begun and recorded `running`; no signal falls between the check and the start (A-33; §19.2)
- [ ] A wait made before anything is sent (the MCP pin check) observes the first stop request and ends the action not begun; the task pauses and is never halted `tool_unavailable`
- [ ] The arm failure of `req_cli_005_one_stop_signal_pauses_at_the_next_boundary` is explained, and fixed in the test or the product
- [ ] Tests by state with mutations; `m5_signals` green; gate; CI per platform with skips; book; CHANGELOG; linear history
