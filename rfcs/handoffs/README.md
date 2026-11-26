# Implementation handoffs

Handoffs are companion execution documents for RFCs (RFC 000, "Companion handoffs"). Their state is the state of their RFC. Each directory holds:

- `implementation-handoff.md` — scope, module layout, types, behaviours with requirement ids, tests, and what to return;
- `task-breakdown-pr-plan.md` — ordered pull requests with dependencies;
- `acceptance-qa-checklist.md` — what the reviewer checks, keyed to requirement ids.

## Milestone M1 — read and implement in this order

| Order | Handoff | RFC | Depends on |
|---|---|---|---|
| 1 | [001 Task state machine and action lifecycle](./001-task-state-machine-and-action-lifecycle/implementation-handoff.md) | [RFC 001](../done/001-task-state-machine-and-action-lifecycle.md) | — |
| 2 | [002 Decision schema, validation and built-in tools](./002-decision-schema-and-validation-order/implementation-handoff.md) | [RFC 002](../done/002-decision-schema-and-validation-order.md) | 001 |
| 3 | [003 Effects, grants and approvals](./003-effects-grants-and-approvals/implementation-handoff.md) | [RFC 003](../done/003-effects-grants-and-approvals.md) | 001, 002 |
| 4 | [004 Persistence and recovery](./004-persistence-checkpoints-and-recovery/implementation-handoff.md) | [RFC 004](../done/004-persistence-checkpoints-and-recovery.md) | 001–003 |
| 5 | [010 CLI, configuration and the M1 demonstration](./010-cli-and-operator-interface/implementation-handoff.md) | [RFC 010](../done/010-cli-and-operator-interface.md) (M1 part) | 001–004 |

## Milestone M2

| Order | Handoff | RFC | Depends on |
|---|---|---|---|
| 1 | [005 Execution worker, sandbox profile, confinement probe](./005-execution-worker-and-sandbox-profile/implementation-handoff.md) | [RFC 005](../done/005-execution-worker-and-sandbox-profile.md) | 001–004, 010 |
| 2 | [005b The interpreter download survives a flaky network (0.8)](./005b-download-robustness/implementation-handoff.md) | RFC 005; 011g's CI | 005 |

## Milestone M3

| Order | Handoff | RFC | Depends on |
|---|---|---|---|
| 1 | [006 Snapshots, rollback, promotion, lifecycle, fork](./006-workspace-snapshots-and-promotion/implementation-handoff.md) | [RFC 006](../done/006-workspace-snapshots-and-promotion.md) | 001–005, 010 |

## Milestone M4

| Order | Handoff | RFC | Depends on |
|---|---|---|---|
| 1 | [007 Context assembly, compression, redaction, measurement](./007-context-assembly-and-observation-pipeline/implementation-handoff.md) | [RFC 007](../done/007-context-assembly-and-observation-pipeline.md) | 001–006, 010 |
| 2 | [008 The OpenAI-compatible backend adapter](./008-model-backend-adapter/implementation-handoff.md) | [RFC 008](../done/008-model-backend-adapter.md) | 007 |
| 3 | [007b The fixed instructions: the goal is the task (0.8)](./007b-instructions-the-goal-is-the-task/implementation-handoff.md) | RFC 007; finding of L1 | 007, 008 |
| 4 | [008b Spike S2: how the decision reaches a small local model](./008b-decision-format-spike/implementation-handoff.md) | RFC 008; M4 report §6 | 007b |

## Release 0.9

