# Handoff 014d — The daemon on Windows, on-demand start, and the book's page (release 0.14, phase 3)

**RFC.** [RFC 014](../../accepted/014-local-daemon.md), read with its **Amendment 1**, items 4, 5, 10 and 11. **From.** The scope of 0.14 (`ROADMAP.md`, "Release 0.14", phase 3). It is the last handoff of the daemon.

**Specification** (written before this handoff):
- **external design §19.5:** "Windows" (the pipe, its DACL, the client's check of the server, the client's process, the own tree by Job Object) and "On-demand start"; the configuration example's `serve`;
- REQ-SEC-016 and REQ-CONF-010 (the Win32 calls in `mimlys-exec`'s module for the daemon's transport);
- THR-39; REQ-CLI-008.

**Owner's decision (2026-11-27):** the scope of 0.14. Windows is in 0.14. **On-demand start is the cut** if this phase runs long. Say early if you see that coming: Windows is not cut.

**Embargo:** none.

## Today

As found on `main` at `1c87e80`; check them on today's `main`.

- **The daemon serves Linux and macOS.** On Windows every command runs embedded, and `mimlys daemon …` says that Windows has no daemon in this release (`main.rs:768`, `commands/status.rs:622`).
- **`mimlys-exec`'s Windows confinement** already has the Job Object (`AssignProcessToJobObject`, `IsProcessInJob`: `confine/windows.rs`) and the token's SID (`token_sid`: `confine/windows/acl.rs:821`). The MCP servers run in a Job Object of their own (`process_wrap`).
- **The transport's helper** for a peer that has gone (`daemon/socket.rs`, 014c-6) is Unix's; the frames (`mimlys-exec::frame`), the hello, `stop` and the routing are the platform's own only where they touch the socket.
- **The daemon's harness for the tests** (`tests/common/served.rs`) and the CI's served step run on Linux and macOS.
- **Two short idle seams in `daemon_served.rs`** (400 ms at line 647, 300 ms at line 686) may flake as the lifecycle test's did on Rosetta (`main`'s run `46119145949`; the review of 014c-6). Nobody has checked.

## Items

0. **The idle seams decide by state.** Look at the two seams of `daemon_served.rs`. Where a test waits for the daemon to idle out, it waits for the state (the daemon's process gone, its log's line), never for a time chosen to be long enough. Where it relies on a task to keep the daemon busy, say so in the test and check it.
1. **The named pipe** (§19.5, "Windows").
   - **The name:** `\\.\pipe\mimlys-d-<hash>` of the user's SID and the data directory's canonical path.
   - **Creation:** as its first instance (`FILE_FLAG_FIRST_PIPE_INSTANCE`), with a DACL for the user's SID alone, set through `SECURITY_ATTRIBUTES`, never the default.
   - **The client's check:** `GetNamedPipeServerProcessId`, then that process's token user, which must be the client's own SID. Otherwise nothing is sent, and it is refused as §20 says.
   - **The daemon's check:** `GetNamedPipeClientProcessId` for the session's pid, and the own tree by `IsProcessInJob` against the worker's Job Object and each MCP server's. A peer whose process cannot be opened is refused.
   - **Where the code lives:** every Win32 call in a new module of `mimlys-exec`, beside the confinement modules, each `unsafe` block justified in a comment (REQ-CONF-010, amended). The rest of the daemon stays in `mimlys` and forbids `unsafe`.
   - **The macOS helper's equivalent:** say what a pipe whose client has gone answers on Windows, and read it as the peer's end the same way.
2. **The daemon on Windows,** with everything 014c and 014c-5 gave Linux and macOS: the hello and `stop`, the lifecycle (`start` detached as a process with no console, `--foreground`, `stop`, `restart`, `status`), the store's lock, the scheduler and the line, the verified session, the log, `model probe` through the daemon, `--detach`, and `status` in both modes. Remove the "Windows has no daemon" lines.
3. **On-demand start** (§19.5, "On-demand start"; the cut point).
   - **The behaviour:** with `[runtime] serve = "on_demand"`, the first client that finds no daemon starts one, detached, waits for its hello within LIM-O23, says once that it started one (with its pid), and sends its request through it.
   - **Two clients at once:** the store's lock decides, the second daemon exits naming the first, and its client connects to the first.
   - **A failed start** says why, and the command does not fall back to embedded.
   - **On all three platforms.**
4. **The book's page "Running mimlys as a daemon"** (RFC 014's Consequences; the reviews of 014b-5 and 014c).
   - **The two modes side by side:** what each is, how to start, stop and restart a daemon, `serve = "on_demand"`, and that a client that finds none runs embedded.
   - **What differs, said plainly:**
     - **closing the terminal:** in served mode the task goes on; in embedded mode it stops with the process. Ctrl-C pauses in both;
     - **`--detach`** needs a daemon;
     - **a daemon reads its configuration and its secrets when it starts:** an edit or a changed secret is refused until `mimlys daemon restart`.
   - **The line, and every queued reason** across both modes, in one table.
   - **Repeats:** a broken connection is answered with what the store recorded. A repeated creation after `delete --audit` makes a new task (external design §4).
   - **Who may reach a daemon** (the user's own processes, never the daemon's own tree), and the residual of THR-39, in a user's words.
   - **Upgrading:** `mimlys daemon restart` after installing a new release; any client can stop any daemon.
   - **A tested example** of each new command, as the book's other pages have.

## Tests (by state)

- **The served run on Windows:** the CI's served step runs on `windows-latest` too, with its own `not served on windows-latest` line, by name, as on Linux and macOS.
- **The pipe:**
  - a second pipe instance refused (squatting): the client finds the server's owner is not its user and sends nothing. Inject the owner if CI cannot make a second user, as 014c did for the uid;
  - the DACL holds only the user's SID (read it back);
  - a confined tool (the AppContainer) cannot open the pipe;
  - a request from the worker's Job Object, or an MCP server's, is refused and recorded.
- **A second terminal on Windows:** approve, answer and cancel from a second client. A running host tool cancelled from a second client is gone within LIM-O2.
- **On-demand:**
  - the first client starts the daemon and the second uses it;
  - two at once leave one daemon;
  - a start that fails is said, and runs nothing embedded;
  - `manual` (the default) never starts one.
- **The book:** each example is tested, as the others are (`book_examples`).

## Mutations

Run each, and say which test kills it:
- the pipe made without its DACL (the default);
- the client's check of the server's owner removed;
- the own-tree check by Job Object removed;
- on-demand falling back to embedded after a failed start;
- two clients at once leaving two daemons.

## Book and CHANGELOG

- **The book's page,** above, in `SUMMARY.md` beside "How a task runs". The CLI and configuration references gain `serve`, and Windows in the daemon's rows.
- **The CHANGELOG:** the daemon on Windows, and on-demand start, under Added.

## Return

- **PRs on `main`,** a stack if you like, each with its CI. Close throwaway PRs and delete their branches when their result is in the request.
- **The request:** `.git-exclude/review-requests/014d-daemon-windows-on-demand/README.md`, with:
  - the PRs and their CI runs;
  - the mutations, each with what killed it;
  - the choices items 1 and 3 leave to you, each with its reason;
  - the served run on Windows, and what it cannot serve, with why;
  - what is still open, and why;
  - anything in §19.5 that Windows showed to be wrong or unclear. Say it, and do not work around it.
