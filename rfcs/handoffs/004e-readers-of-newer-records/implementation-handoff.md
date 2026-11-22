# Handoff 004e — Readers that tolerate a newer release's records; output to a closed pipe (release 0.13, phase 1)

**RFC.** [RFC 004](../../done/004-persistence-checkpoints-and-recovery.md) (records), [RFC 010](../../done/010-cli-and-operator-interface.md) (the output). **From.** The scope of 0.12, §8 (both items carried), brought into 0.13 by the owner's decision of 2026-11-21 (`ROADMAP.md`, "Release 0.13", phase 1).

**Specification** (written before this handoff):
- REQ-MAIN-006 and REQ-CLI-009 (new);
- external design §22, "A reader meets a newer release's word";
- the common rule of `rfcs/handoffs/README.md` on unknown variants, amended for store readers.

**Why now:**
- 0.13 adds words (the wait kind `plan`, the rejection `plan_required`, `halt_detail`), and 0.14 (the daemon) adds the most.
- A reader of 0.12.0 already fails on one: 0.11.1's `events` failed with `storage failure: deserialize` on 0.12's `task.admission_changed`.
- The freeze needs older readers to stay usable.

**Embargo:** none.

## Today

These are as found on `main` at `d1a3ff0`; check them on today's `main`.

- **Every record is read through one function,** `parse` (`mimlys-persist/src/store.rs:38`), into the typed value. A field it does not know is ignored, because no record type sets `deny_unknown_fields`. A **value** it does not know (an event name, a state, a wait kind, a reason) fails the whole read as `storage failure: deserialize`, and the command with it.
- **`task list` reads the state as text** (`list_tasks`) and survives an unknown state.
- **`task show`, `events`, `metrics`, `status`'s task lines, `task output`,** and every writer that loads the task, do not.
- **The common rule of every handoff says** "unknown variants are deserialization errors". It is right for input from outside (configuration, the model, a tool) and wrong for a record that a newer release of mimlys wrote.
- **The CLI's output goes through `println!`** (`output.rs:26`, `:46`, `:54`). `println!` panics when its reader has gone (`mimlys events <task> | head`), and Rust ignores `SIGPIPE`, so the write fails with `EPIPE`.

## Items

1. **A reader shows what it does not know (REQ-MAIN-006).**
   - **Find every value a store reader parses** that a newer release could extend, and list them in the request:
     - event names;
     - task states;
     - wait kinds;
     - pending items;
     - halt, failure and rejection reasons;
     - action statuses and outcomes;
     - effects;
     - the rest you find.
   - **For each,** a read keeps the record. The unknown word is held by its name and its raw fields, marked unknown to this release. The listing goes on.
   - **What each reader shows:**
     - `events` shows such an event by its name and fields;
     - `task show` shows the state or reason by its name, with `(unknown to this release)`;
     - `metrics` counts it by its name;
     - `status` lists the task;
     - `task output` works for the task's actions that it can read.
   - **`--json`** keeps the field as written, and adds what marks it unknown. Propose the shape. It must be one rule for every kind of value, not one per field.
   - **Design it once,** in the persistence layer or the core types, not per command.
   - **Keep the typed value** for every known word: the twelve stages, the runtime and the writers must never see an "unknown" state as if it were a known one.
2. **A writer refuses a task it cannot read whole.**
   - **The commands:** any command that would change or run such a task (`task resume`, `approve`, `reject`, `answer`, `grant`, `revoke`, `rollback`, `promote`, `task delete`, `task cancel`, `pause`, recovery at the start).
   - **The refusal** names the word and says that the release that wrote it is needed.
   - **Recovery at the start** leaves such a task as it is and says so once. It must not halt it, rewrite it or fail the start.
   - **`task delete` and `prune`:** say whether either may still remove such a task, which the user may want after going back a release. Propose; do not decide alone.
3. **Output to a closed pipe ends quietly (REQ-CLI-009).**
   - **One output path:** every line to standard output goes through a write whose error is handled. A broken pipe ends the command quietly, with the exit code its work earned so far (0 for a reader). It is never a panic, and never a "broken pipe" message on standard error.
   - **Check `events --follow | head`:** it must also end, rather than follow forever into a closed pipe.
   - **Windows' closed pipe** (`ERROR_NO_DATA`) is the same case.
   - **`eprintln!`:** leave it on standard error, but say what you find.
