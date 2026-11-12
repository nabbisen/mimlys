//! The message catalog (REQ-CFG-006, D-17): every user-facing string of the
//! command line lives here, as a constant or a small formatting function,
//! so that another language can be added without touching the commands.
//! Texts are short, name the thing and what to do next (CLR-1 … CLR-3).

use mimlys_core::task::{HaltReason, TaskState, WaitKind};

/// `mimlys check`: the configuration is valid.
pub const CHECK_OK: &str = "configuration ok";
/// The data directory in use.
pub fn data_dir(path: &str) -> String {
    format!("data directory: {path}")
}
/// `status` and the start of a task: the engine's context cannot hold a request
/// (REQ-MDL-003); one line saying what to do for the engine that reported it.
/// `tokens` is per request, so with llama.cpp's `slots` the `-c` that gives
/// every slot enough is the figure times the slots. The run goes on.
pub fn context_too_small_warning(
    engine: Option<mimlys_model::openai::context::Engine>,
    tokens: u64,
    needed: u64,
    slots: Option<u64>,
) -> String {
    use mimlys_model::openai::context::Engine;
    let suggested = needed.next_power_of_two();
    let remedy = match (engine, slots) {
        (Some(Engine::Ollama), _) => format!(
            "set `PARAMETER num_ctx {suggested}` in the model's Modelfile and create the model from it (`ollama create`)"
        ),
        (Some(Engine::LlamaCpp), Some(slots)) if slots > 1 => format!(
            "start llama-server with `-c {}` ({suggested} for each of its {slots} parallel slots), or with `--parallel 1 -c {suggested}`",
            suggested.saturating_mul(slots)
        ),
        (Some(Engine::LlamaCpp), _) => format!("start llama-server with `-c {suggested}`"),
        (None, _) => format!("raise the engine's context length to at least {needed} tokens"),
    };
    format!(
        "warning: the engine's context is {tokens} tokens per request and a request can need about {needed}: {remedy}"
    )
}
/// `check` and `status`: the configured data directory is inside the
/// temporary directory (REQ-WS-001); one line, naming the key and the risk.
pub fn data_dir_in_temp_warning(path: &str) -> String {
    format!(
        "warning: runtime.data_dir ({path}) is inside the temporary directory: a cleaner of temporary files can delete tasks"
    )
}
/// `check` and `status` (macOS, an Xcode selected): the runtime could not make
/// `xcrun`'s cache, so host tools that build with the Xcode fail until it
/// exists (external design §13.3); one line naming the file and the remedy.
pub fn xcrun_cache_missing_warning(file: &str) -> String {
    format!(
        "warning: xcrun's cache {file} is missing and could not be made: host tools that build with Xcode will fail; run `env -i PATH=/usr/bin:/bin /usr/bin/xcrun --find clang` once"
    )
}
/// The configuration file in use, or the defaults.
pub fn config_source(path: Option<&str>) -> String {
    match path {
        Some(p) => format!("configuration: {p}"),
        None => "configuration: built-in defaults (no file found)".to_owned(),
    }
}
/// `debug_prompts` is on (REQ-OBS-004).
pub const DEBUG_PROMPTS_WARNING: &str = "warning: logging.debug_prompts is on; prompts and model output are written to the log at the debug level (set logging.level to \"debug\" or \"trace\" to see them)";
/// A task was created.
pub fn task_created(id: &str, state: &TaskState) -> String {
    format!("task {id} created ({})", state_text(state))
}
/// A task changed state.
pub fn task_state(id: &str, state: &TaskState) -> String {
    format!("task {id}: {}", state_text(state))
}
/// The human reading of a state.
pub fn state_text(state: &TaskState) -> String {
    match state {
        TaskState::Created => "created, not started".into(),
        TaskState::Running => "running".into(),
        TaskState::WaitingUser { wait_kind } => match wait_kind {
            WaitKind::Approval => "waiting for your approval".into(),
            WaitKind::Question => "waiting for your answer".into(),
        },
        TaskState::Paused => "paused".into(),
        TaskState::Halted { halt_reason } => format!("halted: {}", halt_text(*halt_reason)),
        TaskState::Recovering => "recovering".into(),
        TaskState::Completed { completion } => format!(
            "completed ({})",
            match completion {
                mimlys_core::task::Completion::Verified => "verified by the completion check",
                mimlys_core::task::Completion::Asserted =>
                    "asserted by the model, no check configured",
            }
        ),
        TaskState::Failed { reason } => format!("failed: {}", failure_text(reason)),
        TaskState::Cancelled => "cancelled".into(),
    }
}
/// Why a task halted, and what to do (CLR-3).
pub fn halt_text(reason: HaltReason) -> &'static str {
    match reason {
        HaltReason::BudgetSteps => "the step budget is used up; resume with --extend-steps",
        HaltReason::BudgetRejections => {
            "too many rejected decisions in a row; resume to let the model try again"
        }
        HaltReason::BudgetFailures => {
            "too many failed actions in a row; resume to let the model try again"
        }
        HaltReason::BudgetDeadline => "the deadline passed; resume with --extend-deadline",
        HaltReason::Convergence => {
            "the model repeated the same decision; resume to let it try again"
        }
        HaltReason::ApprovalRejected => "you rejected an action and asked to halt",
        HaltReason::ApprovalExpired => "an approval request expired",
        HaltReason::QuestionExpired => "a question expired",
        HaltReason::Recovered => "recovered after a restart; resume to continue",
        HaltReason::UnknownOutcome => {
            "an action's outcome is unknown after a restart; inspect the workspace before resuming"
        }
        HaltReason::WorkspaceQuota => "the workspace quota was reached",
        HaltReason::BackendUnavailable => {
            "the model backend could not be reached; check the backend and resume"
        }
        HaltReason::ToolUnavailable => "a tool could not be reached; resume to try again",
        HaltReason::Policy => "the policy did not allow the completion check to run",
    }
}
/// Why a task failed.
pub fn failure_text(reason: &mimlys_core::task::FailureReason) -> String {
    match reason {
        mimlys_core::task::FailureReason::ModelGaveUp { reason } => {
            format!(
                "the model gave up: {}",
                serde_json::to_string(reason).unwrap_or_default()
            )
        }
        mimlys_core::task::FailureReason::Unrecoverable { detail } => {
            format!("unrecoverable: {detail}")
        }
        mimlys_core::task::FailureReason::WorkspaceLost => "the workspace is gone".to_owned(),
    }
}
/// A step finished.
pub fn step_line(index: u32, outcome: &str, detail: Option<&str>) -> String {
    match detail {
        Some(d) => format!("step {index}: {outcome} ({d})"),
        None => format!("step {index}: {outcome}"),
    }
}
/// An event, one line.
pub fn event_line(at: &str, name: &str, detail: &str) -> String {
    if detail.is_empty() {
        format!("{at} {name}")
    } else {
        format!("{at} {name} {detail}")
    }
}
/// The approval prompt's header.
pub fn approval_header(task_id: &str, step: Option<u32>) -> String {
    match step {
        Some(n) => format!("Approval required · task {task_id} · step {n}"),
        None => format!("Approval required · task {task_id}"),
    }
}
/// The approval prompt's options line (REQ-CLI-002).
pub const APPROVAL_OPTIONS: &str = "[a]pprove   [r]eject   [h]alt (reject and halt)   [g]rant …";
/// The prompt character.
pub const PROMPT: &str = "> ";
/// Reject: ask for a note.
pub const REJECT_NOTE_PROMPT: &str = "Note for the model (optional, Enter to skip): ";
/// Grant: ask for the kind and scope.
pub const GRANT_PROMPT: &str = "Grant (`tool NAME`, `host_tool NAME`, `network DOMAIN[:PORT]`): ";
/// A grant was issued.
pub fn grant_issued(id: &str, kind: &str) -> String {
    format!("grant {id} issued: {kind}")
}
/// A grant was revoked.
pub fn grant_revoked(id: &str) -> String {
    format!("grant {id} revoked")
}
/// A grant changed the grant set while an approval was pending.
pub const APPROVAL_WITHDRAWN_AFTER_GRANT: &str =
    "the pending request was withdrawn because the grants changed; the model will propose again";
