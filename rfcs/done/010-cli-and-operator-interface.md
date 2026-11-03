# RFC 010 — CLI and operator interface

**Status.** Implemented (0.7.0; accepted 2026-10-27)
**Tracks.** Decisions D-17 and D-18 of the startup review; findings M-13, X-05, X-06, A-16; requirements REQ-CFG-001 … REQ-CFG-006, REQ-CLI-001 … REQ-CLI-006, REQ-APR-009.
**Touches.** The `mimlys` crate (commands, configuration loading, approval prompt, follow mode, logging), the configuration schema, external design §4, §10.1, §19. Spans M1 (task control, approvals, fakes, `check`) to M5 (servers); each milestone's part is handed off separately.

## Summary

Fix the operator interface for v1: one TOML configuration file with a schema version, refused unknown keys and secrets by reference; a `mimlys` binary that embeds the runtime, exposes the control API as commands with stable `--json` output, renders the approval prompt from a fixed escaped template, and follows a task's events live. The user and the administrator are two channels: commands and configuration.

## Motivation

- The drafts listed configuration areas without a format, versioning or validation rules (M-13).
- "Operator" conflated the user who gives goals with the administrator who sets policy (X-05); the deployment variant was undecided (A-16).
- The approval screen is the one place where human control is exercised; its rendering must be the runtime's, escaped and fixed (R-01, R-04).

## Decision

The normative text is external design §19.1 (configuration shape and validation), §19.2 (commands and exit codes), §10.1 (rendered approval request), §4 (control API) and the requirements named above. In brief:

1. **Deployment v1:** the `mimlys` binary runs the runtime in process and spawns the worker; the CLI process is the session, the operating-system user is the identity (D-18). Daemon mode is DR-002.
2. **Configuration:** one TOML file located by `--config`, `MIMLYS_CONFIG` or the platform default; `config_version = 1`; every default from the limits register; unknown keys, inline secrets and out-of-range budgets refused at startup with exit code 1 naming the key; `mimlys check` validates and exits; no hot reload.
3. **Commands:** `run`, `task new|show|list|resume|pause|cancel`, `approve`, `reject`, `answer`, `grant`, `revoke`, `rollback`, `promote`, `events`, `metrics`, `status`, `check`, `gc`, `--version`; exit codes 0, 1, 2; `--json` everywhere with field names that are part of the contract.
4. **Approval prompt:** the runtime's rendered request (ED §10.1) in a fixed template; every external string quoted and escaped, invisible characters removed with a notice; options approve, reject with note, reject and halt, grant.
5. **Follow mode:** `mimlys run` creates and starts a task, prints events as they happen and prompts when the task waits for the user; a stop signal pauses at the next boundary, a second cancels; exit within LIM-O1.
6. **Messages:** every user-facing string lives in one catalog so other languages can be added later (D-17); English only in v1.
7. **Logging:** structured lines on standard error; never prompt content, file content or secrets at the default level.

## Alternatives considered

- A daemon with a thin client from the start: needs an authentication design before any task can run; deferred (DR-002).
- Environment variables for every setting: scattered, unversioned, easy to leak; rejected in favour of one file plus secret references.
- Free-form approval prompts composed per tool by the tool author: inconsistent and spoofable; rejected in favour of one template fed by the canonical action.

## Consequences

- Configuration and CLI are external contracts (REQ-MAIN-001); changes are announced in the changelog.
- M1 ships the commands needed to exercise RFCs 001–004 with the fake backend; `promote`, `rollback` (M3), servers (M5) and the tier display (M2) follow.

## Open questions

Whether `mimlys run` should default to `--hold` when the profile grants anything beyond the workspace; decided from M1 usage.

## References

Requirements §21; external design §4, §10.1, §19; startup review D-17, D-18.
