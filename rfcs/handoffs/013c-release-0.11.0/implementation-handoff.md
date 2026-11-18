# Handoff 013c — Release candidate 0.11.0

**Scope.** `ROADMAP.md`, "Release 0.11". Phases 1 to 3 are landed on `main` (011n, 009h, 006c with 006c-6, 010f with 010f-4, 009i, 011o with 011o-4, 011p). Part 1 is the last code of the release, and part 2 is the candidate. **The owner authorized the release on 2026-11-18.** The tag, the publish and the GitHub release are the architect's, after the review of part 2.

**Specification** (amended before this handoff):
- the limits register: LIM-C2's range and `read_file`'s `max_chars`, LIM-I19's scope, LIM-I22 (new);
- REQ-CTX-006, REQ-TASK-001, REQ-WS-014, REQ-MCP-001 and REQ-PERS-005 (its 0.11 status);
- external design §8's `read_file` row and §13.5's `prune`.

The book's two untrue sentences about the user's copies (`storage.md`, `external-tools.md`) are corrected already.

**Model.** Handoff 013b (0.10.0), without the embargo: nothing here is private, so the candidate branch is pushed for CI as usual.

## Part 1 — four fixes before the candidate

One PR from `main`, or one per item, as you prefer. Check the lines below on today's `main`.

1. **Every tool's result is bounded as the model reads it (LIM-I19).**
   - **Today:** `read_file` accepts `max_chars` up to LIM-C2's old ceiling, 64 000 (`builtin/read_file.rs:31`), twice LIM-C1, and the result enters the observation whole. `list_dir` and `search_files` return up to LIM-I7 (500) entries, each with a path of any length. An over-budget request is only counted (`context.rs:294`, `over_budget`) and sent all the same.
   - **`read_file`:** `max_chars` at most LIM-I19 (8 000), which is the schema's maximum the model sees. Retire `LIM_C2_EXCERPT_CEILING_CHARS`.
   - **`list_dir` and `search_files`:** bounded in total by LIM-I19, by dropping trailing entries and saying so with their existing `truncated`, never by cutting a path, which would name a file that does not exist.
   - **The last bound, in the runtime:** every observation's payload, from any provider, is bounded by LIM-I19 with 009i's `bound` (move it into `mimlys-core`, where both providers can use it). A tool's own bound comes first, so this one only acts on a tool that forgot. It is said once, as 009i's are (`content_truncated`).
2. **A refused creation leaves no workspace,** whatever refused it.
   - **Today:** after `materialize`, an error from the provider (a contained panic), the store (a full disk) or the clock returns without removing the copy of the scope. Only 009i's definitions refusal cleans up. `task prune` lists tasks only (`commands/prune.rs:195`), so a directory with no task is never found, counted or removed.
   - **The rule:** a guard from the materialization until the task is recorded. On any error, the provider is let go (Windows: an open handle keeps a directory) and the workspace removed. Replace 009i's own cleanup with it.
   - **`task prune`:** its plan lists each directory of `<data>/workspaces` that no task refers to (a plain name, not a link, read without following) as `no task: left by a refused creation`, with its footprint, and removes it under the same confirmation. `--json` gives it a shape of its own.
3. **An MCP server's message is bounded before it is parsed (LIM-I22, 8 MiB).**
   - **Today:** the SDK's codec has no maximum (`JsonRpcMessageCodec::default()`), so a server that writes one endless line costs memory up to the call's deadline (`client.rs:217`).
   - **The change:** a reader between the child's standard output and the transport counts the bytes since the last newline. Past LIM-I22 it ends the stream with an error, so the message is never held whole. The server is then stopped as a server that ended, by today's path for a server that ends during a call, and the log says why at `warn`, naming the server and the bound.
   - **Test it end to end** with the test server (a tool that writes past the bound): the call's outcome is what a server that ends during a call gives today, the runtime's memory does not hold the message, and the next call starts the server again within LIM-X3.
