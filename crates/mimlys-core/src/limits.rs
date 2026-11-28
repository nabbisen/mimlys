//! Every number of the limits register used by this crate, as a named
//! constant whose doc comment carries the `LIM-` identifier
//! (`docs/src/maintainers/limits-and-defaults.md`). Nothing is written twice.

/// LIM-T1: steps per task, default.
pub const LIM_T1_STEPS: u32 = 40;
/// LIM-T1: steps per task, configurable range (inclusive).
pub const LIM_T1_STEPS_RANGE: (u32, u32) = (1, 1000);
/// LIM-T2: consecutive rejections before halt, default.
pub const LIM_T2_CONSECUTIVE_REJECTIONS: u32 = 3;
/// LIM-T2: configurable range (inclusive).
pub const LIM_T2_CONSECUTIVE_REJECTIONS_RANGE: (u32, u32) = (1, 20);
/// LIM-T3: executions per action (one plus runtime retries), default.
pub const LIM_T3_EXECUTIONS_PER_ACTION: u32 = 2;
/// LIM-T3: configurable range (inclusive).
pub const LIM_T3_EXECUTIONS_PER_ACTION_RANGE: (u32, u32) = (1, 5);
/// LIM-T4: identical consecutive decisions before halt, default.
pub const LIM_T4_IDENTICAL_DECISIONS: u32 = 3;
/// LIM-T4: configurable range (inclusive).
pub const LIM_T4_IDENTICAL_DECISIONS_RANGE: (u32, u32) = (2, 10);
/// LIM-T5: task deadline from creation, default, in milliseconds (2 h).
pub const LIM_T5_DEADLINE_MS: i64 = 2 * 60 * 60 * 1000;
/// LIM-T5: configurable range in milliseconds (1 min to 24 h, inclusive).
pub const LIM_T5_DEADLINE_MS_RANGE: (i64, i64) = (60 * 1000, 24 * 60 * 60 * 1000);
/// LIM-T6: consecutive failed actions before halt, default.
pub const LIM_T6_CONSECUTIVE_FAILURES: u32 = 5;
/// LIM-T6: configurable range (inclusive).
pub const LIM_T6_CONSECUTIVE_FAILURES_RANGE: (u32, u32) = (1, 50);