| Order | Handoff | RFC | Depends on |
|---|---|---|---|
| 1 | [009b Four advisories against locked dependencies](./009b-dependency-advisories/implementation-handoff.md) | RFC 009, RFC 004 | — |
| 2 | [012 Decisions as native tool calls](./012-decisions-as-native-tool-calls/implementation-handoff.md) | [RFC 012](../done/012-decisions-as-native-tool-calls.md) | 007b, 008b, 009b |
| 3 | [011i The sweeper's stream test needs a margin](./011i-sweeper-test-margin/implementation-handoff.md) | RFC 011 | 011c |
| 4 | [012b Release candidate 0.9.0](./012b-release-0.9.0/implementation-handoff.md) | RFC 012 (complete with 0.9.0) | every 0.9 handoff |

## Release 0.10

| Order | Handoff | RFC | Depends on |
|---|---|---|---|
| 1 | [013 Operation on one machine, part one (M7a)](./013-operation-on-one-machine/implementation-handoff.md) | [RFC 013](../done/013-operation-on-one-machine.md) decisions 1 to 4 | — |
| 2 | [008c One HTTP stack, one sha2](./008c-dependency-hygiene/implementation-handoff.md) | RFC 008 | — |
| 3 | [011j Linux: narrow /etc and /opt](./011j-linux-read-set/implementation-handoff.md) | RFC 011 | — |
| 4 | [004b Event and action schema versions](./004b-schema-versions/implementation-handoff.md) | RFC 004 | — |
| 5 | [012c The engine context warning (B5)](./012c-engine-context-warning/implementation-handoff.md) | RFC 012 | — |
| 6 | [002b Test hygiene](./002b-test-hygiene/implementation-handoff.md) | RFC 002 | — |
| 7 | [010b A book page on the request-to-response flow](./010b-request-to-response-page/implementation-handoff.md) | RFC 010 | 013 (its storage words) |
| 8 | [009c rypsok 0.9.0, compatibility measured](./009c-rypsok-0.9.0/implementation-handoff.md) | RFC 009 | — |
| 9 | [009d What rypsok's reply changes in mimlys](./009d-rypsok-letter-changes/implementation-handoff.md) | RFC 009 | — |
| 10 | [011k Spike S3: one Verus proof of the permission-lock decision](./011k-verus-spike/implementation-handoff.md) | RFC 011 | — |
| 11 | [016 The first Verus proof lands](./016-formal-proofs-with-verus/implementation-handoff.md) | [RFC 016](../done/016-formal-proofs-with-verus.md) | 011k |
| 12 | [011l Linux read set, follow-up](./011l-linux-read-set-follow-up/implementation-handoff.md) | RFC 011 | 011j |
| 13 | [009e rypsok 0.10.0, compatibility measured](./009e-rypsok-0.10.0/implementation-handoff.md) | RFC 009 | 009c, 009d |
| 14 | [009f Can a stale tool list reach mimlys?](./009f-rmcp-tool-list-cache/implementation-handoff.md) | RFC 009 | 009e |
| 15 | [017 The worker and the interpreter modules admitted by their files](./017-admit-files-not-their-directories/implementation-handoff.md) | [RFC 017](../done/017-admit-files-not-their-directories.md) | — |
| 16 | [002c A rejection reaches the model with its reason and detail](./002c-rejections-reach-the-model/implementation-handoff.md) | RFC 002, RFC 007 | 002b, 010b |
| 17 | [004c The next start finishes or undoes a pending snapshot journal](./004c-snapshot-journals-at-start/implementation-handoff.md) | RFC 004, RFC 006 | — |
| 18 | [010c The log carries the step's ids; `debug_prompts` does what it says](./010c-logs/implementation-handoff.md) | RFC 010 | — |
| 19 | [002d Test hygiene, follow-up](./002d-test-hygiene-follow-up/implementation-handoff.md) | RFC 002 | 002b |
| 20 | [010d One stop signal must end `mimlys run`](./010d-one-stop-signal/implementation-handoff.md) | RFC 010 | — |
| 21 | [010e A stop signal interrupts only what can be undone](./010e-interrupt-what-can-be-undone/implementation-handoff.md) | RFC 010 | 010d |
| 22 | [011m No Unix socket outside the write set; a tool's tree ends with it; a tool writes only its own task's workspace](./011m-unix-sockets-tool-tree-task-write-set/implementation-handoff.md) | RFC 011 | — |
| 23 | [006b Promotion is always reversible; snapshots survive unusual names](./006b-promotion-always-reversible/implementation-handoff.md) | RFC 006 | — |
| 24 | [003b The approval card hides no argument; a stop signal at a prompt ends the process](./003b-approval-card-and-prompts/implementation-handoff.md) | RFC 003, RFC 010 | — |
| 25 | [009g Every MCP request is bounded; a destination is a string; what a server sends is cleaned](./009g-mcp-requests-bounded/implementation-handoff.md) | RFC 009 | — |
| 26 | [002e A failed restore is never silent; recovery survives a crash; a built-in never blocks](./002e-loop-and-built-ins-audit/implementation-handoff.md) | RFC 002, RFC 004 | — |
| 27 | [013b Release candidate 0.10.0](./013b-release-0.10.0/implementation-handoff.md) | RFC 013 (part one complete with 0.10.0) | every 0.10 handoff |

## Release 0.14

The local daemon (ROADMAP, "Release 0.14"; RFC 014 with its Amendment 1). Each later handoff is written when its turn comes.

| Order | Handoff | RFC | Depends on |
|---|---|---|---|
| 1 | [014a What a long-lived process needs](./014a-long-lived-process/implementation-handoff.md) | RFC 014 | — |

## Release 0.13

Adaptive effort, part one (ROADMAP, "Release 0.13"; RFC 015 with its Amendment 1). Each later handoff is written when its turn comes.

| Order | Handoff | RFC | Depends on |
|---|---|---|---|
| 1 | [015a Several backends, locality that is checked, and the fitness probe](./015a-backends-and-the-probe/implementation-handoff.md) | RFC 015 | — |
| 2 | [004e Readers that tolerate a newer release's records; output to a closed pipe](./004e-readers-of-newer-records/implementation-handoff.md) | RFC 004, RFC 010 | — |
| 3 | [015b Plan first: a plan shown and approved before anything acts](./015b-plan-first/implementation-handoff.md) | RFC 015 | — |
| 4 | [015-0 The M8 measurements: levels, the swap, and the probe](./015-0-m8-measurements/implementation-handoff.md) | RFC 015 | 015a (the probe) |
| 5 | [015c Effort levels: a budget per task, on the backend it names](./015c-effort-levels/implementation-handoff.md) | RFC 015 | 015a, 015-0 |
| 6 | [013f The registry's tests under load](./013f-registry-tests-under-load/implementation-handoff.md) | RFC 013 | — |
| 7 | [015d Release candidate 0.13.0](./015d-release-0.13.0/implementation-handoff.md) | RFC 015 (part one complete with 0.13.0) | every 0.13 handoff, 013f |

## Release 0.12

The user's copies, then operation on one machine (ROADMAP, "Release 0.12"). Each later handoff is written when its turn comes.

| Order | Handoff | RFC | Depends on |
|---|---|---|---|
| 1 | [004d The user's copies](./004d-the-users-copies/implementation-handoff.md) | RFC 004, RFC 010 | — |
| 2 | [013d-0 The operation theme's measurements](./013d-0-operation-measurements/implementation-handoff.md) | RFC 013 | — |
| 3 | [013d Operation on one machine, part two](./013d-operation-on-one-machine/implementation-handoff.md) | RFC 013 | 2 |
| 4 | [013e Release candidate 0.12.0](./013e-release-0.12.0/implementation-handoff.md) | — | 1 to 3 |

## Release 0.11

The audit's remainder, in three phases (ROADMAP, "Release 0.11"). Each later handoff is written when its turn comes.

| Order | Handoff | RFC | Depends on |
|---|---|---|---|
| 1 | [011n Windows and confinement: the small fixes](./011n-windows-and-confinement-small/implementation-handoff.md) | RFC 011 | — |
| 2 | [009h MCP and context: the small fixes](./009h-mcp-and-context-small/implementation-handoff.md) | RFC 009 | — |
| 3 | [006c The workspace: restore verifies first, the walk sees everything, modes are kept](./006c-workspace-0.11/implementation-handoff.md) | RFC 006 | — |
| 4 | [010f The start gate: a stop signal and an action's start are one step](./010f-start-gate/implementation-handoff.md) | RFC 010 | — |
| 5 | [009i What reaches the model is bounded as a whole](./009i-what-reaches-the-model/implementation-handoff.md) | RFC 009 | — |
| 6 | [011o Windows: a host tool reaches only its own task](./011o-windows-task-isolation/implementation-handoff.md) | RFC 011 | — |
| 7 | [011p macOS: the tree is swept from outside the sandbox](./011p-macos-sweep-outside/implementation-handoff.md) | RFC 011 | — |
| 8 | [013c Release candidate 0.11.0 (part 1: four fixes; part 2: the candidate)](./013c-release-0.11.0/implementation-handoff.md) | — | 1 to 7 |

## Milestone M5

| Order | Handoff | RFC | Depends on |
|---|---|---|---|
| 1 | [009 The MCP client, rypsok as the first external tool server](./009-mcp-client-and-tool-pinning/implementation-handoff.md) | [RFC 009](../done/009-mcp-client-and-tool-pinning.md) | 001–008, 010 |

## Milestones M6a–M6c

| Order | Handoff | RFC | Depends on |
|---|---|---|---|
| 1 | [011a The Linux backend and host tools](./011a-confinement-linux/implementation-handoff.md) | [RFC 011](../done/011-confinement-backends.md) | 001–010 |
| 2 | [011b The Windows backend](./011b-confinement-windows/implementation-handoff.md) | RFC 011 §3, §7 | 011a |
| 3 | [011d Linux hardening and read-set hygiene (0.7.1)](./011d-linux-hardening/implementation-handoff.md) | RFC 011 Amendment 1 | 011a, 011b |
| 4 | [011c The macOS backend](./011c-confinement-macos/implementation-handoff.md) | RFC 011 §4, §5 | 011a, 011d |
| 5 | [011e Windows: removing every trace before an uninstall (0.8)](./011e-windows-cleanup/implementation-handoff.md) | RFC 011 §7 | 011b, 011d |
| 6 | [011f Windows: the permission lock fails closed (0.8)](./011f-windows-permission-lock/implementation-handoff.md) | RFC 011 §7; D1 | 011b, 011e |
| 7 | [011g The descendant-walk test must not walk the test process (0.8)](./011g-walk-test-isolation/implementation-handoff.md) | RFC 011; `main`'s CI after 011c-6 | 011c |
| 8 | [011h Release candidate 0.8.0](./011h-release-0.8.0/implementation-handoff.md) | RFC 011 (complete with 0.8.0) | every 0.8 handoff |

Common rules for every handoff:

- The specifications are the source of truth: `docs/src/maintainers/requirements.md` (REQ ids), `external-design.md` (ED §), `limits-and-defaults.md` (LIM ids), `glossary.md`. Tests validate the specification, not the code; every test that verifies a `T` requirement names it in its name or doc comment (REQ-TEST-001), for example `req_task_002_illegal_transition_is_refused`.
- Rust 2024, Rust 2018+ module style (`foo.rs` plus `foo/`), tests in `src/<mod>/tests.rs` (REQ-ENG-003), `unsafe` forbidden (REQ-ENG-001), no I/O or protocol dependency in `mimlys-core` (REQ-ARCH-002; `scripts/check-core-deps.sh`).
- Every number comes from the limits register as a named constant with the `LIM-` id in its doc comment; nothing is hard-coded twice.
- Fixed vocabularies are Rust enums with `serde` renames in snake_case (CLR-6). An unknown value is an error on input from outside the store (the configuration, the model's reply, a tool's result); a record that a newer release of mimlys wrote is read with the unknown value kept by name and marked, never failing the read (REQ-MAIN-006, external design §22; from 0.13).
- A design defect found while implementing goes back as a question in the review request, not as a local redesign (project rules).
- **No open useless PRs, and no useless branches** (the owner's rule of 2026-11-26). A PR is closed, and its branch deleted, as soon as its role is finished, by the architect or by the dev team. They are never left for the owner.
  - **Finished means:** its work is on `main`, a fast-forward, a squash or a cherry-pick; or it was a throwaway (a mutation, a diagnosis, a stress run) whose result is recorded; or it was superseded.
  - **The architect** closes the PRs it lands, and deletes their branches, at the landing. It checks first that the work is on `main`, and archives the heads in `.git-exclude/archive/`.
  - **The dev team** closes its own throwaway and superseded PRs, and deletes their branches, when their result is in the request. It removes its local worktree and branch when the PR is closed. A worktree with uncommitted work is committed first, or kept with the reason said in the request.
  - **The request names what is still open,** and why, so that nothing stays open without a reason.
- The review request goes to `.git-exclude/review-requests/<id>-<topic>/README.md` (the dev team's outbox; the architect's review comes back under `.git-exclude/reviewed/<id>-<topic>/`), with an entry point that cites the RFC, the handoff, and the REQ ids and ED sections implemented, so a reviewer who did not design the feature can check conformance.
