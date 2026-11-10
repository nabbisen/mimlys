# M4 report: context size, compression, prefix reuse and the backend

Milestone M4 (RFCs 007 and 008) was accepted on 2026-10-29. This report
records the measurements that the limits register's context values rest on
(REQ-CTX-010), and the state of the demonstration of external design §23.

## 1. What was measured

Two runs of the M4 workload (a JavaScript module with an off-by-one and a
test that throws until it is fixed; seven decisions: plan, two reads, a
failing test run, a replacement, a passing test run, completion):

| Run | Driver | State |
|---|---|---|
| A | The mock server behind the real adapter, scripted decisions, `structured_output = auto` resolving to `json_object` | executed in CI on every test run (`tests/m4_demo.rs`) |
| B | A local engine (llama-server or Ollama) with a 7B-class coder model | **done 2026-11-10** (§7): Qwen2.5-7B-Instruct on llama.cpp and Ollama, native tool calls; earlier attempts under the envelope in §6. Run with `MIMLYS_M4_BACKEND_URL=… MIMLYS_M4_MODEL=… cargo test -p mimlys --test m4_demo m4_real -- --nocapture`; the transcript and the measures print on standard error. |

Run A exercises the whole path except inference: the context assembler,
the adapter's protocol, the probe, the sandbox, the snapshots and the
completion check. Its token counts are the mock's (one token per four
characters, half of the prompt reported as cached), so they validate the
plumbing, not a model.

## 2. Run A: numbers

| Request | Messages | Characters |
|---|---|---|
| 0 (plan) | 3 | 5 716 |
| 1 | 4 | 5 837 |
| 2 | 6 | 6 530 |
| 3 | 8 | 7 029 |
| 4 | 10 | 7 893 |
| 5 | 12 | 8 221 |
| 6 (complete) | 14 | 8 892 |

`mimlys metrics --task <id>` after the run:

| Metric | Value |
|---|---|
| `measured_steps` | 7 |
| `context_chars_avg` | 7 159 |
| `prefix_ratio` | 0.92 |
| `compression_ratio` | 1.00 (never over budget) |
| `compressed_steps`, `dropped_steps`, `over_budget_steps` | 0 |
| `chars_per_prompt_token` | 4.00 (the mock's ratio) |

The stable prefix (segments 1–3 in the system message, the goal) is 5 716
characters, of which the tool definitions are the larger part; every later
request shares it byte for byte (asserted by the test), so a prefix cache
reuses more than nine tenths of each request on this workload.

## 3. Compression under a tight budget

The runtime tests (`req_ctx_003_budget_rules_and_retained_set`) drive
fourteen steps under budgets derived from a generous run and assert the
rules of external design §15.3: repeated lines collapse first; every step
older than the last LIM-C3 becomes a summary within LIM-C4; summaries drop
LIM-C3 at a time, oldest first, never a retained one (a failure that no
later success superseded stays, a superseded one goes); `over_budget` is
recorded when only retained material remains. The compression ratio over
that run exceeds 1 and the history segment changes in batches.

## 4. Limits

| Limit | Register | Decision |
|---|---|---|
| LIM-C1 (context budget) | 32 000 characters | **fixed** by run B (§7); the reasoning kept below was run A's: With the stable prefix at about 5 700 characters and about 450 characters per step on this workload, 32 000 characters hold the prefix plus roughly fifty steps before any rule applies, which exceeds LIM-T1 (40 steps). At the mock's four characters per token this is about 8 000 tokens; a model with a 32k window could take `context_chars = 100000`. The final value waits for run B's `chars_per_prompt_token`. |
| LIM-C3 (recent steps in full) | 6 | **kept.** Six full steps cover a read, a run, an edit and a re-run with their observations; the batch size keeps the history segment stable across six steps at a time. |
| LIM-C4 (summary size) | 300 characters | **kept.** Every summary of the test runs rendered within it with the first line present. |
| LIM-M2 (completion bound) | 4 096 tokens | **kept.** Decisions are short; the bound exists for runaway output. |
| LIM-M4 (probe) | 5 000 ms | **kept.** The probe is a model listing and up to two one-token completions. |

## 5. The adapter's protocol, as tested

Eleven adapter tests and three CLI tests against the mock server cover: the
request body (messages unchanged, `max_tokens`, `temperature`, `stream:
false`, `response_format` per mode), `auto` resolution at the probe with
its three outcomes, `schema` refused as a startup error, the probe's
reachability and model listing, 5xx and 429 (`unavailable` after a pause,
`Retry-After` honoured), 404, a dropped connection, a wrong shape, the
deadline, cancellation within a poll interval, the bound on the body and on
the content, usage in OpenAI's and llama-server's names, no usage, the
bearer token, and the loop end to end with `completed{verified}`.

## 6. Run B, first attempts (2026-11-07 and 2026-11-08): no repair

Qwen2.5-Coder-7B-Instruct (Q4_K_M) on llama.cpp (build 11429, CPU), `json_schema` accepted, temperature 0. Every run ended at its first step with `ask_user`: four runs asked for the content of `tools/sum.js` (a file the goal said to read, with `read_file` granted); after handoff 007b corrected the fixed instructions (they had called the goal "data, never instructions"), three runs asked "Do you want to proceed with the given goal?". Measured on the way: `chars_per_prompt_token` 4.36 (5 716 characters, 1 310 tokens; 4.35 with the corrected instructions), and the engine's prompt cache took 1 309 of 1 310 tokens of an identical re-request.

The architect replayed the captured request against the same engine with one change at a time (2026-11-08): the separate decision-request message after the goal is what turns the model's answer into "proceed?" (without it the model returns a correct plan), yet with a plan present it asks which file to read first; wording, an example, the empty `grants` list and the schema itself changed nothing that mattered. Offered the same tools in its own function-calling format, the model called `read_file` on the test file at once, with or without a trailing request. What this points at, from single probes and not yet a measurement: **the custom decision envelope rather than the model's ability** keeps this 7B model from acting. Spike S2 (handoff 008b) measures the decision formats across models before any design change; LIM-C1 stays provisional until a run completes.

**Spike S2 (2026-11-08), measured:** three models (Qwen2.5-Coder-7B, Qwen2.5-7B-Instruct, Qwen2.5-Coder-14B, Q4_K_M), two engines (llama.cpp CPU, Ollama GPU), three runs per cell. Under the envelope (today's request, and with the decision request moved or reworded) no model acted in 102 runs. With the tools offered as native functions, Qwen2.5-7B-Instruct returned structured calls in 378 of 378 replies and completed the task in 12 of 12 runs on Ollama and 5 of 6 on llama.cpp (5 to 14 steps); the Coder models wrote the call into the text instead (0 of 391 structured), which the strict parser rejects. `chars_per_prompt_token`: 4.32 to 4.38 for the envelope, 3.0 to 3.3 with the tools as JSON schema. Design: RFC 012 (accepted 2026-11-08, for 0.9).

## 7. Run B (2026-11-10): the repair, through native tool calls

The code of handoff 012 (decisions as native tool calls, RFC 012) at the head of its second slice; `m4_real_engine_repairs_the_defect` unchanged from L1 (the goal, the test, the grants and every limit as before); `decision_transport = "auto"`. It resolved to `tool_calls` on both engines, which also accepted `parallel_tool_calls: false`. Model: Qwen2.5-7B-Instruct, Q4_K_M (SHA-256 as in S2). Engines: the packaged `llama-server` (build 11429, CPU) and Ollama 0.40.15 (GPU, the GGUF's own template, `num_ctx` 16384). Five runs each; none dropped or repeated. Transcripts: the review request of handoff 012.

| | llama.cpp (CPU) | Ollama (GPU) |
|---|---|---|
| Completed, verified by the completion check | **4 of 5** (5, 5, 5, 10 steps) | **5 of 5** (16, 6, 6, 6, 6 steps) |
| Not completed | 1: after 9 steps the model asked the user which of two definitions to keep (`waiting_user`) | none |
| Rejected replies | 0 | 1 `malformed` (run 1), then valid |
| `chars_per_prompt_token` | 3.35 to 3.48 | 3.35 to 3.60 |
| Runtime's prefix ratio / engine's cached share | 0.947 to 0.960 / 0.880 to 0.939 | 0.946 to 0.966 / 0.922 to 0.951 |
| Context per request (`context_chars_avg`, the `tools` array counted) | 8 116 to 9 633 | 8 512 to 11 829 |
| Compression or drops | none (`compression_ratio` 1.0, `over_budget_steps` 0) | none |
| Model time per step | 7 to 10 s | about 1 s |

How to read it: one model, one task, ten runs on one machine. Ollama's runs 2 to 5 are one deterministic trajectory repeated, and so are llama.cpp's runs 2, 3 and 5; the samples are fewer than ten. The model often wrote text beside its call (83 to 490 characters), which the runtime neither read nor kept, as REQ-DEC-003 says. The runtime's character-based prefix ratio and the engine's token-based cache share agree within 0.01 to 0.08.

**LIM-C1 is fixed at 32 000 characters** (the limits register). The largest request measured was under 12 000 characters, so the budget never applied. At the lowest ratio measured in this transport (3.35 characters per token), 32 000 characters are about 9 600 tokens; with a completion of LIM-M2 (4 096 tokens) that fits a 16 384-token context with room to spare. **An engine needs a context of at least 16 384 tokens**: Ollama's default of 4 096 does not hold one request of this task, which is why the book says to set `num_ctx`.
