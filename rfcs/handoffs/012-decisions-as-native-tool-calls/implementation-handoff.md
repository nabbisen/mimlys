# Handoff 012 — Decisions as native tool calls (release 0.9)

**RFC.** [RFC 012](../../accepted/012-decisions-as-native-tool-calls.md) (accepted 2026-11-08) · **Specification.** REQ-MDL-009 (new), REQ-MDL-004, REQ-DEC-003, REQ-CTX-001, REQ-CTX-003, REQ-CTX-010 (amended); external design §15.4 (new), §15.1, §17 (`decision.received`), §19.1 (`decision_transport`); limits LIM-C1, LIM-D1, LIM-M4 · **Evidence it rests on.** Spike S2 (handoff 008b), the M4 report §6 · **Release.** 0.9.

## 1. Scope

Requests ask for the decision through the engine's native function calling. The reply's one structured call is mapped onto the existing decision envelope, then passes the twelve stages unchanged. The envelope stays as the transport for an engine that refuses `tools`. Nothing is repaired. Then run B of M4, the real-model run, on both engines.

## 2. Behaviour (binding)

**Configuration and probe (REQ-MDL-009, external design §19.1)**
- `[backend] decision_transport = "auto" | "tool_calls" | "envelope"`, default `auto`.
- The probe in `status` sends one request with one function. HTTP 200 resolves `auto` to `tool_calls`. A refusal of `tools` (for example llama.cpp without jinja: HTTP 500 `tools param requires --jinja flag`) resolves it to `envelope`, with the reason in `status`.
- `tool_calls` configured and refused is `backend_unavailable` with the reason; never a silent fallback.
- `status` shows `decision_transport` in force. A probe that times out says `timeout (the model may still be loading)`.
- `structured_output` applies to the envelope transport only.

**The functions**
- One function per pinned tool. Its name is the tool's with `.` written as `__`, and must match `^[A-Za-z0-9_-]{1,64}$`. Its parameters are a JSON-schema object generated **deterministically** from the pinned descriptor list. The pinned definition and its hash do not change.
- Four reserved functions: `plan(steps: array of string)`, `ask_user(question: string)`, `complete(summary: string)`, `give_up(reason: string)`.
- A tool whose function name is invalid, or equals another's or a reserved one, is unavailable in this transport with the reason, at pin time.

**The reply (REQ-DEC-003, amended)**
- The decision is the reply's structured `tool_calls`: exactly one entry, naming an offered function, whose `arguments` string is exactly one JSON object within LIM-D1.
- Zero calls, several, an unknown name or unparseable arguments are `malformed` and consume the rejection counter. The rejection feedback says to call exactly one function.
- A call written into the reply's text is **never** read. Text beside a structured call is neither parsed nor placed in the context; its length is `text_chars` in `decision.received`.
- The request sends `parallel_tool_calls: false` where the engine accepts it, and no `tool_choice`.
- The mapped envelope is hashed canonically for `decision.received{hash, transport, text_chars?}`.

**The context (REQ-CTX-001, external design §15.4)**
- Same segments and labels, with these changes:
  - the definitions travel as `tools`;
  - earlier decisions are `assistant` messages with their call and id;
  - their observation or rejection is the `tool` message answering that id, still labelled;
  - the plan is an earlier `plan` call with its acknowledgement;
  - there is no decision-request segment.
- **A request never ends with an `assistant` message** (both engines refuse it).
- The fixed instructions of this transport keep REQ-CTX-002's three statements and say "call exactly one of the provided functions" instead of describing the envelope; the envelope keeps 007b's text.
- **Budget (REQ-CTX-003):** the character count includes the `tools` array. Prefix stability (REQ-CTX-010) is recorded for both transports.

## 3. Tests (each names its requirement; each with a mutation check)

- **Schema generation:** golden output for every built-in tool's descriptor; the same input gives the same bytes; an MCP tool name with `.`; invalid and colliding names unavailable with the reason.
- **Strict reply,** against a local HTTP stand-in that returns scripted chat-completion bodies:
  - one call → the mapped decision;
  - none, two, an unknown name, `arguments` not an object, over LIM-D1, a call only in the text → `malformed`;
  - text beside one call → accepted, `text_chars` recorded, the text absent from the next request.
- **Context shape:** for a three-step history, every call has its `tool` answer with the matching id; the labels are intact; the last message is not `assistant`; the `tools` array is counted in the budget.
- **Probe:** 200 → `tool_calls`; the jinja refusal → `envelope` with the reason; `tool_calls` configured and refused → `backend_unavailable`; a probe timeout's detail.
- **End to end with the fake path:** the M4 mock demonstration passes in both transports (the fake backend, or the stand-in, gains a `tool_calls` mode).
- **Mutations, at least:** a second call accepted; a text call read; the trailing assistant message allowed; the `tools` array not counted; the fallback silent when `tool_calls` is configured.

## 4. Run B (the real-model run; it decides 0.9's "done when")

- Model: Qwen2.5-7B-Instruct, Q4_K_M, the file on disk (SHA-256 recorded in S2).
- Engines: the packaged `llama-server` (say which build ran) and the packaged Ollama with the model created from the local GGUF, its own template, `num_ctx` 16384. Each started by hand on `127.0.0.1` and stopped afterwards; no service enabled.
- Run `m4_real` as `docs/src/maintainers/m4-report.md` §1 says, with `decision_transport = "auto"` (and say what it resolved to). At least **five runs per engine**.
- Report per run: outcome, steps, `chars_per_prompt_token`, prefix ratio against the engine's cached tokens, `text_chars` seen, whether `parallel_tool_calls` was accepted, model and wall time; the transcripts.
- Do not change the goal, the test, the grants or a limit. If a run does not complete, report it as it came and stop tuning.
- The architect fixes LIM-C1 and writes run B into the M4 report from your numbers.

## 5. Documents

- `docs/src/reference/configuration.md`: `decision_transport`, what `auto` does, which models were measured to return structured calls and which did not (S2), and Ollama's `num_ctx`.
- `cli.md`: the `status` field.
- The README and getting started: replace 0.8's "the model, plainly" sentence only after run B completes, with what run B shows; otherwise keep it.
- CHANGELOG.

## 6. Return

`.git-exclude/review-requests/012-native-tool-calls/README.md` (the change, the gate, CI per platform with skips, the mutations) and run B's results in the same package or in `L1-m4-real-model` at the top. Questions go into the request, never into a local redesign.
