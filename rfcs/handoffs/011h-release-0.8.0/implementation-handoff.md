# Handoff 011h — Release candidate 0.8.0 (M6c; RFC 011 complete)

**RFC.** [RFC 011](../../accepted/011-confinement-backends.md): 0.8.0 ships its last part (the macOS backend) · **Scope.** `ROADMAP.md`, "Release 0.8" (rescoped by the owner on 2026-11-08: the real-model run and LIM-C1 moved to 0.9) · **Model.** The 0.7.1 candidate (R1), with its corrections folded in from the start.

Prepare the candidate on a branch `release-0.8.0` from `main`. **Do not tag, publish or merge**: the owner decides who runs the commands of §4.

## 1. Versions and the changelog

- Workspace version `0.8.0` in `Cargo.toml` and every inter-crate `version = "0.7.1"` pin of the manifests; `Cargo.lock` follows (`cargo update -w`, nothing else updated).
- `CHANGELOG.md`: `## [0.8.0] - <date of the green run>` holding the current `Unreleased` content, regrouped only where an entry sits under the wrong heading; a fresh empty `## [Unreleased]` above it. Name under **Changed** every change of an external contract since 0.7.1 (the `process_limit` termination, the refusal at load of a command that leaves the read paths on every platform, the `unavailable` kinds for `permissions` and `download`), so that a user upgrading reads them in one place.

## 2. Documents (words only; no design change)

- `README.md` ("Quick start"), `docs/src/getting-started.md`, `ROADMAP.md` (status line): 0.8.0 is M1 to M6c; macOS is no longer "follows in 0.8": state its confinement as the book's macOS section does, in one sentence (Seatbelt, every mandatory clause enforced, `cargo test` as the demonstration).
- **The model, honestly** (the owner's rule: no user may be misled). In the README's quick start, `getting-started.md` and `docs/src/reference/configuration.md` where a model is configured: one plain sentence that in 0.8 the local models measured (M4 report §6) did not complete a task under the decision format, and that 0.9 changes how decisions are requested (RFC 012). The example model becomes `qwen2.5:7b-instruct` (the one that completed the task in spike S2 under native calls), with no claim that it works in 0.8. Do not touch the M4 report.
- **RFC 011 moves to `done/`** with the release: `git mv rfcs/accepted/011-confinement-backends.md rfcs/done/`; its Status line begins `Implemented (0.8.0; accepted 2026-10-29).` followed by the line's existing text; `rfcs/README.md` moves its row from "Accepted" to "Implemented" (`0.8.0 (M6a to M6c)`); fix every link to the old path (`scripts/check-rfcs.sh` and `check-doc-refs.sh` find them).
- The threat register's review-log row for 0.8 is the architect's; leave it.

## 3. The gate, the CI, the interpreters workflow

On the branch: fmt; clippy on Linux, `x86_64-pc-windows-gnu` and `aarch64-apple-darwin` (the last two check-only); `cargo test --workspace --all-features --no-fail-fast`; `cargo deny check`; `check-unsafe`, `check-core-deps`, `check-rfcs`, `check-doc-refs`, `check-req-tests` (report only, its counts); `mdbook build docs`; `cargo publish --workspace --dry-run` at 0.8.0. Then the CI on the branch's pull request (all nine jobs), and the `interpreters` workflow by `workflow_dispatch` on the branch. Every command's result and every run id, platform by platform, with the skip counts.

## 4. What the owner runs (listed exactly, not run)

The sequence as for 0.7.1, at this version: land the candidate on `main` (rebase first if `main` moved), check what will be tagged, **`git tag -s 0.8.0 -m "mimlys 0.8.0"`** (signed, no `v`), push the tag, `cargo publish --workspace` with the owner's own token (never written to a file or a command line you keep), and the fallback order crate by crate. Then the GitHub release: give the release notes as a ready text (from the changelog, with the honest model sentence of §2 and a link to the M6c report), to be created as the latest release for tag `0.8.0`.

## 5. Return

`.git-exclude/review-requests/R3-release-0.8.0/README.md`: files changed, the gate as it ran, the run ids with skips, the dry-run's output tail, §4's commands, the release notes, and anything you could not do.
