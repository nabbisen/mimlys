# The hostile corpus (REQ-SEC-013, REQ-TEST-017)

One directory of cases, one harness: `crates/mimlys/tests/hostile_corpus.rs` reads every `*.json` file under the kind directories below, runs it against the production function its `check` field names, compares the outcome with the file's `expect`, and fails naming the case file. It is part of `cargo test`. **A case added later is a file; the harness is not edited.** (A new `check`, a new production function to attack, is a change to the harness; see the end.)

The harness lives in the `mimlys` crate because it is the one crate that depends on all the others: core (decisions, paths, the step loop, the markers), exec (the built-in tools, the sandbox) and mcp (the import) are all public there, so one test can drive them without a dependency cycle or a copy of a fixture.

## Kinds

| Directory | Holds | Checks |
|---|---|---|
| `decisions/` | hostile model output, as the strict parser, the loop and the approval display take it | `decision.parse`, `decision.approval_display`, `decision.approval_bounded`, `loop.reject`, `loop.authority_rejections`, `loop.authority_approval` |
| `paths/` | hostile paths: traversal, absolute and Windows forms, case and normalization, the existence leak, symbolic links | `path.canonicalize`, `path.relation`, `decision.validate`, `fs.symlinks` |
| `tool-results/` | hostile output of a tool, before the model reads it | `result.sanitize`, `result.segment`, `loop.tool_result` |
| `mcp-descriptions/` | a hostile description from an external server | `mcp.import_description` |
| `modules/` | hostile WebAssembly modules | `sandbox.module` |

The harness fails if a kind has no case, if a check has no case, if a file in a kind directory is not a `.json` file, if a case names a check that does not belong in its directory, and if a case does not parse.

## A case file

A JSON object. Every case has:

- `check` (string), the production function the case runs against (the table above, and below).
- `note` (string), one sentence: what the case shows.
- `origin` (string, optional), the test the case replaced, if it replaced one.

and the fields of its check, which are all required unless said otherwise. A field that is not known is an error, and a field that is missing is an error: **deleting an expected-outcome field makes the case fail, it never makes it weaker.**

Text and bytes. An input (`input`, `inputs[]`, `module`, `description`, `stdout`) is one of:

