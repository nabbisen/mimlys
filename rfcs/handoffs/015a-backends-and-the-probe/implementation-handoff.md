# Handoff 015a — Several backends, locality that is checked, and the fitness probe (release 0.13, phase 1)

**RFC.** [RFC 015](../../accepted/015-adaptive-effort.md), decisions 3 and 5, read with its **Amendment 1** (items 2 to 5 and 7). **From.** The scope of 0.13 (`ROADMAP.md`, "Release 0.13", phase 1).

**Specification** (amended before this handoff):
- REQ-CFG-007, REQ-MDL-010, REQ-MDL-011 and REQ-SEC-010, rewritten;
- external design §19.1 (the example, and the validation and REQ-SEC-010 paragraph), §19.2 (the `model probe` row, and `status`'s backends), §19.3 (the engine gate's `max_in_flight`), §20 (four rows) and §22 (`config_version`);
- LIM-M4's note;
- THR-30's and THR-40's controls.

**Owner's decision (2026-11-21):** a level never changes where a task's data goes; locality is checked against the URL; REQ-SEC-010 is checked at every start; nothing is chosen silently.

**Not here:** the effort levels themselves (`[effort.<level>]`, `--effort`, the backend recorded per step) are 015c. Until then every task runs on the backend `default`.

**Embargo:** none.

## Today

These are as found on `main` at `b6c4396`; check them on today's `main`.

- **One backend.** `Config` has `backend: BackendSection` (`config.rs:153`), `CONFIG_VERSION` is 1, and the app holds one `OpenAiBackend`.
- **Locality is only declared.** `BackendSection.locality` (`config.rs:583`) is what the adapter reports (`openai.rs:117`, `:930`). The one validation (`config.rs:1222`) checks `remote` against `remote_backend_allowed`. Nothing compares the declaration with the URL, so `locality = "local"` with a URL on another machine loads.
- **REQ-SEC-010's profile rule is checked once.** It is checked at creation (`control.rs:328`, the only call of `locality()`). `resume_task` (`control.rs:929`) and a recovered task's start do not check it.
- **`status` probes one backend** (`app.rs:576` `probe_backend_of`), with llama.cpp's `/props` and Ollama's `/api/ps` already read for the context (012c). `status --json` has one `backend` object.
- **The engine gate is keyed by URL** (RFC 013 decision 7), with `max_in_flight` read from the one section.

## Items

1. **`[backends.<name>]` and `config_version` 2.**
   - **The table:** each entry has today's keys of `[backend]`. A name is a short identifier (propose the rule: lowercase ASCII, digits, `_` and `-`, at most 32).
   - **Version 1:** a version-1 file is read with its `[backend]` as `backends.default`, and `check` prints one deprecation line that shows the version-2 form.
   - **Refused, each naming the version that form needs:**
     - a version-2 file with `[backend]`;
     - a version-1 file with `[backends]`.
   - **Refused, naming both entries:** two entries on one URL with different `max_in_flight`. Compare URLs as the gate does; say how in the request.
   - **No `default`:** every command that runs steps says plainly that no backend is named `default` and lists the configured ones; it never picks one. `check` says the same. (In 015c, a level that names no backend is refused in that case.)
2. **Locality checked against the URL (REQ-SEC-010).**
   - **The rule:** `local` is allowed only for a loopback host: `localhost`, `127.0.0.0/8` and `::1`, compared as written in the URL, never resolved. Any other host must be `remote`, or the configuration is refused, naming the backend and the host.
   - **`remote` with a loopback URL** is allowed (a tunnel).
   - **The `fake` backend** has no URL and is local.
   - **Test the edges:** `127.1.2.3`, `[::1]`, `localhost.` (with a dot, refused or allowed: propose), `0.0.0.0` (not loopback as a destination: refused for `local`), `::ffff:127.0.0.1`, an IPv6 zone, upper case `LOCALHOST`, and userinfo in the URL.
