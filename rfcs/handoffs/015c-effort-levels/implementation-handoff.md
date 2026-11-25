# Handoff 015c — Effort levels: a budget per task, on the backend it names (release 0.13, phase 3)

**Status (2026-11-26):** items 1 to 5 and the acceptance landed at `439c4b1`, and 015c-6 at `cda5eb5` (`.git-exclude/reviewed/handoff-015c/README.md`, `.git-exclude/reviewed/handoff-015c-6/README.md`); 015c-7 next.

**RFC.** [RFC 015](../../accepted/015-adaptive-effort.md), decisions 1, 4 and 7, read with its **Amendment 1** (items 1 to 4) and the decisions at the review of handoff 015-0. **From.** The scope of 0.13 (`ROADMAP.md`, "Release 0.13", phase 3) and the [M8 report](../../../docs/src/maintainers/m8-report.md).

**Specification** (amended before this handoff):
- REQ-TASK-013, REQ-CFG-007, REQ-OBS-010 and REQ-PERS-015;
- **external design §19.4 (new, the whole of it)**, and also:
  - §17.1 (`task.created{effort?}`), §17.2 (`by_effort`);
  - §19.1 (the example's `[effort.*]` and `effort_max`), §19.2 (`--effort` on `run` and `task new`);
  - §20 (the three refusals at creation);
- limits LIM-E4 and LIM-E5 (`quick`'s shipped `steps` 22 and `deadline` 10 min), and LIM-C3's range per level.

**Owner's decision (2026-11-21):** the scope of 0.13. **Review of 015-0 (2026-11-25):** `.git-exclude/reviewed/handoff-015-0/README.md`, §2. `quick` ships two keys; `deep` ships none and is refused until it is configured; the probe is not a gate.

**Embargo:** none.

**Cut point (ROADMAP):** if this handoff runs long, 0.13 ships without it, and the levels move to 0.14. Say early if you see that coming.

## Today

These are as found on `main` at `9945fe0`; check them on today's `main`.

- **One backend per runtime.** `RuntimeDeps` takes one `backend` and one `backend_name` (`runtime.rs:56`), and the CLI builds it from `default` (`app.rs:793`; `backend_ready` requires `default`, `app.rs:923`). REQ-SEC-010's check at every start already reads the task's profile against that backend (`control.rs:967`, `loop_.rs:239`).
- **LIM-C3 is a constant** (`runtime/context.rs:14`, used at `:293` to `:307`). No key reads it.
- **A step records no backend** (`step.rs:71`). The task record has the backend's descriptor at creation (`task.rs:300`, `{kind, model?, locality}`), without its name.
- **No `[effort.*]` section is parsed** (`config.rs`), and no `--effort` flag exists.
- **The M4 real-engine test's verdict is a text** (`tests/m4_demo.rs:490`, `contains("i <= n")`). A verified completion that repairs the defect another way fails the test (the M8 report §3).

## Items

1. **The M4 test's verdict is the completion check** (first, because the acceptance below runs it).
   - **The verdict:** `m4_real_engine_repairs_the_defect` passes when the task is `completed` with the completion check `verified`, and on nothing else. The text assertion goes.
   - **The level:** the test takes it from `MIMLYS_M4_EFFORT` (absent: no flag). For `deep`, the test writes a second backend from `MIMLYS_M4_DEEP_BACKEND_URL` and `MIMLYS_M4_DEEP_MODEL`, and names it in `[effort.deep]`.
   - **Not landed:** the throwaway branch's cell knobs (`MIMLYS_M4_CONTEXT_CHARS`, `_STEPS`, `_DEADLINE`). `--effort` replaces them.
2. **The configuration** (§19.4, "The envelope", "`check` refuses", "`effort_max`").
   - **`[effort.quick]`, `[effort.standard]`, `[effort.deep]`**, each with the five keys.
   - **Shipped:** `quick` has `steps = 22` and `deadline = "10m"`; the other two have none. A key in the file replaces the shipped one.
   - **`effort_max`** in `[policy.ceiling]` and in each `[policy.profiles.<name>]`.
   - **`check`** refuses each case §19.4 lists, naming the level, the key and the bound. The locality rule (every level local, or all on one remote URL) is new here: 015a had no levels to apply it to.
3. **Choosing and recording** (§19.4, "Choosing", "Recorded and fixed").
   - **`--effort <level>`** on `run` and `task new`. A name outside the three is a usage error (exit 2).
   - **Without the flag:** `standard`, or the lower `effort_max`.
   - **A fork:** its source's level unless the flag is given, its envelope resolved now.
   - **Refused at creation, and nothing is created:**
     - a level with no key, shipped or configured;
     - a level above `effort_max`;
     - a level whose backend is unreachable.
   - **The record:** `effort{level, chosen_by, envelope}` on the task record and in `task.created`, every key resolved. All of it is optional, and the schema version does not change. A record without it reads as `standard`, `chosen_by: default`, with the envelope of its own budgets.
4. **The runtime runs a task on its own backend and envelope.**
   - **Several backends:** the runtime holds the configured backends by name, each with its adapter and its endpoint's gate. **At every start** it resolves the task's backend by the name in its envelope:
     - a name no longer configured is refused at the start, naming the configured ones, and never moved to another (Amendment 1, item 4);
     - a model changed under the same name goes on, with the notice at the start;
     - REQ-SEC-010's check runs against that backend.

     How the runtime holds them is yours to design. Keep one place that resolves a name. A command that runs no task must not open a connection to every backend.
   - **The envelope:** the task's `steps`, `deadline` and `context_chars` are what it runs with; a flag still wins, as today. `recent_steps` replaces the constant LIM-C3 in the context assembly, read from the task.
   - **Per step:** `backend`, `model` and `transport` on the step record (REQ-PERS-015), as optional fields.
   - **`runtime.started{tier, backend}`** keeps `default`'s descriptor; the tasks record their own.
5. **What the user sees** (§19.4, "Shown").
   - **`run` and `task new`:** the level line first.
   - **`task show`:** the cost, as §19.4 lists it.
   - **`status`:** the three levels, with `not configured` where a level would be refused.
   - **`metrics`:** `by_effort`.
   - **`--json`:** each in its form: `task show` gains `effort` and `cost`, `status` gains `effort`, `metrics` gains `by_effort`. Add the fields to the CLI reference's JSON table.

## Tests (by state, §3c)

- **Precedence**, per key: shipped, file, profile, flag. Each case is one task whose recorded envelope is checked whole.
- **Each `check` refusal**, and that a valid configuration of 0.12 loads unchanged and runs at `standard` exactly as before (the same requests, byte for byte, through the mock).
- **Each refusal at creation** (no key, above `effort_max`, backend unreachable): no task, no event, the message naming what to set.
- **A task on a second backend:**
  - its requests go to that backend's mock and none to `default`'s;
  - a restart resolves it again by name;
  - a backend removed from the file refuses the start, naming the configured ones;
  - a remote level under a profile without `external` halts as REQ-SEC-010 says.
- **`recent_steps`:** a task at `recent_steps` 2 compresses from its third step back. At 6, the requests are those of today.
- **The record and the readers:** a 0.12 task record reads as `standard`. The new fields are absent when they would be empty.
- **What is shown:** `task show`, `status` and `metrics`, human and `--json`, for a task at each level.

## Mutations

Run each and say which test kills it:
- the flag losing to the level;
- the shipped `quick` keys dropped;
- `deep` unconfigured accepted as `standard`;
- `effort_max` ignored without the flag;
- the task's backend taken from `default` at a restart;
- `recent_steps` ignored.

## Acceptance: the shipped levels, measured

On the machine of run B, under the owner's limit for it (8 threads, nice 19, the thermal guard, as in the M8 report):
- **Runs:** the M4 task (item 1's verdict) at `--effort quick`, `standard` and `deep`, with `deep` naming the 14B-Instruct backend:
  - Ollama: 3 runs each;
  - `llama-server`: 3 runs each of `quick` and `standard`. `deep` there is the report's known 0 of 5; run it once to confirm, no more.
- **The report:** add a section "9. The shipped levels" to `docs/src/maintainers/m8-report.md`, in the form of §3, with what `task show` said each task cost.

This is the ROADMAP's "the M4 task under each level, measured".

## Book and CHANGELOG

- **`configuration.md`:** a section "Effort levels", from §19.4's last paragraph, with the example of a `deep` that names a larger model. Say what each level is and is not, in the words of §19.4.
- **`how-a-task-runs.md`:** a level is chosen at creation and never changes.
- **`cli.md`:** the rows of `run`, `task new`, `task show`, `status` and `metrics`, and the JSON table.
- **The CHANGELOG** under Unreleased, each user-visible change additive.

## Return

- **PRs** on `main`, as many as you need, each with its CI.
- **The request:** `.git-exclude/review-requests/015c-effort-levels/README.md`, with:
  - the PRs and their CI runs;
  - the mutations, each with what killed it;
  - the acceptance's numbers;
  - anything in §19.4 that the code showed to be wrong or unclear. Say it, and do not work around it.

## After review: 015c-6

The review of 2026-11-26 (`.git-exclude/reviewed/handoff-015c/README.md`) approved #212 to #216. They landed as one linear series, `main` fast-forwarded to `439c4b1` (CI of #216). Four of your questions are decided against what the code does today; each is a small change. External design §19.4, §5.2, §15.3 and §20 were amended with the landing.

1. **A task's context budget follows its backend** (your question 3).
   - **What is wrong:** the envelope records the backend's `context_chars` at creation, so a user who lowers it for an engine whose window is too small (the remedy `status` gives) reaches only new tasks. The halted task, resumed, overflows again.
   - **Change:**
     - the envelope records `context_chars` only when the level sets it;
     - at each step the budget is the level's value bounded by the backend's as it is then, or the backend's alone when the level sets none (`loop_.rs:569`);
     - `task show` names which: the level's value, or `the backend's (N)`.
   - **A record of this release's builds that holds a resolved `context_chars`** reads as the level's own. Nothing of a release carries one, so no migration is needed.
2. **`task show` names both step counts** (your question 4).
   - **What is wrong:** `cost      1 of 22 steps` and `backends  default (…): 2 steps` in one view, about one task. The numbers contradict each other unless the reader knows that the ending step is not counted.
   - **Change:** the cost line says both, each named: `7 steps decided; 6 of 22 counted against the budget` (§19.4). The JSON keeps `steps_used` as the counter and gains `steps_decided`. The CLI reference says what each is.
3. **An unreachable backend refuses every creation that starts a task** (your question 1).
   - **What is wrong:** today `run` refuses on a down engine with `--effort standard` and not without it, for the same level on the same backend. One rule, whether or not a flag was typed.
   - **Change:**
     - `run`, and `task new` without `--hold`, at every level, with or without `--effort`: refused with the backend's name and the reason, and nothing is created (§20);
     - `task new --hold` contacts no backend.
   - **This changes 0.12's behaviour** (a task created and then halted `backend_unavailable`), so the CHANGELOG entry goes under **Changed**.
4. **A `running` task whose backend is gone is halted, not left `running`** (your question 6).
   - **Change:** halt `backend_unavailable` with `halt_detail` `backend_not_configured`, naming the backend and the configured ones (§5.2, §19.4). Nothing is sent. `resume` after the file is fixed goes on.

**Accepted as you did them** (no change): questions 2, 5 and 7 to 11. The spec now says 7, 9, 10 and 11 in its own words. Question 2 changes no outcome: `standard` always counts, and it runs on `default`, which is what an unconfigured `deep` would name.

**Also, in the book (done with the landing):** the `deep` example in `configuration.md` no longer sets `steps = 60` and `deadline = "4h"`. The M8 report found no measurement behind them, and an example reads as advice.

**Tests:**
- item 1: lowering a backend's `context_chars` between two processes lowers a running task's budget; a level's own value stays, bounded by the backend's;
- item 2: the human and JSON forms for a completed task;
- item 3: `run` without a flag on a closed port creates nothing; `task new --hold` on it creates the task and contacts nothing;
- item 4: the halt, its detail and its event, and no request sent.

**Mutations:** for items 1, 3 and 4, one each, and say which test kills it.

**Process, from the lead's notes:** one `pkill -f` was refused by the permission system, and one `cd` used an absolute path. Kill only by the pids you recorded, and keep `cd` relative.

**Return:** `.git-exclude/review-requests/015c-6-after-review/README.md`.

## After review: 015c-7

The review of 2026-11-26 (`.git-exclude/reviewed/handoff-015c-6/README.md`) approved #217. It landed as one commit, `cda5eb5`: the tree of its head `ea1591e`, which CI ran, with your two `wip` and `fmt` commits squashed at the landing. Three of your questions need one more small change each. External design §5.2, §19.4 and §20 were amended with the landing.

1. **A `running` task whose backend cannot be made is halted too** (your question 2).
   - **What is wrong:** a configured backend that cannot be made leaves the task `running`, with every step failing. It is the shape item 4 removed, and it is real outside the tests: a key by reference that the environment does not hold.
   - **Change:** halt `backend_unavailable`, with `halt_detail` `backend_unusable` and the reason. Nothing is sent.
   - **For both halts,** the printed line gives both remedies: fix the configuration and resume, or fork the task onto another level (`task new --from-task <task> --effort <level>`).
2. **`task show --json` says the context budget in force** (your question 3).
   - **What is wrong:** a script cannot learn the budget a task runs with, only the level's own value.
   - **Change:** `cost.context_chars` and `cost.context_source` (`level` or `backend`), as the configuration is now, the same as the human line.
3. **The unreachable message names what to fix** (your question 4).
   - **What is wrong:** for `standard` on `default` it says to fix "the backend that [effort.standard], the level of this task, names", a section that does not exist.
   - **Change:** name `[backends.<name>]`. Name `[effort.<level>]` only when that level's `backend` key chose the backend.

**Accepted as you did them:**
- **Question 1:** "decided" means finished. §19.4 now says so.
- **Question 5:** `--hold` prints no probe notices; they come at the start.
- **`m7b_cpu_part` failing on this machine:** its cgroup is not delegated here, and CI ran it delegated.

**Tests:** one per item. Item 1: the halt, its detail and its event; no request; the resume after the fix. Item 2: both sources, in JSON. Item 3: the line at `standard` without a section, and at a level whose `backend` key chose the backend.

**Mutations:** one for item 1. Say which test kills it.

**Return:** `.git-exclude/review-requests/015c-7-after-review/README.md`.
