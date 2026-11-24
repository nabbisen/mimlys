# RFC 015 — Adaptive effort, part one: effort levels, a model per level, the fitness probe, the restatement gate

**Status.** Accepted (2026-11-10, by the owner); Amendment 1 (2026-11-21, the scope of 0.13). Milestone M8, after 0.10. Normative text written 2026-11-10 (requirements, limits, threat register). Drafted by the owner with the startup architect; reviewed by the architect, whose changes are marked *[review]* and argued in `.git-exclude/reviewed/rfc-013-015-review/README.md`. For the owner's sign-off after RFC 013. The owner's decision of 2026-11-10 (D-E1 to D-E4 of the expansion proposal): adopt the theme; this RFC is its first part; the automatic choice and movement of the level (Phase C) and parallel attempts (Phase D) are a later RFC that cites this one.
**Tracks.** The expansion proposal `.git-exclude/reviewed/owner-expansion-2026-11/README.md`; the M4 report §7 (run B); spike S2 (the Coder models never returned a structured call); the owner's discussion of the request-to-response flow (2026-11-10).
**Touches.** `mimlys-core` (the effort level on the task; budgets per level; the backend recorded per step; the restatement gate), `mimlys-model` (several backends; the fitness probe), `mimlys-exec` (the sandbox's optimisation level per effort), `mimlys` (configuration `[effort.<level>]` and `[backends.<name>]`, `run --effort`, `model probe`, `task show`, the book); requirements §5, §6, §7, §13, §14, §19, §21; external design §4, §5, §6, §9, §12, §15, §17, §19, §22; the limits register (new LIM-E2, LIM-E3); the threat register (new THR-40).
**Release.** 0.13, before 1.0 (the owner's decision in the scope of 0.12, 2026-11-18; 1.0's condition 6). RFC 013's admission is a prerequisite for `deep` only insofar as `deep` consumes more; nothing here depends on RFC 014.

## Summary

A task should cost what it is worth. This RFC gives the user one visible knob, **effort**, with three named levels (`quick`, `standard`, `deep`), each a configured envelope of budgets and context handling, and each naming the backend (the model) it runs on. It adds a **fitness probe** that tells, per configured model, whether the model actually acts under its transport on this machine, a **backends table** in the configuration so that more than one model can be named, the **backend recorded per step** so that the audit basis holds when a task changes models, and a **restatement gate**: with `--plan-first`, the model's first decision must be a plan, shown to the user before any action *(amended: the model must plan before it acts or completes, and may ask first; Amendment 1, item 8)*. Effort is a budget, never a permission: no level grants a tool, a path, a destination or an effect that the profile and the grants do not; the twelve stages, the approvals and the confinement are identical at every level.

## Motivation

- Every task runs under the same budgets, the same model and the same depth of checking, whether it renames a function or repairs a test suite across three crates (requirements §7; `[backend]` is one section). The user's only lever is per-task budgets within the ceiling, with no names and no guidance.
- Run B (M4 report §7) showed one model finishing the M4 task in 5 to 16 steps at about one second per step on the GPU and 7 to 10 s on the CPU, with the prefix 0.95 stable. S2 showed two other models never returning a structured call. Nothing in the product tells a user which of their configured models is in the first class, before they wait on a task that plans forever; the transport probe tells whether the engine accepts tools, not whether the model acts.
- The owner's flow discussion found two honest gaps: there is no interpretation phase (the goal is passed verbatim and is never rewritten, by design), so a misread goal is discovered by its actions; and the completion check is the only verification the runtime performs on meaning.
- Realistic constraints decide the shape: one consumer GPU holds one model at a time, a swap costs seconds to tens of seconds (to be measured), CPU inference is 7 to 10 s per step for a 7B model, and repeated runs repeat (run B's identical trajectories), so any diversity between attempts must be explicit.

## Decision

1. **Effort levels (REQ-TASK-013, new; REQ-CFG-007, new).** `[effort.quick]`, `[effort.standard]`, `[effort.deep]`, each with: `backend` (a name from decision 3), `steps`, `deadline`, `executions`, `context_chars` (each within the ceiling and the register's ranges), `compression` (`normal | eager`, the LIM-C3/LIM-C4 pair per level). Shipped defaults: `quick` (smaller budgets, `eager`), `standard` (today's values), `deep` (larger budgets, `normal`). Every level is verified the same way: by the completion check. *[review]* The draft's `sandbox_opt` is removed: no measurement shows the sandbox's optimisation level matters for a task's cost, and a second compiled-interpreter cache would need one. It can return with a measurement. A level is chosen at creation: `run --effort <level>` and `task new --effort <level>`, default `standard`; the profile may cap the levels it allows (`[policy.profiles.<name>] effort_max`). The level, its envelope and who chose it (`user` in this RFC) are recorded on the task (`task.created` gains `effort`), shown in `task show` and in `status`, and never change during a task in this RFC.
2. *[review, decided by the owner on 2026-11-10]* **Self-review: deferred to part two.** It would cost one model request at every completion with nothing acting on its result, and its value is unmeasured; in part two it can feed the level's escalation. The completion check stays the only judge. Its requirement and limit are assigned in part two.
3. **Backends as a table (REQ-CFG-007; REQ-MDL-010, new).** `[backends.<name>]` replaces the single `[backend]`; an existing `[backend]` section is read as `backends.default` with a deprecation line in `check` (REQ-MAIN-001: one release of both forms). Each entry keeps today's keys, and each passes the locality rule of REQ-SEC-010 on its own. *[review]* Until 1.0 a level's backend must be local (the owner's D-E2); `check` refuses a level that names a remote backend. The configuration's version (`config_version`, REQ-MAIN-002) becomes 2; a version-1 file is read with the `[backend]` mapping for one release. `status` lists every backend with its probe result (reachable, model listed, transport) and, where the engine reports it, the model currently resident; a level whose backend is unreachable makes `run --effort <level>` refuse with the reason.
4. **The backend recorded per step (REQ-PERS-015, new; REQ-PERS-012 amended).** Each step record carries the backend name, the model and the transport it was decided with; the task keeps the set of backends it used. In this RFC a task uses one backend, so the set has one member; the field exists so that the later RFC's level changes leave a complete audit basis. *[review]* This is a persistence-schema change: `SCHEMA_VERSION` 3, with a migration from 2 tested against a store written by 0.9.0, as handoff 009b's fixture does for 0.8.0. *[note 2026-11-12]* Handoff 002c (the rejection's detail on the step) takes version 3 first; this decision takes the next one, with a migration from the version before it.
5. **The fitness probe (REQ-MDL-011, new; LIM-E2, LIM-E3, new).** *[review]* `mimlys model probe [<backend>]`, a command of its own, because it writes (a scope, a task, a result) and RFC 013 makes `status` a read command; `status` shows the last result per backend with its date. The probe runs, per backend, one scripted mini-task against the real model: a throwaway scope created by the runtime under `<data_dir>/tmp/probe-<id>/` with one small file; the built-in read and write tools only, under a probe profile that grants nothing else (no host tools, no servers, no network); a goal that asks for one small edit; a completion check on the file's content; at most **LIM-E2** steps (candidate 8) and **LIM-E3** wall time (candidate 120 s). The result per backend is one of `acts` (completed and verified), `plans_only` (no `act` within the budget), `no_structured_call` (only `malformed`), `unreachable`, with the step count, the model time per step and the date, recorded under the data directory and shown by `status` as *measured on this machine on <date>*. The probe's task is deleted with its scope when it ends; it writes no event into the user's task history and it never touches a user's project. The book's model page cites the probe as the way to answer "is this model appropriate".
6. **The restatement gate (REQ-TASK-015, new).** *[review]* Named `plan_first`, not `hold`: `task new --hold` already means "create without starting" (the CLI reference), and one word must not mean two things. A task created with `--plan-first` requires the model's first decision to be a `plan`: any other first decision is rejected with `state{reason: plan_required}` and the fixed hint "state your plan first" *(amended: only an `act` or a `complete` is rejected; `ask_user` and `give_up` are allowed before the plan; Amendment 1, item 8)*. The plan is shown to the user (`task show`, and `run` prints it); the user resumes as is, resumes with an added note (which becomes a labelled `user` segment, never a rewrite of the goal), or cancels. Nothing acts before the resume. Without `--plan-first`, behaviour is unchanged.
7. **Cost visibility (REQ-OBS-010, new).** `task show` answers "what did this cost": the level, steps used of the envelope, model time, prompt and completion tokens where reported, and the backend. `metrics` aggregates per level.
8. **Measurement before fixing numbers.** The shipped envelopes, LIM-E2 and LIM-E3, and the `quick`/`deep` defaults are set from a report on the reference machine: the M4 task under each level, time and tokens per level, the swap time between a 7B and a 14B model on Ollama and on two llama-server processes, and the fitness probe's results for the models measured so far. The report is a book page like the M4 report.

## Alternatives considered

- **Choose the level automatically from the start.** It is the goal of the theme, but its rules need the measurements this RFC produces and the stall signals it instruments; shipping `auto` first would be a surprise generator. Deferred to the next RFC, with the rule-based choice and the escalation on LIM-T2/T4/T6 signals as its content.
- **Let the model rank or judge its own work.** Model text is untrusted (REQ-DEC-003's spirit); the completion check remains the only judge. A recorded, never-acted-on self-review is deferred to part two (decision 2).
- **Switch models per step.** A swap costs seconds and a model per step would destroy the prefix stability that makes local inference affordable (run B: 0.95). Levels change the model only between tasks here and only at phase boundaries in the next RFC.
- **A fitness probe without a real task (a single request).** It is what the transport probe already does, and S2 showed it does not predict acting. The mini-task is the smallest thing that does.
- **Let the model rewrite the goal at the restatement gate.** The goal segment is the user's words by invariant; the model's plan is shown beside it and the user may add a note. Rejected.

## Consequences

- A user can say how much a task is worth and see what it cost; a user can learn before a task whether a model acts on this machine; a plan-first task shows its plan before touching anything.
- New configuration (`[effort.*]`, `[backends.*]`, `effort_max`), new flags, new `status` and `task show` fields, a new rejection reason (`plan_required`), two LIM entries, seven new REQ entries and one amendment (REQ-PERS-012). Additions under REQ-MAIN-001, with the `[backend]` form kept for one release.
- The fitness probe runs a real model and real tools: THR-40 (new): the probe's scope or tools reach a user's file; controls: a runtime-created scope under the data directory, a probe profile with no grants beyond the two built-in tools, deletion at the end, the probe refused when the store is in use by a task (RFC 013's read-only open applies to its reads only).
- The next RFC (Phase C and D) gets its inputs: envelopes, the per-step backend, the probe's classes, the stall signals already in ED §6.5, and the measured swap costs.

## Open questions

1. *(Settled: the self-review is deferred to part two.)*
2. The names of the levels across languages of the book, once DR-008 arrives: `quick`, `standard`, `deep` are fixed identifiers; their descriptions translate.
3. Whether the fitness probe may run while another task is running on the same store (proposal: no, refused with the reason, until RFC 014 gives one scheduler).
4. Whether `effort_max` belongs to the profile (per user role) or to the ceiling (per installation); proposed: both, the ceiling bounding the profiles as every other budget.

## References

The expansion proposal `.git-exclude/reviewed/owner-expansion-2026-11/README.md`; the M4 report §7; spike S2; RFC 006 (fork, for the later RFC), RFC 007 and RFC 008 (context and backend), RFC 012 (transport), RFC 013; requirements REQ-TASK-001 … 012, REQ-DEC-003, REQ-CAP-001 … 010, REQ-CTX-001 … 010, REQ-MDL-001 … 009, REQ-PERS-012, REQ-CFG-001 … 006, REQ-CLI-001 … 006; external design §5, §6, §9, §12, §15, §17, §19, §22; limits LIM-T1 … T6, LIM-C1 … C5, LIM-M1 … M4, LIM-S*; invariants INV-01, INV-13.

## Amendment 1 (2026-11-21, the scope of 0.13)

The decisions were written against 0.9's code. Read against 0.12.0's code, and reviewed with the owner's philosophy (`.git-exclude/reviewed/scope-0.13/README.md`, §2), they read as follows. The owner authorized them on 2026-11-21.

1. **A level's envelope (decision 1).** It is `backend`, `steps` (LIM-T1), `deadline` (LIM-T5), `context_chars` (LIM-C1) and `recent_steps` (LIM-C3).
   - `executions` is removed: the code has no task-wide count of executions, and LIM-T3 is an action's retries, a profile's guard and not effort.
   - `compression = normal | eager` becomes `recent_steps`, a number the user can read. LIM-C4 is fixed, so it had nothing else to set.
   - A level's `context_chars` is at most its backend's, which is sized to the engine's window. `check` refuses a larger one, naming both.
   - **A level is an overlay:** a flag (`--steps`, `--deadline`) wins over the level, the level over the profile, and the profile over the register's default; the ceiling bounds all of them.
   - The guard counters (LIM-T2, T3, T4, T6) stay the profile's.
   - `standard` ships with no keys, so it is today's behaviour exactly.
   - **A fork takes its source's level** unless `--effort` is given.
2. **A level never changes where a task's data goes (decision 3).** D-E2 keeps *escalation* local-only until 1.0; part one has no escalation, and a rule for every level would refuse a valid 0.12 configuration. Allowing every backend REQ-SEC-010 permits would let one knob choose both how much and where, and effort is a budget, never a permission. Therefore:
   - every level's backend is local, or every level's backend has the same remote endpoint (one URL); `check` refuses anything else, naming the levels and their endpoints;
   - part two's escalation moves between levels, so D-E2 holds by construction.
3. **Locality is checked, not only declared (REQ-SEC-010).**
   - A backend whose URL's host is not loopback (`localhost`, `127.0.0.0/8`, `::1`) must be declared `remote`, or `check` refuses it, naming the host.
   - A loopback URL may be declared `remote` (a tunnel to another machine).
   - REQ-SEC-010 is checked at every start of a task, not only at its creation: the task's backend is resolved by its name, and a remote backend under a profile without `external` halts the task before anything is sent.
4. **Nothing is chosen silently (decisions 1 and 3).**
   - A level that names no backend runs on the backend named `default`; a version-1 file's `[backend]` is that backend.
   - In a version-2 file without a `default`, `check` refuses a level that names none.
   - A task whose backend was removed from the configuration is refused at its start, naming it and the configured backends. Nothing falls back to another backend.
   - A task whose backend's model changed under the same name goes on, with a notice at its start, and each step records its model.
5. **The backends table (decision 3).**
   - Entries on one URL must agree on `max_in_flight` (LIM-O12 is counted per URL).
   - `status` probes the backends at once, within one LIM-M4. Per backend it shows: reachable, model listed, resident (Ollama's `/api/ps`; always for a llama-server, which holds one model; `unknown` otherwise), transport, context, and the last probe result with its date.
   - `config_version` 1 is read, with a deprecation line in `check`, through 0.14, and it is removed by the release that prepares 1.0, before the freeze.
   - A version-2 file with `[backend]`, or a version-1 file with `[backends]`, is refused, naming the version that form needs.
6. **No store migration (decision 4).**
   - Steps and tasks are JSON records whose types accept unknown fields, so the backend, model and transport per step, the task's set of backends, and `effort` are optional record fields.
   - `SCHEMA_VERSION` stays 4, and 0.12.0 can still open a 0.13 store. The *[review]* and *[note]* sentences of decision 4 are superseded.
   - The probe's results are a file under the data directory, not a table.
7. **The fitness probe (decision 5).**
   - Open question 3 is settled by LIM-O3: one store has one writer, so the probe is refused while another process holds the store, and the refusal names the holder.
   - The probe's requests pass RFC 013's engine gate, and its built-in tools take no admission slot.
   - Without a name, `model probe` probes the local backends only. A remote backend is probed only when named, and only under `remote_backend_allowed`. The command says that it sends a fixed mini-task and nothing of the user's.
   - The probe's profile cannot promote.
   - **A fifth class, `incomplete`:** the model acted but the task was not completed and verified within LIM-E2 and LIM-E3, or it asked the user (nobody answers a probe) or gave up; the detail says which. Without it, a model that acts badly would be filed under a class that says something untrue about it.
   - **The verdict is the runtime's.** The runtime reads the file in the probe's workspace after `complete`; the model's word that it finished is never the verdict.
8. **Plan first (decision 6).** "The user resumes" collided with `task resume` (paused and halted tasks). Therefore:
   - the task waits as `waiting_user{wait_kind: plan}`, and the plan is shown as an approval card is;
   - `approve <task> [--note TEXT]` goes ahead, the note a labelled `user` segment;
   - `reject <task> --note TEXT` asks for a new plan;
   - `reject --halt` and `task cancel` act as today, and `run` prompts.
   - `ask_user` and `give_up` are allowed before the plan, because asking acts on nothing. `act` and `complete` are rejected `state{reason: plan_required}`, which counts toward LIM-T2.
   - Only the first plan is gated.
   - `task show` shows the plan in force for every task.
9. **Open question 4:** `effort_max` is a key of both the profile and the ceiling, and the ceiling bounds the profiles.
10. **The measurement (decision 8).**
    - It runs on the machine of run B: the M4 task at each level; the swap between a smaller and a larger instruct model of one family; the probe's class for Qwen2.5-7B-Instruct and spike S2's Coder models.
    - The models are pinned by hash.
    - The owner's authorization covers downloading the larger model.

**Sharpened at the review of handoff 015a (2026-11-22):**
- **The probe's classes.** An action proposed and rejected is still acting: `plans_only` means that no action was ever proposed. `incomplete` and `plans_only` both carry how the probe ended (`not_finished`, `not_verified`, `asked_user`, `gave_up`).
- **`unreachable` is not recorded.** It is said, with exit code 1, and the last measurement stays. It measures the engine's availability, not the model, and `status` shows that live.
- **A model backend's redirect is never followed** (REQ-SEC-010), so a "local" engine cannot pass a task's content to another host.
- **A halt `policy` names its rule** in `halt_detail`.
