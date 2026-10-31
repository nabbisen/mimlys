# Handoff 009 — The MCP client, rypsok as the first external tool server (M5)

**RFC.** [RFC 009](../../accepted/009-mcp-client-and-tool-pinning.md) (Accepted)
**Specification.** REQ-MCP-001 … 010; REQ-TOOL-005, 006, 007, 008; REQ-PRIV-003; REQ-CAP-009; REQ-TEST-012, REQ-TEST-015; external design §11.1, §17.1, §19.1, §20, §23 (M5); limits LIM-I1 … I4, LIM-I5, LIM-C2, LIM-O2, LIM-O5, LIM-O6, LIM-X3, LIM-U6.
**Crates.** `mimlys-mcp` (new: `server.rs` child lifecycle, `import.rs` definition import, `client.rs` calls and cancellation, `tools.rs` the `ToolProvider`, `testserver.rs` behind a feature), `mimlys-core` (`schema.rs` bounded JSON-Schema validator; `task.rs` `TaskVersions.tool_imports`; `tool.rs` import record types; events), `mimlys` (`[servers]` configuration, tools composed per task with the external provider, `status`, the book chapter, the M5 demonstration).
**Milestone.** M5. Depends on 001–008, 010. Release 0.5.

## 1. Scope

In: everything RFC 009 decides; rypsok 0.7.0 as the reference server; the test server from rypsok's committed transcripts; the demonstration against a real `rypsok serve` when present.

