# Handoff 015-0 — The M8 measurements: levels, the swap, and the probe (release 0.13, phase 2)

**RFC.** [RFC 015](../../accepted/015-adaptive-effort.md), decision 8, read with its **Amendment 1**. **From.** The scope of 0.13 (`ROADMAP.md`, "Release 0.13", phase 2; the scope's §2.6).

**Purpose.** This handoff measures and writes the report. It builds nothing for the product. Handoff 015c (the effort levels) is written from the report, and the numbers it fixes are registered at its review:
- the `quick` and `deep` envelopes (`standard` ships with no keys: today's behaviour exactly, REQ-TASK-013);
- LIM-E2, the probe's steps (provisional 8);
- LIM-E3, the probe's wall time (provisional 120 s).

**Owner's authorization (2026-11-21):** the scope of 0.13. It covers downloading the one larger model below.

**Embargo:** none.

## The machine

Run B's machine: the one of the M4 report §7. It has the GPU, Ollama, and the packaged `llama-server`.
- **Say what it had:** CPU and cores, RAM, GPU and its memory, OS and kernel, the engines' versions and the flags each ran with (`llama-server`: `--jinja`, `-c`, `-ngl`; Ollama: `num_ctx`, `OLLAMA_MAX_LOADED_MODELS`, `OLLAMA_KEEP_ALIVE`).
- **Do not upgrade an engine for this.** If you must, say so and why, and measure the 7B standard cell again on the new version.
- **Use the real binaries:** the release build of `main` at the commit you start from. Name that commit.

## The models (pinned by SHA-256)

| Model | Quantization | On the machine | Role |
|---|---|---|---|
| Qwen2.5-7B-Instruct | Q4_K_M | yes (S2, run B) | the reference: what `standard` runs on today |
| **Qwen2.5-14B-Instruct** | Q4_K_M | **download** (about 9 GB) | the larger model of the same family |
| Qwen2.5-Coder-7B-Instruct, Qwen2.5-Coder-14B-Instruct | Q4_K_M | yes (S2) | the probe only (§3) |

- **For the download, record:** the source (the publisher's own repository), the file name and its SHA-256. A model is data, not a program: no binary or script comes with it, and none is run.
- **If the publisher ships the file in parts:** record each part's hash. If you merge the parts for Ollama, use the installed llama.cpp's own tool and record the merged file's hash too.
- **On Ollama, use the same GGUF files** (a Modelfile `FROM <file>`, the GGUF's own template). If you use a library tag instead, record its digest and say that the bytes differ from the llama-server ones.

## What to measure

Each cell gives the median of its runs and the spread (minimum to maximum). Say where a trajectory repeated itself (the M4 report §7 found runs that were one deterministic path, so fewer samples than runs).

1. **The M4 task at candidate envelopes.**

   **Candidates.** A level is an overlay of `backend`, `steps`, `deadline`, `context_chars` and `recent_steps` (REQ-TASK-013).
   - **Not settable before 015c:** `recent_steps` stays LIM-C3 (6) in every cell. Say whether any cell's compression reached it, that is, whether `over_budget_steps` was above 0 with more than 6 steps.
   - **Settable now:** steps and deadline through `--steps` and `--deadline`, `context_chars` through the backend's key, and the backend through the model.

   | Cell | Backend | `context_chars` | `--steps` | `--deadline` |
   |---|---|---|---|---|
   | S (standard) | 7B-Instruct | 32 000 (LIM-C1) | 40 (LIM-T1) | 2 h (LIM-T5) |
   | Q1 (quick, candidate) | 7B-Instruct | 12 000 | 12 | 10 min |
   | Q2 (quick, tighter) | 7B-Instruct | 8 000 | 8 | 5 min |
   | D (deep, candidate) | 14B-Instruct | 32 000 | 60 | 4 h |

   - **Q1's and Q2's `context_chars` sit around the largest request measured** (under 12 000 characters, the M4 report §7), so compression is exercised. That is what the report must show: whether it costs completion. If both complete as S does, add a Q3 at 6 000 characters to find where it fails. If Q2 already fails, say how it failed.
   - **On the task:** run each cell on the M4 task (`m4_real_engine_repairs_the_defect`, unchanged). You may change the harness on the throwaway branch to take the cell's values from the environment.
   - **One extra cell, on the 14B:** run D's model on the M4 task at S's limits too (D-at-S), so that the model and the envelope are not mixed in one number.
   - **Runs:** Ollama (GPU), 5 runs per cell; llama.cpp, 3 runs per cell for S and D, more if the time allows. A 14B cell on the CPU may be slow. If one run exceeds an hour, say so and stop that cell.
   - **Record per cell:**
     - completed and verified out of the runs, and how the others ended;
     - steps;
     - wall time of the task, and model time per step;
     - prompt, cached and completion tokens;
     - `context_chars_avg`, `compression_ratio`, `over_budget_steps`;
     - the rejections by code.

2. **The swap between the 7B and the 14B.** One destination for all levels holds either way: two `llama-server` processes on loopback are both local (Amendment 1, item 2). The swap's cost is paid once per task, since a level never changes during a task. Measure it as such.
   - **Ollama, one engine, both models:**
     - the time from a task's first request naming the other model to its first decision;
     - for each direction (7B to 14B, 14B to 7B), when the other model is resident and when nothing is resident;
     - against the same model already resident.

     Record what Ollama kept in memory (`/api/ps`) and the GPU memory used. Say whether both fit at once on this GPU, and what `OLLAMA_MAX_LOADED_MODELS` did.
   - **llama.cpp, two processes**, one model each, each with its own port:
     - whether both fit in memory at once, with the layers each could put on the GPU;
     - the first decision's time after an idle period, against a warm one.
   - **Runs:** 3 per case.
   - **Say what `status` showed** for each backend while you swapped (`resident yes | no | unknown`).

3. **The fitness probe on every model measured.**
   - **Runs:** `mimlys model probe <backend>` for the four models above on both engines, 3 runs each, at the provisional LIM-E2 (8) and LIM-E3 (120 s).
   - **Record per run:** the class and its detail, the steps it used, and its wall time.
   - **Expected:** S2's finding: the Instruct models act, and the Coder models do not return the structured call. If a probe says otherwise, it is the most important finding of the report: give the transcript.
   - **The time limit:** say whether 120 s cut any run of a capable model short, most likely the 14B on the CPU. A probe that calls a slow but capable model `incomplete{not_finished}` misleads the user. Give the slowest `acts` run's steps and time on each engine.
   - **Propose LIM-E2 and LIM-E3 with the rule that gives them.** For example: twice the most steps an `acts` run used, and twice the slowest `acts` run on the slowest engine the book supports, bounded by what a user will wait for at a prompt. State the rule, not only the number.

## What the report proposes

- **The `quick` and `deep` envelopes,** each as its five keys (or fewer: a key a level does not set is the profile's), with the cell that supports it.
  - **`quick`:** a level that is cheaper and still completes the M4 task. If no candidate completes it as reliably as S, say so. A `quick` that fails a seven-decision task is the wrong default, and the report may propose `quick` with S's `context_chars` and only fewer steps and a shorter deadline.
  - **`deep`:** the larger model, if D completes at least as reliably as S. If it does not, say what deep should be on this machine.
- **LIM-E2 and LIM-E3,** each with its rule.
- **What the swap means for a user:** one line each for Ollama and for two `llama-server` processes, which the book can use when 015c documents a second backend.
- **What could not be measured, and why.**

## Rules

- **Nothing here goes into product code.**
  - The throwaway branch carries the harness changes and any measuring scripts; its PR, if any, says "not for merge".
  - If a harness change would serve 1.0-prep too (the M4 task with a small pinned model on CI, the owner's condition 1), propose it in the request; do not land it.
- **The report changes no limit.** The limits are mine to register.
- **Transcripts:** in the request's `evidence/`, one file per run, named by cell, engine and run.

## Return

- **The report, as one PR** on `main`:
  - `docs/src/maintainers/m8-report.md`, in the form of the M4 and M7b reports;
  - its line in `docs/src/SUMMARY.md`, after the M7b report;
  - what was measured, the numbers each with its source, the proposals with their rules, and what could not be measured.
- **The request:** `.git-exclude/review-requests/015-0-m8-measurements/README.md`, with:
  - the PR and its CI run;
  - the machine, the models' hashes, and the engines' versions and flags;
  - anything in the measurements that contradicts RFC 015 or its Amendment 1 (for example, that one GPU cannot hold both models, so that a `deep` on Ollama always pays a swap).
