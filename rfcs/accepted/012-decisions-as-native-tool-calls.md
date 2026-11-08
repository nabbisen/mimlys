# RFC 012 — Decisions as native tool calls

**Status.** Accepted (2026-11-08, by the owner). For release 0.9.
**Tracks.** The M4 report §6 (run B never got past its first step); spike S2 (handoff 008b, `.git-exclude/review-requests/S2-decision-format/README.md`).
**Touches.** `mimlys-model` (the OpenAI-compatible adapter: the request and the reply), `mimlys-core` (context assembly per transport, the fixed instructions, the budget's count), `mimlys` (configuration, `status`, the book); requirements REQ-MDL-003, REQ-MDL-004, REQ-DEC-003, REQ-CTX-001, REQ-CTX-002, REQ-CTX-003; external design §15, §19.1, §20; limits LIM-C1.
**Release.** 0.9.

## Summary

Ask the model for its decision through the engine's native function calling, not through a JSON envelope written as text. Each pinned tool is offered as a function, and the four other decision kinds (`plan`, `ask_user`, `complete`, `give_up`) as four reserved functions. The reply's single tool call is mapped onto the existing decision, which then passes the same twelve validation stages. The envelope stays only for an engine that refuses `tools`. Nothing is repaired: a reply without exactly one structured call is `malformed`, as today.

## Motivation

S2 ran the M4 task on the real runtime, three models (Qwen2.5-Coder-7B, Qwen2.5-7B-Instruct, Qwen2.5-Coder-14B, Q4_K_M), two engines (llama.cpp, Ollama) and four request formats, three runs per cell:

- **Under today's envelope no model acted in any of 102 runs.** Each wrote a correct plan or asked for a file, then asked "Do you want to proceed?" or repeated the plan. Neither wording (F2, the 007b instructions) nor `response_format` changed an outcome class.
- **Offered as native functions, Qwen2.5-7B-Instruct acted and finished**: `completed{verified}` in 12 of 12 runs on Ollama and 5 of 6 on llama.cpp, in 5 to 14 steps. It returned structured `tool_calls` in 378 of 378 replies.
- **The two Coder models never returned `tool_calls`** (0 of 391 replies). They wrote the call into the text, often after prose and often malformed; they completed only when a harness read the call out of the text.
- `tool_choice: "required"` made no difference. A request ending in an assistant message is refused by both engines under `response_format` (HTTP 400). Native requests count 3.0 to 3.3 characters per token, against 4.4 for the envelope, because the schemas travel as JSON.

The envelope is the cause, not the models' ability: the same 7B model that never acts under it finishes the task through the channel it was trained to act in. A user who configures a capable local model today sees a task waiting for an answer it never needed; that is the product's defect.

## Decision

1. **A decision transport per backend.** `[backend] decision_transport = "auto" | "tool_calls" | "envelope"`, default `auto`. With `auto`, the probe in `status` sends one request with one function. If the engine accepts it (HTTP 200), the transport is `tool_calls`; if it refuses `tools` (for example llama.cpp without jinja: HTTP 500 `tools param requires --jinja flag`), it is `envelope` and `status` names the reason. `status` always shows the transport in force. `tool_calls` configured explicitly and refused by the engine is `backend_unavailable` with that reason, never a silent fallback.
2. **The functions.** Every pinned tool becomes one function. Its parameters are a JSON-schema object generated from the pinned descriptor list, deterministically: the pinned definition and its hash do not change. Its name is the tool's name with `.` written as `__` (MCP tools are `<server>.<tool>`), and it must match `^[A-Za-z0-9_-]{1,64}$`. Four reserved functions carry the other decisions: `plan(steps)`, `ask_user(question)`, `complete(summary)`, `give_up(reason)`. A tool whose function name is invalid, or equals another tool's or a reserved one, is unavailable in this transport with the reason, at pin time (as an over-long description is today).
3. **The reply, strictly (REQ-DEC-003 holds).** The decision is the reply's `tool_calls`: exactly one entry, naming an offered function, whose `arguments` string is exactly one JSON object within LIM-D1. Zero calls, more than one, an unknown name or arguments that do not parse are `malformed`, and consume the rejection counter. A call written into the text is never read: the adapter repairs nothing, and a model that does not produce structured calls is not supported in this transport (the book says which models were measured). Text beside the call is not part of the decision: it is neither parsed nor put into the context, and its length is recorded in `decision.received`. The request asks for one call (`parallel_tool_calls: false` where the engine accepts it). It sends no `tool_choice`: S2 measured no effect, and it is one compatibility risk fewer.
4. **The mapped decision is the decision.** The call becomes the existing envelope (`act` with the tool and its arguments, or the reserved kind with its field) before stage 1 of the twelve. Schema, validation order, rejection codes, events and the decision schema version are unchanged. `decision.received` hashes the canonical form of the call.
5. **The context in this transport (REQ-CTX-001, amended).** The segments keep their order and provenance, with three changes:
   - the tool definitions travel as `tools`, not as a system segment;
   - an earlier decision is an assistant message carrying its call; its observation, or the rejection of it, is the `tool` message answering that call, still labelled (`<<observation …>>`, `<<feedback kind="rejection">>`);
   - the stored plan is an earlier `plan` call with its acknowledgement, and there is no decision-request message, so a request never ends with an assistant message.

   The fixed instructions of this transport keep the three statements of REQ-CTX-002 and say "call exactly one of the provided functions" instead of describing the envelope.
6. **The budget counts what is sent (REQ-CTX-003, amended).** The context's character count includes the `tools` array. LIM-C1 is fixed from a completed run in this transport, with its `chars_per_prompt_token`.
7. **The engines, as measured.** llama.cpp needs its chat template with tools (jinja, on by default in current builds). Ollama's OpenAI endpoint cannot set the context length and defaults to 4096 tokens: the book says to set `num_ctx` in the Modelfile. A `status` probe that times out while the engine loads a model says so: `detail: timeout (the model may still be loading)`, instead of a bare timeout.
8. **The demonstration and the book.** The M4 real-model run (run B) uses Qwen2.5-7B-Instruct and `tool_calls`. The book's model example names it and states that the Coder variants measured did not return structured calls.

## Alternatives considered

- **Keep the envelope and tune the words.** S2 measured F2 and the 007b text: no outcome class changed on any model or engine.
- **Read a call written into the text.** It would make the Coder models work in part (Coder-14B 3 of 3, Coder-7B 0 to 3 of 3, depending on template and engine), but it is the repair REQ-DEC-003 forbids. It turns prose, and anything a tool result smuggles into prose, into actions. Rejected.
- **Grammar-constrained native calls.** Not measured; if an engine offers them, they fit under decision 3 unchanged.
- **Drop the envelope entirely.** An engine that refuses `tools` would then be unusable. The envelope stays as the stated fallback, and `status` shows which transport is in force.

## Consequences

- A capable instruct model on a local engine can finish a task; the Coder models measured cannot, and the book says so.
- The fixed instructions and the context's shape differ per transport. Prefix stability (REQ-CTX-010) is measured for both.
- A configuration change (`decision_transport`), a new `status` field, and new `decision.received` fields. Under REQ-MAIN-001 these are additions, not breaking changes.
- The 007b instructions remain the envelope transport's text.

## Open points for the implementation (measured, not decided here)

- How often text accompanies a structured call (decision 3), and whether `parallel_tool_calls: false` is accepted by both engines.
- The probe of decision 1 on Ollama when the model is not loaded yet.

## References

M4 report §6; spike S2 (handoff 008b); RFC 007, RFC 008; requirements REQ-DEC-003, REQ-MDL-003, REQ-MDL-004, REQ-CTX-001 … 003, REQ-CTX-010; limits LIM-C1, LIM-D1, LIM-M4.
