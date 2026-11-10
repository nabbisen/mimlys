# Handoff 009c — rypsok 0.9.0: compatibility, measured (release 0.10)

**RFC.** [RFC 009](../../done/009-mcp-client-and-tool-pinning.md) (the MCP client; rypsok as the first server) · **Specification.** REQ-MCP-001 … 009, external design §11.1, §19.1 · **From.** rypsok's letter of 2026-11-11 (`.git-exclude/upstream/rypsok/receive/2026-11-11-to-mimlys-team.md`): rypsok 0.9.0 (2026-11-08), MCP revision 2026-07-28, "several" contract changes since 0.8.0.

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
