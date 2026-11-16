# Handoff 009h — MCP and context: the small fixes (release 0.11, phase 1)

**RFC.** [RFC 009](../../done/009-mcp-client-and-tool-pinning.md). **Found by.** The audit before 0.10 (A-12, A-25, A-27, A-29, A-30, A-39), the reviews of 009g and 013b-1, and the re-check of 2026-11-16 (`ROADMAP.md`, "Release 0.11", phase 1).

**Specification** (amended before this handoff):
- external design §11.1: "A destination is judged as it is sent" and "What a server writes on standard error is bounded";
- §6.4: `destination_not_canonical`;
- §8: the notice `content_omitted{count}`;
- REQ-CTX-008 and REQ-MCP-005.

**Embargo:** none.

## Items

The lines below are as found on `main` at `70ab081`; 011n changed nothing here.

1. **A-12: a destination is judged as it is sent.** Today `grant.rs:259-291` judges the parse of a `url::Url`, while `tools.rs:649` sends the raw string.
   - Admit a destination only when its parse serializes back to the same text (allow an empty path's `/`) and holds no user information.
   - Otherwise it is `denied`, `destination_not_canonical`, with a hint that gives the canonical form.
   - Apply the same check to the re-validation after `redirected`.
   - Never rewrite the string: the hash and the approval are of what the model wrote.
   - **Tests:** the audit's strings (a backslash, whitespace, control characters, `@` user information, a numeric host's short forms, an explicit `:443`) are refused, and one `m5_mcp` test shows that the server receives exactly the judged string.
2. **A-25: the other invisible characters.**
   - `sanitize` (`observation.rs:335-347`) also removes the default-ignorable characters and the missing format characters, as a hand table of ranges: U+00AD, U+034F, U+115F–1160, U+17B4–17B5, U+180B–180F, U+3164, U+FE00–FE0F, U+FFA0, U+FFF0–FFFB, U+1BCA0–1BCA3, U+1D173–1D17A, U+E0000–E0FFF, and the format characters U+0600–0605, U+06DD, U+070F, U+0890–0891, U+08E2, U+110BD.
   - U+2800 and U+FFFC are visible symbols, and stay.
   - Pins are unaffected (they hash the raw listing).
   - **Tests:** one unit test per class.
3. **A-27: a server's standard error is bounded.** `client.rs:203-211` reads lines with no bound.
   - **A line length bound:** read with a cap, mark the cut, and discard the rest up to the newline.
   - **A rate bound:** past it, write one line, "N lines dropped".
   - **The level:** lines stay at `info`. Operators read a server's errors there.
   - **The values:** propose them from measurement, and I register them as limits at review.
   - **Test:** the test server floods a line with no newline, and many lines. Memory and the number of log lines stay bounded.
4. **A-29 (a) and (b): the pin is compared on the connection the call uses.**
   - (a) Pass the expected hash into `Server::call` and compare it with `live.current` of the same `Live` the call is sent on, so that a restart between the pin check and the call is seen (today `tools.rs:621` and `tools.rs:646` take the server twice).
   - (b) Clear `dirty` **before** the listing (`server.rs:418`, after the listing at `:403`), so that a `list_changed` arriving during it is not lost.
   - (c) is closed by a stated decision (ROADMAP).
   - **Tests:** with the test server's `die`, `change_definition` and `FAIL_LIST_AFTER`, a definition changed by a restart between the check and the call is refused.
5. **A-30: a failed start counts.**
   - Set `started_once` when a start is attempted (before `Connection::start`), not after the first good listing (`server.rs:419`). Then a server that fails `initialize` or its listing every time is bounded by LIM-X3.
   - **The words:** when the limit is reached for a server that never served, the reason keeps the last start's own reason, for example `restart_limit_reached (last: spawn:not_found)`.
   - **Test:** `MIMLYS_TESTSERVER_FAIL_LIST_AFTER=0` gives 1 + LIM-X3 starts.
6. **The MCP parts of A-39:**
   - **(a) No dropped content without a word.** When there is no `structuredContent`, the content blocks after the first text block are not passed on, and the observation carries `content_omitted{count}` (`tools.rs:351-355`).
   - **(b) A server runs in a session of its own.** Use `setsid` (`process_wrap`'s session wrapper; the `process-session` feature) instead of a process group (`client.rs:184`), so that it has no controlling terminal. The kill of the group is unchanged.
   - The escape of a grandchild that detaches itself is closed by a stated decision (ROADMAP).
7. **One wait for an unanswering server.**
   - `McpTools` remembers, per server, a failed relisting at the step's boundary (`tools.rs:579-581`), with its reason, cleared at the next boundary.
   - `execute` then answers `tool_unavailable` with that reason without waiting again, so an unanswering server costs one LIM-O6, not two.
   - **Test:** `HANG_LIST_AFTER` with a server deadline of 1 s. The action ends within one wait (a timed assertion of LIM-O6's own size, as 009g's are).
8. **The stack-margin guard** (013b-1).
   - A debug-only knob (honoured only under `cfg(debug_assertions)`) sets the command thread's stack to a quarter of `RUNTIME_STACK_BYTES`.
   - The test harness sets it for the heaviest path, the MCP restarts test, and may set it for every test that starts the binary. Then a growth past a quarter fails in CI with a 4× margin.
   - A release build ignores it.

## Mutations

- a destination judged on its parse again;
- a class missing from `sanitize`;
- an unbounded line;
- the pin compared on another connection;
- `dirty` cleared after the listing;
- `started_once` set after the listing;
- blocks dropped without the notice;
- a process group instead of a session;
- the second wait;
- the knob ignored.

## Book and CHANGELOG

- Book:
  - `external-tools.md`: a destination's canonical form, a server's standard error, and the omitted content;
  - `decisions-and-tools.md`: the new `denied` reason.
- CHANGELOG: Fixed (each item) and Changed (`destination_not_canonical`, `content_omitted`).

Return: `.git-exclude/review-requests/009h-mcp-context/README.md`.
