# Repository guards

**Status:** 2026-11-03. **Informative**, except where it cites a requirement. The checks that keep the repository honest: what each one verifies, what it may not do, and how it is wired. A guard that is not listed here is not part of the gate.

## 1. The gate (REQ-TEST-014)

| Where | Check | Verifies |
|---|---|---|
| `check` job, Linux, Windows, macOS (and Linux aarch64) | `cargo fmt --check`, `cargo clippy -D warnings`, `cargo build`, `cargo test --all-features --no-fail-fast`; before the tests, the pinned interpreter is fetched once with retries and verified (`scripts/fetch-interpreter.sh`), cached by its SHA-256, and the tests use it through `MIMLYS_QJS_WASM` | formatting, lints, build, tests; no test downloads the interpreter itself (handoff 005b) |
| `rosetta` job, `macos-latest` | installs Rosetta, adds `x86_64-apple-darwin`, runs `cargo test --workspace --all-features --target x86_64-apple-darwin --no-fail-fast` | REQ-PLAT-002 (macOS x86_64): the Intel build and tests under translation; it does not show an Intel Mac's kernel, and a problem here never hides the native macOS result |
| `msrv` job, Linux | build and test on the toolchain named by `rust-version` (REQ-PLAT-001) | the stated minimum is real |
| `package` job, Linux | `cargo publish --workspace --dry-run` | the six crates package and verify in dependency order |
| `deny` job | `cargo deny --all-features --log-level warn check advisories licenses bans sources`, with cargo-deny 0.20.2 installed from crates.io and cached (not a Docker action) | REQ-SEC-012 |
| `rules` job | `scripts/check-core-deps.sh`, `check-unsafe.sh`, `check-rfcs.sh`, `check-doc-refs.sh`, `check-req-tests.sh` (report only), `mdbook build docs` (pinned 0.5.4, installed from crates.io and cached, not a GitHub release download) | REQ-ARCH-002, REQ-ENG-001, RFC 000, the registers, the book |
| `interpreters` workflow, monthly and on demand | rebuilds the pinned interpreter from its recipe and compares the hash | REQ-ARCH-007; a mismatch is a review item, never silently accepted |

A plain `cargo test --workspace` (default features) does not build the test-only binaries, so it skips the suites that need them (the M1 and M2 harness tests among them); the gate is `--all-features`, and the CI jobs run that.

Downloads in the workflows (handoff 005b). GitHub's release downloads answered HTTP 504 intermittently for about an hour on 2026-11-08 and turned a test-only change red on eight jobs. What remains downloaded, and how: the pinned interpreter by `scripts/fetch-interpreter.sh` (`curl --fail --retry 8 --retry-all-errors`, exponential backoff, about four minutes in all, then the SHA-256 check) and the `actions/cache` entry keyed by that hash; `mdbook` from crates.io (cargo's index, not a release asset); the Rust toolchain by `dtolnay/rust-toolchain` (its own curl retry). `cargo-deny` is installed the way `mdbook` is: `cargo install cargo-deny --locked --version 0.20.2` from crates.io, cached by `actions/cache` (key `cargo-deny-0.20.2-<OS>`). 0.20.2 is the version the Docker action it replaced shipped (its Dockerfile fetched the release with `curl --silent -L … | tar`, without `--fail` or a retry, on every run). The advisory database is still fetched by cargo-deny itself, from github.com/RustSec/advisory-db, on every run; observed with the network down and a local copy present: cargo-deny logs `failed to fetch advisory database … Could not connect` and exits 1. A failed fetch therefore fails the job, closed; it does not fall back to a stale copy (`--disable-fetch` would, and is not used). A GitHub outage can still fail that one job, once per run, for that reason.

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

## 3a. The hostile corpus (REQ-SEC-013, REQ-TEST-017)

The corpus is the set of tests that feed hostile input to the runtime and whose doc comment names REQ-SEC-013: hostile decisions (`validate`, `decision`), paths (`path`, the built-in tools' symbolic-link tests), tool results and model output (the runtime tests of REQ-SEC-011), descriptions (`mimlys-mcp`, THR-25) and modules (the sandbox's malformed-module tests). It runs on every change because it is part of `cargo test --all-features`. A new hostile case joins it by naming REQ-SEC-013. A data-driven corpus (one fixture directory read by one harness) is a candidate for the replanning after M6c, not a requirement of 0.8.

## 4. Other scripts

`check-core-deps.sh` (the core depends on no I/O or protocol crate), `check-unsafe.sh` (`unsafe` only under `crates/mimlys-exec/src/confine/`, each block with a `SAFETY:` line), `check-doc-refs.sh` (every `REQ-`, `LIM-` and `THR-` cited is defined, except in `rfcs/proposed/`, where an RFC may cite the identifiers it proposes and which must resolve from its acceptance on; every requirement appears in the external design's coverage table).