/// A rejection note above LIM-D2.
pub fn note_too_long() -> String {
    format!(
        "the note is longer than {} characters; shorten it",
        mimlys_core::limits::LIM_D2_REASON_CHARS
    )
}
/// The user typed something the prompt does not understand.
pub const PROMPT_UNKNOWN: &str = "type a, r, h or g";
/// A question from the model.
pub fn question_header(task_id: &str) -> String {
    format!("Question from the model · task {task_id}")
}
/// Answer prompt.
pub const ANSWER_PROMPT: &str = "Your answer: ";
/// The grant text could not be parsed.
pub const GRANT_PARSE_ERROR: &str =
    "could not read the grant; use `tool NAME`, `host_tool NAME` or `network DOMAIN[:PORT]`";
/// The task waits and the terminal is not interactive.
pub fn pending_non_interactive(task_id: &str, kind: WaitKind) -> String {
    match kind {
        WaitKind::Approval => format!(
            "task {task_id} is waiting for your approval; run `mimlys task show {task_id}` then `mimlys approve {task_id}` or `mimlys reject {task_id}`"
        ),
        WaitKind::Question => format!(
            "task {task_id} is waiting for your answer; run `mimlys task show {task_id}` then `mimlys answer {task_id} <text>`"
        ),
    }
}
/// A stop signal arrived once.
pub const SIGNAL_PAUSING: &str = "stop signal: pausing at the next step (press again to cancel)";
/// A stop signal arrived twice.
pub const SIGNAL_CANCELLING: &str = "stop signal: cancelling the task";
/// The exit after a signal took longer than LIM-O1; the step was interrupted.
pub const SIGNAL_INTERRUPTED: &str =
    "the step did not reach a boundary in time; its execution was cancelled";
