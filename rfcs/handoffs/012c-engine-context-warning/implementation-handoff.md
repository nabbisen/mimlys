# Handoff 012c — `status` warns when the engine's context is too small (release 0.10; bridge item B5)

**RFC.** [RFC 012](../../done/012-decisions-as-native-tool-calls.md) (the transport whose requests run B measured) · **Specification.** REQ-MDL-003 (the adapter), external design §20; limits LIM-C1, LIM-M2 · **Found.** Run B: Ollama's default context of 4 096 tokens cannot hold one request of the M4 task; the book tells users to set `num_ctx`, and nothing tells them they have not.

- **Read the engine's context length where the engine reports it:**
  - llama.cpp's `GET /props`: `n_ctx`, or the per-slot value;
  - Ollama: the model's `num_ctx` from `POST /api/show`, else its default.

  Do this inside the existing probe and its LIM-M4 bound. An engine that reports nothing gives `context: unknown`, never a guess.
- **The need:** the budget's tokens at the lowest characters-per-token measured in the transport in force (3.35 for `tool_calls`, 4.32 for the envelope; the M4 report §6, §7), plus LIM-M2. Name both ratios as constants with their source. `status` shows `context: <n> tokens (needs about <m>)`. When `n < m`, one line says what to do: Ollama's `num_ctx` in a Modelfile, or llama.cpp's `-c`. The run goes on.
- **Tests,** against the stand-in: llama.cpp-shaped `/props`; Ollama-shaped `/api/show`; neither (`unknown`); a small context warns with the remedy; a sufficient one does not. Mutation: the comparison inverted.

Return: `.git-exclude/review-requests/012c-context-warning/README.md`; `cli.md` and the configuration chapter updated.
