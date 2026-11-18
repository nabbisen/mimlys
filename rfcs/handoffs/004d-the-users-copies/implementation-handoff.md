# Handoff 004d — The user's copies (release 0.12, phase 1)

**RFC.** [RFC 004](../../done/004-persistence-checkpoints-and-recovery.md) (blobs), [RFC 010](../../done/010-cli-and-operator-interface.md) (the command). **Found by.** The review of 009i, the re-review of 2026-11-18, and the scope of 0.12 (`ROADMAP.md`, "Release 0.12", phase 1).

**Specification** (amended before this handoff):
- REQ-PERS-005, rewritten as the rule;
- external design §19.2's `task` row (`output`) and §18's `unavailable` row (`detail.component`).

**Owner's decision (2026-11-18):** the command is `mimlys task output <task> [<action>] [--stdout | --stderr | --result]`.

**Embargo:** none.

## Today

These are as found on `main`; check them on today's `main`.

- **No blob is written.** The store has `put_blob`, but nothing calls it, and every execution's `stdout_blob` and `stderr_blob` are `None` (`loop_.rs:1495`). A stream's excerpt (LIM-C2, `observation.rs` `normalize_stream`) is its only copy.
- **`put_blob` refuses more than LIM-S4** (`store.rs:874`, 1 MiB). A host tool's stream may hold LIM-H4 (4 MiB).
- **`full_payload`** (009i) is kept when a result was cut, but no command shows it.
- **Nothing removes a blob file whose record a crash never wrote.** `orphan_blobs` is used by a test only.
- **Of every `unavailable` error, only the permissions refusals and an interpreter's failed download carry `detail.component`** (`app.rs:112-125`).

## Items

1. **A stream's whole text, as a blob.**
   - **When:** a stream whose excerpt is not its whole text is written whole as a blob, and its execution references it (`stdout_blob`, `stderr_blob`). A stream that fits its excerpt writes none: the excerpt is the whole.
   - **What the blob holds:** the text `normalize_stream` makes, decoded, cleaned and redacted, before the cut. It is never the raw bytes, so a secret never reaches the disk.
   - **The bound:** LIM-S4 for a script's stream and LIM-H4 for a host tool's. `put_blob` takes the bound that applies.
   - **The order:** the blob file and its row are written before the execution's record that names them. A crash between them leaves a row nothing names (it is the task's, and goes with it) or a file without a row.
   - **After a crash:** the next start (writers only) removes each file of `<data>/blobs` that has no row.
   - **What already counts blobs** (`task delete`, `prune`, the storage view, the workspace quota if it counts them) keeps doing so. Check each one.
2. **`mimlys task output <task> [<action>] [--stdout | --stderr | --result]`.**
   - **The action:** the task's latest that ran, unless named, and the copy is its latest execution's. Say in the request whether earlier attempts need a way to be shown.
   - **Without a flag,** say what the action has: its result, its standard output and its standard error. For each, its size, whether the model's copy was cut, and the flag that shows it. Say plainly when the action has none of them (an external tool has no streams).
   - **With a flag,** print the copy itself:
     - `--stdout` and `--stderr`: the blob, or the excerpt when nothing was cut, exactly as stored, to standard output;
     - `--result`: the observation's `full_payload`, or its payload when nothing was cut, as JSON.
   - **On a terminal,** a carriage return that ends no line is printed as `\r`, so no text can hide what precedes it on its line. Piped, the output is exact. Every other control character is already removed by the cleaning.
   - **`--json`** gives the same as one object, for example `{task_id, action_id, execution_id, copies: [{kind, bytes, cut, ...}]}` without a flag, and the copy with its kind and size with one. Propose the shape.
   - **Refusals:** an unknown action is `not_found`, and an action that never ran says so.
   - **It is a reader:** served by the read-only open (REQ-PERS-013), so it works while another runtime holds the store.
3. **`detail.component` on every `unavailable`.** List each source of the kind (the backend, the worker, a server, the provider, permissions, download, …) and give each a stable word, the component as the runtime already names it where it does. Propose the list, and I register it at review.

## Tests (by state, §3c)

- **A script and a host tool whose output exceeds its excerpt:** the blob exists, is referenced, holds the whole cleaned text, and holds no planted secret (an `env_secrets` value printed by the tool). A short output writes none.
- **A host tool's stream between LIM-S4 and LIM-H4** is stored whole.
- **`task output`:**
  - the index without a flag;
  - each flag on a cut and an uncut copy;
  - `--result` of an external tool's cut result (the test server's flood) against `full_payload`;
  - a carriage return shown as `\r` on a terminal and exact through a pipe;
  - while another runtime holds the store;
  - an unknown action, and one that never ran.