3. **REQ-SEC-010 at every start.**
   - **What is a start:** `resume_task`, a recovered task's start under `auto_resume`, and `run`'s own start all check the backend's locality against the task's profile before any request is sent.
   - **A refusal** halts the task `policy`, with a line naming the backend and the profile (§20).
   - **Find every start,** and list them in the request.
4. **`status` per backend (REQ-MDL-010).**
   - **The probes run at once,** bounded by one LIM-M4 overall, so `status` with three backends takes no longer than with one.
   - **Per backend, the lines say:**
     - reachable;
     - model listed;
     - resident: Ollama's `/api/ps` (yes or no); always `yes` for a llama-server; `unknown` for another engine;
     - the transport in force, with its reason;
     - the context and its warning (012c's lines, per backend);
     - the last fitness probe, `<class>, measured on this machine on <date>`, or `not probed`.
   - **`--json`:** a `backends` array (each with its `name`). `backend` stays as the `default` entry's view until the release that prepares 1.0, and the CHANGELOG announces it as deprecated. Propose the shape.
   - **The other commands that start a task** print 012c's context warning for `default` only (the backend that runs the task).
5. **`mimlys model probe [<backend>]` (REQ-MDL-011, THR-40).**
   - **The setup:** the runtime makes a scope under `<data_dir>/tmp/probe-<id>/` with one small file.
   - **The task:**
     - a fixed goal that asks for one small edit of that file;
     - a profile of `read_file`, `write_file` and `replace_in_file` (propose the exact set), with no promotion, no host tool, no server and no network;
     - LIM-E2 steps and LIM-E3 of wall time.
   - **The model** is the probed backend's.
   - **The verdict is the runtime's:** after `complete` it reads the file in the probe's workspace. The model's word is never the verdict.
   - **Classes:** `acts`, `incomplete` (with the detail: not finished within the bounds, asked the user, or gave up), `plans_only`, `no_structured_call` or `unreachable`. With each: the steps, the model time per step, the transport, the model and the date.
   - **The record:** written atomically to `<data_dir>/probe-results.json`, one entry per backend (the latest), owner-only like the store's other files. Propose its shape.
   - **Afterwards:** the probe's task, its workspace and its scope are deleted when it ends, whatever the class, crash included. The next writing start removes a probe left by a crash: say how it is found.
   - **Its history:** it writes no event into a user's task history. If the task's own events are written while it runs, they go with the task.
   - **Without a name:**
     - every local backend is probed, one after the other;
     - the remote ones are left out, and the command says which, and how to probe one.
   - **A remote backend** is probed only when named and when `remote_backend_allowed`. The command first says that it sends a fixed mini-task and nothing of the user's.
   - **It is a writer:**
     - refused while another process holds the store, naming the holder (LIM-O3);
     - its model requests take the engine gate, and a stop request ends the gate's wait as for a task (RFC 013 Amendment 1);
     - its tools take no admission slot.
   - **A stop signal** (Ctrl-C) ends the probe and deletes what it made. The class is then not recorded: say so on the terminal.
   - **LIM-E2 (8) and LIM-E3 (120 s) are provisional.** The M8 report (015-0) sets them.
6. **The book:**
   - `configuration.md`: the backends table, version 2, the locality rule with the tunnel case, and the deprecation;
   - `cli.md`: `model probe` and `status`;
   - the model page: "is this model appropriate", answered by the probe;
   - Getting started's configuration example in version 2.

## Tests (by state, §3c)

- **Version 1 and version 2:**
  - a version-1 file loads as `default`, with the deprecation line;
  - each refused mix names the version;
  - a 0.12.0 test configuration loads unchanged.
- **The locality matrix of item 2:**
  - every edge;
  - `local` with a LAN address refused, naming the host;
  - `remote` on loopback allowed.
- **REQ-SEC-010 at a start:**
  - a task created under a local backend, the configuration edited to make it remote under a profile without `external`;
  - `task resume` halts it `policy`, and the mock engine received no request;
  - the same at a recovered start with `auto_resume`.
- **Two entries on one URL** with different `max_in_flight`: refused; with the same value, one gate.
- **`status` with three backends**, one of them not answering: the whole within one LIM-M4, each line right, `resident` for each engine kind (the mock can speak `/props` and `/api/ps`).
- **The probe, against the mock engine scripted per class:**
  - `acts`;
  - `incomplete` (each detail);
  - `plans_only`;
  - `no_structured_call`;
  - `unreachable`.
- **The probe's isolation:**
  - nothing outside `<data_dir>` changes, and the user's tasks and events are untouched;
  - the scope and task are gone after each class, and after a crash at each persistence point (next start);
  - refused while another process holds the store, naming it;
  - a remote backend not probed without a name, and refused when named without `remote_backend_allowed`.
- **The book:** a `<!-- tested -->` example of `model probe` with the fake backend, if the fake can script it. Say if it cannot.

## Mutations

Each of these must be killed:
- the URL check removed (a LAN URL declared `local` loads);
- the start check removed (a resumed task sends to the remote backend);
- the probes run one after another (the LIM-M4 bound broken);
- the probe's verdict taken from the model's `complete`;
- the probe's scope not deleted;
- a remote backend probed without a name;
- `max_in_flight` disagreement accepted.

## Book and CHANGELOG

- **The book:** as item 6.
- **CHANGELOG:**
  - **Added:** several backends; `model probe`; `status` per backend.
  - **Changed:**
    - `config_version` 2, with 1 read through 0.14;
    - a URL that is not loopback must be declared `remote` (a 0.12 configuration that declared one `local` stops loading and says why);
    - the profile's rule is checked at every start;
    - `status --json`'s `backend` is deprecated.
  - **Security:** say whether you assess the locality check as fixing a vulnerability of 0.12.0. My reading is that it is hardening, because the administrator writes the configuration; argue it either way.

## Return

`.git-exclude/review-requests/015a-backends-and-the-probe/README.md`, with:
- the PRs and their CI runs;
- the mutations;
- the proposed shapes: the backend name rule, `status --json`'s `backends`, and `probe-results.json`;
- the list of starts;
- the design questions.

## After review: 015a-5

The review of 2026-11-22 (`.git-exclude/reviewed/handoff-015a/README.md`) approved parts 1 and 2 (#187 to #190) and landed them on `main` as they were tested. Your questions found gaps of the guarantee this handoff builds. They close here, before 004e.

**Specification** (amended with the landing): REQ-SEC-010 and REQ-MDL-011, external design §5.2 (`halt_detail`), §19.2 (the probe's row) and §20 (the redirect row), RFC 015 Amendment 1 (sharpened), and THR-30.

### Items

1. **No redirect is followed by a model backend.**
   - **The change:** the adapter's HTTP client is built with `redirect::Policy::none()`. This covers the decision requests, `/v1/models`, `/props` and `/api/ps`.
   - **What a redirect becomes:** the request fails like an unreachable backend, naming the host the `Location` points to, never the URL (it may carry a password). It is retried as LIM-M3 says, then halts `backend_unavailable`, and `status` shows it on the backend's line.
   - **Test:** the mock answers `307` to a second mock. The second mock receives nothing, and the line names its host.
   - **Look for other clients** that carry a task's content, and say what you find. The interpreter download is verified by hash and is not in scope.
2. **`halt_detail` on a halt `policy`.**
   - **What it holds:** an optional object on the `halted` state of the task's record, in `task show --json` and in the halt's `task.state_changed` event, naming the rule:
     - `remote_backend_forbidden`, with `backend` and `profile`;
     - the completion check's refusal, with your word for it (propose it).
   - **Compatibility:** it is additive and is absent on older records.
   - **Tests:** both causes, through `--json`.
3. **Every command that can drive a step checks for a `default` first.**
   - Derive the set from what can lead to a step (your list of starts, §5 of your request), not from a hand list. `grant` and `revoke` are in it.
   - **Test:** `grant` and `revoke` with no `default` are refused before anything is driven.
4. **One backend's failure never hides the others in `status`.**
   - A refused `schema` mode, an unreadable `fake` script, or any other failure of one entry is said on that entry's line, and the rest are shown.
   - `status` exits 0, as it does with the context warning. The CHANGELOG says so under Changed.
5. **An empty `[backends]` header in a version-1 file is refused,** as the mix is. Detect it from the parsed TOML table, not from the typed section.
6. **The holder is named by every writer** that the store's lock refuses, through the one function `model probe` uses.
7. **The probe's classes** (REQ-MDL-011 as amended):
   - **What counts as acting:** a step counts as acting when its decision was an `act`, accepted or rejected after parsing (`schema`, `denied`, …), not only when an action record was made. A model whose every call to `write_file` is invalid is not `plans_only`.
   - **How it ended:** `incomplete` and `plans_only` both carry how the probe ended:
     - `not_finished`;
     - `not_verified` (new: it said `complete`, and the file was not as asked);
     - `asked_user`;
     - `gave_up`.
   - **`unreachable`** is said, exits 1, and is not recorded, so the backend's last measurement stays. Remove it from the record's classes. The reader shows an `unreachable` written by this branch's earlier builds as unknown, or drops it: say which.
   - **The book and `--json`** follow these changes.
8. **The book** on each of the above.
9. **A test defect of 004d-4, found on `main`'s CI.**
   - **What failed:** `ed_13_1_a_frame_the_runtime_refuses_ends_as_worker_died` failed once on `ubuntu-24.04-arm` (run `45015424621`) with `HostFault{Unavailable}`, which is the worker's spawn failing (`worker.rs:658`), where `WorkerDied` was expected. The same code passed on the PR's run.
   - **The likely cause:** `tests/worker.rs`'s `fake_worker` writes a new executable script per test while other tests of the binary fork. A fork holding the write handle makes the `exec` fail with `ETXTBSY`. That is the hazard `xcrun/tests.rs` and `host_tool/tests.rs` already avoid.
   - **The fix:**
     - confirm the cause first (make the spawn's reason reach the assertion message);
     - move every test of the file that writes a script to execute onto one runner written once, which runs its body as data;
     - search the other test files for the same pattern.

### Answers to the other questions

**Part 1:**
- **Q3, the stricter name rule:** accepted.
- **Q4, `localhost.` and `*.localhost` refused for `local`:** accepted.
- **Q5, the `fake` backend may be declared `remote`:** kept. The book says that its declaration is taken as written, because it sends nothing.
- **Q7, `/api/ps` read once before the probe:** accepted.
- **Q9, version-1 keys named in the version-2 form:** accepted.
- **Q11:** answered by the landing, which is the whole stack at once.

**Part 2:**
- **Q4, exit codes:** accepted (0 for a recorded class; 1 when nothing is measured; a stop exits 0, as a pause does), with `unreachable` now exiting 1 (item 7).
- **Q5 (not measured), Q6 (the scope exception), Q7 (leftovers at every writing start), Q8 (LIM-X1), Q10 and Q11:** accepted.
- **Q12, the book's example:** accepted as it is. A tested example that shows each class is for the release that prepares 1.0 (condition 4).

### Mutations

Each of these must be killed:
- a redirect followed;
- `halt_detail` absent;
- `grant` with no `default` driven;
- one bad backend stopping `status`;
- an empty `[backends]` in version 1 accepted;
- a rejected `act` not counted as acting;
- `unreachable` recorded.

### Return

`.git-exclude/review-requests/015a-5-after-review/README.md`.
