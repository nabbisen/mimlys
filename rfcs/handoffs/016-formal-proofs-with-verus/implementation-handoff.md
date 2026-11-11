# Handoff 016 — The first Verus proof lands (release 0.10)

**RFC.** [RFC 016](../../accepted/016-formal-proofs-with-verus.md) (accepted 2026-11-11, with the owner's corrections C1 to C8) · **Specification.** REQ-TEST-018 (new), REQ-CONF-008, external design §13.5; threat THR-36 · **From.** Spike S3 (handoff 011k; branch `S3-verus-spike`, PR #40).

**Status (2026-11-11): merged** (`9efd124`; CI `43001772426`, 10 of 10 with the new `verus` job). The proof passes; the vacuity guard requires m1 to fail and it does; the archive's hash is compared before unpacking; y1 and x5 survive the proof and fail the named tests, as recorded in `repository-guards.md` §3b. Two additions beyond the handoff are accepted: the `req-test-map.txt` line and the CHANGELOG entry. Making the `verus` job a required status check on `main` is a repository setting: the owner's. Nothing else is open here.

## 1. What lands (from the spike's branch, on a fresh branch from `main`)

- The production edits in `crates/mimlys-exec/src/confine/dacl_wait.rs`: `wait_ms`, `decide_from`, `if let` for the `Held` test, and the specifications under `cfg_attr(verus_keep_ghost, …)` / `#[cfg(verus_keep_ghost)]`. The comments say what the proof is and what it does not prove, as RFC 016 decision 4 words it, without "spike S3".
- The `unexpected_cfgs` check-cfg line for `verus_keep_ghost` in `mimlys-exec`'s manifest.
- `verify/` (outside the workspace's members, never packaged): the root that includes the production module by `#[path]`, `run.sh`, and a `README.md` that says what is proved, what is not, and how to run it.
- **Not** the throwaway workflow `verus-spike.yml`, which stays on the spike's branch.

## 2. The CI job (REQ-TEST-018; RFC 016 decision 3)

A new job in `ci.yml`, Linux only; no existing job changes.
- **Verus:** the pinned release `0.2026.11.02.8bf68ee`, downloaded with a retry as in handoff 005b. Its archive's SHA-256 is **compared** with the recorded value (`f3a0119292df8855e4435da8130bee1873b0e6a60c306accf81a08ea687bba3c` for `verus-0.2026.11.02.8bf68ee-x86-linux.zip`, from the spike's run; check it again on the first run), and the job fails on a mismatch. The unpacked directory is cached by version.
- **Toolchain:** the Rust toolchain Verus names (1.99.1), pinned in the job with the same action as the others. It moves only with a Verus upgrade.
- **The proof:** `sh verify/run.sh`, which must report every function verified and no error.
- **The vacuity guard:** the job applies mutation m1 (a second `Held` mapped to `Write`) to a copy in the job's workspace, runs the proof, and **requires it to fail**, then restores the file. A Verus that proved everything would turn the job red.

## 3. The record (REQ-TEST-018's "what every proof carries")

In `repository-guards.md`, a section for this proof:
- the property it states (P1 to P3, and the part of P4 that is proved, under the hypothesis RFC 016 decision 4 names);
- the mutations that fail it: m1 to m3, x1 to x4, x6;
- the two that survive and the tests that hold them instead: x5, the first attempt waiting, held by `req_conf_008_held_then_released_writes_after_the_wait`; and an attempt added outside the `Held` branch, held by `req_conf_008_free_lock_writes_without_waiting`. **Run this second mutation once and record that it survives;**
- its run time;
- the job's pin and hash.

## 4. Tests and gate

The seven existing tests of `dacl_wait` pass unchanged. The usual gate, plus `cargo publish --workspace --dry-run` and `cargo package --list` showing no `verify/` entry; `Cargo.lock` unchanged. CI 9 of 9 with skips, plus the new job, whose log shows the pass and the vacuity guard's required failure.

## 5. Return

`.git-exclude/review-requests/016-verus-proof/README.md`. History linear. After the merge, the branch `S3-verus-spike` and PR #40 can go: the owner decides on remote branches.
