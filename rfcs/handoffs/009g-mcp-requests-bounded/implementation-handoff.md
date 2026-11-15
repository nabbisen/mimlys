# Handoff 009g — Every MCP request is bounded; a destination is a string; what a server sends is cleaned (release 0.10)

**RFC.** [RFC 009](../../done/009-mcp-client-and-tool-pinning.md) · **Found by.** The independent audit before 0.10, which audited commit `f5a94fa`. The owner accepted the triage on 2026-11-14.

**Specification** (amended before this handoff):
- external design §11.1: "Every request to a server is bounded" and "A destination is a string";
- REQ-MCP-005.

The rest follows the text as it stands:
- REQ-CTX-007 and REQ-CTX-008 ("before it enters the context or a display");
- §11.1 on a server that cannot be reached at the pin check.

**The defects, verified in the code at `f5a94fa`** (`crates/mimlys-mcp/src/` unless named):
- **A-08, high.** The relist at `server.rs:252` has no timeout, and the tokio `live` mutex is held across it (`:248`). `list_all_tools` (`client.rs:222`; rmcp pages without a limit or a timeout) is reached from:
  - the pin check in `execute` (`tools.rs:518`, before the only cancel poll at `client.rs:288`);
  - `refresh_pins` (`tools.rs:467-469`, which blocks the loop thread);
  - `Servers::shutdown`, which waits on the same mutex (`server.rs:404`).

  So a server that stops answering hangs the step, every other call and the exit.
- **A-11, high.** `mimlys-core/src/hooks.rs:62,67` match only `Value::String`, so an array or object destination skips both the `https` check and the grant check. The import checks only that the parameter exists (`import.rs:109-119`). For an `external_read`, which needs no approval, any URL is reached.
- **A-09.** `mimlys-core/src/schema.rs:164-196` tries every `anyOf`/`oneOf` branch, bounded only by depth, synchronously in `execute` after the call's deadline (`tools.rs:378-380`). A few hundred bytes of schema cost exponential CPU, and a non-recursive `$defs` chain is as bad as a recursive one.
- **A-10.** Parameter names (`import.rs:218`, and the fallback description at `:215`), enum values (`:266-276`) and nested keys (`:332`) enter the definitions unsanitized. Tag characters and bidi overrides reach the prompt; ESC does not, since JSON escapes it.
- **A-13.** These parts are never redacted:
  - (a) `ToolError.detail`, built from the server's message (`tools.rs:358-376`), although it is serialized into the prompt (`mimlys-core/src/runtime/context.rs:705`);
  - (b) object keys (`mimlys-core/src/context.rs:302-305`), which are not sanitized or bounded either (`tools.rs:432-435`);
  - (c) a secret straddling the excerpt's cut, because the cut (`tools.rs:414`) comes before the redaction (`loop_.rs:1329`).
- **A-28.** A server's name, version and revision (`mimlys/src/messages.rs:497-523`), and the unavailable-tool lines with raw tool and parameter names (`:526-528`), are printed raw to the terminal.
- **A-31.** `refresh_pins` maps `Err(Unavailable)` to "changed" (`tools.rs:474-494`). A server that cannot be reached for a moment unpins every one of its tools for the task, and the model is told `definition_changed`. That contradicts §11.1 as amended on 2026-11-14 in the review of 010e-5.

## Items

1. **Every request has a deadline and observes the interruption** (A-08).
   - A listing, and each of its pages, gets LIM-O6; a call gets LIM-O5, as today.
   - Every request observes the step's interruption token.
   - A listing stops at a bound on pages and on tools. Propose the values from what rypsok and the servers we document list, and I register them as limits at review.
   - No lock is held across a wait that is not bounded: the pin check, the call and `shutdown` never queue behind a hung listing.
   - A listing that times out is `tool_unavailable` with its reason; the tools stay pinned (item 7).
2. **A destination is a string** (A-11).
   - A destination parameter whose schema type is not `string` makes the tool unavailable at import, with the reason (shown in `status`).
   - At stage 7, a destination argument present with any other type is `denied` with `destination_not_granted`.
3. **Validation has a budget** (A-09).
   - A count of node visits per validation; past it, the result is `output_invalid` with a detail that says the schema was too costly.
   - A cap on schema size and width at import; past it, the tool is unavailable with the reason.
   - Validation runs off the async executor and within the call's deadline.
4. **Names and values from the server are cleaned** (A-10).
   - A parameter name must match a fixed pattern (`[A-Za-z0-9_.-]`, as a plain name on the approval card) and be at most **64 characters**, as a tool's name is. Otherwise the tool is unavailable (`definition_invalid`, naming the parameter by position). This applies to nested keys too.
   - **Why 64** (review of handoff 003b): no bound on a parameter's name exists today, so the approval card's refusal `arguments_do_not_fit` is reachable. With LIM-I3's 8 parameters, names of up to about 290 characters always fit, so 64 makes the refusal unreachable. Add a test that the longest admitted tool, with 8 parameters of 64 characters, still renders a card.
   - Enum values are sanitized and bounded like descriptions.
   - The fallback description uses the cleaned name.
5. **Redaction covers everything that reaches the context** (A-13 a to c).
   - Redact `cause` (its `detail`) and object keys; sanitize and bound the keys.
   - Redact the whole text before any cut.

   A-13 (d), case and percent-encoded variants, is beyond §15.3 and stays as it is.
6. **What a server sends reaches the terminal cleaned** (A-28). Sanitize every server-supplied string before display: name, version, revision, tool and parameter names, reasons.
7. **An unreachable server keeps its tools pinned** (A-31). In `refresh_pins`, `Err(Unavailable)` skips the server and keeps `dirty`, as `RestartLimitReached` already does. The call then gives `tool_unavailable`.

## Tests (by state, §3c)

- **1:** the test server's new knob that answers the first `tools/list` and never another, after `list_changed`.
  - The next action ends within LIM-O6 as `tool_unavailable`.
  - A stop signal during it is honoured.
  - Another call on the same server is not blocked.
  - The process exits within LIM-O1.
  - A server sending endless pages stops at the bound.

  REQ-MCP-007, REQ-CLI-005.
- **2:** `destination_arguments = ["urls"]` with `urls: array<string>`: the tool is unavailable at import with the reason. A forged array value at stage 7 is denied. REQ-MCP-005.
- **3:** the auditor's schema with N = 6 is refused at import. A schema under the cap but costly on one result ends `output_invalid` within the deadline.
- **4:** a property key with tag characters, and an enum value with a bidi override: the tool is unavailable, or the value is cleaned, as the rule says.
- **5:**
  - a server error echoing a configured secret: the cause in the prompt and in the store holds the redaction;
  - a key equal to a secret: redacted;
  - a secret straddling the cut: no prefix of it survives.

  REQ-CTX-007.
- **6:** a server whose name and version carry `ESC[2J` and an OSC sequence: `status` prints them cleaned.
- **7:** a server unreachable at a step boundary: its tools stay pinned, and no `tool.unpinned` is emitted.

## Mutations

- the listing's deadline removed;
- the lock held across the listing;
- the array destination admitted;
- the visit budget removed;
- `cause` not redacted;
- `Unavailable` counted as changed.

## Book and CHANGELOG

- `external-tools.md` covers destinations, the bounds and what makes a tool unavailable.
- CHANGELOG: Fixed, citing the audit's ids.

Return: `.git-exclude/review-requests/009g-mcp-audit/README.md`.
