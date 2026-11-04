# Repository guards

**Status:** 2026-11-03. **Informative**, except where it cites a requirement. The checks that keep the repository honest: what each one verifies, what it may not do, and how it is wired. A guard that is not listed here is not part of the gate.

## 1. The gate (REQ-TEST-014)

| Where | Check | Verifies |
|---|---|---|
| `check` job, Linux, Windows, macOS (and Linux aarch64) | `cargo fmt --check`, `cargo clippy -D warnings`, `cargo build`, `cargo test --all-features --no-fail-fast` | formatting, lints, build, tests |
| `rosetta` job, `macos-latest` | installs Rosetta, adds `x86_64-apple-darwin`, runs `cargo test --workspace --all-features --target x86_64-apple-darwin --no-fail-fast` | REQ-PLAT-002 (macOS x86_64): the Intel build and tests under translation; it does not show an Intel Mac's kernel, and a problem here never hides the native macOS result |
| `msrv` job, Linux | build and test on the toolchain named by `rust-version` (REQ-PLAT-001) | the stated minimum is real |
| `package` job, Linux | `cargo publish --workspace --dry-run` | the six crates package and verify in dependency order |
| `deny` job | `cargo deny check advisories licenses bans sources` | REQ-SEC-012 |
| `rules` job | `scripts/check-core-deps.sh`, `check-unsafe.sh`, `check-rfcs.sh`, `check-doc-refs.sh`, `check-req-tests.sh` (report only), `mdbook build docs` (pinned, 0.5.4) | REQ-ARCH-002, REQ-ENG-001, RFC 000, the registers, the book |
| `interpreters` workflow, monthly and on demand | rebuilds the pinned interpreter from its recipe and compares the hash | REQ-ARCH-007; a mismatch is a review item, never silently accepted |

A plain `cargo test --workspace` (default features) does not build the test-only binaries, so it skips the suites that need them (the M1 and M2 harness tests among them); the gate is `--all-features`, and the CI jobs run that.

Rules for the workflows: tools are pinned (the `mdbook` version in use, not `latest`); actions are pinned by commit hash with the tag in a comment; a new job never changes what an existing job runs.

## 2. `scripts/check-rfcs.sh` (RFC 000, optional invariants)

Status line matches the folder; numbers unique; every RFC listed in `rfcs/README.md`; and, since 2026-11-03, every relative link in `rfcs/**/*.md` resolves, every `rfcs/handoffs/NNN-*` directory belongs to an RFC numbered NNN in some state folder, and no handoff directory holds a state subfolder. This is why a handoff exists only for an RFC: work without an RFC (a repository guard, a spike) is ordered by a task file, which is volatile, and what it decides that must outlive it is written here.

## 3. `scripts/check-req-tests.sh` (REQ-TEST-001, REQ-SEC-004)

It lists every requirement marked `T` that no test names, and every invariant `INV-01` … `INV-14` that no test names. A test names a requirement by its identifier in its function name (`req_task_002_…`) or its doc comment.

- **Report only** until the architect has decided which gaps get tests; it prints and exits 0. It is turned into a failing check when the list is empty.
- **A mapping file**, `scripts/req-test-map.txt`, holds the requirements met structurally, one line each with the reason (REQ-TEST-014 is the CI gate itself; REQ-ENG-001 is `check-unsafe.sh`). A mapped requirement is not listed. The file is reviewed like code: a line without a reason is an error.
- **No test is written to empty the list.** A test that names a requirement it does not verify is worse than a listed gap.
- **What is searched.** The test files only: a `tests.rs` beside a module and everything under a crate's `tests/`. A mention in ordinary source (a doc comment on an implementation: `(REQ-ARCH-002)`) is documentation, not a test, and does not count.
- The state on 2026-11-03: the intake review (N-05) found 8 of 14 invariants and 29 of 187 `T` requirements named by no test, searching every Rust file. Searching the test files only, as above, the script finds **8 of 14 invariants and 46 of 189 `T` requirements** (four more are mapped as structural): 21 requirements are named only in source comments. The invariants agree; the requirements differ by that 21, and the script's number is the one to track.

## 4. Other scripts

`check-core-deps.sh` (the core depends on no I/O or protocol crate), `check-unsafe.sh` (`unsafe` only under `crates/mimlys-exec/src/confine/`, each block with a `SAFETY:` line), `check-doc-refs.sh` (every `REQ-`, `LIM-` and `THR-` cited is defined; every requirement appears in the external design's coverage table).