4. **The rule in the handoffs' README:** the common rule is amended with this handoff (see that file). Make the code match it.
5. **A flake seen in 015a-5, of the same family:** `mimlys-persist`'s `reader::tests::req_pers_013_a_task_found_recovering_is_recovery_pending` failed once: `recovery_pending()` was false because the lock probe found the lock held.
   - **The likely cause:** a child forked by another test of the binary holds a duplicate of the lock file's descriptor, and so the lock, until it execs. That crate's tests spawn processes at `found_recovering_tests.rs:429` and `store/tests.rs:906`.
   - **Confirm it** (a storm, as 015a-5's item 9 did), then fix it the way that keeps the test's meaning. Say how.
6. **The book:**
   - `getting-started.md` or `storage.md`: going back a release (what an older mimlys shows, and what it refuses);
   - `cli.md`: the unknown marker in text and in `--json`.

## Tests (by state, §3c)

- **A store with a record of each kind of unknown value,** written by the test as a newer release would (raw SQL into a store made by this release):
  - `events`, `task show`, `task list`, `metrics`, `status` and `task output` succeed and show the word with its marker, in text and in `--json`;
  - every other record of the store reads as before.
- **Each writer of item 2:**
  - refused on that task, naming the word;
  - the store unchanged byte for byte in its rows;
  - a start with such a task recovers the others and leaves this one.
- **A 0.12 store's records** read exactly as before. No marker appears anywhere.
- **`mimlys events <task> | head -1`, `task show | head -1`, `status | head -1`** and a `--json` command: each exits with its code, with no panic and nothing on standard error. `events --follow | head -1` ends. Use a pipe closed by the test, on Unix and on Windows.
- **The flake of item 5:** reproduced before the fix and not after, with the counts.

## Mutations

Each of these must be killed:
- an unknown event fails `events` again;
- an unknown state reaches a writer as a known one (for example, `waiting_user` without its kind);
- a writer acts on such a task;
- recovery halts such a task;
- `println!` restored on one path;
- `events --follow` goes on after the pipe closes.

## Book and CHANGELOG

- **The book:** item 6.
- **CHANGELOG:**
  - **Changed:** an older reader is no longer the limit, from 0.13 on: a 0.13 reader shows a 0.14 word by name. Say plainly that 0.12.0 and earlier still fail as before.
  - **Fixed:** a command whose output was piped into `head` panicked.

## Return

`.git-exclude/review-requests/004e-readers-of-newer-records/README.md`, with:
- the PRs and their CI runs;
- the inventory of item 1;
- the proposed `--json` shape;
- the answer on `delete` and `prune`;
- the mutations;
- the flake's evidence;
- the design questions.

## After review: 004e-5

The review of 2026-11-23 (`.git-exclude/reviewed/handoff-004e/README.md`) approved parts A and B (#195 to #198) and landed them as one linear series (CI of PR #199). Three small items remain before 015b.

**Specification** (amended with the landing): external design §22, the shape of a newer word in text and JSON, the writers' refusal, and free text.

### Items

1. **The free-string test passes by key order.**
   - **What survived:** my mutation (`find_words` takes the first place that holds the word, with no sentinel check) survived `mimlys-core`'s and `mimlys-persist`'s `newer` tests.
   - **Why:** `serde_json`'s maps are sorted, and in `req_main_006_the_place_named_is_the_vocabularys_not_a_free_string` the vocabulary field (`code`) sorts before the free one (`reason`).
   - **The test to add:** a record whose free string sorts *before* its vocabulary field, for example `task.admission_changed` with `reason` equal to an unknown `state` word. Add one in a list as well, if a record has one.
   - **Then kill the mutation.**
2. **Standard error never panics either (part B, Q3).**
   - **The change:** one handled write for standard error, beside `put`. Every `eprintln!` of the `mimlys` crate goes through it.
   - **When standard error is closed,** the command ends with the exit code it earned (1 for an error line, not 101), with no panic.
   - **Test:** `2>&1 | head -1`, and standard error closed from the start, on a command that writes an error and on one that writes a warning.
3. **A guard, so that no direct print comes back (part B, Q2).**
   - **The guard:** deny `clippy::print_stdout` and `clippy::print_stderr` in the `mimlys` crate's library and binary, with `allow` in its test binaries.
   - **Direct handles:** add `disallowed_methods` for `std::io::stdout` and `std::io::stderr` outside the output module and the prompt's terminal, if clippy can scope it cleanly. If it cannot, say so, and the two lints are enough.
   - **Show it works:** the guard fails on one reintroduced `println!` and one `eprintln!`.

### Answers to the other questions

**Part A:**
- **Q1, the JSON shape:** accepted, and written into §22.
- **Q2, `task delete` and `prune` refuse such a task:** accepted. The release that wrote it deletes it; the narrow allowance is not worth its rules.
- **Q3, any record of the task, events included, refuses a writer:** accepted. A writer never acts on a task it cannot read whole.
- **Q4, said at every start:** accepted.
- **Q5, error kind `state`:** accepted.
- **Q6, free text not marked:** accepted for 0.13, and stated in §22. Whether such fields should be typed is carried to the contract's inventory before the freeze.
- **Q7, Q9 and Q10:** accepted.
- **Q8, the method relies on serde's message:** accepted. It is pinned by the tests and fails loudly, never by reading wrong.

**Part B:**
- **Q1, a writer goes on to the end of its task with its output dropped:** accepted. The task's integrity comes before its output.
- **Q4, `events --follow` on macOS and Windows ends at the next write:** accepted, and stated in the book. No `unsafe` outside `confine/`.
- **Q5, a failure that is not a pipe ends with 1 and one line:** accepted.

### Return

`.git-exclude/review-requests/004e-5-after-review/README.md`.
