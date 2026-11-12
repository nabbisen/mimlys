# Handoff 010c — Acceptance checklist (0.10)

- [ ] A line written during a step carries `task_id`, `step_id` and, with an action, `action_id`
- [ ] `debug_prompts = true` writes the requests' messages and the replies at `debug`, redacted; never a secret; off writes none of it at any level
- [ ] REQ-OBS-004 named by a test of each clause; mutations of the span and of the redaction fail
- [ ] Gate; CI per platform with skips; CHANGELOG; linear history
