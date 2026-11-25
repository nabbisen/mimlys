# M8 report: effort levels, the swap and the fitness probe

Milestone M8 (RFC 015 with its Amendment 1; handoff 015-0) names three
things that its defaults wait for: the `quick` and `deep` envelopes (decision
1, REQ-TASK-013), what a swap between a smaller and a larger model costs, and
the fitness probe's two bounds, LIM-E2 (steps) and LIM-E3 (wall time)
(REQ-MDL-011). This report records the measurements made on 2026-11-25 on the
machine of run B (the M4 report, §7), and the values they point to. It builds
nothing for the product and registers no limit: the register is the
architect's, and handoff 015c is written from this page.

The harness change (the M4 real-engine test taking a cell's values from the
environment, and printing more) and the measuring scripts live on the
throwaway branch `m8-measure-1`, never on `main`; the raw transcript of every
run is in the review request of handoff 015-0. Nothing in the product was
changed to measure.

**Three conditions of this measurement change how the numbers read, and are
stated first.**

1. **The machine was shared and hot.** Other projects ran test suites and
   benchmarks on the same 32 threads during most of the measurement; the
   load average reached 58 and the CPU 96 C. Every CPU timing below says
   whether it was taken **uncapped** (the packaged defaults: 16 threads;
   taken before load was recorded, and on a machine seen at a load average of
   58 while the 14B ran) or **capped** (below). **Uncapped timings are void
   for comparison and are upper bounds**; their outcomes (completed or not,
   steps, trajectory) are not. The capped and the Ollama series after the
   first day were recorded: their one-minute load averages were 3 to 29 (never
   above the machine's 32 threads) at 66 to 96 C, and the thermal guard
   waited in 27 episodes, 287 checks of 30 s (about 2.4 hours) in all
   (`evidence/load.log` has the load, the temperature and the five busiest
   processes before and after every series and run; each run's header has
   the load around it). The Ollama cells S to D-at-S of §3.1 ran before load
   was recorded; they use the GPU and a fraction of one core.
2. **The owner's limit for the machine, set during the measurement, after the
   first `llama-server` series:** every CPU engine of the measurement since then ran at `nice -n 19`
   (an effective nice of 15, the shell's own being -4), pinned to cores 0 to 7
   (`taskset`), `llama-server` with `-t 8`, and a thermal guard held every run
   while the CPU was above 85 C until it read 80 C or lower (each wait is in
   `load.log`). This **deviates from the packaged defaults the handoff names**
   (16 threads, all cores). The wall times and tokens per second of the capped
   cells are therefore not comparable with run B, nor with a machine that has
   all its cores free. The cap was set after the uncapped series S, Q1, Q2
   (7B) and D, D-at-S (14B) of llama.cpp had run; those were repeated capped
   where the time allowed (§3.2).
3. **The engines on the machine are newer than run B's.** Run B used Ollama
   0.40.15 and `llama-server` build 11429; the machine now has Ollama 0.40.17
   and `llama-server` build 12359 (system packages, updated between the two
   measurements; no engine was installed or changed for this report). The
   standard cell (S) was measured again on the new versions, as the handoff
   asks.

## 1. What was measured

| Item | Measure | Where |
|---|---|---|
| The M4 task at candidate envelopes (§3) | S, Q1, Q2, Q3 (6 000 chars), Q4 (4 000 chars), D, D-at-S on Ollama (GPU), 5 runs each; S, Q1, Q2, D, D-at-S on `llama-server` (CPU), 3 to 6 runs each, uncapped; S, Q1 and a new cell QS (S's 32 000 characters with Q1's 12 steps and 10 minutes) and D, capped | Ollama 0.40.17; `llama-server` build 12359 |
| The swap (§4) | a request naming the other model, nothing resident, the same model, with `OLLAMA_MAX_LOADED_MODELS` 1, 2 and 0; the M4 task through the product after each; two `llama-server` processes, warm and after an idle period | the same |
| The probe (§5) | `mimlys model probe` on the four models, both engines, 3 runs each | the same |

## 2. The machine, the models, the engines

**Machine.** AMD Ryzen 9 9950X (16 cores, 32 threads), 61 348 MiB of RAM, an
NVIDIA GeForce RTX 5060 Ti with 16 311 MiB (driver 620.73.04, CUDA 13.5; the
desktop held 0.8 to 1.1 GiB of it throughout, nothing else used the GPU: the
`nvidia-smi` before every series is in the evidence), an AMD integrated GPU
that no engine used, CachyOS (rolling) with Linux 7.3.6. The user's own
Ollama service was not running; the measurement used an `ollama serve` of its
own on a non-default port with its own home and model store, so the user's
Ollama state was neither read nor written.

**Release binaries.** `mimlys` 0.12.0 built with `cargo build --release` from
`main` at `32f4ae4` (SHA-256 of `mimlys` `62b32439…ad0a`, of `mimlys-worker`
`fd6db20f…cc2`); the harness branch adds test code only.

**Models** (Q4_K_M, SHA-256 of the files as downloaded or stored; the first
three are Qwen's own repositories):

| Model | Files | SHA-256 |
|---|---|---|
| Qwen2.5-7B-Instruct | two parts | `dfce12e3…580db`, `539cf93f…f3d72a`; merged with the installed `llama-gguf-split` for Ollama: `1875fb29…fae4a49` (the same merged hash as run B) |
| Qwen2.5-14B-Instruct (**downloaded**) | three parts, from `https://huggingface.co/Qwen/Qwen2.5-14B-Instruct-GGUF`, `qwen2.5-14b-instruct-q4_k_m-0000{1,2,3}-of-00003.gguf` | `a09ea5e7…53904f68`, `21b9457d…bdba41ae`, `c8d37006…3d9976` (each equal to the repository's LFS object id); merged for Ollama: `f80eeedb…d6cd1a` |
| Qwen2.5-Coder-7B-Instruct | one file | `509287f7…894d3c` |
| Qwen2.5-Coder-14B-Instruct | one file | `c1e65973…f8d940` |

Ollama used the same GGUF bytes (a Modelfile `FROM <file>` with
`PARAMETER num_ctx 16384`, the GGUF's own chat template; the blob's hash is the
file's). `llama-server` loaded the first part of a split file. The 14B parts
now sit beside the other model files in the user's model store (`<models
dir>`), and nowhere else.

**Engines and flags.**

| Engine | Version | Flags and environment |
|---|---|---|
| Ollama (GPU) | 0.40.17 (CUDA; `OLLAMA_VULKAN` on, only the RTX was listed) | `OLLAMA_HOST=127.0.0.1:11436`, `OLLAMA_KEEP_ALIVE=5m`, `OLLAMA_NO_CLOUD=1`, `OLLAMA_MAX_LOADED_MODELS` 0 (auto), 1 or 2 as each section says, `OLLAMA_NUM_PARALLEL` unset; the runner's own flags: `-c 16384 -np 1 --flash-attn auto -b 1024 -ub 1024` (7B), `-b 512 -ub 512` (14B); all layers on the GPU (29 of 29 for the 7B) |
| `llama-server` (CPU) | build 12359, commit b1b770d90d, package `llama-cpp` 1.0.0; **no usable GPU device** (`--list-devices`: none) | `--jinja -c 16384 --host 127.0.0.1 --alias <model>`; uncapped: the defaults (16 threads, 4 slots); capped: `-t 8` under `nice -n 19 taskset -c 0-7` |

The transport resolved to `tool_calls` on both engines for the Instruct
models. Temperature 0, `timeout_ms` 600 000, `context_chars` as the cell says.

## 3. The M4 task at the candidate envelopes

`m4_real_engine_repairs_the_defect`, unchanged but for reading the cell's
`context_chars`, `--steps` and `--deadline` from the environment. A cell is
**completed** when the task ends `completed` with the completion check
`verified`. The test's last assertion (the file contains the text `i <= n`)
is stricter than the completion check: the 7B on Ollama fixes the defect with
`return total + n`, which the completion check verifies and the assertion
rejects, so the test as written exits non-zero for a correct repair; the
tables count the completion check, as the product does. The first request of
every Ollama run found the model already loaded (the harness's `status`
loads it; §4 measures that load).

| Cell | Backend | `context_chars` | `--steps` | `--deadline` |
|---|---|---|---|---|
| S | 7B-Instruct | 32 000 | 40 | 2 h |
| Q1 | 7B-Instruct | 12 000 | 12 | 10 min |
| Q2 | 7B-Instruct | 8 000 | 8 | 5 min |
| Q3, Q4 | 7B-Instruct | 6 000, 4 000 | 8 | 5 min |
| D | 14B-Instruct | 32 000 | 60 | 4 h |
| D-at-S | 14B-Instruct | 32 000 | 40 | 2 h |
| QS | 7B-Instruct | 32 000 | 12 | 10 min |

`recent_steps` stayed LIM-C3 (6) in every cell, the only value settable before
handoff 015c.

### 3.1 Ollama (GPU, `num_ctx` 16 384), 5 runs per cell

Medians, the smallest and largest in brackets. "Steps" are `step.started`
events; the budget counter is one lower for D (the completing step is not
counted) and the 7B's rejected first reply is not counted either.

| Cell | Completed | Steps | Wall time of the task | Model time per step | Prompt / cached / completion tokens | `context_chars_avg`, `compression_ratio` | `over_budget_steps` | Rejections |
|---|---|---|---|---|---|---|---|---|
| S | **5 of 5** | 7 | 8.57 s (8.56-9.07) | 1 190 ms (1 185-1 250) | 17 331 / 16 408 (14 466-16 408) / 635 | 8 543, 1.0 | 0 | 1 `malformed` per run |
| Q1 | **5 of 5** | 7 | 8.56 s (8.51-9.65) | 1 187 ms (1 174-1 301) | 17 331 / 16 408 / 635 | 8 543, 1.0 | 0 | 1 `malformed` per run |
| Q2 | **5 of 5** | 7 | 8.57 s (8.52-8.73) | 1 183 ms (1 178-1 205) | the same | 8 543, 1.0 | **4** | the same |
| Q3 (6 000) | **5 of 5** | 7 | 8.62 s (8.56-8.62) | 1 188 ms (1 184-1 192) | the same | 8 543, 1.0 | **7** | the same |
| Q4 (4 000) | **5 of 5** | 7 | 8.57 s (8.56-8.62) | 1 188 ms (1 183-1 192) | the same | 8 543, 1.0 | **7** | the same |
| D | **5 of 5** | 5 | 14.97 s (14.92-17.07) | 2 935 ms (2 926-3 363) | 11 799 / 11 009 (9 066-11 009) / 592 (592-645) | 8 143, 1.0 | 0 | none |
| D-at-S | **5 of 5** | 5 | 14.97 s (14.93-15.37) | 2 938 ms (2 930-3 020) | 11 799 / 11 009 / 592 | 8 143, 1.0 | 0 | none |

How to read it: **every Ollama cell is one deterministic trajectory repeated
five times**, so each cell has one independent sample, not five. The 7B read
both files, ran the test, replaced the line and ran the test again, after one
`malformed` first reply; the 14B read both files, replaced the line and ran
the test, with no rejection and one step fewer. The 14B took 2.5 times the
time per step of the 7B and finished the task in 1.7 times the wall time.

**Compression never reached `recent_steps` on this task.** The task has at
most seven requests and LIM-C3 keeps the last six in full, so there was
nothing to compress: `compressed_steps` and `dropped_steps` are 0 in every
Ollama cell. A `context_chars` below the request's own size (8 543 characters
here) only raised `over_budget_steps` (4 at 8 000, 7 at 6 000 and at 4 000):
the request is sent whole (§15.3 of the external design: `over_budget` when
only retained material remains) and the model answered the same. So on this
task, for this model, `context_chars` of 12 000, 8 000, 6 000 or 4 000 costs
no completion and saves nothing.

### 3.2 `llama-server` (CPU, build 12359), 7B-Instruct

Two sets. **Uncapped** (the packaged defaults, 16 threads, load not recorded
and heavy at times: outcomes valid, **timings void**) and **capped** (8
threads at nice 19 under the thermal guard, load recorded in the run's header).

| Cell | Completed | Steps of the completed runs (counter) | The others | Wall time, median (range) | Model time per step | Compression | Distinct trajectories |
|---|---|---|---|---|---|---|---|
| S, uncapped, 6 runs | 5 of 6 | 4, 4, 4, 4, 8 | 1: 21 steps, then `halted convergence` (LIM-T4: identical decisions) | 63.8 s (50.5-346.1), void | 11.6 s (10.0-16.4), void | none (ratio 1.0, `over_budget` 0) | 3 (runs 2-5 are one) |
| S, capped, 3 runs | **3 of 3** | 4, 4, 11 | none | 55.9 s (55.9-247.6) | 11.1 s (11.1-20.6) | none | 2 (runs 2, 3 are one) |
| Q1, uncapped, 5 runs | 3 of 5 | 4, 5, 10 | 2: 12 steps, `halted budget_steps` | 148.1 s (55.8-301.0), void | 15.2 s (11.1-25.0), void | **active in 3 runs**: `compressed_steps` 12, 12 and 7, ratio 1.03 to 1.05, `over_budget` 0 | 5 (all different) |
| Q1, capped, 3 runs | **3 of 3** | 4, 4, 8 | none | 73.1 s (60.5-100.0) | 12.0 s (11.1-14.6) | none | 2 |
| Q2, uncapped, 5 runs | **1 of 5** | 4 | 4: 8 steps, `halted budget_steps` | 108.7 s (44.9-121.2), void | 13.5 s (8.9-15.1), void | `over_budget_steps` 6 (3-6), `dropped_steps` 1 in the four 8-step runs | 4 |
| QS (32 000, 12 steps), capped, 3 runs | **0 of 3** | | 3: 12 steps, `halted budget_steps` | 178.9 s (171.5-381.0) | 14.9 s (14.3-31.7) | none | 3 (runs 2, 3 share their token counts) |

Across the 7B's runs on this engine, the steps of a completed run were 4 (ten
runs), 5, 8 (two), 10 and 11; the runs that did not complete either went on
past 12 steps (the longest, 21, ended in `convergence`) or were stopped by
the step budget of the cell. The model is not repeatable here (the CPU
engine's runs are not one trajectory as Ollama's are), so the cells have few
independent samples and the small counts above are not significant: **S and Q1
completed 8 of 9 and 6 of 8 runs respectively, Q2 1 of 5, QS 0 of 3.** What
the table does show is **where a completion sits**: a completed run took at
most 11 steps, and the one run that was let go past 12 steps (S's, which
reached 21) halted in `convergence` without completing. Whether a run cut at
12 would have completed later is not known. The 8-step budget of Q2 cuts
runs of a completing length (an 8-step and an 11-step completion would not
fit in a budget of 8). Whether the 12 000 of Q1 or
the 32 000 of QS is the better `context_chars` is **not separable with these
samples** (6 of 8 against 0 of 3 is the opposite of what a cheaper context
would predict, and is a handful of runs); compression itself was active only
in runs of ten steps or more, and the one such run that completed (10 steps,
7 compressed) shows that it can complete.

**The uncapped Q2 reached the floor.** In its 8-step runs `over_budget_steps`
was 6 and `dropped_steps` 1 with more than six steps: the summaries could
not shrink the request under 8 000 characters because the last six steps are
kept in full (LIM-C3), and the request went out over budget. That is the
case in which `recent_steps` would matter; it is not settable until 015c.

### 3.3 `llama-server` (CPU), 14B-Instruct (the cell D)

| Cell | Completed | How the others ended | Wall time | Model time per step |
|---|---|---|---|---|
| D, uncapped, 3 runs | **0 of 3** | 31 steps, `halted convergence`, one `malformed` reply at the start | 16.0 min (16.0-23.5), void | 31.0 s (31.0-45.5), void |
| D-at-S, uncapped, 1 run | **0 of 1** | the same trajectory (31 steps, `convergence`) | 14.9 min, void | 28.8 s, void |
| D, capped, 1 run | **0 of 1** | 31 steps, `halted convergence`, the same trajectory | 20.7 min | 40.0 s |

**The same 14B GGUF that completes the task in five steps on Ollama does not
complete it on `llama-server`**: all five runs (four uncapped, one capped) are one trajectory that reads,
replaces, runs and replaces again until three identical decisions end it. The
chat template and the tool-call handling differ between the two engines (the
GGUF's own template through Ollama's runner; `--jinja` in `llama-server`), and
the model's behaviour with them; this report does not look inside. It makes
the finding of §6 below, that the effort level's model is not the only
variable of a level.

## 4. The swap between the 7B and the 14B

### 4.1 Ollama, one engine, both models

**Both models do not fit at once.** At `num_ctx` 16 384 the 7B holds 5.25
GiB and the 14B 11.09 GiB (`/api/ps`, all of it in GPU memory, 6 384 and 12 368
MiB on `nvidia-smi` with the desktop's 0.9 GiB), against a card of 15.5 GiB
that the engine offered (14.4 GiB available). **`OLLAMA_MAX_LOADED_MODELS` of 2,
of 1 and of 0 (auto, the default) all gave the same result:** loading the
other model unloaded the first (`/api/ps` held one model after every
request in all three configurations, in 90 requests).

A one-token request naming model B, Ollama's own `total_duration`, 3 runs per
case, median (range), in milliseconds; "cold" means the model files'
page-cache was dropped first (`posix_fadvise`; the page cache could not be
fully dropped without `sudo`, so cold is a lower bound):

| Case | `MAX_LOADED` 1 | `MAX_LOADED` 2 | `MAX_LOADED` 0 (default) |
|---|---|---|---|
| the same model already resident (7B; 14B) | 18 (18-23); 28 (27-39) | 18 (15-24); 31 (27-32) | 33 (18-98); 33 (30-36) |
| nothing resident, then 7B | 1 814 (1 805-2 561) | 1 519 (1 513-1 553) | 1 764 (1 480-1 829) |
| nothing resident, then 14B | 2 220 (2 182-2 249) | 2 033 (2 011-2 055) | 2 397 (2 102-2 534) |
| nothing resident, cold, 7B; 14B | 2 634 (2 518-3 147); 3 558 (3 496-3 831) | 2 631 (2 615-2 755); 3 531 (3 509-3 575) | 3 489 (2 515-4 143); 2 962 (2 885-4 026) |
| **7B resident, then 14B** | **3 349 (3 231-3 392)** | 3 155 (3 024-3 340) | 2 537 (2 483-3 311) |
| **14B resident, then 7B** | **2 473 (2 444-2 831)** | 2 813 (2 559-2 852) | 2 670 (2 572-3 260) |
| 7B resident, then 14B, cold | 5 290 (4 970-5 822) | 4 716 (4 596-4 881) | 3 931 (3 744-4 477) |
| 14B resident, then 7B, cold | 3 447 (3 289-3 527) | 3 432 (3 198-3 566) | 2 967 (2 947-2 972) |

So **a swap costs 2.4 to 3.4 seconds when the files are cached and 3.3 to 5.8
when they are not, against 0.02 to 0.03 s when the model is resident**: a
level that names the other model pays it once, at the task's first request.
Through the product: `mimlys status` reads `resident yes` for the backend
whose model `/api/ps` holds and `no` for the others (backends on one URL with
one model, `default` and `o7` here, agree), the harness's first request (its
status probe, which loads the model: 2.1 to 5.1 s against 2.2 to 2.4 s warm,
noisy because the machine was shared) is where the load appears, and the
first decision of the task itself then takes the same time as with a model
resident: 1.4 s for the 7B and 4.5 s for the 14B (`evidence/swap-max1-*`). The
M4 task completed in all 24 of these runs (the same trajectories as §3.1).

### 4.2 Two `llama-server` processes (CPU), one model each

Ports 8091 (7B) and 8092 (14B), each with its own context of 16 384 per slot.
**Both fit in memory at once:** the resident set was 8.7 GB for the 7B and 18.1
GB for the 14B at their first requests (the 14B's includes its mapped file and
four slots of cache) of the machine's 59.9 GiB, with about 27 GB free
afterwards; under memory pressure from other projects the resident sets fell
to 2.8 and 5.2 GB and the first request paid for reading the file again.
**The layers each could put on the GPU: none**, the packaged `llama-server`
is a CPU build and lists no device; a GPU build of `llama-server` was not
measured (§8).

First decision of a one-step task (the M4 goal; the time from the step's start
to the reply), capped, three runs:

| Case | 7B | 14B |
|---|---|---|
| the first request after the process started (once) | 21.9 s | 30.9 s (63.8 s with the file not in the page cache) |
| warm, run after run | 5.7, 4.6, 7.9 s | 29.3, 29.6, 30.1 s |
| after 300 s of idle | 3.3, 6.7, 3.8 s | 41.1, 30.1, 28.3 s |

An idle period of five minutes cost nothing measurable (the process keeps its
model and its prompt cache); **a swap between two `llama-server` processes is
zero**: both stay loaded and the level's backend is a different port. `status`
read `resident yes` for each reachable process (a `llama-server` holds one
model) and showed the other, stopped, one as `unreachable`. Note that
`resident yes` does not say that the model's pages are still in RAM: after
the other projects' memory use the 7B's resident set was a third of its
size, and the first request waited for the disk.

## 5. The fitness probe

`mimlys model probe <backend>`, three runs each, at the register's provisional
LIM-E2 (8 steps) and LIM-E3 (120 s), the model through the engine as in §2.
Classes: `acts`, `no_structured_call`; no run was `incomplete` or
`plans_only`. Wall time is the command's, including the first run's load of
the model.

| Model | Engine | Class | Steps | Model time per step | Wall time |
|---|---|---|---|---|---|
| 7B-Instruct | Ollama | `acts` x3 | 2, 2, 2 | 0.55-0.68 s | 1.2-1.8 s |
| 7B-Instruct | `llama-server` (capped) | `acts` x3 | 2, 2, 2 | 4.3-8.1 s | 8.8-16.5 s |
| 14B-Instruct | Ollama | `acts` x3 | 3, 3, 3 | 0.79-0.96 s | 2.5-5.5 s |
| 14B-Instruct | `llama-server` (capped) | `acts` x3 | **4**, 2, 2 | 10.4-11.5 s | 21.1-**43.9 s** |
| Coder-7B | Ollama | `no_structured_call` x3 | 3, 3, 3 | 0.95-1.05 s | 3.0-6.7 s |
| Coder-7B | `llama-server` (capped) | `no_structured_call` x3 | 3, 3, 3 | 8.3-10.7 s | 25.2-33.7 s |
| Coder-14B | Ollama | `no_structured_call` x3 | 3, 3, 3 | 1.07-1.27 s | 3.4-8.8 s |
| Coder-14B | `llama-server` (capped) | `no_structured_call` x3 | 3, 3, 3 | 9.8-15.1 s | 29.8-47.2 s |

**The expected finding holds on both engines**: the Instruct models act, and
the Coder models return no structured call (the engine's warnings read "a
reply carries no valid decision", problem `no_call`: the reply has text and no
call; three such replies end the probe, hence 3 steps). Nothing in the probe
contradicts spike S2. **No run was cut short by the 120 s**: the slowest
`acts` run was 43.9 s (14B-Instruct on `llama-server`, 4 steps, the first run
after the process started, capped to 8 threads at nice 19 on a shared machine)
and the slowest probe of any class 47.2 s. The most steps an `acts` run used:
4 (3 on Ollama).

## 6. Proposals

Not registered by this report; the architect registers them.

### 6.1 The `quick` and `deep` envelopes

| Level | `backend` | `steps` | `deadline` | `context_chars` | `recent_steps` | Supporting cell |
|---|---|---|---|---|---|---|
| `standard` | the profile's | none | none | none | none | S: today's behaviour (REQ-TASK-013) |
| **`quick`** | the 7B-Instruct class | **12** | **10 min** | **12 000** | none (the profile's 6) | Q1 |
| **`deep`** | the 14B-Instruct | **60** | none (the profile's 2 h) | none (the profile's) | none | D, on Ollama only |

**`quick`.** Q1 completed 5 of 5 on Ollama and 6 of 8 on `llama-server`
(S: 5 of 5 and 8 of 9), so it is **not shown to be as reliable as S, nor
less**: its failures are runs that wandered past the budget, as S's own failure
did (21 steps, `convergence`), and every completion of any 7B run took at most
11 steps. The two values that the measurement does support are the steps and
the deadline: **`steps` = 12** (the least that holds the longest completion,
11, and cuts runs at the point where none completed: the budget of 8 in Q2
would have stopped completed runs, 1 of 5 completed), and **`deadline` = 10
minutes** (rule: twice the slowest completed run of the level, 247.6 s capped,
which is 8.3 minutes, rounded up to the value of the cell). The
`context_chars` of 12 000 cost nothing where it applied (Ollama; the
llama.cpp runs that completed with compression on, 7 of 10 steps) and saves
a request's size only in a run that goes past six steps, where it is what
makes `quick` cheaper (the wandering S run sent 83 805 prompt tokens; Q1's
12-step runs sent 35 000); **if the architect prefers a level that changes
only what is measured, `quick` is `steps` 12 and `deadline` 10 min with S's
`context_chars`**, and the 12 000 is then the open question of §8. Q2 (8
steps, 8 000 characters, 5 minutes) is **rejected**: 1 of 5 on `llama-server`.
`recent_steps` cannot be proposed: no cell could set it, and the
compression's floor was reached only in Q2.

**`deep`.** D completed 5 of 5 on Ollama, at least as reliably as S, in one
step fewer, at 2.5 times the model time per step, and the swap to it costs 2.4
to 3.4 s. **On `llama-server` on the CPU it completed 0 of 5**, so `deep` is
the 14B **for the engine that runs it as Ollama does**, and not a promise
about every engine: `deep` on a `llama-server` of this build is a level that
makes the task worse (§7). The budgets are not supported by the task: D used
5 steps and 15 s, so the 60 steps and 4 hours of the candidate have no
measurement behind them. What the measurement gives is a **rule**: 60 steps
at the 14B's slowest time per step seen on the CPU (45 s, uncapped) is 45
minutes, twice that is 90 minutes, which is under the profile's 2 hours, so
**`deep` sets no deadline** and `steps` 60 as the candidate had it (1.5 times
S's 40, the 14B's 31-step wander ended in `convergence` long before). If
the architect wants the 4 hours, the measurement does not forbid it.

### 6.2 LIM-E2 and LIM-E3

| Limit | Proposed | Rule |
|---|---|---|
| LIM-E2 | **8** | twice the most steps an `acts` run used (4, 14B-Instruct on `llama-server`; 3 on Ollama) |
| LIM-E3 | **120 s** | about 2.7 times the slowest `acts` run on the slowest engine measured (43.9 s, a 14B on 8 threads of a shared CPU), rounded to the whole minutes a person will wait at a prompt; a probe that calls a slow but capable model `incomplete{not_finished}` misleads, and 120 s cut none here |

Both are the provisional values; the measurement does not move them. A machine
with fewer cores than the 8 used here would run the 14B's probe slower; at
that slowness the 120 s is the bound a user would still sit through.

### 6.3 What a swap means for a user, in one line per setup

- **Ollama on one 16 GB GPU, a 7B and a 14B:** the models never both stay
  loaded (whatever `OLLAMA_MAX_LOADED_MODELS` says), so a task on the other
  level's model pays one swap of 2.4 to 5.8 seconds at its first request and
  nothing after it.
- **Two `llama-server` processes on one machine (CPU build), one model each:**
  there is no swap, both stay loaded (27 GB of 60 GB here) and a level changes
  the port; a first request after a long idle or after the system has taken
  the pages back is the one slow request (up to a minute for the 14B).

## 7. What this contradicts or sharpens in RFC 015 and its Amendment 1

- **One GPU cannot hold both models** (the question the handoff names): a `deep`
  on Ollama on a 16 GB card always pays a swap, and `OLLAMA_MAX_LOADED_MODELS`
  does not change that. The RFC's expectation that a swap costs "seconds to
  tens of seconds" is met at the seconds end: 2.4 to 5.8 s.
- **Decision 1 / Amendment 1, item 1 ("a level that names a larger model is
  better for the hard task"):** `deep`'s model is not the only variable. The
  same 14B GGUF completes the M4 task on Ollama and not on `llama-server` (§3.3),
  and the same 7B completes it every time on Ollama and 5 of 6 on
  `llama-server`. A level names a backend, and a backend is an engine and a
  model: the book should say that a level is as good as its backend on this
  task, and the fitness probe (which passes the 14B on `llama-server`) does not
  detect this: **the probe says `acts` for a model and an engine on which the
  M4 task does not complete**. It measures that the model calls tools, not
  that it finishes a task.
- **`context_chars` per level (Amendment 1, item 1):** on this task it has no
  effect below the request's own size, because the last six steps are kept
  in full (LIM-C3), so `context_chars` alone is not a way to make a short
  task cheaper; it takes effect in long tasks (§3.2, Q1's compressed runs). A
  level that wants a cheaper short task needs `recent_steps` (015c), which this
  measurement could not set.
- **Motivation, "7 to 10 s per step for a 7B on the CPU":** 10 to 16 s
  (uncapped), 11 to 21 s (capped); "5 to 16 steps": 4 to 11 steps when it
  completed, 21 when it did not. On the GPU, "about one second per step":
  1.2 s for the 7B, 2.9 s for the 14B.
- **Amendment 1, item 5 (`status`: resident):** confirmed (§4). `resident yes`
  for a `llama-server` means the process holds the model, not that its pages
  are in memory.
- **Run B reproduced on newer engines:** Ollama 0.40.17 gave the 7B one
  trajectory of 6 steps with one `malformed` reply (run B: 16, 6, 6, 6, 6);
  `llama-server` 12359 completed 5 of 6 (run B: 4 of 5).

## 8. What could not be measured, and why

- **A GPU build of `llama-server`.** The packaged one lists no device; Ollama's
  runner is a different program. So "the layers each could put on the GPU" is
  none, and two `llama-server` processes sharing a GPU were not measured.
- **The 14B on the CPU at the packaged defaults on an idle machine.** The
  machine was shared and hot, and the owner's cap (8 threads, nice, thermal
  guard) applies; the 14B figures on the CPU are for that cap. The uncapped
  14B runs stay in the evidence, labelled, and are void for timing.
- **Whether the 14B on the CPU at 8 threads can finish a task within the
  hour:** it takes 40 s per step (a request of up to 15 000 characters), so
  the one capped run, which ended itself in `convergence` after 31 steps, took
  20.7 minutes; a task that completes in the five steps the 14B needs on
  Ollama would take about four minutes, and a 60-step task about 40 minutes
  if the requests did not grow, which stays inside the hour. No capped 14B run
  completed the task, so the figure for a completing run is an extrapolation;
  no cell was stopped by the one-hour rule. D-at-S was not repeated capped
  (it had the same trajectory as D uncapped).
- **`recent_steps`, and so the `eager` compression of the RFC:** not settable
  before 015c.
- **Q3 on `llama-server`:** the handoff asks for it when both Q1 and Q2
  complete; Q2 did not (1 of 5).
- **Which of Q1's two differences (12 000 characters, 12 steps) matters** on
  `llama-server`: the samples are too few (§3.2).
- **Two models on one GPU at a smaller context** (a `num_ctx` that would let
  the 7B and the 14B fit together): not tried; the measurement used the 16 384
  that the M4 report requires.
- **Cold-disk loads exactly:** the page cache could not be dropped without
  `sudo`; `posix_fadvise` evicts what it can, and the machine restarted twice
  during the measurement, so some first loads were cold in fact.
- **More than one trajectory on Ollama:** at temperature 0 every cell repeated;
  there is no sample of the engine's variation.