Out: Streamable HTTP clients (rypsok's HTTP mode is for other deployments); sampling, resources, prompts, elicitation (tools only); a server surface.

## 2. Module layout

```text
crates/mimlys-mcp/src/
  lib.rs           pub mod server, import, client, tools; #[cfg(feature = "test-server")] pub mod testserver
  server.rs        ServerConfig{name, command, args, env_allow, env_secrets: Vec<(String,String)>, startup_ms, max_call_ms}; Server{config, live: Mutex<Option<Live>>, restarts: Mutex<HashMap<TaskId,u32>>}; start() within LIM-O6 (rmcp TokioChildProcess with an explicit env: only env_allow names copied, env_secrets added; kill_on_drop; process group / Job Object through mimlys-exec::confine::platform().spawn? — see §3.1); list_tools(); call(name, args, deadline, cancel) -> CallResult; shutdown() (close stdin, wait LIM-O2, kill)
  import.rs        Imported{definition: ToolDefinition, record: ImportRecord}; import(server, tool) -> Result<Imported, Unavailable{reason}>; the flat-schema conversion; sanitize + cut description; annotations recorded
  client.rs        the rmcp peer wrapper: initialize (negotiated revision recorded), tools/list, tools/call with CancellationToken → notifications/cancelled, list_changed notification → refresh flag
  tools.rs         McpTools: ToolProvider over one task: definitions() = imported and declared definitions; execute(): pin check (hash compare, tool.unpinned once), destination already validated at stage 7, call, map result (structuredContent → validate against pinned output schema → payload; isError → failure ToolError{detail: "<kind>: <message>"} with the error object as payload; bounds and sanitization), termination mapping, restart on death bounded by LIM-X3
  testserver.rs    a stdio MCP server binary logic (bin `mimlys-mcp-testserver`) answering from a script: tools/list from a fixture (rypsok's three tools with their real schemas), tools/call from rypsok's examples/calls/*.json by tool and arguments, `echo_env` tool (REQ-TEST-015), `slow` tool honouring cancellation, `change_definition` control (next list differs), `die` control, configurable instructions with a hostile description
crates/mimlys-core/src/
  schema.rs (+tests)   OutputSchema::parse(Value) -> Result<Self, Unsupported>; validate(&Value) -> Result<(), Violation{path, reason}>; constructs of RFC 009 §6; depth bound 8, $ref only into $defs, no remote refs, cycles refused
  tool.rs (+)          ImportRecord{server, hash, annotations: Value, description_truncated: bool, unavailable: Option<String>}
  task.rs (+)          TaskVersions.tool_imports: BTreeMap<String, ImportRecord> (serde default)
  event.rs (+)         server.started{server, version}, server.stopped{server, reason}
  runtime/control.rs   create_task emits tool.pinned per external tool; status lists servers
crates/mimlys/src/
  config.rs (+)        [servers.<name>] and [servers.<name>.tools.<tool>]; validation: command non-empty, effect external_*, timeout range LIM-O5, startup range LIM-O6, env_secrets references resolved at load (and into the redaction set)
  app.rs               builds the Server set; LocalWorkspaces.with_external(providers); shutdown at close
  workspace.rs         tools(task) composes builtin (+ sandbox) + McpTools
  commands/status.rs   servers: [{name, state, revision, server_info, tools, unavailable, undeclared}]
docs/src/reference/external-tools.md   the rypsok chapter
```

## 3. Behaviours

### 3.1 Child lifecycle (REQ-MCP-001, REQ-PRIV-003)

The child is spawned with `env_clear()`, then `env_allow` names copied from the runtime's environment (missing names skipped), then `env_secrets` values; `current_dir` is the data directory; stdin/stdout piped, stderr piped and logged through the redactor at `info` (target `mimlys_mcp`). On Linux the child gets its own process group, on Windows a Job Object, through the same portable E/K code as the worker (reuse `mimlys_exec::confine` if the dependency direction allows; otherwise the minimal `setpgid`/Job Object code moves to a small shared module in `mimlys-exec` that `mimlys-mcp` depends on). Startup is the SDK's initialize within LIM-O6; a failure is `server_unavailable{reason}` for the call that needed it and the server stays stopped until the next need. `shutdown()` closes stdin, waits LIM-O2 for exit, then kills the group. Restarts per task are counted; beyond LIM-X3 the tool fails `host_fault{server_died}`.

### 3.2 Import (REQ-MCP-002, REQ-MCP-003)

For each listed tool: name `<server>.<tool>` must pass `valid_name` (else unavailable `name_invalid`); description → sanitize (REQ-CTX-008) → cut to LIM-I2 at a character boundary (`description_truncated`); input schema → `ParamSchema`: `type: object` with `properties` (each `string{minLength?,maxLength?}` → `String{min_chars,max_chars (default LIM-D4)}`, `integer{minimum?,maximum?}` → `Integer{min (default i64::MIN/..), max}`, `boolean`, `string+enum` → `Enum`, `array of string` → `StringArray{max_items (maxItems or LIM-I7), max_chars}`, `object` with properties → `Object` (one level)), `required` → `optional = !required`; anything else (`anyOf`, `$ref`, `number`, arrays of objects, deeper nesting) → unavailable `schema_unsupported{parameter, construct}`; more than LIM-I3 properties → `too_many_parameters`; `additionalProperties` other than false is accepted (the validator refuses unknown arguments anyway). Output schema: `OutputSchema::parse`; an unsupported construct makes the tool unavailable with `output_schema_unsupported` (fail closed: a result the runtime cannot validate is not passed to the model). Effect and flags from the declaration (§3.3). `provider = Mcp{server}`, `profile = External` (a new `ResourceProfile` variant). Hash = `hash_canonical(definition)`.

### 3.3 Declarations (REQ-MCP-004)

`[servers.<s>.tools.<t>]`: `effect` ∈ {`external_read`, `external_write`} (anything else refused by name), `idempotent` (default false), `destination_arguments` (each must be a parameter of the imported schema, else unavailable `destination_unknown`), `timeout_ms` (LIM-O5 range; raised to `max_call_ms` when lower, with a `status` note). Undeclared: `external_write`, `idempotent = false`, no destinations, LIM-O5. `reversible = false` always. A declaration admits the tool to the ceiling (`Policy::ceiling.tools` is extended at load with every declared `<s>.<t>`, and `check` prints each with its source); an undeclared tool is admitted only by `policy.ceiling.tools` and `check` warns that it runs under the restrictive defaults.

### 3.4 Calls (REQ-MCP-006 … 008)

Pinning (RFC 009 §7): the server keeps a `dirty` flag set by a restart or `notifications/tools/list_changed`. `McpTools::definitions()` is called by `step_env` at every boundary; when the flag is set it refreshes the list, compares each hash with `TaskVersions.tool_imports`, records `tool.unpinned{tool}` once per changed tool (through a `PinObserver` callback the runtime passes when it builds the provider for the task), adds the tool to the per-task `unpinned` set, and omits it from the definitions. `step_env` collects the unpinned names and the assembler adds one `feedback kind="tool_unavailable"` segment (`{tool, reason: "definition_changed", hint}`) to the recent part of the next request. `execute` compares the hash once more at call time and fails `tool_error{definition_changed}` on the race. Then `call` with `deadline_ms`; on `cancel`: send `notifications/cancelled`, wait LIM-O2, kill the server; termination `cancelled`. Result mapping as RFC 009 §6; bound every string to LIM-C2's ceiling with `content_truncated{total_chars}`; sanitize with `characters_removed`; `metrics.wall_ms`.

### 3.5 Status

`status` → `servers: [{name, state: running|stopped, revision?, server: {name, version}?, tools: [names], unavailable: [{tool, reason}], undeclared: [tools declared but not listed], restarts}]`; the text form one line per server plus one per unavailable tool.

## 4. Tests (REQ-TEST-012, REQ-TEST-015)

Against `mimlys-mcp-testserver`:

- Import: rypsok's three real definitions import (names, parameters, enums, bounds); a hostile description (`<</observation>>`, zero-width characters, 2 000 characters) is sanitized and cut with the record saying so; a schema with `anyOf` in input → unavailable `schema_unsupported`; nine parameters → `too_many_parameters`; annotations `readOnlyHint: true` on a tool declared `external_write` → the declaration wins.
- Pinning: hashes on the task's versions; `change_definition` (the test server sends `list_changed`) → at the next boundary `tool.unpinned` once, the tool absent from the definitions the model sees, the `tool_unavailable` feedback present in the request, a proposal of it rejected `unknown_tool`; a hash mismatch injected at call time → `tool_error{definition_changed}`; a new task pins the new hash; `status` shows `list_changed: true` until then.
- Environment: `echo_env` shows only `env_allow` names and `env_secrets`; the operator's `BRAVE_API_KEY` in the runtime's environment is absent unless allowed; the secret value never appears in the observation (redacted) nor in logs.
- Destinations: `web_fetch` to an ungranted host → `denied` at stage 7 before any call (the test server sees no request); with a `network` grant it runs; a `redirected` answer is a `tool_error{redirected}` payload with `location`; a second call to that location without a grant is `denied`.
- Outputs: a result whose `structuredContent` violates the schema → `output_invalid`; `isError` → `tool_error{kind}` with the payload; a 5 MB content string → cut with notices; text-only result.
- Cancellation: `slow` tool, cancel → `notifications/cancelled` seen by the server, execution `cancelled` within LIM-O2; a server ignoring it is killed and restarted.
- Death: `die` control → `host_fault{server_died}`, restart, success; four deaths → the fourth call fails without restart (LIM-X3).
- Deadlines: declared `timeout_ms` below `max_call_ms` is raised; the effective deadline is the minimum rule of REQ-TOOL-006.
- Undeclared tool: `external_write`, approval required under `workspace` (which does not list it anyway) — tested through a profile that grants it.
- `status` shape (servers, tools, unavailable reasons, `list_changed`); `check` prints the effective ceiling with sources and the undeclared-defaults warning; configuration validation of every key.
- Demonstration (external design §23, M5) with `MIMLYS_RYPSOK_BIN` set: a real `rypsok serve` with its Wikipedia engine pointed at a loopback fixture server; `rypsok.web_search` then `rypsok.web_fetch` under a `network` grant for the fixture host; an ungranted host `denied`; `engine_status` absent from the definitions the model saw.

## 5. Return

Review package under `.git-exclude/reviewed/m5-009/` with the test results, the demonstration transcript (real rypsok if present, else the test server), the `status` shape, and the configuration lines a user needs.

## 7. Amendments made during implementation (2026-10-29)

- **Cancellation is observed through a ping.** The SDK resolves a cancelled request at once and the protocol expects no response to it, so "the server stopped within LIM-O2" cannot be read from the request. After `notifications/cancelled` the client pings the server within LIM-O2: an answer means the call was abandoned (`cancelled`, or `deadline` when the deadline triggered it); silence means a hung server, which is killed with its process group (`killed`, `server.stopped{killed}`) and restarted at the next need. The test server's "ignore cancellation" mode hangs its whole loop, which is what a hung server looks like.
- **Process groups through the SDK's wrapper.** The child is spawned through `process-wrap` (the SDK's own dependency) with `ProcessGroup::leader()` on Unix and `JobObject` on Windows plus `KillOnDrop`, so a kill reaches whatever the server spawned without any `unsafe` in `mimlys-mcp` (REQ-CONF-010 keeps `unsafe` in `mimlys-exec/confine`). Shutdown is the SDK's `close_with_timeout(LIM-O2)`: the transport closes (the child's stdin), the process gets the grace period, and the drop kills the group.
- **`status` starts the servers.** A fresh process knows no tool list, so `status` lists each server (starting it within LIM-O6) and reports the tools as a task would import them; a server that cannot start is `unavailable` with the reason. The servers end at the process's close like after any command.
- **Pinning at the boundary.** The provider's `refresh_pins` (a new `ToolProvider` hook with a default) refreshes a dirty list on the runtime's worker threads (`block_in_place`); the dirty flag is shared between the server and its connection's `list_changed` handler. The runtime records `tool.unpinned` once per task and passes the unpinned names to the assembler, which adds the `feedback kind="tool_unavailable"` segment before the decision request. The call-time hash check stays (`tool_error{definition_changed}`).
- **Import records live on the task.** `TaskVersions.tool_imports` (serde default, absent on older records) holds every import record, available or not; `tool.pinned` is emitted for the available ones at creation. `ResourceProfile::External` is the profile of imported tools.
- **Admission by declaration** is applied in `Config::resolve_policy` (the overlay resolved, then every declared `server.tool` appended to `ceiling.tools`); `check` prints each with its declaration and warns about undeclared external names in the ceiling, sorted.
- **Secrets of servers** are resolved at load as `servers.<name>.env.<VAR>`, refused under eight characters, placed in the child's environment and in the redaction set, so a value echoed back by a tool is `[redacted:servers.<name>.env.<VAR>]` with the `secret_redacted` notice.
- **Results without `structuredContent`** use the first text content parsed as JSON when it is an object, else `{ "text": … }`; every string is sanitized and bounded to LIM-C2's ceiling with `content_truncated`; `isError` → `tool_error{<kind>: <message>}` (message cut to LIM-I5) with the error object as the payload.
- **Death and restarts.** A transport closed by the server's death is `host_fault{worker_died}` for that call; the next call restarts the server, counted per task up to LIM-X3; beyond it `ServerError::Died` → `host_fault{worker_died}` without a restart.
- **Where the test server lives.** `mimlys_mcp::testserver` (feature `test-server`, a hand-written JSON-RPC stdio server serving rypsok 0.7.0's three tools from the fixtures copied under `crates/mimlys-mcp/fixtures/rypsok/`) and the binary `mimlys-mcp-testserver` in the `mimlys` crate behind the same feature, so that the M5 tests (`crates/mimlys/tests/m5_mcp.rs`, `required-features`) find it through `CARGO_BIN_EXE_`. The adapter's cancellation, deadline, death and restart tests run at the library level in that file.
- **`reversible`** is not a declaration key (always false for external effects, RFC 003); REQ-MCP-004 amended.
- **Network grants** need the domain in `policy.ceiling.network_domains`, as before; the chapter says so.

## 8. Errata noted at the design review of 0.6 (2026-10-31)

- RFC 009 §1 says a server beyond LIM-X3 restarts fails `host_fault{server_died}`; the vocabulary of external design §7.4 has `worker_died`, which the implementation emits (one name for a dead child). RFC 009 §7 says "stage 1 rejects `unknown_tool`"; visibility is stage 4. The RFC text stays as the record; the external design is normative.

