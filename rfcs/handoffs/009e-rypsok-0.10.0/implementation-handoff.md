# Handoff 009e — rypsok 0.10.0: compatibility, measured (release 0.10)

**RFC.** [RFC 009](../../done/009-mcp-client-and-tool-pinning.md) · **Specification.** REQ-MCP-001 … 009, external design §11.1 · **From.** rypsok's letter of 2026-11-11 announcing 0.10.0 (`.git-exclude/upstream/rypsok/receive/2026-11-11-0.10.0-released.md`). **Method.** As handoff 009c: rypsok from crates.io into `.git-exclude/tmp/`, run as mimlys runs it, no product change.

**Status (2026-11-11): merged.** rypsok 0.10.0 works with mimlys unchanged: the same import, results, pins and revision; it ends 1 ms after its input closes; the fixture still matches; the book says 0.10.0. The SDK's tool-list cache, noted in §2 point 4, is handoff 009f.

Measure 0.10.0 against mimlys at `main`, and confirm or refute the letter's stdio points:

1. **The download.** Install with `cargo install rypsok --locked --version 0.10.0`. The crate is what we run; the letter's `SHA256SUMS` and `gh attestation verify` concern their release files, so check one binary that way too and say whether it matches, as a data point.
2. **The same matrix as 009c:**
   - the import (each tool admitted, or unavailable with its reason);
   - one `web_search`;
   - one `web_fetch`;
   - an `isError` (`denied`);
   - `redirected`;
   - `engine_status` not granted;
   - the negotiated revision.
3. **Pins.** The letter says no tool definition changed from 0.9.0. Compare the definition hashes mimlys pins under 0.9.0 and 0.10.0. They must be equal; if one differs, name the field.
4. **The `tools/list` cache hints** (`cacheScope`, `ttlMs`, beside the `tools` array, on the 2026-07-28 revision). Show whether mimlys's `rmcp` client sees them on the revision it negotiates, and that they change nothing in the import.
5. **Shutdown.** mimlys closes the server's input first. Confirm 0.10.0 ends by itself within LIM-O16, and how long it took.
6. **`rypsok check`** prints `tool call maxima in ms: …`. Record the line; mimlys does not read it.
7. **The M5 fixture.** Does it still match 0.10.0? Update it in a separate commit if not.

Then the book's version line (`--version 0.9.0`) becomes 0.10.0, in the same branch, if every point holds.

Return: `.git-exclude/review-requests/009e-rypsok-0.10.0/README.md`, transcripts in `evidence/`; scratch removed.
