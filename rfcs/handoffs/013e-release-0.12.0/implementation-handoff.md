# Handoff 013e — Release candidate 0.12.0

**Scope.** `ROADMAP.md`, "Release 0.12". Phases 1 and 2 are landed on `main`: 004d to 004d-5 (the user's copies, the worker's frame), 013d-0 (the M7b report), and 013d with 013d-4 (operation on one machine, part two). The engine is Wasmtime 50.0.3; 0.11.1 shipped that alone. **The owner authorized preparing the release on 2026-11-21.** The tag, the publish and the GitHub release are the architect's, after the review and the owner's final word.

**Model.** Handoff 013c, part 2 (0.11.0). Nothing is private, so the candidate branch is pushed for CI as usual.

## 1. Versions and the CHANGELOG

- **Versions:** already `0.12.0` on `main` (the architect's commit after 0.11.1: with 0.11.1 on crates.io, `main`'s `0.11.0` requirements resolved to the published 0.11.1 and the packaging dry-run failed). Check that every inter-crate pin and `Cargo.lock` agree.
- **The section:** `## [0.12.0] - <date of the green run>` holds "Unreleased". The engine's update already shipped in `## [0.11.1]` (2026-11-21), and "Unreleased" refers to it in one line; keep it so.
- **Lead with `### Security`,** one paragraph per finding that affects 0.11.1, each with its id (0.11.1 is the release a user upgrades from). Look at least at:
  - the worker's frame: a stream above about 600 KB ended the worker, and on an action that cannot be undone that is `unknown_outcome`;
  - anything else the review files of 0.12 name as security.

  If none of 0.12's own fixes is a vulnerability, say so in one sentence instead of inventing one.
- **"Changed" leads with what an upgrading user meets first:**
  - `sandbox.output_bytes` is at most 16 MiB;
  - the worker protocol is version 2, which matters only with a worker of another build;
  - a second runtime's executions now wait for a slot (LIM-X5 = 2), and what `waiting` means;
  - one model request at a time per engine (`max_in_flight`);
  - the CPU part of clause C on each platform, with Linux `unavailable` in practice and the `systemd-run` way;
  - the new `status`, `metrics`, `task show` and `task output` fields;
  - `runtime reap`.
- **`SCHEMA_VERSION` and the other versions:** say whether they changed in 0.12, and if a 0.11 binary can open a 0.12 store, check it as 013c did.

## 2. Documents

- **`README.md` and ROADMAP's status line:** 0.12.0 is 0.11.1 plus the user's copies and operation on one machine (M7b). ROADMAP's milestone table says M7b is in 0.12.
- **RFC 013** moves from `accepted/` to `done/`, since both its parts are now shipped. Its Status line says so.
- **The book pages** the review files name, and the getting-started version sentence.
- **The threat register's review row for 0.12** is the architect's; leave it.

## 3. The gate

On the branch:
- fmt; clippy on Linux, and for macOS and Windows with the C compiler stubbed;
- `cargo test --workspace --all-features --no-fail-fast`, the signal tests in the foreground;
- `cargo deny check`;
- the scripts;
- `mdbook build docs`;
- `cargo publish --workspace --dry-run` at 0.12.0 **on a clean registry cache**.

Then CI on the draft PR, every job, with skips counted (and the delegated step's clause lines on the Ubuntu job), and the `interpreters` workflow.

## 4. The release notes

`release-notes-0.12.0.md`, beside the request, built from the CHANGELOG:
- Read this first;
- Install;
- What is new (`task output`; several runtimes on one machine);
- Changed, if you script mimlys;
- Platforms (the CPU part per platform);
- **Known limitations:**
  - the CPU part on Linux;
  - LIM-H4 fixed;
  - the residuals of the threat register that still stand.

No security advisory, as for 0.10.0 and 0.11.0, unless you think the owner's reason no longer holds.

## 5. Return

`.git-exclude/review-requests/013e-release-0.12.0/README.md`, with the notes beside it. It holds:
- the branch's head and its commits;
- the CI runs;
- the gate's results;
- the dry run's tail;
- anything you could not do.

The commands that push to `main`, tag, publish and create the release are not yours.
