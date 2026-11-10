# Handoff 012b — Release candidate 0.9.0 (RFC 012 complete)

**RFC.** [RFC 012](../../done/012-decisions-as-native-tool-calls.md): 0.9.0 ships it · **Scope.** `ROADMAP.md`, "Release 0.9". Its "done when" is met on `main`: run B (the M4 report §7), LIM-C1 fixed, prefix stability measured for both transports, CI green, and the threat register's 0.9 row (written by the architect). The CHANGELOG is this candidate's · **Model.** Handoff 011h (0.8.0), with what its review corrected.

Prepare the candidate on a branch `release-0.9.0` from `main`. **Do not tag, publish or merge**: the architect does that after the Go (the owner's rule).

## 1. Versions and the changelog

- Workspace version `0.9.0` in `Cargo.toml` and every inter-crate `version = "0.8.0"` pin. `Cargo.lock` follows by `cargo update -w`, and nothing else moves.
- `CHANGELOG.md`: `## [0.9.0] - <date of the green run>` holds the `Unreleased` content, with an empty `## [Unreleased]` above it.
  - Begin with two sentences: what 0.9.0 is (decisions as native tool calls; one model measured to finish the M4 task), and what does not change (the envelope stays as `decision_transport = "envelope"`).
  - Keep every entry. Make sure **Changed** leads with what an upgrading user notices first: with the default `auto`, an engine that accepts `tools` is now asked natively, so requests change shape.
  - Name the additive contract fields once: `status`'s `decision_transport`, `transport_reason`, `parallel_tool_calls`; `decision.received`'s `transport`, `text_chars`.

## 2. Documents (words only; no design change)

- `README.md`, `docs/src/getting-started.md`, `ROADMAP.md` (status line): 0.9.0 is M1 to M6c plus decisions as native tool calls. "The model, plainly" stays as 012-4 wrote it, bounded to run B (one model, one task, ten runs on one machine).
- **RFC 012 moves to `done/`**: `git mv rfcs/accepted/012-decisions-as-native-tool-calls.md rfcs/done/`. Its Status line becomes exactly:

  > **Status.** Implemented (0.9.0; accepted 2026-11-08, by the owner). Run B: Qwen2.5-7B-Instruct finished the M4 task in 9 of 10 runs (the M4 report §7).

  `rfcs/README.md` moves its row to "Implemented" (`0.9.0`). Fix every link to the old path (`check-rfcs.sh`, `check-doc-refs.sh`).
- The threat register's row is on `main` already; leave it.

## 3. The gate, the CI, the interpreters workflow

On the branch:
- fmt;
- clippy on Linux, and check-only on `aarch64-apple-darwin` (and `x86_64-pc-windows-gnu` if a MinGW compiler is available; otherwise say so and rely on the `windows-latest` job);
- `cargo test --workspace --all-features --no-fail-fast`;
- `cargo deny check`;
- the five scripts;
- `mdbook build docs`;
- `cargo publish --workspace --dry-run` at 0.9.0.

Then the CI on the pull request (nine jobs), and the `interpreters` workflow by `workflow_dispatch` on the branch. Report every result and run id, platform by platform, with skips. **History stays linear**: no merge commits (the repository refuses them on `main`).

## 4. The release notes

A ready text, `release-notes-0.9.0.md` in the request, built from the changelog:
- **"Read this first: the model"**: what run B measured and its bounds, that a model must return structured calls (the Coder variants measured did not), and Ollama's `num_ctx`;
- **Install**;
- **What is new**;
- **Changed: read this if you script mimlys**: the request shape under `auto`, how to keep 0.8's (`envelope`), the new fields;
- **Security**: the advisories removed;
- **Platforms**;
- **Known limitations**: the residuals of the 0.9 threat row; the audit is in progress, not done.

No claim beyond the M4 report.

## 5. Return

`.git-exclude/review-requests/R4-release-0.9.0/README.md` and `release-notes-0.9.0.md` beside it: files changed, the gate, the run ids with skips, the dry run's tail, anything you could not do. The commands that land, tag, publish and create the GitHub release are not yours to list or run: the architect runs them after the Go.
