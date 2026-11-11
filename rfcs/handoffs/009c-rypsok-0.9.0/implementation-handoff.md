# Handoff 009c — rypsok 0.9.0: compatibility, measured (release 0.10)

**RFC.** [RFC 009](../../done/009-mcp-client-and-tool-pinning.md) (the MCP client; rypsok as the first server) · **Specification.** REQ-MCP-001 … 009, external design §11.1, §19.1 · **From.** rypsok's letter of 2026-11-11 (`.git-exclude/upstream/rypsok/receive/2026-11-11-introduction-and-three-questions.md`): rypsok 0.9.0 (2026-11-08), MCP revision 2026-07-28, "several" contract changes since 0.8.0.

**Status (2026-11-11, second): merged** (`29b470c`; CI `42929028836`, 9 of 9). rypsok 0.9.0 works with mimlys unchanged: every tool imported, a real search and fetch, `isError` mapped, `redirected` checked again. The fixture follows 0.9.0. The negotiated revision is 2025-11-25 (rmcp offers 2026-07-28; rypsok answers with its latest handshake revision, which is version negotiation working as intended; recorded in RFC 009's Status). Nothing is open here.

**Status (2026-11-11): rypsok's reply adds what to check.** The rypsok team listed the changes from 0.8.0 to 0.9.0 that a stdio client sees (`.git-exclude/upstream/rypsok/receive/2026-11-11-revisions-changes-and-limits.md`): all three tool definitions changed, so pins see a change; `engine_status`'s output schema gained a `oneOf` and an `anyOf` (our validator supports both and local `$ref`: confirm on the real schema); the error vocabulary is unchanged, with new `detail.kind` values. Use that list as item 3's starting point and confirm each item, rather than rebuilding it. Also measure the MCP revision mimlys's `rmcp =3.9.0` negotiates over stdio (rypsok accepts 2024-11-05 to 2025-11-25 in the handshake; 2026-07-28 is served stateless). The changes the letter makes necessary in mimlys are handoff 009d, not this one.

mimlys was built against rypsok's 0.7.0/0.8.0 contract. Measure 0.9.0 against it before anyone relies on the combination.

1. **Install rypsok 0.9.0** from crates.io (`cargo install rypsok --locked --version 0.9.0 --root .git-exclude/tmp/rypsok`), not a downloaded binary. Run it as mimlys runs it: a child over stdio, `env_allow = ["PATH"]`, no Brave key (Wikipedia only), and `web_fetch` with a granted domain.
2. **Against mimlys at `main`:**
   - the import: every tool admitted or unavailable, with its named reason;
   - the output schemas within the bounded validator's subset;
   - the descriptions within LIM-I2;
   - one `web_search` and one `web_fetch` end to end, with the observation as the model would see it;
   - an `isError` answer mapped to `tool_error{<kind>}`;
   - `engine_status` not granted by default;
   - `max_call_ms` against rypsok's stated bounds (search 2.8 s at most, fetch 10 s);
   - the MCP protocol revision negotiated, and what `rmcp =3.9.0` says to 0.9.0's server.
3. **Read rypsok's changelog from 0.8.0 to 0.9.0** and list each contract change with its effect on mimlys: none, a configuration change, a code change, or a book change.
4. **The M5 suite's fixture.** Say whether its scripted server still matches 0.9.0's contract. If it does not, update it in a separate commit and say what changed.

No product change in this handoff: a needed code change comes back as a finding, and I decide it. The network is needed for the end-to-end calls (Wikipedia; one fetched page): say which hosts were contacted.

Return: `.git-exclude/review-requests/009c-rypsok-0.9.0/README.md`, with the transcripts in `evidence/`; remove `.git-exclude/tmp/rypsok` afterwards.
