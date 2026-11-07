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
        "warning: xcrun's cache {file} is missing and could not be made: host tools that build with Xcode will fail; run `xcrun --find cc` once"
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
pub const DEBUG_PROMPTS_WARNING: &str =
    "warning: logging.debug_prompts is on; prompts and model output are written to the log";
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
        backend_line.push_str(&format!("; {reach}; structured output {mode}"));
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
        "{path} is one of mimlys's own directories (the workspaces, the temporary directory, the interpreters, the worker); the worker cannot run without it, so it is not revoked here"
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
/// One configured MCP server in `status` (RFC 009 §8).
pub fn server_line(s: &mimlys_mcp::ServerStatus) -> String {
    let mut line = format!("server          {} ({})", s.name, s.state);
    if let Some(d) = &s.detail {
        line.push_str(&format!(": {d}"));
    }
    if let (Some(name), Some(version)) = (&s.server_name, &s.server_version) {
        line.push_str(&format!("; {name} {version}"));
    }
    if let Some(r) = &s.revision {
        line.push_str(&format!("; revision {r}"));
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
/// `auto` structured output ended at plain text (RFC 008 §2).
pub fn structured_output_none() -> &'static str {
    "structured output: none (the server refused json_schema and json_object; decisions are parsed strictly all the same)"
}
/// Status: a clause that is unavailable.
pub fn status_unavailable(reason: &str) -> String {
    format!("unavailable     {reason}")
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
