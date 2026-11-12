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
| `verus` job, Linux | the Verus proof of the permission-lock decision (`sh verify/run.sh`), then the vacuity guard (`sh verify/guard.sh`): see §3b | REQ-TEST-018 (RFC 016) |
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

- The state on 2026-11-12, after handoff 002b item 1: **21 of 210 `T` requirements** listed and 0 of 14 invariants (nine mapped as structural). Three are listed on purpose because the code does not yet do what they say (REQ-OBS-004, REQ-OBS-008, REQ-PLAT-004: handoffs 010c, 002c and 002d); the rest are named by the handoffs of RFC 013 part two, 014 and 015, and REQ-SCHED-002 and REQ-SCHED-005 by handoff 002d.

## 3a. The hostile corpus (REQ-SEC-013, REQ-TEST-017)

The corpus is **data**: one directory of case files, `crates/mimlys/tests/corpus/hostile/<kind>/*.json`, read by one harness, `crates/mimlys/tests/hostile_corpus.rs` (a test of the `mimlys` crate, so part of `cargo test --all-features` and run on every change; no `build.rs`). The kinds are the families of REQ-SEC-013: `decisions/` (the strict parser, the loop's rejections of model output, the approval display; model text changes no authority), `paths/` (traversal, absolute and Windows forms, case and normalization, the existence leak, symbolic links), `tool-results/` (the sanitizer, the context markers, a five-megabyte hostile result through the loop), `mcp-descriptions/` (THR-25) and `modules/` (the sandbox's malformed modules). The harness lives in the `mimlys` crate because it is the one crate that depends on core, exec and mcp, whose public interfaces the cases attack; the cases need no private item and no copy of a fixture.

**A case** is a small JSON file: a `check` naming the production function it runs against, a `note`, the hostile input and the expected outcome (the rejection code, the denial reason, the cleaned text, the refusal). The harness runs every file, compares, and fails naming the file (`paths/canonicalize-dotdot-escapes.json: path error: expected "escapes", got ...`). Every field of the expected outcome is required and an unknown field is an error, so a case that lost its expectation fails instead of passing weaker. **To add a case, add a file** (copy one of the same `check`; write the expectation from what the product must do; run `cargo test -p mimlys --test hostile_corpus`); the format and the fields of each check are in the corpus's `README.md`. The harness fails when a kind directory has no case, when a check has no case, when a file is not a well-formed case, and (through `scripts/check-req-tests.sh`) when its doc comment stops naming REQ-SEC-013 and REQ-TEST-017.

**What is not data.** Two hostile tests stay tests: the Seatbelt generator's escaping of a hostile path (macOS only, and the functions are private to the generator) and the M5 test of a hostile description and bad output from a real external server process (it drives the `mimlys` binary and a server). Both still name REQ-SEC-013. A new hostile test of that kind names REQ-SEC-013 in its doc comment, as before; a new hostile *input* for a function the harness already attacks is a case file.

**Mutations** (2026-11-12; each applied alone to the production file, the harness run, the file restored and compared byte for byte): the corpus fails, naming the case files, when the strict parser's size bound is removed (`decisions/parse-too-large.json`) or its duplicate-key classification (`parse-duplicate-*.json`); when `WorkspacePath` stops refusing a `..` that leaves the root (seven cases, among them `paths/canonicalize-dotdot-escapes.json`, `decisions/loop-traversal.json` and the existence-leak case) or a control character; when the sanitizer stops removing the right-to-left override (the sanitizer case, the approval display, the five-megabyte result and the MCP description); when the import stops cutting a description to LIM-I2; when a module without `_start` is reported as another fault; and when a search stops skipping and counting symbolic links. Deleting an expected field from a case (`removed`, `closing_line_count`), changing an expected value, removing a case's `check`, moving every case of a kind or of a check away, adding a field the check does not know and adding a stray file each fail too. The outputs are with the request of handoff 002b item 2.

## 3b. The Verus proof of the permission-lock decision (REQ-TEST-018, RFC 016)

**What is checked.** `decide` and its pure core in `crates/mimlys-exec/src/confine/dacl_wait.rs` (REQ-CONF-008, external design §13.5, THR-36). The specifications are in that file under `cfg_attr(verus_keep_ghost, …)` and `#[cfg(verus_keep_ghost)]`, which only Verus sets, so the product's compiler does not see them and no crate depends on Verus. `verify/dacl_wait.rs` includes the production module with `#[path]`: the checker reads the text that ships, never a copy. `verify/` is outside the workspace's members and is not packaged.

**The property.**
- **P1.** `decide_from` returns `Write` only if the first attempt returned `Acquired`, or the first was `Held` and the second `Acquired`; the same for `WriteAfterAbandoned` and `Abandoned`. No write without a lock that was given.
- **P2.** A lock `Held` on both attempts is `Refuse(Held)`.
- **P3.** A `Failed` attempt that decides (the first, or the second after a `Held`) is `Refuse(LockUnavailable)`.
- **P4, in part, and under a hypothesis.** Every call of the attempt inside `decide` passes `0` or the limit saturated at `u32::MAX` (`wait_ms` is proved to saturate), and the one second call in the code sits in the `Held` branch (an assertion). The hypothesis is `decide`'s `requires` on the closure (its precondition admits exactly those two values). No caller of `decide` is verified, so the contract is not checked against the real closure.

