# Handoff 008b — Spike S2: how the decision reaches a small local model (report only)

**RFC.** [RFC 008](../../done/008-model-backend-adapter.md) (the backend adapter) · **Specification.** REQ-MDL-004, REQ-DEC-003, REQ-CTX-001, REQ-CTX-002; external design §15; the M4 report §6 · **Kind.** Spike: measurements and a report, **no product change**. Whatever it shows, a change to how decisions are requested is an RFC of the architect's, written from your numbers.

## Status (2026-11-08): done

The report is accepted (`.git-exclude/review-requests/S2-decision-format/README.md`): its matrix is the basis of RFC 012 (accepted 2026-11-08, for 0.9) and of the M4 report §6. Nothing is open in this handoff.

## 1. Why

Run B of M4 never got past its first step (M4 report §6). Replaying the captured request against the same engine, the architect found that the model answers `ask_user` under the custom decision envelope, whatever the wording, but calls `read_file` at once when the same tools are offered in its own function-calling format. Single probes, not a measurement. This spike measures it.

## 2. Matrix

**Formats** (each built from what the runtime sends today; the runtime's twelve validation stages are not touched, since this is about what the model returns):

| Id | Format |
|---|---|
| F1 | today's request on `main` (envelope, `response_format` json_schema, the decision request as its own message) |
| F2 | F1 with the decision request's words carried at the end of the last message instead of as a message of its own |
| F3 | native tool calling: every granted tool as a function with its parameters as a JSON schema, and `plan`, `ask_user`, `complete`, `give_up` as four reserved functions; `tool_choice` "required" where the engine accepts it; the reply's tool call mapped back to the envelope |
| F4 | F3 without `tool_choice` |

**Models** (official repositories, GGUF Q4_K_M, SHA-256 verified against the repository's listing; weights only, no executable is downloaded): Qwen2.5-Coder-7B-Instruct (on disk), Qwen2.5-7B-Instruct, Qwen2.5-Coder-14B-Instruct. Add one more family only if the first three agree, and say which.

**Engines:** the packaged `llama-server` (llama.cpp) and the packaged `ollama` (both installed by the owner), each started by hand on `127.0.0.1` for the run and stopped afterwards; no service enabled, nothing outside `~/.local/share/mimlys-m4/` and Ollama's own store. Say which build ran (CPU or GPU) and how fast; a CPU build is acceptable if every cell can still be run.

## 3. What to measure per cell (model × format × engine)

A driver outside the product (a throwaway script or test under `.git-exclude/tmp/` or a branch, never merged) runs the M4 demonstration's task end to end: each reply is mapped to the envelope and fed to the real runtime, or, if that is impractical for F3/F4, to a faithful stand-in that executes the six built-in tools on a copy of the workspace and returns the observations as the runtime would. Per cell, three runs at the runtime's temperature (0):

- the outcome (`completed{verified}`, `halted` with its reason, or stuck on `ask_user`), steps, the first decision's kind;
- how many replies parse as a valid decision; how many tool calls name an unknown tool or invalid arguments;
- for F3/F4: whether the engine returns `tool_calls` or puts the call in the text (Qwen2.5-Coder on llama.cpp put it in a code block in the architect's probe); what the engine needs (`--jinja`, a chat template) for the former;
- prompt tokens, `chars_per_prompt_token`, the engine's cached tokens per step, wall time.

## 4. Return

`.git-exclude/review-requests/S2-decision-format/README.md`: the matrix as a table, the transcripts in `evidence/`, every model's SHA-256, the engines' builds, what each engine accepted or refused, and your reading of it. No change to the product, the M4 report or the limits register. Remove the driver's scratch files when done; keep the models.