- a **string**: that text, as UTF-8 bytes;
- an **object**: that JSON value, serialized compactly (a decision written as an object);
- an **array of parts**: a string, `{"repeat": "text", "times": N}`, `{"hex": "ff fe"}`, `{"hex_repeat": "ff", "times": N}` or `{"closing_marker": "observation"}` (the production marker line of that label). `N` is a number, or a named limit with an optional offset: `"LIM_D1_DECISION_BYTES"`, `"LIM_D1_DECISION_BYTES+1"`, `"LIM_I6_APPROVAL_CHARS*2"` (the limits the harness knows: D1, I6, I2, C1 and C2's ceiling).

Hostile characters are written as JSON escapes (`​`, `‮`, `\u0000`, `󠁁`), so that a reviewer sees them.

## Checks and their expectations

- `decision.parse`: `input`, `expect`: `{"malformed": "<problem>"}` (rejected `malformed`; `not_object`, `trailing_text`, `multiple_values`, `duplicate_key`, `too_large`), `{"accepted": {"kind": "complete"}}`, or `{"accepted_act": {"tool": "<name exactly as written>"}}`. Runs `decision::parse_strict`.
- `decision.validate`: `inputs` (a list), `expect`: `{"code": "<rejection code>", "identical": bool, "contains": [text], "lacks": [text]}`. Runs `validate::validate` with the tools `read_file` and `git_push` and the permissive hooks; every input must be rejected with the code; `identical` says whether the rejections serialize to the same text.
- `decision.approval_display`: `input`, `expect`: `{"lines": n, "lacks_chars": [text], "contains": [text]}`. Validates the decision against `delete_path` and renders the approval request the approver reads.
- `decision.approval_bounded`: `input`, `expect`: `{"max_chars": N, "ellipsis_on_line": i, "unchanged_line": {"index": i, "value": text}}`. The same, with the rendering bounded by LIM-I6.
- `loop.reject`: `input` (the model's whole reply), `expect`: `{"step_outcome", "task_state", "tool_calls": n, "unwritten": [persistence lines]}`. One step of the real runtime loop with a scripted backend and fake store and tools, under the shipped policy: the reply must be rejected, the task must still run, no tool must have been called and none of the named lines (`authorized`, `running`, ...) may have been written.
- `loop.authority_rejections`: `profile`, `decisions` (a list of objects), `expect`: `{"step_outcomes": [...], "authority_unchanged": bool, "tool_calls": n, "requests": n, "model_view_unchanged": bool, "hidden_from_model": [text]}`. Steps the loop once per decision and compares the task's grants, budgets, deadline, versions and profile before and after.
- `loop.authority_approval`: `policy` (`shipped` or `approval_write_file`), `decisions`, `expect`: `{"step_outcome", "pending_approval": "pending", "tool_calls", "authority_unchanged"}`.
- `path.canonicalize`: `input`, `rules` (`posix` or `windows`), `expect`: `{"error": "<path_error>"}` (`escapes`, `absolute`, `empty`, `control_character`, `invalid_name`) or `{"ok": "<canonical path>"}`. Runs `WorkspacePath::canonicalize_with`.
- `path.relation`: `a`, `b`, `rules`, `expect`: `{"equal": bool, "same_as": {"sensitive", "insensitive"}, "starts_with": {"sensitive", "insensitive"}}` (`a` lies under `b`).
- `fs.symlinks` (Unix only; on another platform the case is skipped and the skip is counted by `skips.log`): `workspace_files`, `outside_files` (maps of path to text), `links` (`{"path", "outside"}`: a link in the workspace to a path in the directory outside it, `""` being that directory), `steps` (`{"tool", "arguments", "expect"}`), each run through the validator and the real built-in tools. A step's `expect` has eight fields, all required: `failed`, `payload_equals` / `payload_differs` (JSON pointer to value), `notices_include`, `entries_include`, `entries_lack_prefix`, `outside_absent`, `outside_present`; a step that expects nothing of one writes it empty (`{}` or `[]`), so that deleting a field fails the case (`missing field`) instead of weakening it.
- `result.sanitize`: `input`, `expect`: `{"clean": text, "removed": n}`. Runs `observation::sanitize`.
- `result.segment`: `label`, `attribute`, `body_field`, `input` (used as the attribute value and as the body field), `expect`: `{"lines", "opening_starts_with", "body_starts_with", "closing_line", "closing_line_count"}`. Runs `context::write_segment`.
- `loop.tool_result`: `decisions`, `tool_result` (`payload_nesting`, `stdout`, `stderr`), `expect`: `{"step_outcomes", "observations", "max_stream_chars", "stdout_lacks_chars", "next_request_chars_at_most"}`. A scripted tool answers with the hostile result; the loop, the store and the next request to the model are measured.
- `mcp.import_description`: `server`, `tool` (`name`, `description`, `input_schema`, `annotations`), `declaration`, `benign_description`, `expect`: `{"description_truncated", "max_description_chars", "description_lacks_chars", "hash_stable", "hash_differs_from_benign"}`. Runs `mimlys_mcp::import::import`.
- `sandbox.module`: `module` (bytes, or WebAssembly text), `expect`: `{"compile": "rejected"}` or `{"compile": "accepted", "run": {"termination": "host_fault", "detail": "invalid_module"}}` (the serialized termination). Compiles with the production sandbox and, when it compiles, runs it.

## Adding a case

1. Choose the kind directory and copy a case of the same check.
2. Change the input and the expectation; write the expectation from what the product must do, not from what it does now. A case that was written from the output of the code it tests proves nothing.
3. Run `cargo test -p mimlys --test hostile_corpus`. A failing case prints `<kind>/<file>.json: <what differed>`.
4. To see that the case can fail, weaken the production check it targets and run it again (repository guards §3a lists how this was done for each kind).

## Not in the corpus

- `crates/mimlys-exec/src/confine/macos/tests.rs`, `rfc_011_a2_strings_are_escaped`: macOS only, and the functions it attacks (`escape`, `text`) are private to the Seatbelt generator.
- `crates/mimlys/tests/m5_mcp.rs`, `m5_output_validation_and_import_reasons`: it drives a real external server process and the `mimlys` binary.

## A new check

A new production function to attack is a new `check`: a struct for its fields and a function in `crates/mimlys/tests/hostile_corpus.rs`, a line in its `match` and in `KINDS`, a paragraph above and a first case. The harness then requires a case of it for ever.