/// A task list header.
pub const LIST_HEADER: &str = "task id                               state                 updated                   footprint     goal";
/// Nothing to list.
pub const LIST_EMPTY: &str = "no tasks";
/// `events --follow`: the runtime that ran the task is gone and nothing has recovered the store.
pub const FOLLOW_RUNTIME_GONE: &str = "the runtime that ran this task is gone; recovery pending: the next command that writes recovers it";
/// `events`: nothing.
pub const EVENTS_EMPTY: &str = "no events";
/// Runtime status lines.
pub fn status_lines(
    version: &str,
    data_dir: &str,
    backend: &str,
    model: Option<&str>,
    locality: &str,
    probe: Option<&crate::app::BackendProbe>,
    tier: &str,
) -> Vec<String> {
    let mut backend_line = match model {
        Some(m) => format!("backend         {backend} {m} ({locality})"),
        None => format!("backend         {backend} ({locality})"),
    };
    if let Some(p) = probe {
        let mode = p
            .structured_output
            .map(|m| {
                serde_json::to_value(m)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_owned))
                    .unwrap_or_default()
            })
            .unwrap_or_else(|| "unresolved".to_owned());
        let reach = match (p.reachable, p.model_listed) {
            (false, _) => "unreachable".to_owned(),
            (true, Some(false)) => "reachable, model not listed".to_owned(),
            (true, _) => "reachable".to_owned(),
        };
        backend_line.push_str(&format!("; {reach}"));
        // The transport in force, with the engine's reason when `auto` chose the envelope
        // (REQ-MDL-009); structured output belongs to the envelope transport only.
        let transport = match p.decision_transport {
            Some(t) => t.as_str(),
            None => "unresolved",
        };
        backend_line.push_str(&format!("; decision transport {transport}"));
        if let Some(reason) = &p.transport_reason {
            backend_line.push_str(&format!(" ({reason})"));
        }
        match p.decision_transport {
            Some(mimlys_core::native::DecisionTransport::ToolCalls) => {
                if p.parallel_tool_calls == Some(false) {
                    backend_line.push_str("; parallel_tool_calls not accepted");
                }
            }
            Some(mimlys_core::native::DecisionTransport::Envelope) => {
                backend_line.push_str(&format!("; structured output {mode}"));
            }
            None => {}
        }
        // The engine's context length against the need (REQ-MDL-003); never a guess.
        if let Some(needed) = p.context_needed_tokens {
            match (p.context_tokens, p.context_engine) {
                (Some(n), _) => backend_line.push_str(&format!(
                    "; context: {n} tokens per request (needs about {needed})"
                )),
                (None, Some(mimlys_model::openai::context::Engine::Ollama)) => {
                    backend_line.push_str(&format!(
                        "; context: unknown (Ollama sets it when it loads the model; needs about {needed})"
                    ));
                }
                (None, _) => {
                    backend_line.push_str(&format!("; context: unknown (needs about {needed})"));
                }
            }
        }
        if let Some(d) = &p.detail {
            backend_line.push_str(&format!(" ({d})"));
        }
    }
    vec![
        format!("mimlys {version}"),
        format!("data directory  {data_dir}"),
        backend_line,
        format!("tier            {tier}"),
    ]
}
/// One declared host tool in `status` and `toolchain list` (RFC 011 §6);
/// `unavailable` names why this machine cannot run it yet (RFC 011 §7).
pub fn host_tool_line(
    name: &str,
    command: &str,
    network: bool,
    unavailable: Option<&str>,
) -> String {
    let mut line = if network {
        format!(
            "host tool       {name}: {command} (the dependency step: network allowed, external_read)"
        )
    } else {
        format!("host tool       {name}: {command} (offline)")
    };
    if let Some(reason) = unavailable {
        line.push_str(&format!(
            " · unavailable: {reason}; run `mimlys toolchain grant <path>` and start again"
        ));
    }
    line
}
/// `toolchain list`: one path of the read set (REQ-CONF-008); on Windows
/// whether it carries the container's entry.
pub fn read_set_line(path: &str, applied: Option<bool>) -> String {
    match applied {
        Some(true) => format!("read            {path}  (permission applied)"),
        Some(false) => format!("read            {path}  (permission missing)"),
        None => format!("read            {path}"),
    }
}
/// `toolchain list`: one declared path under its tool.
pub fn declared_path_line(path: &str, applied: Option<bool>) -> String {
    match applied {
        Some(true) => format!("  {path}  (permission applied)"),
        Some(false) => format!("  {path}  (permission missing: `mimlys toolchain grant {path}`)"),
        None => format!("  {path}"),
    }
}
/// `toolchain list` on Windows: the AppContainer in use.
pub fn container_line(profile: &str, sid: &str) -> String {
    format!("container       {profile} ({sid})")
}
/// `toolchain grant|revoke` where no permission is needed.
pub const TOOLCHAIN_NOTHING_TO_APPLY: &str = "nothing to apply on this platform: the kernel takes the declared paths from the worker's specification (only Windows needs directory permissions)";
/// `toolchain grant`: the question.
pub fn toolchain_grant_confirm(path: &str, profile: &str) -> String {
    format!(
        "Grant the worker's container {profile} read access to {path} and everything beneath it? (reversible with `mimlys toolchain revoke`)"
    )
}
/// `toolchain revoke`: the question.
pub fn toolchain_revoke_confirm(path: &str) -> String {
    format!(
        "Remove the worker's container permission from {path}? Host tools reading it become unavailable."
    )
}
/// `toolchain grant|revoke`: not confirmed.
pub const TOOLCHAIN_NOT_CONFIRMED: &str = "nothing changed; confirm with --yes";
/// `toolchain grant`: done.
pub fn toolchain_granted(path: &str) -> String {
    format!("permission applied to {path}; start mimlys again for the worker to use it")
}
/// `toolchain revoke`: done.
pub fn toolchain_revoked(path: &str) -> String {
    format!("permission removed from {path}")
}
/// `toolchain revoke` of a directory mimlys itself needs.
pub fn toolchain_revoke_runtime_path(path: &str) -> String {
    format!(
        "{path} is one of mimlys's own paths (the workspaces, the temporary directory, the interpreters, the worker binary and the interpreter modules' files); the worker cannot run without it, so it is not revoked here"
    )
}
/// `toolchain revoke --all` where nothing was changed outside the data directory.
pub const TOOLCHAIN_REVOKE_ALL_NOTHING: &str = "nothing was changed outside the data directory on this platform, so there is nothing to remove (only Windows changes directory permissions)";
/// `toolchain revoke --all`: the plan.
pub fn toolchain_revoke_all_plan(profile: &str, rows: &[(String, &str, &str)]) -> Vec<String> {
    let mut lines = vec![format!(
        "container       {profile} (the AppContainer profile will be deleted)"
    )];
    if rows.is_empty() {
        lines.push("no directory carries a permission for the container".to_owned());
    }
    for (path, source, found) in rows {
        lines.push(format!("remove          {path}  ({source}, {found})"));
    }
    lines.push(
        "mimlys applies the entries of its own directories again at its next start; delete the data directory too if mimlys is being uninstalled"
            .to_owned(),
    );
    lines
}
/// `toolchain revoke --all`: the question.
pub fn toolchain_revoke_all_confirm(count: usize, profile: &str) -> String {
    format!("Remove {count} permission(s) and delete the container profile {profile}?")
}
/// `toolchain revoke --all`: without a terminal and without `--yes`.
pub const TOOLCHAIN_REVOKE_ALL_NEEDS_YES: &str = "nothing changed; `toolchain revoke --all` asks before it removes anything: run it in a terminal or add --yes";
/// `toolchain revoke --all`: one line per result.
pub fn toolchain_revoke_all_removed(path: &str) -> String {
    format!("removed         {path}")
}
/// `toolchain revoke --all`: one failure.
pub fn toolchain_revoke_all_failed(path: &str, reason: &str) -> String {
    format!("FAILED          {path}: {reason}")
}
/// `toolchain revoke --all`: the profile.
pub fn toolchain_revoke_all_profile(deleted: bool, record_deleted: bool) -> String {
    format!(
        "container profile deleted: {}; record file deleted: {}",
        if deleted { "yes" } else { "no" },
        if record_deleted { "yes" } else { "no" }
    )
}
/// A `host_tool` grant for a tool this machine cannot run yet.
pub fn host_tool_unavailable_grant(name: &str, reason: &str) -> String {
    format!(
        "host tool {name} is unavailable on this machine ({reason}); the grant would change nothing until `mimlys toolchain grant <path>` is run and mimlys started again"
    )
}
/// The servers are not started: a reader creates no data directory to run them in.
pub fn servers_not_started() -> String {
    "servers: not started (no data directory yet)".to_owned()
}
/// `status`: the files the dependency step's worker reads and no other
/// worker does (REQ-CONF-011).
pub fn dependency_step_reads(files: &[String]) -> String {
    format!(
        "dependency step also reads {} (its worker only; the offline tools do not)",
        files.join(", ")
    )
}
/// One configured MCP server in `status` (RFC 009 §8).
pub fn server_line(s: &mimlys_mcp::ServerStatus) -> String {
    let mut line = format!("server          {} ({})", s.name, s.state.replace('_', " "));
    if let Some(d) = &s.detail {
        line.push_str(&format!(": {d}"));
    }
    if let (Some(name), Some(version)) = (&s.server_name, &s.server_version) {
        line.push_str(&format!("; {name} {version}"));
    }
    if let Some(r) = &s.revision {
        line.push_str(&format!("; revision {r}"));
    }
    if !s.declared.is_empty() {
        line.push_str(&format!("; declared tools: {}", s.declared.join(", ")));
        return line;
    }
    line.push_str(&format!(
        "; tools: {}",
        if s.tools.is_empty() {
            "none".to_owned()
        } else {
            s.tools.join(", ")
        }
    ));
    if s.list_changed {
        line.push_str("; list changed: a new task pins the new definitions");
    }
    line
}
/// A listed tool the import refused (RFC 009 §2).
pub fn server_unavailable(tool: &str, reason: &str) -> String {
    format!("  unavailable   {tool}: {reason}")
}
/// A declared tool the server does not list.
pub fn server_undeclared(tool: &str) -> String {
    format!("  not listed    {tool}: declared in the configuration, not served")
}
/// `check`: an external tool in the ceiling, with its source (RFC 009 §4).
pub fn ceiling_external(tool: &str, source: &str) -> String {
    format!("ceiling: {tool:<32} {source}")
}
/// `check`: a host tool in the ceiling, with the declaration that admits it (RFC 011 §6).
pub fn ceiling_host_tool(name: &str) -> String {
    format!("ceiling: host_tool:{name:<22} declared by [host_tools.{name}]")
}
/// A command that runs steps was given the fake backend without a script.
pub const FAKE_BACKEND_NO_SCRIPT: &str = "no backend to decide with: the fake backend needs `backend.script` (a file of decisions), or configure a model with `backend.kind = \"openai_compatible\"`, `url` and `model`";
/// `check`: an external tool admitted without a declaration.
pub fn ceiling_undeclared_warning(tool: &str) -> String {
    format!(
        "warning: {tool} is in policy.ceiling.tools without a [servers.<name>.tools.<tool>] declaration: it runs as external_write, not idempotent, approval required"
    )
}
/// `check`: a server's `shutdown_ms` is short (LIM-O16).
pub fn server_shutdown_short_warning(server: &str, ms: i64) -> String {
    format!(
        "warning: servers.{server}.shutdown_ms is {ms}: a server that is still ending by itself after {ms} ms is killed with its group; rypsok takes up to 3000 ms"
    )
}
/// `auto` structured output ended at plain text (RFC 008 §2).
pub fn structured_output_none() -> &'static str {
    "structured output: none (the server refused json_schema and json_object; decisions are parsed strictly all the same)"
}
/// `decision_transport = "tool_calls"` is configured and the engine refuses `tools`
/// (REQ-MDL-009): the backend is unavailable; there is no fallback.
pub fn tool_calls_refused(reason: &str) -> String {
    format!(
        "decision transport: tool_calls is configured and the engine refuses it ({reason}); the task will halt backend_unavailable. Set backend.decision_transport = \"auto\" or \"envelope\", or fix the engine"
    )
}
/// Status: a clause that is unavailable.
pub fn status_unavailable(reason: &str) -> String {
    format!("unavailable     {reason}")
}
/// Status: the store as a reader finds it (REQ-PERS-013). A reader never says
/// "locked": another runtime's lock is `in use`, and the reader reads on.
pub fn store_lines(view: &crate::app::StoreView) -> Vec<String> {
    use crate::app::StoreState;
    match view.state {
        StoreState::Absent => {
            vec!["store: not created yet (the first command that writes creates it)".into()]
        }
        StoreState::Free => vec!["store: free".into()],
        StoreState::InUse => vec![match &view.holder {
            Some(h) => format!("store: in use by runtime {} since {}", h.pid, h.since),
            None => "store: in use by another runtime".into(),
        }],
        StoreState::RecoveryPending => vec![
            "store: free".into(),
            "recovery pending: the next command that writes recovers it".into(),
        ],
        StoreState::LockUntested => {
            vec!["store: not testable for another runtime on this filesystem".into()]
        }
    }
}
/// A byte count for people: `1.5 MiB`, and plain bytes below a KiB.
pub fn bytes_text(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["KiB", "MiB", "GiB", "TiB", "PiB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1024.0;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}
/// An age for people: `40s`, `12m`, `5h`, `3d`.
pub fn age_text(seconds: i64) -> String {
    let s = seconds.max(0);
    match s {
        0..=59 => format!("{s}s"),
        60..=3599 => format!("{}m", s / 60),
        3600..=86_399 => format!("{}h", s / 3600),
        _ => format!("{}d", s / 86_400),
    }
}
/// `status --storage`: the lines (REQ-WS-013). `shown` tasks are listed.
pub fn storage_lines(view: &crate::storage::StorageView, shown: usize) -> Vec<String> {
    let b = &view.bytes;
    let line = |name: &str, bytes: u64| {
        let exact = if bytes >= 1024 {
            format!("  ({bytes} bytes)")
        } else {
            String::new()
        };
        format!("  {name:<28}{:>12}{exact}", bytes_text(bytes))
    };
    let fs = match &view.filesystem.name {
        Some(name) => format!("{name} (reflinks: {})", view.filesystem.reflink.word()),
        None => format!(
            "not known on this platform (reflinks: {})",
            view.filesystem.reflink.word()
        ),
    };
    let mut lines = vec![
        format!("store            {}", view.path),
        format!("filesystem       {fs}"),
        "bytes by kind".to_owned(),
        line("working trees", b.working_trees),
        line("snapshot objects, exclusive", b.snapshot_objects_exclusive),
        line("snapshot objects, shared", b.snapshot_objects_shared),
        line("blobs", b.blobs),
        line("interpreters", b.interpreters),
        line("database and WAL", b.database),
        line("temporary", b.temporary),
        line("total", b.total),
    ];
    if view.unreadable_manifests > 0 {
        lines.push(format!(
            "  {} snapshot manifest(s) could not be read; their objects are not counted",
            view.unreadable_manifests
        ));
    }
    if view.tasks.is_empty() {
        lines.push("no tasks".to_owned());
        return lines;
    }
    lines.push(format!(
        "tasks that hold the most (what deleting one frees; {} of {})",
        shown.min(view.tasks.len()),
        view.tasks.len()
    ));
    for t in view.tasks.iter().take(shown) {
        lines.push(format!(
            "  {}  {:<16}  {:>5}  {:>10}  {}",
            t.task_id,
            t.state,
            age_text(t.age_seconds),
            bytes_text(t.total),
            serde_json::to_string(&t.goal).unwrap_or_default()
        ));
    }
    if view.tasks.len() > shown {
        lines.push(format!(
            "  and {} more (`--json` lists every task)",
            view.tasks.len() - shown
        ));
    }
    lines
}
/// Status: running tasks.
pub fn status_running(count: usize) -> String {
    format!("running tasks   {count}")
}
/// Status: pending items.
pub fn status_pending(task_id: &str, kind: WaitKind) -> String {
    format!(
        "pending         task {task_id} waits for {}",
        match kind {
            WaitKind::Approval => "an approval",
            WaitKind::Question => "an answer",
        }
    )
}
/// An interpreter row.
pub fn interpreter_line(
    language: &str,
    installed: bool,
    hash_prefix: &str,
    build: Option<&str>,
) -> String {
    format!(
        "{language:<12} {:<13} {hash_prefix}…  {}",
        if installed {
            "installed"
        } else {
            "not installed"
        },
        build.unwrap_or("from the configuration")
    )
}
/// `interpreters install` of a language that is not configured.
pub fn unknown_language(language: &str, known: &[&str]) -> String {
    format!("no interpreter named {language}; configured: {known:?}")
}
/// A language without a shipped artifact needs `--from`.
pub fn no_artifact(language: &str) -> String {
    format!("mimlys ships no build for {language}; give the module with --from FILE")
}
/// Downloading an artifact.
pub fn downloading(url: &str) -> String {
    format!("downloading {url}")
}
/// An interpreter was installed.
pub fn interpreter_installed(language: &str, path: &str) -> String {
    format!("{language} installed at {path}")
}
/// Status: one clause, by its letter and its word (REQ-CLI-006).
pub fn status_clause(letter: char, name: &str, state: &str) -> String {
    format!("clause {letter} {name:<12} {state}")
}
/// Status: the engine.
pub fn status_engine(version: &str) -> String {
    format!("engine          wasmtime {version}")
}
/// `rollback`: done.
pub fn rolled_back(snapshot_id: &str, reason: &str) -> String {
    format!("workspace restored to snapshot {snapshot_id} ({reason})")
}
/// `rollback --promotion`: done.
pub fn promotion_reversed(id: &str, files: usize) -> String {
    format!("promotion {id} reversed: {files} path(s) restored in the project")
}
/// `promote`: the header.
pub fn promote_header(task_id: &str, changes: usize) -> String {
    format!("Promotion of task {task_id}: {changes} change(s) against the project")
}
/// `promote`: one change.
pub fn promote_change(kind: &str, path: &str, bytes: u64) -> String {
    format!("  {kind:<9} {path} ({bytes} bytes)")
}
/// `promote`: excluded paths.
pub fn promote_excluded(paths: &[String]) -> String {
    format!(
        "  excluded  {} (never promoted: LIM-W4, LIM-W5)",
        paths.join(", ")
    )
}
/// `promote`: the confirmation question.
pub const PROMOTE_CONFIRM: &str =
    "Write these changes to the project? The targets are backed up first.";
/// `promote`: not confirmed.
pub const PROMOTE_NOT_CONFIRMED: &str =
    "nothing written; confirm with --yes, or review with --patch";
/// `promote`: nothing to do.
pub const PROMOTE_NOTHING: &str = "the workspace equals the project: nothing to promote";
/// `promote`: done.
pub fn promoted(id: &str, files: usize, backup: &str) -> String {
    format!(
        "promotion {id}: {files} path(s) written; backup snapshot {backup} (reverse with `mimlys rollback <task> --promotion {id}`)"
    )
}
/// `task delete`: the plan.
pub fn delete_plan(
    task_id: &str,
    f: &mimlys_core::snapshot::Footprint,
    audit: bool,
) -> Vec<String> {
    let mut lines = vec![
        format!("Deleting task {task_id} removes:"),
        format!("  workspace      {} bytes", f.workspace_bytes),
        format!(
            "  snapshots      {} ({} bytes of objects only this task uses; {} bytes shared stay)",
            f.snapshots, f.object_bytes_exclusive, f.object_bytes_shared
        ),
        format!("  blobs          {} bytes", f.blob_bytes),
    ];
    if f.promotions > 0 {
        lines.push(format!(
            "  promotions     {} backup(s): those promotions can no longer be reversed",
            f.promotions
        ));
    }
    lines.push(if audit {
        "  audit          the task's events and records go too (--audit)".into()
    } else {
        "  audit          kept (add --audit to remove the events and records)".into()
    });
    lines
}
/// `task prune`: nothing matched.
pub const PRUNE_NOTHING: &str = "no terminal task matches: nothing to prune";
/// `task prune`: the confirmation question (asked once).
pub const PRUNE_CONFIRM: &str = "Delete these tasks?";
/// `task prune`: not confirmed.
pub const PRUNE_NOT_CONFIRMED: &str = "nothing deleted; confirm with --yes";
/// `task prune`: the plan, before anything goes (REQ-WS-014).
pub fn prune_plan(found: &[crate::commands::prune::Candidate], audit: bool) -> Vec<String> {
    let todo: Vec<_> = found.iter().filter(|c| c.kept.is_none()).collect();
    let mut lines = vec![format!(
        "Pruning deletes {} terminal task(s): each one's workspace, snapshots and blobs",
        todo.len()
    )];
    for c in found {
        lines.push(match c.kept {
            None => format!(
                "  {}  {:<10}  {:>5}  {:>10}",
                c.task_id,
                c.state,
                age_text(c.age_seconds),
                bytes_text(c.bytes())
            ),
            Some(why) => format!(
                "  {}  {:<10}  {:>5}  kept: {why} (`mimlys rollback <task> --promotion <id>` reverses it; `mimlys task delete <task>` deletes anyway)",
                c.task_id,
                c.state,
                age_text(c.age_seconds)
            ),
        });
    }
    let total: u64 = todo.iter().map(|c| c.bytes()).sum();
    lines.push(format!(
        "total: {} task(s), {} ({total} bytes)",
        todo.len(),
        bytes_text(total)
    ));
    lines.push(if audit {
        "audit: the tasks' events and records go too (--audit)".into()
    } else {
        "audit: kept (add --audit to remove the events and records)".into()
    });
    lines
}
/// `task prune`: one task deleted.
pub fn prune_deleted(task_id: &str, f: &mimlys_core::snapshot::Footprint) -> String {
    format!(
        "deleted {task_id}: {} freed",
        bytes_text(f.workspace_bytes + f.object_bytes_exclusive + f.blob_bytes)
    )
}
/// `task prune`: one task's deletion failed.
pub fn prune_failed(task_id: &str, why: &str) -> String {
    format!("failed  {task_id}: {why}")
}
/// `task prune`: all done.
pub fn prune_done(count: usize, freed: u64) -> String {
    format!("pruned {count} task(s), {} freed", bytes_text(freed))
}
/// `task prune`: stopped at a failure; the rest is untouched and named.
pub fn prune_stopped(failed: &str, why: &str, untouched: &[String], deleted: usize) -> String {
    let rest = if untouched.is_empty() {
        "no other task was left".to_owned()
    } else {
        format!("not touched: {}", untouched.join(", "))
    };
    format!("prune stopped at {failed} ({why}) after {deleted} deleted; {rest}")
}
/// `task delete`: the confirmation question.
pub const DELETE_CONFIRM: &str = "Delete?";
/// `task delete`: not confirmed.
pub const DELETE_NOT_CONFIRMED: &str = "nothing deleted; confirm with --yes";
/// `task delete`: done.
pub fn deleted(task_id: &str) -> String {
    format!("task {task_id} deleted")
}
/// A footprint, one line.
pub fn footprint_line(f: &mimlys_core::snapshot::Footprint) -> String {
    format!(
        "footprint {} bytes workspace, {} snapshot(s), {} bytes objects ({} shared), {} bytes blobs",
        f.workspace_bytes,
        f.snapshots,
        f.object_bytes_exclusive,
        f.object_bytes_shared,
        f.blob_bytes
    )
}
/// A metric line.
pub fn metric_line(name: &str, value: u64) -> String {
    format!("{name:<40} {value}")
}
/// One metric line with a ratio.
pub fn metric_ratio(name: &str, value: f64) -> String {
    format!("{name:<40} {value:.2}")
}
/// An error of the control API, for humans.
pub fn error_line(kind: &str, message: &str) -> String {
    format!("error ({kind}): {message}")
}
/// A recovered task at startup.
pub fn recovered(task_id: &str, disposition: &str, state: &str) -> String {
    format!("recovered task {task_id}: {disposition}, now {state}")
}
/// The task view's header.
pub fn show_lines(view: &mimlys_core::runtime::TaskView) -> Vec<String> {
    let t = &view.task;
    let mut lines = vec![
        format!("task      {}", t.task_id),
        format!("state     {}", state_text(&t.state)),
        format!(
            "goal      {}",
            serde_json::to_string(&t.goal).unwrap_or_default()
        ),
        format!("scope     {}", t.scope.root),
        format!("profile   {}", t.profile),
        format!("steps     {} of {}", t.counters.steps, t.budgets.steps),
        format!("deadline  {}", t.deadline.to_rfc3339()),
    ];
    if let Some(plan) = &view.plan {
        lines.push(format!("plan      version {}", plan.version));
        for (i, step) in plan.steps.iter().enumerate() {
            lines.push(format!(
                "          {}. {}",
                i + 1,
                serde_json::to_string(step).unwrap_or_default()
            ));
        }
    }
    if let Some(source) = t.forked_from {
        lines.push(format!("forked    from task {source}"));
    }
    lines.push(footprint_line(&view.footprint));
    for s in &view.snapshots {
        lines.push(format!(
            "snapshot  {}  {}  {} bytes, {} files",
            s.snapshot_id,
            s.reason.name(),
            s.bytes,
            s.files
        ));
    }
    for p in &view.promotions {
        lines.push(format!(
            "promotion {}  {} path(s) at {}{}",
            p.promotion_id,
            p.files.len(),
            p.applied_at.to_rfc3339(),
            if p.reversed_at.is_some() {
                " (reversed)"
            } else {
                ""
            }
        ));
    }
    if !view.grants.is_empty() {
        lines.push("grants".to_owned());
        for g in &view.grants {
            lines.push(format!(
                "          {}  {}",
                g.grant_id,
                serde_json::to_string(g.kind()).unwrap_or_default()
            ));
        }
    }
    lines
}
