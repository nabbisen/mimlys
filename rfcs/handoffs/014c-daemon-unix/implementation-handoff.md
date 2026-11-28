# Handoff 014c — The daemon on Linux and macOS (release 0.14, phase 2)

**Status (2026-11-28):** items 0 to 7 landed at `82eb524` (`.git-exclude/reviewed/handoff-014c/README.md`); 014c-5 next.

**RFC.** [RFC 014](../../accepted/014-local-daemon.md), read with its **Amendment 1**, items 4 to 11. **From.** The scope of 0.14 (`ROADMAP.md`, "Release 0.14", phase 2), after 014a and 014b.

**Specification** (written before this handoff):
- **external design §19.5 (new), whole;** §4 (requests, the request id, repeats); §20 (five new rows);
- REQ-ARCH-009, REQ-PERS-014, REQ-SEC-016 (amended: the mutual check, the own tree, the hello, the configuration and secrets), REQ-SCHED-008, REQ-CLI-008 (amended: `restart`, `--foreground`, idle, a dead daemon);
- THR-39 (amended);
- LIM-O13 and LIM-O14 (now design choices), LIM-O20 and LIM-O21 (new: the log).

**Owner's decision (2026-11-27):** the scope of 0.14, as recommended, re-read with the owner's philosophy.

**Purpose.** A task started in one terminal is followed, approved, answered or cancelled from another, on Linux and macOS. Windows and on-demand start are 014d's.

**Embargo:** none.

## Today

As found on `main` at `9244d7b`; check them on today's `main`.

- **The client and its one transport:** `mimlys::client::Client` over `Arc<dyn Transport>`. The `Transport` trait has `send`, and the follower's side (`attach`, `changes`, `failure`, `ended_driver`), and one implementation, `embedded::InProcess`. `Transport::send` cannot say that a connection broke (014b-6's open point 5).
- **Repeats:** `Response::into_outcome` and `Did::into_reply` (014b-7) are the two ways to read an answer, and neither can make a success an error. The deletion's memory of the ids it sent is the client's, in process.
- **The framing:** `write_frame` and `read_frame` in `mimlys-exec/src/protocol.rs` are generic over `Serialize`/`DeserializeOwned` and `Read`/`Write`, with a frame limit. `PROTOCOL_VERSION` and the frame limit are the worker's.
- **The registry's directory** (`registry.rs:235-262`): `$XDG_RUNTIME_DIR/mimlys`, else `$XDG_DATA_HOME/mimlys/run`; on macOS, `~/Library/Application Support/mimlys/run`. Its entries are `Slot` and `Gate`; no daemon is recorded.
- **Nothing of the transport exists:** no Unix socket in product code, no peer credentials, no detaching self-spawn. tokio's `net` feature is declared by no crate. `rustix` (already a dependency) has a safe `getsid(pid)`.
- **The logs** go to standard error only; `[logging]` has `level` and `debug_prompts`.
- **`[runtime]`** has `data_dir` and `admission_slots`.

## Items

