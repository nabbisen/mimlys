# Handoff 005b — The interpreter download survives a flaky network; CI no longer depends on one (release 0.8)

**RFC.** [RFC 005](../../done/005-execution-worker-and-sandbox-profile.md) (the sandbox profile and its interpreters) · **Specification.** REQ-SBX-001; external design §12 (the paragraph after the profile table) and §18 (`unavailable`, `component: "download"`); limits LIM-O9, LIM-O10 (new) · **Found.** 011g's follow-up CI, run `42465321601`: five attempts, every failure an HTTP 504 from GitHub's release downloads (the QuickJS artifact in nine tests, mdbook, the cargo-deny container) · **Release.** 0.8.

## 1. Why

`interpreters install` (`crates/mimlys/src/commands/interpreters.rs`, `download`) makes one request with no deadline and no retry, and a failure is reported as `internal`, which tells the user the runtime is at fault. A user on a flaky network sees a bare failure; the CI turns a one-file test change red on eight jobs whenever GitHub's downloads degrade.

## 2. Behaviour (binding: external design §12)

- One download is bounded by **LIM-O9**; a transient failure (no connection, a timeout, HTTP 429 or 5xx) is retried **LIM-O10** times after the waits the register states; any other status is not retried. The SHA-256 is verified before anything is stored, as now.
- A download that still fails is the error kind `unavailable`, `detail.component = "download"`, the message naming the URL, the last status, and the remedy (`mimlys interpreters install <language> --from FILE`). Typed, never recognized by its text.
- One `info` line per retry (the status and the wait).

## 3. The CI (`.github/workflows/ci.yml`)

- The tests that install QuickJS no longer download it themselves on CI. One step per job fetches the pinned artifact (`curl --fail --retry 5 --retry-all-errors`), keyed by its SHA-256 in `actions/cache` (pinned by SHA, as every action), and the tests install it with `--from` when a test-only variable names the file (the tests' helper, not the product). Without the variable (a developer's machine) the tests download as today, through the product's retry.
- `mdbook`: install it so that a transient failure is retried (the pinned release with `curl --retry`, or the action with a retry wrapper); keep the version pinned.
- `cargo-deny`: say what its action does on a failed image pull; if nothing can be configured, say so. Do not replace the action in this handoff.

## 4. Tests

- Unit tests of the retry decision as a pure function (which failures are retried, the number of retries and their waits, the deadline), naming REQ-SBX-001 (pinned WASM-hosted interpreters) and external design §12.
- A test with a local HTTP stand-in that answers 504 twice and then the artifact: installed after two retries; one that answers 404: no retry, `unavailable`/`download`, the remedy named; one that answers 504 every time: `unavailable` after LIM-O10 retries. Mutation: no retry; the first test fails.

## 5. Return

`.git-exclude/review-requests/005b-download/README.md`: the change, the gate, CI per platform with skips, the mutation, and whether the CI ran without any network download of the interpreter.