/// LIM-D1: size of one decision object, in bytes (256 KiB).
pub const LIM_D1_DECISION_BYTES: usize = 256 * 1024;
/// LIM-D2: `reason` of a decision, in characters.
pub const LIM_D2_REASON_CHARS: usize = 500;
/// LIM-D3: plan items.
pub const LIM_D3_PLAN_ITEMS: usize = 20;
/// LIM-D3: characters per plan item.
pub const LIM_D3_PLAN_ITEM_CHARS: usize = 200;
/// LIM-D4: goal, question, answer and summary text, in characters.
pub const LIM_D4_TEXT_CHARS: usize = 4000;
/// LIM-I1: tool definitions shown to the model, all together, in characters.
pub const LIM_I1_TOOL_DEFINITIONS_CHARS: usize = 8000;
/// LIM-I2: description of one tool, in characters.
pub const LIM_I2_TOOL_DESCRIPTION_CHARS: usize = 400;
/// LIM-I3: parameters of one tool.
pub const LIM_I3_TOOL_PARAMETERS: usize = 8;
/// LIM-I4: nesting depth of a tool's input schema.
pub const LIM_I4_SCHEMA_DEPTH: usize = 2;
/// LIM-I5: length of a notice, hint or error message, in characters.
pub const LIM_I5_MESSAGE_CHARS: usize = 200;
/// LIM-I19 (handoff 009i, audit A-26; every tool's since handoff 013c): a
/// tool's result, serialized, as the model reads it, in characters; the last
/// bound of every observation's payload, whatever the provider. LIM-C1 (32 000) less LIM-I1
/// (8 000) leaves 24 000 for everything else; the instructions and capabilities
/// take about 1 700, a goal up to LIM-D4 (4 000) and a plan up to LIM-D3 (20 x
/// 200 = 4 000), so 9 700 at worst, and the latest step is never compressed
/// (external design 15): 8 000 leaves the worst case within the budget by 6 300
/// for the other steps' summaries and the request.
pub const LIM_I19_RESULT_TOTAL_CHARS: usize = 8000;
/// LIM-I20 (handoff 009i, audit A-32): one imported tool's rendering to the model, alone, in characters;
/// past it the tool is unavailable (`definition_too_large`). A quarter of LIM-I1
/// (8 000): the eight built-in tools render to about 4 200 (measured: 2 797 for
/// the six file tools, 756 for `run_script`, 636 for `run_wasm`), which leaves
/// about 3 800 for external tools, so two tools at the bound are the most that
/// can be granted beside them; rypsok's `web_search` is 647.
pub const LIM_I20_TOOL_DEFINITION_CHARS: usize = 2000;
/// LIM-I21 (handoff 009i, audit A-32): values of one enumeration in an imported tool's input schema; past
/// it the tool is unavailable (`too_many_enum_values`). LIM-I14's bound for the
/// lists of an output schema (128), so that one number bounds both; the
/// rendering bound above is what limits the values' size.
pub const LIM_I21_ENUM_VALUES: usize = 128;
/// LIM-I22 (handoff 013c): bytes of one message a server writes on its standard
/// output (one JSON-RPC line), counted as they are read, before the message is
/// parsed: LIM-W3's size, 8 MiB. Past it the message is never held whole, the
/// server is stopped as a server that ended, and the log says why.
pub const LIM_I22_SERVER_MESSAGE_BYTES: usize = 8 * 1024 * 1024;
/// LIM-I6: rendered approval request, in characters.
pub const LIM_I6_APPROVAL_CHARS: usize = 4000;
/// LIM-I7: entries returned by `list_dir` or `search_files`.
pub const LIM_I7_ENTRIES: usize = 500;
/// LIM-I9: a string of a rejection's `detail` (a path, an expected value, a
/// tool name, a refused argument), in characters; a longer one is cut. Proposed
/// by handoff 002c; the limits register does not list it yet.
pub const LIM_I9_DETAIL_CHARS: usize = LIM_I5_MESSAGE_CHARS;
/// LIM-I9: entries of a list in a rejection's `detail` (`allowed`,
/// `visible_tools`); a longer list is cut. Proposed by handoff 002c; the
/// limits register does not list it yet.
pub const LIM_I9_DETAIL_ITEMS: usize = 32;
/// LIM-C1: context budget per model request, in characters.
pub const LIM_C1_CONTEXT_CHARS: usize = 32_000;
/// LIM-C1: configurable range in characters.
pub const LIM_C1_CONTEXT_CHARS_RANGE: (usize, usize) = (4000, 1_000_000);
/// LIM-C3: recent steps kept in full under compression.
pub const LIM_C3_RECENT_STEPS: usize = 6;
/// LIM-C3: configurable range, per effort level (`[effort.<level>] recent_steps`, from 0.13).
pub const LIM_C3_RECENT_STEPS_RANGE: (u32, u32) = (1, 50);
/// LIM-E4: steps of the `quick` level, shipped (`[effort.quick] steps`): twice the longest run of
/// the M4 task that completed (M8 report §3.2).
pub const LIM_E4_QUICK_STEPS: u32 = 22;
/// LIM-E5: deadline of the `quick` level, shipped (`[effort.quick] deadline`), in milliseconds
/// (10 min): twice the slowest completed run of the 7B, rounded up (M8 report §3.2).
pub const LIM_E5_QUICK_DEADLINE_MS: i64 = 10 * 60 * 1000;
/// LIM-C4: summary of a compressed step, in characters.
pub const LIM_C4_SUMMARY_CHARS: usize = 300;
/// LIM-C2: observation excerpt head, in characters.
pub const LIM_C2_EXCERPT_HEAD_CHARS: usize = 3000;
/// LIM-C2: observation excerpt tail, in characters.
pub const LIM_C2_EXCERPT_TAIL_CHARS: usize = 1000;
/// LIM-C2: the range of the observation excerpt, in characters (500 to 8 000).
/// Its upper end is LIM-I19: no excerpt, and no `max_chars` of `read_file`,
/// exceeds what the model reads of a result (handoff 013c; the ceiling was
/// 64 000, twice LIM-C1). No configuration key reads it in 0.11: the excerpt is
/// head plus tail above, and `read_file`'s `max_chars` is at most
/// [`LIM_I19_RESULT_TOTAL_CHARS`].
pub const LIM_C2_EXCERPT_CHARS_RANGE: (usize, usize) = (500, LIM_I19_RESULT_TOTAL_CHARS);
/// LIM-W3: single file written by a tool, in bytes (8 MiB).
pub const LIM_W3_FILE_BYTES: usize = 8 * 1024 * 1024;
/// LIM-O4: deadline of a built-in tool invocation, in milliseconds.
pub const LIM_O4_BUILTIN_DEADLINE_MS: i64 = 30_000;