- **A crash between the blob and the execution's record:** the next start removes a file without a row, and the record is consistent (use the crash points of `req_test_007`).
- **`task delete` and `prune`** free the blobs, and the storage view counts them.
- **`detail.component`** is present on each `unavailable` source.
- **The book:** a `<!-- tested -->` example of `task output` (1.0's condition 4).

## Mutations

Each of these must be killed:
- the blob not written;
- the raw bytes stored instead of the redacted text;
- the bound left at LIM-S4 for a host tool;
- the crash sweep removed;
- `\r` printed raw on a terminal;
- the index or a flag reading the excerpt where a blob exists.

## Book and CHANGELOG

- **The book:**
  - `storage.md`'s `blobs/` row (now true);
  - `external-tools.md`'s sentence on `full_payload` (now with the command);
  - `cli.md` (the row and the JSON);
  - a short section on reading what a tool printed.
- **CHANGELOG:** Added (the command), Changed (`detail.component`), Fixed (REQ-PERS-005 was not met).

## Return

`.git-exclude/review-requests/004d-the-users-copies/README.md`, with:
- the PRs and their CI runs;
- the mutations;
- the proposed JSON shape and component list;
- the design questions.

## After review: 004d-4

The review of 2026-11-19 (`.git-exclude/reviewed/handoff-004d/README.md`) approved 004d-1 to 004d-3 and landed them. Your finding on the worker's frame is a defect of 0.11 and earlier, and the release cannot claim LIM-S4 or LIM-H4 until it is fixed. It is fixed here, before the spike, with three small items from your questions.

1. **The worker's frame carries what the bounds allow.**
   - **Today:** `ExecutionRecord`'s `stdout` and `stderr` are `Vec<u8>`, written by `serde_json` as arrays of numbers (three to four characters a byte), while `MAX_FRAME_BYTES` is `2 * LIM-S4 + 64 KiB` (`protocol.rs:120`), which assumes a byte a byte. So a stream above about 600 KB ends the worker. A configured `sandbox.output_bytes` above its default (the runtime passes it as `--output-bytes`, up to 64 MiB) can never be carried, and LIM-H4 (4 MiB) never could.
   - **The change:**
     - the streams travel encoded at about a byte a byte (base64, or a binary part of the frame: propose one, with its cost measured);
     - `PROTOCOL_VERSION` becomes 2;
     - both sides derive the frame limit from the bounds in force: the configured LIM-S4 and LIM-H4, encoded, twice, plus the record's margin.

     A frame the peer refuses is still `worker_died`, never a hang.
2. **The whole copy is whole.**
   - **The blob's bound:** the bound in force (the configured LIM-S4 passed down to the core, not the constant; LIM-H4 for a host tool), times three. A byte the decoding replaces becomes at most three bytes of U+FFFD, so the captured text is never cut again. This is your question 2.
   - **When the capture itself was cut** (the tool printed more than its bound), say so: the index and the JSON report it (for example `capture_cut: true` with the bytes kept). The worker knows when it stopped capturing; carry that in the record. Propose the field.
3. **`--attempt N`** (your question 5). Default: the latest. The index names the attempts the action has, so that "what did it print when it failed?" has an answer.

**Tests (by state):**
- **Through the real worker:**
  - a script that prints exactly its bound, at the default LIM-S4 and at a configured 4 MiB;
  - a host tool that prints LIM-H4 (Linux, and the Windows and macOS jobs through `m6b` and `m6c`).

  Each is stored whole, the worker lives on, and the task completes.
- **A tool that prints past its bound:** the copy holds the bound and `capture_cut` says so.
- **Invalid bytes up to the bound** are stored whole.
- **`--attempt`** on an action retried once, and an attempt that does not exist.

**Mutations:** each of these must be killed:
- the old encoding;
- the frame limit back to the constant;
- the blob's bound without the factor of three;
- `capture_cut` never set;
- `--attempt` ignored.

**Book and CHANGELOG:**
- **CHANGELOG, Fixed:** a tool that printed more than about 600 KB ended the worker (0.11 and earlier), with the effect on an action that cannot be undone, `unknown_outcome`.
- **External design §13.1's frame sentence:** I wrote it as a known defect, and it becomes the rule.

**Return:** `.git-exclude/review-requests/004d-4-the-users-copies/README.md`.
