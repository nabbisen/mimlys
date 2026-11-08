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
/// LIM-I6: rendered approval request, in characters.
pub const LIM_I6_APPROVAL_CHARS: usize = 4000;
/// LIM-I7: entries returned by `list_dir` or `search_files`.
pub const LIM_I7_ENTRIES: usize = 500;
/// LIM-C1: context budget per model request, in characters.
pub const LIM_C1_CONTEXT_CHARS: usize = 32_000;
/// LIM-C1: configurable range in characters.
pub const LIM_C1_CONTEXT_CHARS_RANGE: (usize, usize) = (4000, 1_000_000);
/// LIM-C3: recent steps kept in full under compression.
pub const LIM_C3_RECENT_STEPS: usize = 6;
/// LIM-C4: summary of a compressed step, in characters.
pub const LIM_C4_SUMMARY_CHARS: usize = 300;
/// LIM-C2: observation excerpt head, in characters.
pub const LIM_C2_EXCERPT_HEAD_CHARS: usize = 3000;
/// LIM-C2: observation excerpt tail, in characters.
pub const LIM_C2_EXCERPT_TAIL_CHARS: usize = 1000;
/// LIM-C2: ceiling of an excerpt a tool may be asked for, in characters.
pub const LIM_C2_EXCERPT_CEILING_CHARS: usize = 64000;
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
/// LIM-O1: time to exit after a stop signal, in milliseconds.
pub const LIM_O1_STOP_MS: i64 = 5_000;
/// LIM-W1: workspace size in bytes (1 GiB), best effort.
pub const LIM_W1_WORKSPACE_BYTES: u64 = 1024 * 1024 * 1024;
/// LIM-W1 range.
pub const LIM_W1_WORKSPACE_BYTES_RANGE: (u64, u64) = (64 * 1024 * 1024, 64 * 1024 * 1024 * 1024);
/// LIM-W2: files in a workspace.
pub const LIM_W2_WORKSPACE_FILES: u64 = 200_000;
/// LIM-P3: snapshot retention after a task is terminal, in days.
pub const LIM_P3_RETENTION_DAYS: u32 = 30;
/// LIM-P3 range.
pub const LIM_P3_RETENTION_DAYS_RANGE: (u32, u32) = (1, 365);
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
pub const LIM_S4_OUTPUT_BYTES_RANGE: (u64, u64) = (64 * 1024, 64 * 1024 * 1024);
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
/// LIM-O5: deadline of an external (MCP) tool invocation, in milliseconds.
pub const LIM_O5_EXTERNAL_DEADLINE_MS: i64 = 60_000;
/// LIM-O5: configurable range in milliseconds; never below a server's declared maximum.
pub const LIM_O5_EXTERNAL_DEADLINE_MS_RANGE: (i64, i64) = (1000, 600_000);
/// LIM-O6: startup of an MCP server, in milliseconds.
pub const LIM_O6_SERVER_STARTUP_MS: i64 = 15_000;
/// LIM-O6: configurable range in milliseconds.
pub const LIM_O6_SERVER_STARTUP_MS_RANGE: (i64, i64) = (1000, 120_000);
/// LIM-O8: the wait for each `xcrun --find <tool>` of the warming (one call
/// per listed tool) to fill the cache before the worker starts (macOS, an
/// Xcode selected, the cache absent), in milliseconds. Past it the start goes
/// on and `check` and `status` say the cache is missing (external design
/// §13.3).
pub const LIM_O8_XCRUN_CACHE_MS: u64 = 10_000;
/// LIM-O2: time until a cancelled execution's tree is terminated, in milliseconds.
pub const LIM_O2_CANCEL_MS: u64 = 2_000;
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
/// Shortest secret value the redactor scans for (REQ-CTX-007): shorter
/// values would redact noise, so the configuration refuses them.
pub const MIN_SECRET_CHARS: usize = 8;