**What it does not prove.** `decide` has no postcondition: its call of `decide_from` is inspected, not proved. Not proved, and held by the tests of the module: an attempt added outside the `Held` branch, the order of the calls, and their number. The caller in `windows::acl`, which acts on the decision, is outside the proof, and what Windows does with the mutex is the system's. The book and the threat register say "proved" only of P1 to P3 and of the part of P4 above. The proof guards the decision that D1's remedy rests on; it would not have caught D1 (a lost update in a write that had no lock).

**Mutations** (each applied alone to the production file, the proof run, the file restored and compared byte for byte; Verus 0.2026.11.02.8bf68ee, 2026-11-11):

| Mutation | Result |
|---|---|
| m1 a second `Held` mapped to `Write` | fails: P1 and P2 unmet |
| m2 `Failed` mapped to `Write` | fails: P1 and P3 unmet |
| m3 the second attempt's wait `limit_ms / 2` | fails: the call does not meet the closure's precondition |
| x1 the wait wraps instead of saturating | fails: `wait_ms`'s postcondition |
| x2 `Abandoned` mapped to `Write` | fails: P1 |
| x3 a `Held` first with no second one written as `Acquired` | fails: P1 |
| x4 the second attempt made whatever the first gave | fails: the assertion that it follows a `Held` |
| x6 the second attempt's result ignored | fails: P3 |
| **x5** the first attempt called with the wait instead of `0` | **survives**; held by `req_conf_008_held_then_released_writes_after_the_wait` (it asserts the timeouts `[0, 60000]`; the mutation fails it, and three more) |
| **y1** an extra attempt in the branch where the first was not `Held` | **survives** (run once, 2026-11-11: `3 verified, 0 errors`); held by `req_conf_008_free_lock_writes_without_waiting` (it asserts exactly one attempt; the mutation fails it, and two more) |

**The vacuity guard.** The CI job runs m1 after the proof (`verify/guard.sh`), requires the proof to fail with an unmet postcondition, restores the file and compares it. A Verus that proved everything, or a mutation that no longer applies, turns the job red.

**Run time.** About 0.4 s for the proof (Verus total 334 ms; the SMT part 2 ms), warm. On CI the job takes about 40 s, almost all of it building `mimlys-core` with the pinned toolchain; the Verus download is cached.

**The pin** (`scripts/fetch-verus.sh` is the only place that names them): Verus release `0.2026.11.02.8bf68ee`, `verus-0.2026.11.02.8bf68ee-x86-linux.zip`, SHA-256 `f3a0119292df8855e4435da8130bee1873b0e6a60c306accf81a08ea687bba3c`, **compared** before unpacking (a mismatch fails; the download is retried as in handoff 005b); the unpacked directory is cached by the hash of that script. The Rust toolchain is the one Verus names, `1.99.1`, pinned in the job; it moves only with a Verus upgrade, which is its own change and re-runs the proof and the mutations above. A change to `decide` or `decide_from` re-runs both in the same job; a change of meaning is a change to the `ensures` lines beside the code.

**How the job is watched (the owner's decision of 2026-11-11).** No job is a required status check on `main`: the architect lands commits by fast-forward, often as rebuilt linear series that have no run of their own yet, and a required check would block those pushes. Instead, at no extra cost:
- **Before landing**, the architect checks the `verus` job's result on the run being landed, as for every job. A red `verus` job stops the merge.
- **After every push to `main`**, the architect checks `main`'s run, `verus` included. A red `verus` job is fixed or reverted before the next landing.
- **At each release review**, the candidate's `verus` result and the pin's age are part of the readiness check.
- **Upgrades** of Verus and its toolchain are deliberate: at most one per release, as their own change, never to fix a red job.
- **Developers** need nothing. Only a change to `crates/mimlys-exec/src/confine/dacl_wait.rs` or `verify/` asks for `sh verify/run.sh` locally, and CI runs it either way.

If merging ever moves to pull requests, every job becomes a required check together, as one decision.

## 3c. Tests decide by state, never by a short elapsed time (handoffs 011i, 002b item 3)

A test decides by a process's or the store's state: a marker written, a file that stops growing, a process that is gone, the outcome that only the defect would change. Any duration it holds (a stand-in that hangs, a give-up) is far longer than anything it waits for, so a slow runner cannot fail a correct test and a short bound cannot hide a defect. Where a limit is the contract (LIM-O2, a cancellation seen within 2 000 ms), the test measures from the state change and asserts **five times the limit**; the tight figure is the product's, not a CI runner's. A wall-clock property finer than that (a poll interval) is not asserted until the product has a clock to inject. A lower bound (a refusal after the wait, never before) stays as it is: a slow runner can only lengthen it.

## 4. Other scripts

`check-core-deps.sh` (the core depends on no I/O or protocol crate), `check-unsafe.sh` (`unsafe` only under `crates/mimlys-exec/src/confine/`, each block with a `SAFETY:` line), `check-doc-refs.sh` (every `REQ-`, `LIM-` and `THR-` cited is defined, except in `rfcs/proposed/`, where an RFC may cite the identifiers it proposes and which must resolve from its acceptance on; every requirement appears in the external design's coverage table).