/// LIM-A1: expiry of an approval request or question, default none.
pub const LIM_A1_EXPIRY_MS: Option<i64> = None;
/// LIM-A1: configurable range in milliseconds (1 min to 7 d, inclusive).
pub const LIM_A1_EXPIRY_MS_RANGE: (i64, i64) = (60 * 1000, 7 * 24 * 60 * 60 * 1000);
/// LIM-A2: pending user items per task.
pub const LIM_A2_PENDING_ITEMS: usize = 1;
/// LIM-W4: materialization exclusions, default.
pub const LIM_W4_MATERIALIZE_EXCLUSIONS: [&str; 5] =
    [".git/", ".hg/", ".svn/", "target/", "node_modules/"];
/// LIM-W5: secret patterns excluded from materialization and reading, default.
pub const LIM_W5_SECRET_PATTERNS: [&str; 8] = [
    ".env", ".env.*", "*.pem", "*.key", "id_*", ".ssh/", ".aws/", ".gnupg/",
];

/// LIM-M1: deadline of one model request, in milliseconds (10 min).
pub const LIM_M1_REQUEST_MS: i64 = 600_000;
/// LIM-M1 range.
pub const LIM_M1_REQUEST_MS_RANGE: (i64, i64) = (10_000, 3_600_000);
/// LIM-M2: maximum completion tokens.
pub const LIM_M2_MAX_TOKENS: u32 = 4096;
/// LIM-M2 range.
pub const LIM_M2_MAX_TOKENS_RANGE: (u32, u32) = (256, 65_536);
/// LIM-X1: tasks in `running` at once (v1 is sequential).
pub const LIM_X1_RUNNING_TASKS: usize = 1;
/// LIM-X2: executions in flight, whole runtime.
pub const LIM_X2_EXECUTIONS: usize = 1;
/// LIM-X5: executions of one user at once across runtimes (`[runtime] admission_slots`,
/// RFC 013 decision 5, M7b report §3).
pub const LIM_X5_ADMISSION_SLOTS: usize = 2;
/// LIM-X5 range.
pub const LIM_X5_ADMISSION_SLOTS_RANGE: (usize, usize) = (1, 16);
/// LIM-O12: model requests in flight per backend endpoint per user (`[backend] max_in_flight`,
/// RFC 013 decision 7).
pub const LIM_O12_MAX_IN_FLIGHT: usize = 1;
/// LIM-O12 range.
pub const LIM_O12_MAX_IN_FLIGHT_RANGE: (usize, usize) = (1, 16);
/// LIM-O11: the bound of one admission-registry operation (take, return, reap), in
/// milliseconds; past it the runtime runs unadmitted and says so (RFC 013 decision 7).
pub const LIM_O11_REGISTRY_MS: u64 = 2_000;
/// LIM-O13 (RFC 014, Amendment 1, item 10): idle time after which a daemon exits, in
/// milliseconds (30 minutes); `[runtime] daemon_idle` sets it from [`LIM_O13_DAEMON_IDLE_RANGE_MS`],
/// or `never`.
pub const LIM_O13_DAEMON_IDLE_MS: u64 = 30 * 60 * 1000;
/// LIM-O13 range: one minute to one week, in milliseconds (`never` is not a number).
pub const LIM_O13_DAEMON_IDLE_RANGE_MS: (u64, u64) = (60 * 1000, 7 * 24 * 60 * 60 * 1000);
/// LIM-O14: the wait for a connection to the daemon, and for its hello, in milliseconds.
pub const LIM_O14_DAEMON_CONNECT_MS: u64 = 2_000;
/// LIM-O20: the size of the daemon's log file before it is rotated, in bytes (10 MiB).
pub const LIM_O20_DAEMON_LOG_BYTES: u64 = 10 * 1024 * 1024;
/// LIM-O21: the rotated copies of the daemon's log that are kept.
pub const LIM_O21_DAEMON_LOG_COPIES: usize = 5;
/// LIM-O22 (014c; a design choice registered with it): the largest frame of the daemon's
/// protocol, in bytes (16 MiB): the largest request is a promotion's plan with its text diffs.
pub const LIM_O22_DAEMON_FRAME_BYTES: usize = 16 * 1024 * 1024;
/// LIM-O23 (014c; a design choice registered with it): the wait of `mimlys daemon start` for the
/// daemon it started to answer a hello, in milliseconds: the daemon opens the store, recovers and
/// probes its worker before it listens.
pub const LIM_O23_DAEMON_START_MS: u64 = 120_000;
/// LIM-O24 (014c; a design choice registered with it): the wait of `mimlys daemon stop` for the
/// daemon's answer, in milliseconds: an action that cannot be undone is waited for, up to its own
/// deadline (REQ-CLI-005), and the daemon then ends its worker.
pub const LIM_O24_DAEMON_STOP_MS: u64 = 700_000;
/// LIM-O1: time to exit after a stop signal, in milliseconds.
pub const LIM_O1_STOP_MS: i64 = 5_000;
/// LIM-W1: workspace size in bytes (1 GiB), best effort.
pub const LIM_W1_WORKSPACE_BYTES: u64 = 1024 * 1024 * 1024;
/// LIM-W1 range.
pub const LIM_W1_WORKSPACE_BYTES_RANGE: (u64, u64) = (64 * 1024 * 1024, 64 * 1024 * 1024 * 1024);
/// LIM-W2: files in a workspace.
pub const LIM_W2_WORKSPACE_FILES: u64 = 200_000;
/// LIM-S1: fuel per execution.
pub const LIM_S1_FUEL: u64 = 10_000_000_000;
/// LIM-S1 range.
pub const LIM_S1_FUEL_RANGE: (u64, u64) = (1_000_000, 1_000_000_000_000);
/// LIM-S2: deadline of one execution, in milliseconds.
pub const LIM_S2_DEADLINE_MS: i64 = 30_000;
/// LIM-S2 range.
pub const LIM_S2_DEADLINE_MS_RANGE: (i64, i64) = (1_000, 600_000);
/// LIM-S3: memory of one instance, in MiB.
pub const LIM_S3_MEMORY_MIB: u32 = 256;
/// LIM-S3 range.
pub const LIM_S3_MEMORY_MIB_RANGE: (u32, u32) = (16, 4096);
/// LIM-S4: captured stdout and stderr, each, in bytes.
pub const LIM_S4_OUTPUT_BYTES: u64 = 1024 * 1024;
/// LIM-S4 range.
pub const LIM_S4_OUTPUT_BYTES_RANGE: (u64, u64) = (64 * 1024, 16 * 1024 * 1024);
/// LIM-S5: size of a module or interpreter, in bytes (64 MiB).
pub const LIM_S5_MODULE_BYTES: u64 = 64 * 1024 * 1024;
/// LIM-S6: clock granularity visible to the sandbox, in milliseconds.
pub const LIM_S6_CLOCK_GRANULARITY_MS: u64 = 1;
/// LIM-S7: epoch tick, in milliseconds.
pub const LIM_S7_EPOCH_TICK_MS: u64 = 10;
/// LIM-X3: worker or MCP server restarts per task.
pub const LIM_X3_RESTARTS: u32 = 3;
/// LIM-X4: worker start, including interpreter compilation, in milliseconds.
pub const LIM_X4_WORKER_START_MS: u64 = 60_000;
/// LIM-H1: deadline of a host tool execution, in milliseconds.
pub const LIM_H1_HOST_DEADLINE_MS: i64 = 600_000;
/// LIM-H1: configurable range in milliseconds.
pub const LIM_H1_HOST_DEADLINE_MS_RANGE: (i64, i64) = (10_000, 3_600_000);
/// LIM-H2: memory of a host tool tree, in bytes (2 GiB).
pub const LIM_H2_HOST_MEMORY_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// LIM-H3: processes in a host tool tree.
pub const LIM_H3_HOST_PROCESSES: u32 = 64;
/// LIM-H4: captured output of a host tool, each stream, in bytes (4 MiB).
pub const LIM_H4_HOST_OUTPUT_BYTES: usize = 4 * 1024 * 1024;
/// LIM-H5: names refused directly under a declared read path, beyond LIM-W5's
/// patterns (which also apply): registry and cloud tokens live beside
/// toolchains (`~/.cargo/credentials.toml`). Patterns follow LIM-W5's rules:
/// a trailing `/` names a directory, `*` matches within one name.
pub const LIM_H5_CREDENTIAL_NAMES: [&str; 6] = [
    "credentials",
    "credentials.*",
    ".netrc",
    ".npmrc",
    ".pypirc",
    ".docker/",
];
/// LIM-H6: interval of the macOS tree tracker's watchdogs (clauses M and C
/// over the sandbox's members), in milliseconds. Provisional (RFC 011
/// Amendment 2, settled): the shorter, the fewer short excursions are missed.
pub const LIM_H6_TRACK_INTERVAL_MS: u64 = 100;
/// LIM-H7: the CPU share of a host tool tree, in percent of the machine's
/// cores (half the cores; at least one core is taken whatever the percent).
pub const LIM_H7_CPU_SHARE_PERCENT: u32 = 50;
/// LIM-H7: configurable range, in percent of the machine's cores (one core to
/// all cores is the register's range; a percent below one core is one core).
pub const LIM_H7_CPU_SHARE_PERCENT_RANGE: (u32, u32) = (1, 100);
/// LIM-O5: deadline of an external (MCP) tool invocation, in milliseconds.
pub const LIM_O5_EXTERNAL_DEADLINE_MS: i64 = 60_000;
/// LIM-O5: configurable range in milliseconds; never below a server's declared maximum.
pub const LIM_O5_EXTERNAL_DEADLINE_MS_RANGE: (i64, i64) = (1000, 600_000);
/// LIM-O6: startup of an MCP server, in milliseconds.
pub const LIM_O6_SERVER_STARTUP_MS: i64 = 15_000;
/// LIM-O6: configurable range in milliseconds.
pub const LIM_O6_SERVER_STARTUP_MS_RANGE: (i64, i64) = (1000, 120_000);
/// LIM-O16: grace for an MCP server to end by itself after its standard
/// input is closed, before its process group is killed (REQ-MCP-001), in
/// milliseconds. rypsok's graceful shutdown takes up to 3 s by default; LIM-O2
/// (2 s) cut it short. A call's cancellation keeps LIM-O2 (REQ-MCP-007).
pub const LIM_O16_SERVER_SHUTDOWN_MS: i64 = 5_000;
/// LIM-O16: configurable range in milliseconds (`[servers.<name>] shutdown_ms`).
pub const LIM_O16_SERVER_SHUTDOWN_MS_RANGE: (i64, i64) = (500, 30_000);
/// LIM-O16: `check` warns when a configured `shutdown_ms` is below this, in
/// milliseconds: few servers finish their own shutdown that fast.
pub const LIM_O16_SERVER_SHUTDOWN_WARN_BELOW_MS: i64 = 1_000;
/// LIM-O17 (proposed by handoff 002d item 2; the architect registers it): how
/// long `events --follow` of a `running` task that no runtime holds waits for
/// one to take the store, in milliseconds, before it says `recovery pending`
/// and ends (REQ-PERS-013). A runtime takes the lock in its first moments (a
/// process start, the configuration read, under a loaded or emulated machine a
/// few seconds); ten polls of LIM-O15. Not configurable.
pub const LIM_O17_FOLLOW_HOLDER_WAIT_MS: u64 = 5_000;
/// LIM-O8: the wait for each `xcrun --find <tool>` of the warming (one call
/// per listed tool) to fill the cache before the worker starts (macOS, an
/// Xcode selected, the cache absent), in milliseconds. Past it the start goes
/// on and `check` and `status` say the cache is missing (external design
/// §13.3).
pub const LIM_O8_XCRUN_CACHE_MS: u64 = 10_000;
/// LIM-O9: deadline of one download of an interpreter artifact
/// (`interpreters install`), in milliseconds.
pub const LIM_O9_DOWNLOAD_MS: u64 = 120_000;
/// LIM-O10: retries of a transient download failure (no connection, a
/// timeout, HTTP 429 or 5xx).
pub const LIM_O10_DOWNLOAD_RETRIES: usize = 3;
/// LIM-O10: the waits before those retries, in milliseconds.
pub const LIM_O10_DOWNLOAD_WAITS_MS: [u64; LIM_O10_DOWNLOAD_RETRIES] = [1_000, 2_000, 4_000];
/// LIM-O2: time until a cancelled execution's tree is terminated, in milliseconds.
pub const LIM_O2_CANCEL_MS: u64 = 2_000;
/// LIM-O19 (handoff 014a-5, item 1): the time a cancel allows the step in flight to write its
/// end once its execution has been terminated, in milliseconds. The cancel's wait for the
/// step is LIM-O2 plus this ([`cancel_wait_ms`]); not configurable.
pub const LIM_O19_CANCEL_WAIT_SLACK_MS: u64 = 3_000;

