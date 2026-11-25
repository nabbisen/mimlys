# Handoff 015d — Release candidate 0.13.0

**Scope.** `ROADMAP.md`, "Release 0.13". Its three phases are landed on `main`:
- **phase 1:** 015a with 015a-5 (several backends, checked locality, the fitness probe), 004e with 004e-5 (readers of newer records, a closed pipe), and 015b to 015b-5 (plan first);
- **phase 2:** 015-0, the M8 report;
- **phase 3:** 015c to 015c-7, the effort levels.

**Before the candidate:** 013f (the registry's tests under load) lands first. The candidate is cut from `main` after it.

**The owner authorized preparing the release on 2026-11-26.** The tag, the publish and the GitHub release are the architect's, after the review and the owner's final word.

**Model.** Handoff 013e (0.12.0). Nothing is private, so the candidate branch is pushed for CI as usual.

**The machine's limit:** the owner's limit for this machine holds for every local run: nice 19, the cores you are given, and the thermal guard (wait while above 85 C until 80 C or lower). Read the temperature in its own command, before the build starts.

## 1. Versions and the CHANGELOG

- **Versions:** `0.12.0` on `main`; make every crate `0.13.0`, with every inter-crate pin and `Cargo.lock` agreeing.
- **The section:** `## [0.13.0] - <date of the green run>` holds "Unreleased".
- **`### Security` leads.** Keep its four entries as they are assessed:
  - the three REQ-SEC-010 entries are **hardening**, not fixes of a vulnerability of 0.12.0 or earlier;
  - `url` 2.5.9 is **not reachable**.

  Then one sentence: **no vulnerability of 0.12.0 is fixed by 0.13.0**, unless a review file of 0.13 names one. If you find one, say which, and do not invent one.
- **"Changed" leads with what an upgrading user meets first,** as it does now. Check that these come first:
  - a task is not created when its backend does not answer;
  - `config_version` 2, with 1 read through 0.14;
  - a non-loopback URL must be declared `remote`;
  - a redirect is not followed;
  - the profile's rule checked at every start.
- **The versions sentence:** `CONFIG_VERSION` 1 to 2; `SCHEMA_VERSION` 4, `EVENT_SCHEMA_VERSION` 2, `ACTION_SCHEMA_VERSION` 2, `RECORD_VERSION` 1 and the worker's `PROTOCOL_VERSION` 2 unchanged. Check each against the code at the candidate's head.
- **A 0.12.0 binary on a store that 0.13.0 wrote:** check it as 013c did, and write down what happens. It fails on a task that holds a new word (`wait_kind: plan`, `plan_required`), as 004e found. The notes must say so: going back from 0.13 is safe for tasks without those words, and 0.13's own readers tolerate newer words from now on.

## 2. Documents

- **`README.md`:** the release line and "Upgrading from 0.12.0" (`config_version` 2, locality, the levels, plan first).
- **ROADMAP's status line:** 0.13.0 released. Its milestone table says M8 is in 0.13.
- **RFC 015 stays in `accepted/`:** part two (escalation, and self-review) is not shipped. Its Status line says that part one shipped in 0.13.0.
- **The book's getting-started version sentence,** and every page a review file of 0.13 names.
- **The threat register's review row for 0.13 is the architect's;** leave it.

## 3. The gate

On the branch, under the machine's limit:
- fmt; clippy on Linux, and for macOS and Windows with the C compiler stubbed;
- `cargo test --workspace --all-features --no-fail-fast`, the signal tests in the foreground and alone;
- `cargo deny check`;
- the scripts;
- `mdbook build docs`;
- `cargo publish --workspace --dry-run` at 0.13.0 **on a clean registry cache**.

Then CI on the draft PR, every job, with skips counted (and the delegated step's clause lines on the Ubuntu job), and the `interpreters` workflow.

## 4. The release notes

`release-notes-0.13.0.md`, beside the request, built from the CHANGELOG:
- **Read this first:**
  - `config_version` 2;
  - a URL off this machine must be declared `remote`;
  - a task is not created when its engine does not answer;
  - going back to 0.12.0 (§1).
- **Install.**
- **What is new:** effort levels (`--effort`; `quick` stops a wandering task sooner; `deep` is the larger model you name, refused until configured); several backends; `mimlys model probe`; plan first; `approve --note`; what a task cost in `task show`.
- **Changed, if you script mimlys:** the JSON fields, `status --json`'s deprecated `backend`, `check --json`'s `config.backends`.
- **Platforms.**
- **Known limitations:**
  - the probe says whether a model acts, never whether it finishes, and the engine matters as much as the model (the M8 report: one 14B file completes on Ollama and not on a CPU `llama-server`);
  - one consumer GPU holds one model, so a level on another model pays a swap;
  - the residuals of the threat register that still stand.

No security advisory, as for 0.10.0 to 0.12.0, unless you think the owner's reason no longer holds.

## 5. Return

`.git-exclude/review-requests/015d-release-0.13.0/README.md`, with the notes beside it. It holds:
- the branch's head and its commits;
- the CI runs;
- the gate's results;
- the dry run's tail;
- the 0.12.0 binary's result on a 0.13 store;
- anything you could not do.

The commands that push to `main`, tag, publish and create the release are not yours.