4. **What the CLI says of the user's copies.** Nothing to build, since 0.12 brings the command (ROADMAP). Check that no other page, help text or message says that a stream's whole output or a result's `full_payload` can be read. Correct any that does, as the two book sentences were.

**Tests (by state):**
- `read_file` refuses `max_chars` above 8 000;
- a listing over LIM-I19 is `truncated` with whole entries;
- a fake provider's oversize payload is bounded by the runtime;
- each failure point after materialization leaves no workspace (fakes on every platform; the Windows job for the handle);
- `prune` lists and removes a directory with no task and keeps one a task holds;
- the oversize message of item 3.

**Mutations:** each bound removed, the guard removed, and `prune`'s new listing removed, each killed.

**Return:** `.git-exclude/review-requests/013c-1-before-the-candidate/README.md`.

## Part 2 — the candidate

After part 1 lands, make the branch `release-0.11.0` from `main`. Push it with a draft PR for CI. **Do not tag, publish, merge or create a release.**

### 1. Versions and the CHANGELOG

- Workspace version `0.11.0` in `Cargo.toml` and every inter-crate pin; `Cargo.lock` follows by `cargo update -w` only.
- `## [0.11.0] - <date of the green run>` holds "Unreleased", with the architect's corrections in the review files of 0.11.
- **Lead with `### Security`,** one paragraph per finding that affects 0.10.0, each with its audit id:
  - A-06 on Windows (a host tool of one task reached another task's tree);
  - A-21 on macOS (a tool could outlive its worker);
  - the Windows mask that let a host tool change its files' permissions;
  - the read path over the data directory (011o-4);
  - and the others the review files name as security.

  End it with: **upgrade.**
- **"Changed" leads with what an upgrading user meets first:**
  - Windows's one-time migration at the first start (its time, and what `status` says meanwhile);
  - a read path over the data directory no longer loads;
  - `read_file`'s `max_chars` is at most 8 000;
  - an MCP tool too large to render, or with too many enumeration values, is unavailable;
  - a grant that would exceed LIM-I1 is refused;
  - the new `status --json` and `toolchain` fields.

  Say whether `SCHEMA_VERSION` changed in 0.11, and if it did, that 0.10.0 refuses a 0.11 store.

### 2. Documents

- **`README.md` and ROADMAP's status line:** 0.11.0 is 0.10.0 plus the audit's remainder.
- **The book pages** that the review files name.
- **The threat register's review row for 0.11** is the architect's; leave it.

### 3. The gate

On the branch:
- fmt; clippy on Linux, and for macOS and Windows with the C compiler stubbed, as you did in 011o and 011p;
- `cargo test --workspace --all-features --no-fail-fast`, the signal tests in the foreground;
- `cargo deny check`;
- the scripts;
- `mdbook build docs`;
- `cargo publish --workspace --dry-run` at 0.11.0, **on a clean registry cache**.

Then CI on the draft PR, every job, with skips counted, and the `interpreters` workflow.

### 4. The release notes

`release-notes-0.11.0.md`, beside the request, built from the CHANGELOG:
- "Read this first: security", in user words;
- Install;
- What is new;
- Changed, if you script mimlys;
- Upgrading on Windows (the migration);
- Platforms;
- **Known limitations:**
  - the residuals the threat register states for macOS (the guard's gap, a runtime's crash, the private call);
  - `read_file` over LIM-W3;
  - the user's whole copies, readable from 0.12.

No security advisory: the owner decided for 0.10 that none is needed while mimlys has no user in production, and the CHANGELOG and the notes carry the notice. Say so if you think that no longer holds.

### 5. Return

`.git-exclude/review-requests/013c-release-0.11.0/README.md`, with the notes beside it. It holds:
- the branch's head and its commits;
- the CI runs;
- the gate's results;
- the dry run's tail;
- anything you could not do.