/// LIM-X6 (handoff 014a-5, item 2; registered at its review): how many of the
/// tasks that have ended a provider of workspaces remembers by id, so that a late request for
/// the tools of one of them is refused and builds nothing. The memory is bounded so that a
/// runtime that lives for many tasks keeps nothing per task; a request later than that many
/// endings is not a thing a caller makes (they all refuse an ended task first). Not configurable.
pub const LIM_X6_ENDED_TASKS_REMEMBERED: usize = 1024;

/// LIM-O19: how long a cancel waits for the step in flight to end, in milliseconds, given
/// LIM-O2: the execution is terminated within LIM-O2, then the step writes its end.
#[must_use]
pub const fn cancel_wait_ms(lim_o2_ms: u64) -> u64 {
    lim_o2_ms.saturating_add(LIM_O19_CANCEL_WAIT_SLACK_MS)
}
/// LIM-O7: wait for another runtime's write of the same directory's
/// permissions (Windows), in milliseconds; at the end of it the write is
/// refused (`acl_not_applied`), never made on a stale base.
pub const LIM_O7_PERMISSION_WRITE_WAIT_MS: u64 = 60_000;
/// LIM-C5: steps of a forked task's history shown as context.
pub const LIM_C5_FORK_HISTORY_STEPS: usize = 20;
/// LIM-M3: transport retries of a model request.
pub const LIM_M3_BACKEND_RETRIES: u32 = 2;
/// LIM-M4: backend health probe timeout, in milliseconds.
pub const LIM_M4_PROBE_MS: i64 = 5000;
/// LIM-E2: steps of the fitness probe's mini-task (REQ-MDL-011). Provisional (RFC 015): the
/// measurement report (015-0) sets it.
pub const LIM_E2_PROBE_STEPS: u32 = 8;
/// LIM-E3: wall time of the fitness probe's mini-task, in milliseconds (REQ-MDL-011). It is the
/// task's deadline, so it is within LIM-T5's range. Provisional (RFC 015): the measurement report
/// (015-0) sets it.
pub const LIM_E3_PROBE_MS: i64 = 120_000;
/// Shortest secret value the redactor scans for (REQ-CTX-007): shorter
/// values would redact noise, so the configuration refuses them.
pub const MIN_SECRET_CHARS: usize = 8;
