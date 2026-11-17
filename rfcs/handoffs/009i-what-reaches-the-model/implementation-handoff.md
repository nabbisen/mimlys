# Handoff 009i — What reaches the model is bounded as a whole (release 0.11, phase 2)

**RFC.** [RFC 009](../../done/009-mcp-client-and-tool-pinning.md). **Found by.** The audit before 0.10 (A-26, A-32), the review of 009g, and the re-check of 2026-11-16 (`ROADMAP.md`, "Release 0.11", phase 2).

**Specification** (amended before this handoff):
- external design §11.1, "What reaches the model is bounded as a whole";
- REQ-CTX-005 and REQ-MCP-006.

**Embargo:** none.

## Items

The lines below are as found on `main` at `70ab081`; check them on today's `main`.

1. **A-26: a result is bounded in total.** Today `tools.rs:476` bounds each string at LIM-C2's ceiling (64 000), more than LIM-C1 (32 000) on its own. Strings, keys and notices are unbounded in number (`tools.rs:468, 482, 523, 531`), the payload enters the observation whole (`observation.rs:202`), and the latest step is never compressed (`runtime/context.rs:253-295`).
   - **Bound the serialized payload in total, after the output schema is checked,** so a cut never causes `output_invalid`.
   - **Cut the longest strings first,** so that the JSON stays valid (REQ-CTX-009).
   - **Say each kind of change once per result:** one `content_truncated` and one `characters_removed`, each with its total.
   - **Keep the full bounded result for the user** (REQ-CTX-006, REQ-PERS-005).
   - **Propose the total bound** from what LIM-C1 leaves beside LIM-I1 and the other segments, and I register it as a limit at review.
2. **A-32: the definitions never fail a step.** Parameter names are bounded since 0.10 (LIM-I10), but the number of values in an enumeration is not (`import.rs:327-349`). One admitted tool with 8 parameters of 8 nested fields can exceed LIM-I1 on its own. And LIM-I1 is enforced only when the definitions are rendered (`tool.rs:504-523`), where `loop_.rs:275` turns it into `RuntimeError::Denied` on every step, with the task left `running`.
   - **At import:** render each tool alone, and make it unavailable with a reason (propose the word, for example `definition_too_large`) above a per-tool bound. Bound an enumeration's number of values. Propose both bounds.
   - **At a grant:** refuse a `tool` grant, whether from the command line or a profile at task creation, that would make the visible definitions exceed LIM-I1. Name the tool and the sizes.
   - **As a last guard:** if the visible set exceeds LIM-I1 at a step boundary anyway, halt the task once with a stated reason. Propose it: an existing reason if one fits, otherwise a new one that I register. It must not fail every step.

## Tests (by state, §3c)

- **1:**
  - a test server tool that returns many medium strings and many keys: the observation is within the bound, is valid JSON, carries one notice per kind with totals, and the user's stored copy is whole;
  - a result that conforms before the cut is not `output_invalid` after it.
- **2:**
  - a tool over the per-tool bound is unavailable with the reason;
  - an enumeration over its bound, likewise;
  - granting a tool that would exceed LIM-I1 is refused with the sizes;
  - the guard halts once, and built-in steps are never denied for an MCP tool's size.

## Mutations

- the total bound removed;
- the bound applied before validation;
- one notice per string again;
- no per-tool bound;
- the grant admitted;
- the old per-step `Denied`.

## Book and CHANGELOG

- `external-tools.md` covers the bounds of a result and of a tool's definition, and the grant refusal.
- CHANGELOG: Fixed and Changed (the new words and limits).

Return: `.git-exclude/review-requests/009i-model-context/README.md`.
