# Handoff 013b — Release candidate 0.10.0

**Scope.** `ROADMAP.md`, "Release 0.10". Its "done when" is met on `main` for the code:
- handoffs 011m, 006b (with 006b-5), 003b, 009g and 002e are landed;
- the earlier 0.10 items of the handoff index (013 part one, 008c, 011j, 004b, 012c, 002b, 010b, 009c to 009f, 011k, 016, 011l, 017, 002c, 004c, 010c, 002d, 010d, 010e) should be landed. **Check each one against `main` and list any that is not, or that left an open item** (for example 008c's `sha2` line, which waits for a wasmtime release), in the request's first section.

The threat register's review row for 0.10 is the architect's.

**Model.** Handoff 012b (0.9.0), with what its review corrected: the publish dry run comes after the version bump, on a clean registry cache.

## Part 1 — four fixes before the candidate (public, neutral, pushed as usual)

One PR from `main`. These were found in the reviews of 002e and 009g. None is embargoed text.

1. **The CLI's async runtime runs on a thread with an explicit stack.**
   - On `main`, a debug build already needs about 1,000 KB of the 1,024 KB a Windows main thread has, on the MCP-restarts test path. 002e's first version overflowed it.
   - Run the runtime on a `std::thread::Builder` with a stated stack size (propose the value from a measurement, with a margin of at least 4×), and return its exit code from `main`.
   - Measure the new margin the way you measured the old one (`ulimit -s` cannot limit an explicit stack, so measure the need by shrinking the configured size in a debug build), and state it.
2. **`req_test_007_crash_at_every_point_recovers` must test something.** Its child is named without its module path under `--exact`, so it runs 0 tests.
   - Name it by the full path, as your new crash test does.
   - Make the test assert that the child ran and crashed at each point: no "running 0 tests".
   - If it then fails, that is a finding: report it, and do not loosen the test.
3. **`Unreadable` only for a permission problem.**
   - `WorkspaceError::Unreadable{path}` says "restore its permissions". Map only `PermissionDenied` to it.
   - Any other I/O error of the walk names the path with its own reason, for example `part of the workspace could not be read: <path> (<error kind>)`, and halts the same way, without advice it cannot back.
4. **Small consistency fixes.**
   - The event vocabulary test's title says "thirty-three"; the list holds 36.
   - The comments of the seven constants that "await registration" cite their limits instead (LIM-I10 to LIM-I16; see the patch). The patch is not public yet, so cite the ids only, which reveal nothing.

Tests and mutations as usual (item 2's mutation: the child named without its path again, which must fail). Return: `.git-exclude/review-requests/013b-1-before-the-candidate/README.md`.

## Part 2 — the candidate (local only until the architect says to push)

Prepare a **local** branch `release-0.10.0` from `main` after Part 1 lands. **Do not push it, tag, publish or merge.** The architect reviews it from the local repository, the owner gives the Go, and only then is it pushed for CI and released the same day. So the embargoed text is public for hours, not days.

### 1. The embargoed specification and handoffs land

- Apply `.git-exclude/embargo-0.10/spec-and-index.patch`, as one commit, with the subject "Specification of 0.10's hardening". It holds:
  - the external design, the requirements, the limits and the threat register;
  - ROADMAP;
  - the handoff index rows.
- Move the five handoff folders from `.git-exclude/embargo-0.10/handoffs/` into `rfcs/handoffs/`: `011m-…`, `006b-…`, `003b-…`, `009g-…`, `002e-…`. In each:
  - remove the "Embargoed until 0.10" note;
  - fix the relative links so that they resolve from `rfcs/handoffs/`;
  - keep the "After review" sections.
- This handoff, 013b, stays out of `rfcs/handoffs/` (it is a release task, as 012b's was); add it if you prefer the record, without the note.
- `check-doc-refs.sh` and `check-rfcs.sh` pass.

### 2. Versions and the CHANGELOG

- Workspace version `0.10.0` in `Cargo.toml` and every inter-crate pin. `Cargo.lock` follows by `cargo update -w` only.
- `CHANGELOG.md`: `## [0.10.0] - <date of the green run>` holds `Unreleased` plus every "For the release" text of the five handoffs' review requests (011m, 006b, 006b-5, 003b, 009g, 002e), with the architect's corrections in the review files:
  - the first 006b line, as corrected in 006b-5;
  - the 011m book wording ("where the kernel's Landlock is older than ABI 9").
- **Lead with a `### Security` section**, before Changed. One paragraph per finding that affects 0.9.0, each with its audit id:
  - A-01 and X-02: a confined host tool could start a process outside the confinement through a Unix socket;
  - A-03 and A-04: promotion could delete or leave unrecoverable project files;
  - A-02: the approval card could hide arguments;
  - A-11: an array destination skipped the grant;
  - A-08: an MCP server could hang the runtime;
  - X-01: a tool's processes outlived it.

  End the section with the advice: **upgrade; until then, do not grant host tools or MCP tools to tasks running untrusted code under 0.9.0.**
- **Changed** leads with what an upgrading user meets first:
  - `SCHEMA_VERSION` 4, so 0.9.0 refuses a 0.10 store;
  - `rollback --promotion` asks, so a script must pass `--yes`;
  - below Landlock ABI 9, no Unix socket in a host tool;
  - an MCP parameter name longer than 64 characters makes its tool unavailable.

### 3. Documents

- The book pages that each "For the release" text names (how a task runs, getting started, the reference pages, the confinement and host-tools pages, external tools).
- `README.md` and the ROADMAP status line: 0.10.0 is M1 to M7a, plus the audit's fixes.
- RFC 013 stays in `accepted/`, since its part two is M7b. Its Status line gains "Part one implemented in 0.10.0".
- The threat register's review row for 0.10 is the architect's; leave it.

### 4. The gate

On the local branch:
- fmt;
- clippy on Linux; check-only for macOS and Windows if a toolchain exists, else say so;
- `cargo test --workspace --all-features --no-fail-fast`, signal tests in the foreground;
- `cargo deny check`;
- the scripts;
- `mdbook build docs`;
- `cargo publish --workspace --dry-run` at 0.10.0 **on a clean registry cache**.

The CI and the `interpreters` workflow run after the push, when the architect says.

### 5. The release notes and the advisory

Two texts, beside the request:
- **`release-notes-0.10.0.md`**, built from the CHANGELOG:
  - "Read this first: security", the section above in user words;
  - Install;
  - What is new;
  - Changed, if you script mimlys;
  - Platforms;
  - Known limitations: 0.11's list, by id: A-21 and A-06 on Windows, scheduled.
- **`security-advisory-0.10.0.md`**: a draft of the GitHub security advisory, for the owner to approve and publish:
  - affected `mimlys` ≤ 0.9.0 and fixed in 0.10.0;
  - one entry per finding above: severity, what an attacker needs (a task given host tools or MCP tools that runs untrusted code, or a hostile MCP server), the impact and the workaround;
  - credit to the independent audit.

  No working exploit and no step-by-step path: describe the class and the effect.

### 6. Return

`.git-exclude/review-requests/013b-release-0.10.0/README.md`, with the two texts beside it. It holds:
- the local branch's head, and its commits;
- the files changed;
- the gate's results;
- the dry run's tail;
- anything you could not do.

The commands that push, land, tag, publish and create the release and the advisory are not yours to run.
