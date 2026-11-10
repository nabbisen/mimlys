# Handoff 009d — What rypsok's reply changes in mimlys (release 0.10)

**RFC.** [RFC 009](../../done/009-mcp-client-and-tool-pinning.md) · **Specification.** REQ-MCP-005 and REQ-MCP-001 (amended 2026-11-11), external design §11.1; limits LIM-O16 (new) · **From.** rypsok's reply of 2026-11-11 (`.git-exclude/upstream/rypsok/receive/2026-11-11-to-mimlys-team-reply.md`), checked by the rypsok team against their 0.8.0 and 0.9.0 binaries. This handoff runs beside 009c: 009c measures 0.9.0; this one changes what the letter showed to be wrong in mimlys whatever 0.9.0 measures.

## 1. An `http` destination is judged in both forms (REQ-MCP-005)

rypsok reads an `http` URL over `https`, on port 443, by default. A grant for port 80 alone therefore admitted a connection to 443 that the user never granted. In the destination check (stage 7, and again on `redirected`):
- an `http` URL is admitted only if the task's grants admit it **and** its `https` form on port 443;
- an `https` URL is judged as today.

The rejection names what to grant (`denied`, reason `destination_not_granted`, with the form that failed), and its fixed hint says to propose the `https` URL or to ask for the grant.

Tests: `http://d/` with a grant of `d:80` only is refused; with `d:443` (or `d` without ports) it is admitted; with `d:80` and `d:443` it is admitted; a `redirected` result to an `http` location is judged the same way. Mutation: the `https` form not checked; the first test fails.

## 2. A server's grace before it is killed (REQ-MCP-001, LIM-O16)

At runtime exit, a server's standard input is closed and, after LIM-O2 (2 s), its group is killed. rypsok's graceful shutdown takes up to 3 s by default. Replace the bound with **LIM-O16** (5 000 ms, range 500 to 30 000), overridable per server as `[servers.<name>] shutdown_ms`; `check` warns when a configured value is below 1 000 ms. Cancellation of a *call* keeps LIM-O2 (REQ-MCP-007), unchanged.

Tests: a scripted server that takes 3 s to exit after its stdin closes ends by itself and is not killed; one that never exits is killed after `shutdown_ms`. Mutation: LIM-O2 used again; the first test fails.

## 3. The book: rypsok run with a configuration file the user owns

In `docs/src/reference/external-tools.md` and the configuration chapter, the rypsok example starts rypsok with its own configuration file (`args = ["serve", "--config", "<path>"]`; check the flag against rypsok 0.9.0's `--help` and use what it accepts). That file:
- sets `fetch.follow_cross_host_redirects = false` explicitly, with one sentence saying why: when `true`, rypsok follows a redirect to another host past mimlys's destination check;
- leaves the deadlines at their defaults, or says how to read them.

`max_call_ms` is taken from `rypsok check --config <path>` (a fetch call lasts at most its deadline plus 1 s: 11 s by default; a search call 2.8 s); the example shows the number and the command. The book's "0.7.0 or later" becomes the version 009c measured.

## 4. Return

`.git-exclude/review-requests/009d-rypsok-letter-changes/README.md`: the change, the gate, CI per platform with skips, the mutations.
