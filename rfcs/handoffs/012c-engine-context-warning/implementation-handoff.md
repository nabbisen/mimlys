# Handoff 012c — `status` warns when the engine's context is too small (release 0.10; bridge item B5)

**RFC.** [RFC 012](../../done/012-decisions-as-native-tool-calls.md) (the transport whose requests run B measured) · **Specification.** REQ-MDL-003 (the adapter), external design §20; limits LIM-C1, LIM-M2 · **Found.** Run B: Ollama's default context of 4 096 tokens cannot hold one request of the M4 task; the book tells users to set `num_ctx`, and nothing tells them they have not.

- **Read the engine's context length where the engine reports it:**
  - llama.cpp's `GET /props`: `n_ctx`, or the per-slot value;
  - Ollama: the model's `num_ctx` from `POST /api/show`, else its default.

  Do this inside the existing probe and its LIM-M4 bound. An engine that reports nothing gives `context: unknown`, never a guess.
- **The need:** the budget's tokens at the lowest characters-per-token measured in the transport in force (3.35 for `tool_calls`, 4.32 for the envelope; the M4 report §6, §7), plus LIM-M2. Name both ratios as constants with their source. `status` shows `context: <n> tokens (needs about <m>)`. When `n < m`, one line says what to do: Ollama's `num_ctx` in a Modelfile, or llama.cpp's `-c`. The run goes on.
- **Tests,** against the stand-in: llama.cpp-shaped `/props`; Ollama-shaped `/api/show`; neither (`unknown`); a small context warns with the remedy; a sufficient one does not. Mutation: the comparison inverted.

Return: `.git-exclude/review-requests/012c-context-warning/README.md`; `cli.md` and the configuration chapter updated.

## After review (2026-11-12): what 012c-1 must change before it lands

Measured by the architect against the engines installed on the dev machine, with the model files under `~/.local/share/mimlys-m4/models/` (llama-server build 12110; Ollama 0.40.16 started privately with `OLLAMA_MODELS` under `.git-exclude/tmp/`, and a model created from the local file).

- **llama.cpp's slots.**
  - What was measured:
    - `-c 8192 --parallel 2` gives `default_generation_settings.n_ctx` 4096 and `total_slots` 2: the per-slot value is right.
    - The remedy `-c 16384` would still leave 8192 per slot.
  - When `total_slots` is more than 1, the remedy says the whole: `-c <suggested × slots>`, naming the per-slot figure, or `--parallel 1 -c <suggested>`.
- **Ollama: no assumed 4096.**
  - What was measured:
    - This Ollama chooses the default context from the GPU's memory: its log says `vram-based default context … default_num_ctx=4096` at 15.5 GiB, and a larger GPU gets more.
    - `/api/show` without a `num_ctx` reports nothing about it (`model_info` holds the model's trained maximum, 131072, not the context in force).
    - `/api/ps` reports `context_length` for a loaded model: the value in force.
  - The order is therefore:
    1. `/api/ps`'s `context_length` for the configured model when it is loaded;
    2. else `num_ctx` from `/api/show`'s `parameters`;
    3. else `unknown`.
  - The unknown case reads `context: unknown (Ollama sets it when it loads the model; needs about <m>)`. This answers question 1: the engine's word, never a guess.
- **`run` warns too** (question 3).
  - At the start of a task, after the probe, the same warning line, once, on standard error, like the structured-output notice.
  - When the probe said unknown and the engine is Ollama, `run` reads `/api/ps` once more after the first model reply, within LIM-M4, and warns then if the context is too small. That is the moment the model is loaded.
  - A user who never runs `status` meets the failure this item exists for.
- **The words.** In `status`'s text, `context: <n> tokens` reads `context: <n> tokens per request`: llama.cpp's figure is per slot.

Questions 2, 4 and 5 are accepted as done. Before the request, run against the real engines as above; the request shows the outputs. External design §20 gains the three `status` fields when this lands (the architect).