0. **Two tests that 014b's reviews left** (first, since they guard what this handoff relies on).
   - **The pinning test asserts the invariant** (the review of 014b-5): a changed tool is unpinned before its next call, and never called under its old pin. It does not assert which boundary sees the notice. rmcp spawns the handler of a notification, so no order on the wire makes the boundary certain.
   - **A failed promotion that kept a path** (the lead's mutation of 014b-7, which survived): the repeat's `detail.kept` equals the first answer's, and is not empty.
1. **The socket, and who may reach it** (§19.5, "Where it is").
   - **The socket:** `d-<hash>.sock` in the registry's directory, mode 0600.
   - **The directory's check, by both ends:** owned by the user, mode 0700, not a link. A path past the platform's limit is refused, saying why.
   - **The mutual peer-credential check:** tokio's `UnixStream::peer_cred`, with the `net` feature. Say what macOS gives for the pid, and what the record says when it gives none.
   - **The own tree refused:** the peer's session id (`rustix::process::getsid`) against the sessions of the daemon's children: the worker, and every MCP server, which already runs in a session of its own. The refusal is recorded. Say how you keep the set of sessions current as servers start and end.
2. **The hello and `stop`** (§19.5, "The hello").
   - **Two frames, length-prefixed JSON.** Reuse the framing by moving `write_frame` and `read_frame` where both crates can use them; do not copy them. Give the daemon's protocol its own frame limit.
   - **The two hellos' shapes,** and the `stop` request, are fixed for every version. Write them down as a test that fails if a field is renamed or removed.
   - **Refused:** another runtime version; a different configuration hash; a secret whose keyed hash differs. Each is said as §20 says it.
3. **The lifecycle** (§19.5, "Lifecycle").
   - **The subcommands:** `mimlys daemon start` (detached: a new session, the standard streams closed, returning when the daemon answers a hello), `--foreground`, `stop`, `restart`, `status`.
   - **What stop and restart do:** `stop` pauses at the boundary and lists; `restart` resumes exactly what its stop paused.
   - **The idle exit:** after LIM-O13 (`[runtime] daemon_idle`, `never` allowed).
   - **A dead daemon:** its socket is removed, and the client runs embedded.
4. **One owner and one scheduler** (REQ-PERS-014, REQ-SCHED-008).
   - **The daemon holds the store's lock,** and its driver owns every client's tasks.
   - **The registry:** clients take no slots, and a daemon entry is recorded (a new kind of entry; the registry's entries are versioned, so say what an older reader does with it).
   - **The queued line:** a task that waits in line says so.
5. **The socket transport behind the same client** (item 3 of 014b).
   - **The transport:** `Transport` gains the way to say that a connection broke.
   - **After a break:** the client reconnects within LIM-O14 and sends the same request once more, with the same id, then answers with what the store recorded.
   - **Following:** the follower reads the store directly, and is told of changes by a frame from the daemon, with LIM-O15's poll as a safety.
   - **The routing:** a command finds the daemon by the socket. Embedded mode is unchanged when there is none.
6. **What the user sees.**
   - **`--detach`** on the commands that follow, refused in embedded mode, saying so.
   - **`status`** says the mode, and, when served, what `daemon status` says first.
   - **The log file** with its rotation (LIM-O20, LIM-O21).
   - **Messages:** each line of §20's new rows.
7. **The `Gone` arms** of the five commands (014b-6), now reachable through a repeat over the socket. Decide what each prints, in text and in `--json`, and test them.

## Tests (by state)

- **The whole CLI suite, served:** every integration test of `mimlys` also runs against a daemon, on Linux and macOS in CI. The harness starts one per test environment, with its own data directory and registry (014a-5's `common::Registry`). A test that cannot be served (one about embedded mode itself) says why, by name, as the skip tier does. **Its outputs are the same as embedded,** byte for byte where it compares outputs.
- **A second terminal:**
  - a task started by one client, approved by a second, answered by a second, and cancelled by a second;
  - **a running host tool cancelled from a second client, with its tree gone within LIM-O2** (014a's deferred test);
  - Ctrl-C in a following client pauses, and closing it leaves the task running.
- **The transport's refusals:**
  - a peer of another uid (inject the credentials if CI cannot make a second user);
  - a directory that is not 0700, or not the user's, or a link;
  - a path too long;
  - another version;
  - another configuration or secret;
  - a request from the daemon's own tree (an MCP test server that runs `mimlys approve`): each refused as §20 says, and nothing sent or applied.
- **Repeats over the socket:** the connection cut after a request was applied and before its answer was read; the client reconnects, repeats once, and answers with the recorded outcome. The five `Gone` arms.
- **The lifecycle:**
  - `stop` with a running task: paused, listed, nothing resumed;
  - `restart` resumes it;
  - idle exit after a short LIM-O13 set for the test;
  - a dead daemon's socket removed, and the command embedded;
  - `daemon status`.
- **The registry:** an older reader (0.13.0's `status`, as 015d did) on a registry with a daemon entry.

## Mutations

Run each, and say which test kills it:
- the peer's uid not checked;
- the directory's mode not checked;
- the own-tree refusal removed;
- the hello's version not compared;
- a changed configuration accepted;
- a repeat after a break applied twice;
- `stop` resuming what it paused.

## Book and CHANGELOG

- **The book:** the page "Running mimlys as a daemon" is 014d's, with Windows and on-demand start. In 014c, the CLI reference's rows for `mimlys daemon …`, `--detach` and `status`, and the configuration reference's `daemon_idle`.
- **The CHANGELOG,** under Added: the daemon on Linux and macOS, opt-in.

## Return

- **PRs on `main`,** as a stack if you like, each with its CI. The CI gains the served run of the suite on Linux and macOS. Close throwaway PRs and delete their branches when their result is in the request.
- **The request:** `.git-exclude/review-requests/014c-daemon-unix/README.md`, with:
  - the PRs and their CI runs;
  - the mutations, each with what killed it;
  - the choices items 1, 2, 4 and 7 leave to you, each with its reason;
  - the served run's results, and the tests that cannot be served, with why;
  - what is still open, and why;
  - anything in §19.5 that the code showed to be wrong or unclear. Say it, and do not work around it.

## After review: 014c-5

The review of 2026-11-28 (`.git-exclude/reviewed/handoff-014c/README.md`) approved the stack #231 to #235 and landed it by fast-forward (`main` = `82eb524`). External design §19.5 now holds what the code showed: three frames, the worker's own session, the directory checked when a socket is in it, and the registry's record as a file of its own kind. **Accepted as you did them:**
- the worker's `setsid`;
- the `daemon.refused` event and LIM-O22 to O24;
- one connection per request;
- `--detach` on `reject`;
- the debug-only seams (checked: each reads `cfg!(debug_assertions)`, and the MCP test server is behind its feature);
- the unreachable `Gone` arms of `grant` and `revoke`.

Six things remain before the release.

1. **A peer with no pid is refused** (your open point; REQ-SEC-016).
   - **What is wrong:** it is served, without the own-tree check, so the check fails open.
   - **Change:** refused, saying that the system gave no process id and the daemon cannot tell its own tree from it. Both supported platforms give one, so nothing works less; a platform that does not fails closed.
2. **`restart` puts the line back** (your open point; §19.5).
   - **What is wrong:** a task that waited in line stays `created` after a restart, and nothing says so.
   - **Change:** `stop` lists the queued tasks with how to resume them. `restart` resumes what its stop paused and puts the queued ones back in line, in their order, each said by name. A daemon that ended any other way leaves them `created`, and `status` lists them.
3. **The CI counts served skips apart** (the lead's point).
   - **What is wrong:** "skipped tests on ubuntu-latest: 46" mixes the served run's `not served` lines with the tier's skips. The baseline that every review reads ("0 except windows-latest's 2") is lost.
   - **Change:** the served step prints its own line, `not served on <runner>: <n>`, with the names, and the `skipped tests on …` line counts the embedded run alone, as before.
4. **`daemon status` counts client processes** (your open point).
   - **What is wrong:** it counts connections, which a follower holds and each request opens.
   - **Change:** it counts client processes, by the peer's pid, with their connections beside them, and says which it counts.
5. **`model probe` runs through the daemon** (your served list: 25 tests "not served").
   - **What is wrong:** with a daemon running, `model probe` is refused for the store's lock. A user would have to stop the daemon to learn whether a model acts.
   - **Change:** the probe is a request the daemon runs, with its class and detail as the answer, as in embedded mode. The 25 tests are then served.
6. **`daemon stop` says what it waits for.**
   - **What is wrong:** it may wait up to LIM-O24 (700 s) for an action that cannot be undone, saying nothing.
   - **Change:** the client says which action of which task it waits for, and until when (its deadline), and says again when it ends.

**Tests:** one for each item, the served run included for item 5. **Mutations:** for items 1, 2 and 5, one each, and say which test kills it.

**Return:** `.git-exclude/review-requests/014c-5-after-review/README.md`.
