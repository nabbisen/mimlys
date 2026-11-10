# Handoff 009b — Four advisories against locked dependencies (0.9; first)

**RFC.** [RFC 009](../../done/009-mcp-client-and-tool-pinning.md) (the MCP client, which pins `rmcp`) and RFC 004 (persistence, `rusqlite`) · **Specification.** REQ-SEC-012 (the dependency audit on every change), THR-12 · **Found.** The `dependency audit and licenses` job of PRs #27 to #29 (handoff 012); `main`'s dependencies are the same, so `main` fails it too since the advisories were published.

## 1. What and why

| Advisory | Crate (locked) | Fixed in | Exposure in mimlys |
|---|---|---|---|
| RUSTSEC-2026-0459 | `libsqlite3-sys 0.36.0` (bundled SQLite, through `rusqlite 0.38`) | `>= 0.39.0`, which `rusqlite 0.41` requires | Opening a database whose `-wal` file a less trusted party wrote. The data directory is owner-only and outside every write set; low, but it is the store |
| RUSTSEC-2026-0457 | `rmcp =3.5.0` | `>= 3.7.0` | the OAuth client: not a feature we enable |
| RUSTSEC-2026-0461 | same | `>= 3.6.0` | `StreamableHttpService`: a server; not used |
| RUSTSEC-2026-0462 | same | `>= 3.9.0` | the Streamable HTTP clients: not a feature we enable |

The decision is to **upgrade**, not to ignore. A reasoned ignore would leave the vulnerable code in the build and an expiry to remember. Upgrading removes it, and the audit stays a plain pass.

## 2. Changes

- `rusqlite` to `0.41` (it brings `libsqlite3-sys 0.39`, bundled SQLite at or above 3.54.0). Adapt `mimlys-persist` to any API change. **No change to the schema** (`SCHEMA_VERSION` stays 2), and a store written by 0.8.0 opens and recovers unchanged.
- `rmcp` from `=3.5.0` to `=3.9.0`. Keep the exact pin (why it is exact is in RFC 009's pinning rules; keep the comment) and the same features (`client`, `transport-child-process`). If 3.9 changes behaviour the MCP client relies on (initialization, `tools/list`, `list_changed`, cancellation), adapt and say what in the request.
- `cargo update -p` only for these two and what they require; nothing else moves.

## 3. Tests

- The whole suite, the M5 suite with the rypsok fixture in particular, and `cargo deny check` green.
- A test that a store created by **0.8.0's** `mimlys-persist` (a fixture database with its `-wal`, made with the 0.8.0 tag) opens, passes the integrity check (LIM-P5) and lists its tasks under the new `rusqlite`. It names REQ-PERS-006.

## 4. Return

`.git-exclude/review-requests/009b-advisories/README.md`: the version changes (`Cargo.lock` diff summary), any code change with its reason, the gate, CI 9 of 9 with skips. This lands on `main` before handoff 012's stack, which then rebases on it.
