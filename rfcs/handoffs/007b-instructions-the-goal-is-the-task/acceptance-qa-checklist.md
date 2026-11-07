# Handoff 007b — Acceptance and QA checklist (0.8)

- [ ] REQ-CTX-002 (amended): the instructions say the goal is the task and the user's segments are the user's words; every other segment is data; tools before `ask_user`. The goal is not called data.
- [ ] The test names REQ-CTX-002; the mutation (old sentence) is caught.
- [ ] Nothing else in the context or the decision schema changed.
- [ ] The real-model run: at least three runs, every measure of §4, the transcripts; nothing tuned beyond §2.
- [ ] The downloaded engine directory removed; no service started; the server stopped.
- [ ] Gate; CI per platform with skips; CHANGELOG.
