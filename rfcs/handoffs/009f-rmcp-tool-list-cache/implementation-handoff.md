# Handoff 009f — Can a stale tool list reach mimlys? (`rmcp`'s client cache; release 0.10)

**RFC.** [RFC 009](../../done/009-mcp-client-and-tool-pinning.md) (pinning) · **Specification.** REQ-MCP-002 (discovery and pinning), THR-25 (definition drift) · **From.** Handoff 009e §2 point 4: `rmcp` 3.9.0's client keeps a response cache for `tools/list`, keyed on the `ttlMs` and `cacheScope` hints, with `serve_stale_on_error` **on by default**.

**The question, first, before any change.** mimlys pins the definitions it lists at task creation, and re-checks them at step boundaries and at call time (REQ-MCP-002). If `list_tools` can return a cached list when the server's answer failed, mimlys could pin or keep definitions the server no longer serves, while believing it had just listed them. That is the drift THR-25 guards against, entering through the SDK.

1. **Read and measure.** Read the code path mimlys uses to list tools (`crates/mimlys-mcp/src/client.rs` and what it calls in `rmcp` 3.9.0). Say whether that path can reach the cache and the stale-on-error branch, with the evidence. Use a stand-in server that sends `ttlMs` and then fails a later `tools/list`, and see what mimlys receives.
2. **If it can:** make a failed listing a failed listing. Turn the cache off, or turn stale-serving off, through the SDK's settings, so that a server that cannot list is `unavailable` with its reason. Test it with the stand-in, and record a mutation (the setting restored): the test fails. If the SDK offers no such setting, stop and report; I decide.
3. **If it cannot:** say so, with the evidence, and pin it with a test that would fail if a later change of the call made it reachable.

No other change. Return: `.git-exclude/review-requests/009f-rmcp-tool-list-cache/README.md`.
