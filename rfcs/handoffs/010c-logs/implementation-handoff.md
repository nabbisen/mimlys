# Handoff 010c — The log carries the step's ids, and `debug_prompts` does what it says (release 0.10)

**RFC.** [RFC 010](../../done/010-cli-and-operator-interface.md) (the CLI and its log) · **Specification.** REQ-OBS-004 and external design §17.3 (amended 2026-11-12), REQ-PRIV-004; the configuration chapter's `[logging]` · **Found.** Handoff 002b item 1, slice 1 finding 1 and slice 2 finding 3.

**What is wrong today.**
- `logging.rs` installs a JSON formatter, so the timestamp and the level are there, and tested.
- No line carries a task, step or action id: every `tracing` call site logs its own fields only.
- `logging.debug_prompts = true` only prints the start warning ("prompts and model output are written to the log"); nothing writes them.

1. **The step's span.**
   - Each step runs inside a `tracing` span `step` with `task_id` and `step_id`; `action_id` is recorded on the span once the action exists.
   - The JSON formatter writes the current span's fields, so every line written during a step carries them: the contained panic, a server's warning during a call, the worker's lines.
   - Lines outside a step (start, configuration) carry none.
2. **`debug_prompts`.**
   - When on, at `debug`: each model request's messages as sent, and the model's reply text, as lines of their own.
   - Both go through the same redaction as every line, so the backend key and every `env:` secret never appear (REQ-PRIV-004).
   - The warning stays the first line. When off, nothing of a prompt, a file or a reply is written at any level: handoff 002b's tests already guard that, and they stay.
3. **The words.** Check that `configuration.md`'s line (`true logs prompts and model output, and warns at start`) and the start warning say what the code now does.

**Tests,** naming REQ-OBS-004 (now met) and REQ-PRIV-004:
- a run whose step writes a log line: that line has the ids of the step and of its action;
- with `debug_prompts` on, against the mock: the goal and the reply appear, and the secret never;
- with it off, as today.

**Mutations:**
- the span removed (the ids test fails);
- the redaction skipped for the prompt lines (the secret test fails).

Return: `.git-exclude/review-requests/010c-logs/README.md`.

## After review (2026-11-12): 010c-2

010c-1 is landed. Two changes follow it.

1. **The switch is the request** (question 1). `logging.debug_prompts = true` turns on the prompt target's `debug` lines whatever `logging.level` is: one directive in `logging.rs`. A user who turns the switch on gets what it says, without a second setting to find.
   - The start warning goes back to saying only that prompts and model output are written to the log (redacted).
   - The book's paragraph follows. External design §17.3 and its sample configuration already say this.
   - Tests: on at the default `info`, the request and reply lines appear and the secret never does; off at `trace`, none of them.
   - Mutation: the directive gated on the level again.
2. **The span reaches the threads and tasks that log for a step** (question 2). Two readers log on their own thread or task:
   - the worker's standard-error reader (`mimlys-exec`, "worker trace" lines);
   - the MCP client's standard-error reader (`mimlys-mcp`, `client.rs`).

   Each captures `Span::current()` where it is spawned and logs inside it, so their lines carry the step's ids when a step started them.
   - A reader that outlives its step (the worker serves many steps) takes the span of the execution it reads for, not the span of the step that started the process. Say in the request which you did and why.
   - Tests: a host tool's worker line, and a server's standard-error line during a call, carry the ids.
   - Mutation: the span not captured.

Questions 3 to 6 are accepted as done. Return: `.git-exclude/review-requests/010c-logs-2/README.md`.
