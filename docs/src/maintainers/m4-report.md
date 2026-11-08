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
| B | A local engine (llama-server or Ollama) with a 7B-class coder model | **pending**: no engine is installed on the reference machine (checked 2026-10-28: no `llama-server`, no `ollama`, no model files; an NVIDIA RTX 5060 Ti with 16 GB and 59 GB of RAM are available). Run with `MIMLYS_M4_BACKEND_URL=… MIMLYS_M4_MODEL=… cargo test -p mimlys --test m4_demo m4_real -- --nocapture`; the transcript and the measures print on standard error. |

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
| LIM-C1 (context budget) | 32 000 characters | **kept provisionally.** With the stable prefix at about 5 700 characters and about 450 characters per step on this workload, 32 000 characters hold the prefix plus roughly fifty steps before any rule applies, which exceeds LIM-T1 (40 steps). At the mock's four characters per token this is about 8 000 tokens; a model with a 32k window could take `context_chars = 100000`. The final value waits for run B's `chars_per_prompt_token`. |
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

## 7. What run B will add

The transcript of a real repair, the engine's own cached-token count
against the runtime's prefix ratio, `chars_per_prompt_token` for the
model's tokenizer, the number of steps a 7B-class model needs, and whether
`json_schema` is accepted by the engine's grammar converter. LIM-C1 is fixed
then.
