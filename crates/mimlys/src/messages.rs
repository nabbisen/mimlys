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
/// `status` (macOS): the test that finds a worker's members from outside the
/// sandbox did not hold when it was verified, so the runtime does not use it
/// (handoff 011p, item 4); the worker's own sweeper alone ends a tool's tree, as
/// in 0.10, and the tier is the same. One line, with the reason.
pub fn outside_sweep_unavailable(reason: &str) -> String {
    format!(
        "sweep           the sweep from outside the sandbox is unavailable ({reason}): a tool that kills its worker's sweeper can leave its processes running; the tier is unchanged"
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
pub const DEBUG_PROMPTS_WARNING: &str = "warning: logging.debug_prompts is on; prompts and model output are written to the log (redacted)";
/// A task was created.
pub fn task_created(id: &str, state: &TaskState) -> String {
    format!("task {id} created ({})", state_text(state))
}
/// A task changed state.
pub fn task_state(id: &str, state: &TaskState) -> String {
    task_state_after(id, state, false)
}
/// A task changed state; `rollback_failed`: it halted after a rollback of its workspace failed.
pub fn task_state_after(id: &str, state: &TaskState, rollback_failed: bool) -> String {
    format!("task {id}: {}", state_text_after(state, rollback_failed))
}
/// The path a task halted `workspace_unreadable` is waiting for, naming it: for a refused
/// permission the remedy is to restore it; for any other cause (`kind`, in the words of the
/// failure) nothing is advised that cannot be backed.
pub fn unreadable_line(path: &str, kind: Option<&str>) -> String {
    match kind {
        None => format!(
            "workspace path that could not be read: {path}; restore its permissions, then resume"
        ),
        Some(kind) => format!(
            "workspace path that could not be read: {path} ({kind}); deal with that, then resume"
        ),
    }
}
/// The human reading of a state.
pub fn state_text(state: &TaskState) -> String {
    state_text_after(state, false)
}
/// The human reading of a state; `rollback_failed`: the halt followed a rollback that failed.
pub fn state_text_after(state: &TaskState, rollback_failed: bool) -> String {
    match state {
        TaskState::Created => "created, not started".into(),
        TaskState::Running => "running".into(),
        TaskState::WaitingUser { wait_kind } => match wait_kind {
            WaitKind::Approval => "waiting for your approval".into(),
            WaitKind::Question => "waiting for your answer".into(),
        },
        TaskState::Paused => "paused".into(),
        TaskState::Halted { halt_reason, .. } => {
            format!("halted: {}", halt_line(*halt_reason, rollback_failed))
        }
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
            "an action that cannot be undone did not finish by itself (the runtime stopped, it was interrupted, or it reached a limit), so what it did is unknown; check before resuming"
        }
        HaltReason::WorkspaceQuota => "the workspace quota was reached",
        HaltReason::WorkspaceUnreadable => {
            "part of the workspace could not be read; deal with the cause, then resume"
        }
        HaltReason::BackendUnavailable => {
            "the model backend could not be reached; check the backend and resume"
        }
        HaltReason::ToolUnavailable => "a tool could not be reached; resume to try again",
        HaltReason::Policy => {
            "the policy stopped the task: it did not allow the completion check to run, or the backend is remote and the profile forbids external communication (a line says which)"
        }
        HaltReason::DefinitionsTooLarge => {
            "the definitions of the tools granted to this task are too large to show the model; revoke a tool grant with `mimlys revoke`, then resume"
        }
    }
}
/// Why a task halted when its workspace was to be rolled back and could not be: the line says so
/// (`the workspace was not rolled back`) for the two reasons such a halt has, and is `halt_text`
/// for the rest and when no rollback failed.
pub fn halt_line(reason: HaltReason, rollback_failed: bool) -> String {
    if !rollback_failed {
        return halt_text(reason).to_owned();
    }
    match reason {
        HaltReason::UnknownOutcome => {
            "an action failed or was interrupted and the changes it made to the workspace could not be undone, so what it left is unknown; the workspace was not rolled back; check before resuming"
                .to_owned()
        }
        HaltReason::WorkspaceUnreadable => {
            "part of the workspace could not be read; the workspace was not rolled back; deal with the cause, then resume"
                .to_owned()
        }
        other => halt_text(other).to_owned(),
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
pub const APPROVAL_OPTIONS: &str =
    "[a]pprove   [r]eject   [h]alt (reject and halt)   [g]rant …   [v]iew (every argument in full)";
/// The heading of the arguments in full (`[v]iew`, `task show --arguments`).
pub const ARGUMENTS_IN_FULL: &str = "Arguments in full (sanitized):";
/// `task show --arguments` for a task that waits for no approval.
pub const NO_ARGUMENTS_TO_SHOW: &str =
    "no action is waiting for your approval, so there are no arguments to show";
/// The pending request has no arguments recorded and the action could not be read.
pub const ARGUMENTS_NOT_AVAILABLE: &str = "the arguments of this request are not available";
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
pub const PROMPT_UNKNOWN: &str = "type a, r, h, g or v";
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
pub const SIGNAL_PAUSING: &str = "stop signal: the task pauses when the step in flight has finished (press Ctrl-C again to cancel it)";
/// A stop signal arrived while the user was asked to decide or to answer
/// (REQ-CLI-005). Nothing runs at a prompt and the task holds nothing, so
/// nothing is paused or cancelled: the task waits as it did, and the line says
/// how to go on.
pub fn signal_at_prompt(task_id: &str, kind: WaitKind) -> String {
    match kind {
        WaitKind::Approval => format!(
            "stop signal: {task_id} is still waiting for your approval; decide later with mimlys approve {task_id} or mimlys reject {task_id}"
        ),
        WaitKind::Question => format!(
            "stop signal: {task_id} is still waiting for your answer; answer later with mimlys answer {task_id} <text>"
        ),
    }
}
/// A stop signal arrived twice and nothing in flight cannot be undone.
pub const SIGNAL_CANCELLING: &str =
    "stop signal: cancelling the task, and stopping the step in flight";
/// The step was still running `seconds` after the stop signal (the grace, LIM-O1 less one
/// second), so it was interrupted. Said once the step has ended `cancelled` and the pause is
/// taken, never before; `rolled_back` only when the step wrote `workspace.rollback`.
pub fn signal_interrupted(seconds: u64, rolled_back: bool) -> String {
    let rolled_back = if rolled_back {
        ", and its workspace changes were rolled back"
    } else {
        ""
    };
    format!(
        "the step was still running {seconds} seconds after the stop signal, so it was interrupted{rolled_back}; the task is paused"
    )
}
/// The first stop signal met an action that cannot be undone (REQ-CLI-005):
/// the process waits for it, which its own deadline bounds, and says why, in
/// words for the action's effect. An effect that is not one of the three that
/// cannot be undone is not a reason the process can give: it says that it
/// cannot tell.
pub fn signal_waiting(tool: &str, effect: mimlys_core::action::EffectKind) -> String {
    use mimlys_core::action::EffectKind;
    let why = match effect {
        EffectKind::ExternalWrite => {
            "it changes something outside this machine and cannot be undone"
        }
        EffectKind::HostWrite => {
            "it changes files outside the task's workspace and cannot be undone"
        }
        EffectKind::ExternalRead => {
            "it has sent a request outside this machine that cannot be taken back"
        }
        _ => return signal_waiting_unknown(),
    };
    format!("waiting for {tool} to finish: {why}; press Ctrl-C again to interrupt it anyway")
}
/// The first stop signal found that an action is in flight, or that the store cannot say
/// whether one is: the safe answer is to wait (REQ-CLI-005).
pub fn signal_waiting_unknown() -> String {
    "waiting for the action in flight to finish: mimlys cannot tell whether it can be undone; press Ctrl-C again to interrupt it anyway".to_owned()
}
/// What the second signal's line calls an action in flight when the store cannot say which one.
pub const SIGNAL_UNKNOWN_ACTION: &str = "the action in flight";
/// A further stop signal met an action that cannot be undone (REQ-CLI-005). The line
/// predicts nothing; the halt text and the unknown-action line say what came of it.
pub fn signal_interrupting_anyway(tool: &str) -> String {
    format!("stop signal: interrupting {tool} although it cannot be undone")
}
/// An action whose outcome is unknown, as the commands list it (REQ-CLI-005, REQ-TASK-006,
/// REQ-PERS-008): it cannot be undone and it did not finish by itself, whatever the cause.
pub fn unknown_action_line(
    action_id: &str,
    tool: &str,
    effect: mimlys_core::action::EffectKind,
) -> String {
    let effect = serde_json::to_string(&effect).unwrap_or_default();
    format!(
        "outcome unknown: action {action_id} ({tool}, {}) did not finish by itself and cannot be undone; check what it did before relying on it",
        effect.trim_matches('"')
    )
}
/// A task list header.
pub const LIST_HEADER: &str = "task id                               state                 updated                   footprint     goal";
/// Nothing to list.
pub const LIST_EMPTY: &str = "no tasks";
/// `events --follow`: the task is `running` and no runtime holds the store (yet): the follower waits for one.
pub const FOLLOW_WAITING_FOR_RUNTIME: &str =
    "no runtime holds this task yet; waiting for one to take the store";
/// `events --follow`: the runtime that ran the task is gone and nothing has recovered the store.
pub const FOLLOW_RUNTIME_GONE: &str = "the runtime that ran this task is gone; recovery pending: the next command that writes recovers it";
/// `events`: nothing.
pub const EVENTS_EMPTY: &str = "no events";
/// The first lines of `status`: the version and the data directory.
pub fn status_head_lines(version: &str, data_dir: &str) -> Vec<String> {
    vec![
        format!("mimlys {version}"),
        format!("data directory  {data_dir}"),
    ]
}
/// The tier line of `status`.
pub fn status_tier_line(tier: &str) -> String {
    format!("tier            {tier}")
}
/// What `status` says of a backend's last fitness probe (REQ-MDL-010, REQ-MDL-011): its class and
/// the day it was measured on this machine, `not probed` when there is none, and the reason when
/// the record cannot be read. `model` is the model the backend names now: a result measured with
/// another model is shown with both, never as the answer for this one.
pub fn last_probe_text(
    results: &crate::probe_results::Results,
    name: &str,
    model: Option<&str>,
) -> String {
    use crate::probe_results::Results;
    match results {
        Results::Unreadable(reason) => format!("unreadable ({reason})"),
        Results::Read(_) => match results.of(name) {
            None => "not probed".to_owned(),
            Some(last) => {
                let class = match (&last.detail, last.class_known()) {
                    (_, false) => format!("{} (unknown to this release)", last.class),
                    (Some(detail), true) => format!("{} ({detail})", last.class),
                    (None, true) => last.class.clone(),
                };
                let mut text = format!("{class}, measured on this machine on {}", last.date());
                if let (Some(measured), Some(now)) = (&last.model, model)
                    && measured != now
                {
                    text.push_str(&format!(
                        " with model {measured}, which is not the model this backend names now ({now})"
                    ));
                }
                text
            }
        },
    }
}
/// One backend's line of `status` (REQ-MDL-010): the name, what it is, where it runs, and what
/// the probe found: reachable and whether the model is listed, whether it is resident, the
/// transport in force with its reason, the engine's context against the need, and the last
/// fitness probe. `last` is [`last_probe_text`].
pub fn status_backend_line(
    name: &str,
    kind: &str,
    model: Option<&str>,
    locality: &str,
    probe: Option<&crate::backends::BackendProbe>,
    last: &str,
) -> String {
    let mut line = match model {
        Some(m) => format!("backend         {name}: {kind} {m} ({locality})"),
        None => format!("backend         {name}: {kind} ({locality})"),
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
            (true, Some(true)) => "reachable, model listed".to_owned(),
            (true, None) => "reachable".to_owned(),
        };
        line.push_str(&format!("; {reach}"));
        // Whether the engine holds the model in memory: Ollama's word, always yes for a
        // llama-server, unknown for another engine (REQ-MDL-010).
        if p.reachable && kind == "openai_compatible" {
            let resident = match p.resident {
                Some(true) => "yes",
                Some(false) => "no",
                None => "unknown",
            };
            line.push_str(&format!("; resident {resident}"));
        }
        // The transport in force, with the engine's reason when `auto` chose the envelope
        // (REQ-MDL-009); structured output belongs to the envelope transport only.
        let transport = match p.decision_transport {
            Some(t) => t.as_str(),
            None => "unresolved",
        };
        line.push_str(&format!("; decision transport {transport}"));
        if let Some(reason) = &p.transport_reason {
            line.push_str(&format!(" ({reason})"));
        }
        match p.decision_transport {
            Some(mimlys_core::native::DecisionTransport::ToolCalls) => {
                if p.parallel_tool_calls == Some(false) {
                    line.push_str("; parallel_tool_calls not accepted");
                }
            }
            Some(mimlys_core::native::DecisionTransport::Envelope) => {
                line.push_str(&format!("; structured output {mode}"));
            }
            None => {}
        }
        // The engine's context length against the need (REQ-MDL-003); never a guess.
        if let Some(needed) = p.context_needed_tokens {
            match (p.context_tokens, p.context_engine) {
                (Some(n), _) => line.push_str(&format!(
                    "; context: {n} tokens per request (needs about {needed})"
                )),
                (None, Some(mimlys_model::openai::context::Engine::Ollama)) => {
                    line.push_str(&format!(
                        "; context: unknown (Ollama sets it when it loads the model; needs about {needed})"
                    ));
                }
                (None, _) => {
                    line.push_str(&format!("; context: unknown (needs about {needed})"));
                }
            }
        }
        if let Some(d) = &p.detail {
            line.push_str(&format!(" ({})", detail_text(d)));
        }
    }
    line.push_str(&format!("; last probe: {last}"));
    line
}
/// The line of a backend that cannot be used as configured (a refused `schema` mode, a script
/// that cannot be read): it is said on its own line, and the other backends are shown
/// (REQ-MDL-010; handoff 015a-5, item 4).
pub fn status_backend_unusable_line(
    name: &str,
    kind: &str,
    model: Option<&str>,
    locality: &str,
    reason: &str,
    last: &str,
) -> String {
    let head = match model {
        Some(m) => format!("backend         {name}: {kind} {m} ({locality})"),
        None => format!("backend         {name}: {kind} ({locality})"),
    };
    format!("{head}; cannot be used: {reason}; last probe: {last}")
}

/// A probe's fixed-vocabulary detail as the line shows it: a redirect (`redirect:<host>`, from
/// the adapter) says that it was not followed and where it pointed (REQ-SEC-010); any other
/// detail is shown as it is.
fn detail_text(detail: &str) -> String {
    match detail.strip_prefix("redirect") {
        Some("") => "answers with a redirect, which is never followed".to_owned(),
        Some(host) if host.starts_with(':') => format!(
            "answers with a redirect to {}, which is never followed",
            &host[1..]
        ),
        _ => detail.to_owned(),
    }
}

/// A line about one backend of several, written for `status`: the same line with the backend
/// named, so that two warnings are not taken for one (`status` names them when there are
/// several; with one backend the line is what it always was).
pub fn for_backend(name: &str, line: &str) -> String {
    match line.strip_prefix("warning: ") {
        Some(rest) => format!("warning: backend `{name}`: {rest}"),
        None => format!("backend `{name}`: {line}"),
    }
}
/// One declared host tool in `status` and `toolchain list` (RFC 011 §6);
/// `unavailable` names why this machine cannot run it yet (RFC 011 §7).
pub fn host_tool_line(
    name: &str,
    command: &str,
    network: bool,
    allow_paths: bool,
    unavailable: Option<&str>,
) -> String {
    let mut line = if network {
        format!(
            "host tool       {name}: {command} (the dependency step: network allowed, external_read)"
        )
    } else {
        format!("host tool       {name}: {command} (offline)")
    };
    if allow_paths {
        line.push_str(&format!(" · {ALLOW_PATHS_WORDS}"));
    }
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
/// `toolchain revoke` of a path that holds no permission of the container.
pub fn toolchain_revoke_held_nothing(path: &str) -> String {
    format!("the container held no permission on {path}; nothing was changed there")
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
pub fn toolchain_revoke_all_plan(
    profile: &str,
    rows: &[(String, &str, &str)],
    tasks: Option<usize>,
) -> Vec<String> {
    let mut lines = vec![format!(
        "container       {profile} (the AppContainer profile will be deleted)"
    )];
    if rows.is_empty() && tasks.unwrap_or(0) == 0 {
        lines.push("no directory carries a permission for the container".to_owned());
    }
    for (path, source, found) in rows {
        lines.push(format!("remove          {path}  ({source}, {found})"));
    }
    if let Some(count) = tasks.filter(|n| *n > 0) {
        lines.push(format!(
            "remove          {}  (found by scan)",
            task_entries_summary(count)
        ));
    }
    lines.push(
        "mimlys applies the entries of its own directories again at its next start; delete the data directory too if mimlys is being uninstalled"
            .to_owned(),
    );
    lines
}
/// The per-task permissions as one line (`toolchain list`, `toolchain revoke
/// --all`): how many tasks carry their own entry on their tree and temporary
/// directory (Windows, handoff 011o). Never one line per task.
pub fn task_entries_summary(count: usize) -> String {
    format!("per-task entries on {count} task(s)")
}
/// `status`: the one-time migration of 0.11 has not finished (Windows).
pub const MIGRATION_PENDING: &str = "permissions     not yet moved to this release's layout (the container and the per-task entries): the next command that starts mimlys does it once, which can take a while on a data directory with many tasks; a tool whose permission is read from the new container may show as unavailable until then";
/// `toolchain revoke --all`: the question.
pub fn toolchain_revoke_all_confirm(count: usize, profile: &str) -> String {
    format!("Remove {count} permission(s) and delete the container profile {profile}?")
}
/// `toolchain revoke --all`: without a terminal and without `--yes`.
pub const TOOLCHAIN_REVOKE_ALL_NEEDS_YES: &str = "nothing changed; `toolchain revoke --all` asks before it removes anything: run it in a terminal or add --yes";
/// `toolchain revoke --all`: the per-task entries removed.
pub fn toolchain_revoke_all_tasks_removed(count: usize) -> String {
    format!("removed         {}", task_entries_summary(count))
}
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
///
/// What the server says of itself and of its tools is its own text: it is
/// shown cleaned, on one line, bounded (a name, a version, a revision, a tool, a
/// reason).
pub fn server_line(s: &mimlys_mcp::ServerStatus) -> String {
    let mut line = format!(
        "server          {} ({})",
        shown(&s.name),
        s.state.replace('_', " ")
    );
    if let Some(d) = &s.detail {
        line.push_str(&format!(": {}", shown(d)));
    }
    if let (Some(name), Some(version)) = (&s.server_name, &s.server_version) {
        line.push_str(&format!("; {} {}", shown(name), shown(version)));
    }
    if let Some(r) = &s.revision {
        line.push_str(&format!("; revision {}", shown(r)));
    }
    if !s.declared.is_empty() {
        line.push_str(&format!("; declared tools: {}", shown_list(&s.declared)));
        return line;
    }
    line.push_str(&format!(
        "; tools: {}",
        if s.tools.is_empty() {
            "none".to_owned()
        } else {
            shown_list(&s.tools)
        }
    ));
    if s.list_changed {
        line.push_str("; list changed: a new task pins the new definitions");
    }
    line
}
/// A listed tool the import refused (RFC 009 §2).
pub fn server_unavailable(tool: &str, reason: &str) -> String {
    format!("  unavailable   {}: {}", shown(tool), shown(reason))
}
/// Text that came from a server, for the terminal: one line, without
/// invisible or control characters, bounded.
fn shown(text: &str) -> String {
    mimlys_core::panic_text::bounded_message(text, None)
}
fn shown_list(items: &[String]) -> String {
    items
        .iter()
        .map(|i| shown(i))
        .collect::<Vec<_>>()
        .join(", ")
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
/// What `allow_paths = true` means, in `check` and `status` (A-34): the
/// declaration lifts the rule that a host tool's arguments name no path
/// outside its workspace.
pub const ALLOW_PATHS_WORDS: &str =
    "allow_paths: arguments may name paths outside the workspace (`..`, `/`, `\\`, `~`, a drive)";
/// `check`: a host tool whose declaration says `allow_paths = true`.
pub fn host_tool_allow_paths_warning(name: &str) -> String {
    format!(
        "warning: host_tools.{name} sets allow_paths = true: the model may pass it arguments that name paths outside the workspace (`..`, `/`, `\\`, `~`, a drive)"
    )
}
/// A command that runs steps was given the fake backend without a script.
pub fn fake_backend_no_script(name: &str) -> String {
    format!(
        "no backend to decide with: the fake backend `{name}` needs `backends.{name}.script` (a file of decisions), or configure a model with `backends.{name}.kind = \"openai_compatible\"`, `url` and `model`"
    )
}
/// A backend was asked for by a name the configuration does not have (REQ-CFG-007; RFC 015,
/// Amendment 1). Nothing falls back to another: the user is told which there are. For `default`,
/// the backend a task runs on, the line says that no command picks one.
pub fn no_backend_named(name: &str, configured: &[&str]) -> String {
    let have = if configured.is_empty() {
        "none is configured".to_owned()
    } else {
        format!(
            "configured: {}",
            configured
                .iter()
                .map(|n| format!("`{n}`"))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    if name == crate::backends::DEFAULT {
        format!(
            "no backend is named `{name}`, the one a task runs on ({have}); a command never picks another: name one `[backends.{name}]` (config_version = 2)"
        )
    } else {
        format!("no backend is named `{name}` ({have})")
    }
}
/// A task that was halted `policy` because its backend is remote under a profile without
/// `external` (REQ-SEC-010; external design §20): nothing was sent, and the line says what to
/// change. The backend's name is the one the task runs on.
pub fn start_refused(backend: &str, profile: &str) -> String {
    format!(
        "nothing was sent: backend `{backend}` is remote and profile `{profile}` forbids external communication (external = false), so the task was halted `policy`; make the backend local, or set external = true in [policy.profiles.{profile}] (with policy.remote_backend_allowed = true), then resume the task"
    )
}
/// A task halted `policy` because the completion check was refused (REQ-TASK-007; `halt_detail`
/// `completion_check_refused`): the validator did not authorize it, or it would need an approval,
/// which a check never gets.
pub fn completion_check_refused() -> String {
    "the completion check was refused, so the task was halted `policy`: the policy does not allow the check's tool with its arguments (or it would need your approval, which a check never gets); change the check or the policy, then resume the task".to_owned()
}
/// `check`: the file is `config_version = 1` (REQ-CFG-007, REQ-MAIN-001): read now, through 0.14.
pub fn config_version_1_deprecated() -> &'static str {
    "deprecated: config_version = 1 is read through 0.14: its [backend] is read as [backends.default]; write it as config_version = 2 with [backends.default] in place of [backend] (the configuration below is that form; keys in messages are named by it)"
}
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
pub fn tool_calls_refused(name: &str, reason: &str) -> String {
    format!(
        "decision transport: tool_calls is configured and the engine refuses it ({reason}); the task will halt backend_unavailable. Set backends.{name}.decision_transport = \"auto\" or \"envelope\", or fix the engine"
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
fn plural(count: u32, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

fn pids(items: &[u32]) -> String {
    items
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

fn runtimes_word(count: usize) -> &'static str {
    if count == 1 { "runtime" } else { "runtimes" }
}

/// What a `running` task waits for among the user's runtimes (external design §19.3): the words of
/// `task show`, which `status` uses for the same wait.
pub fn waiting_text(reason: &str, limit: u32, holders: &[u32], url: Option<&str>) -> String {
    let by = format!("{} {}", runtimes_word(holders.len()), pids(holders));
    match reason {
        "engine" => format!(
            "engine (max_in_flight = {limit}; {} in flight{}, held by {by})",
            plural(holders.len() as u32, "request", "requests"),
            url.map(|u| format!(" to {}", shown(u))).unwrap_or_default(),
        ),
        _ => format!(
            "admission (LIM-X5 = {} taken by {by})",
            plural(limit, "slot", "slots"),
        ),
    }
}

/// `task show`: the task is `running` and its execution waits.
pub fn waiting_line(waiting: &mimlys_core::runtime::Waiting) -> String {
    format!(
        "waiting: {}",
        waiting_text(
            &waiting.reason,
            waiting.limit,
            &waiting.holders,
            waiting.url.as_deref()
        )
    )
}

/// `status`: the machine view (external design §19.3, REQ-OBS-009).
pub fn machine_lines(machine: &crate::machine::Machine) -> Vec<String> {
    use crate::machine::{Admission, Holds, RowState};
    let mut lines = Vec::new();
    lines.push(match &machine.admission {
        Admission::Ready { not_yet_used: true } => {
            "admission: not yet used (no runtime of yours has taken a slot)".to_owned()
        }
        Admission::Ready { .. } => format!(
            "admission: ready ({} of {} slots taken)",
            machine.slots.taken, machine.slots.limit
        ),
        Admission::Unavailable { reason } => format!(
            "admission: unavailable ({}); executions go on unadmitted",
            shown(reason)
        ),
    });
    if machine.runtimes.is_empty() {
        lines.push("runtimes: none executing or waiting".into());
    }
    for row in &machine.runtimes {
        let what = match (row.state, row.holds, row.waiting_for) {
            (RowState::Running, Some(Holds::EngineGate), _) => {
                "running, holds an engine gate".to_owned()
            }
            (RowState::Running, _, _) => "running, holds a slot".to_owned(),
            (RowState::Waiting, _, Some(waiting_for)) => format!(
                "waiting: {}",
                waiting_text(
                    waiting_for.name(),
                    row.limit.unwrap_or(0),
                    &row.behind,
                    None
                )
            ),
            (RowState::Waiting, _, None) => "waiting".to_owned(),
        };
        lines.push(format!(
            "runtime {}: {what}; since {}{}; data {}",
            row.pid,
            row.since,
            row.task_id
                .as_deref()
                .map(|t| format!("; task {}", shown(t)))
                .unwrap_or_default(),
            shown(&row.data_dir),
        ));
    }
    for gate in &machine.engine_gates {
        lines.push(format!(
            "engine gate {}: {} of {} in flight{}{}",
            shown(&gate.url),
            gate.in_flight,
            gate.limit,
            if gate.holders.is_empty() {
                String::new()
            } else {
                format!(
                    " ({} {})",
                    runtimes_word(gate.holders.len()),
                    pids(&gate.holders)
                )
            },
            if gate.waiting.is_empty() {
                String::new()
            } else {
                format!(
                    "; waiting: {} {}",
                    runtimes_word(gate.waiting.len()),
                    pids(&gate.waiting)
                )
            },
        ));
    }
    if machine.stale_entries > 0 {
        lines.push(format!(
            "registry: {} stale {} (mimlys runtime reap removes them)",
            machine.stale_entries,
            if machine.stale_entries == 1 {
                "entry"
            } else {
                "entries"
            }
        ));
    }
    lines
}

/// `runtime reap`: one line per entry removed.
pub fn reaped_line(reaped: &crate::registry::Reaped) -> String {
    let what = match &reaped.entry {
        Some(entry) => format!(
            "{} of runtime {} (data {}, since {})",
            entry.kind.name(),
            entry.pid,
            shown(&entry.data_dir),
            entry.since
        ),
        None => format!("file {}", shown(&reaped.stem)),
    };
    format!("reaped {what}: {}", shown(&reaped.reason))
}

/// `runtime reap`: nothing to remove.
pub const NOTHING_TO_REAP: &str = "nothing to reap: every entry of the registry has a holder";

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
/// Status: the CPU part of clause C, listed beside the clauses (REQ-CONF-013).
/// It is not a clause and never changes the tier.
pub fn status_cpu_part(state: &str) -> String {
    format!("clause C {:<12} {state}", "cpu")
}
/// Status: the engine.
pub fn status_engine(version: &str) -> String {
    format!("engine          wasmtime {version}")
}
/// `rollback`: done.
pub fn rolled_back(snapshot_id: &str, reason: &str) -> String {
    format!("workspace restored to snapshot {snapshot_id} ({reason})")
}
/// A promotion found `applying` at start, restored from its backup and recorded `failed`.
pub fn promotion_recovered(r: &mimlys_persist::recovery::RecoveredPromotion) -> String {
    let base = format!(
        "promotion {} of task {} did not finish before the last stop and is recorded failed",
        r.promotion_id, r.task_id
    );
    match (&r.restore_error, r.kept.is_empty()) {
        (Some(e), _) => format!(
            "{base}; restoring the project from its backup failed ({e}); {}",
            r.restore_hint.as_deref().unwrap_or_default()
        ),
        (None, true) => format!("{base}; the project was restored from its backup"),
        (None, false) => format!(
            "{base}; the project was restored from its backup except {} path(s) edited since, left as they are: {}",
            r.kept.len(),
            r.kept.join(", ")
        ),
    }
}

/// `rollback --promotion`: the header of what the reversal will do.
pub fn revert_header(promotion_id: &str, paths: usize) -> String {
    format!("Reversal of promotion {promotion_id}: {paths} path(s) will change in the project")
}
/// `rollback --promotion`: one path.
pub fn revert_entry(action: &str, path: &str) -> String {
    format!("  {action:<9} {path}")
}
/// `rollback --promotion`: the paths edited since the promotion.
pub fn revert_conflicts(paths: &[String]) -> String {
    format!(
        "  edited since the promotion (restoring them would overwrite the edit): {}",
        paths.join(", ")
    )
}
/// `rollback --promotion`: a promotion recorded before edits could be told apart.
pub const REVERT_EDITS_UNKNOWN: &str = "  note: this promotion was recorded before edits since could be told apart; every path above is restored, edited or not";
/// `rollback --promotion`: the confirmation question.
pub const REVERT_CONFIRM: &str = "Restore these paths from the backup?";
/// `rollback --promotion`: the second question, when paths were edited since.
pub const REVERT_OVERWRITE_CONFIRM: &str =
    "Some of these paths were edited since the promotion. Overwrite the edits?";
/// `rollback --promotion`: nothing was changed.
pub const REVERT_NOT_CONFIRMED: &str = "not confirmed: nothing was changed (use --yes to confirm)";
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
/// `task prune`: the confirmation question (asked once), for what the plan holds.
pub fn prune_confirm(tasks: bool, no_task: bool) -> &'static str {
    match (tasks, no_task) {
        (true, true) => "Delete these tasks and workspaces?",
        (false, true) => "Delete these workspaces?",
        _ => PRUNE_CONFIRM,
    }
}
/// `task prune`: the workspaces no task refers to, in the plan (REQ-WS-014): each with why it is
/// there and what it holds, and the total. `with_tasks` says that the plan of the tasks came
/// first.
pub fn prune_no_task_plan(
    found: &[crate::workspace::orphans::Orphan],
    with_tasks: bool,
) -> Vec<String> {
    let mut lines = vec![format!(
        "Pruning {}removes {} workspace(s) that no task refers to:",
        if with_tasks { "also " } else { "" },
        found.len()
    )];
    for orphan in found {
        lines.push(format!(
            "  {}  {}  {:>10}",
            orphan.name,
            crate::workspace::orphans::REASON,
            bytes_text(orphan.bytes)
        ));
    }
    let total: u64 = found.iter().map(|o| o.bytes).sum();
    lines.push(format!(
        "total: {} workspace(s) with no task, {} ({total} bytes)",
        found.len(),
        bytes_text(total)
    ));
    lines
}
/// `task prune`: one workspace with no task removed, or with paths left.
pub fn prune_no_task_removed(
    orphan: &crate::workspace::orphans::Orphan,
    left: &[String],
) -> String {
    if left.is_empty() {
        format!(
            "removed {}: {} freed (no task: left by a refused creation)",
            orphan.name,
            bytes_text(orphan.bytes)
        )
    } else {
        format!(
            "not removed completely {}: {} path(s) are left: {}",
            orphan.name,
            left.len(),
            shown_paths(left)
        )
    }
}
/// The first paths of a list, and how many more.
fn shown_paths(paths: &[String]) -> String {
    const SHOWN: usize = 10;
    let shown: Vec<&str> = paths.iter().take(SHOWN).map(String::as_str).collect();
    let more = paths.len().saturating_sub(SHOWN);
    format!(
        "{}{}",
        shown.join(", "),
        if more > 0 {
            format!(" and {more} more")
        } else {
            String::new()
        }
    )
}
/// `task prune`: workspaces with no task that could not be removed completely: a next
/// prune lists what remains (they are not a deletion's, which the next start finishes).
pub fn prune_no_task_left(left: &[(String, Vec<String>)]) -> String {
    let parts: Vec<String> = left
        .iter()
        .map(|(name, paths)| format!("{name} ({} path(s): {})", paths.len(), shown_paths(paths)))
        .collect();
    format!(
        "{} workspace(s) with no task were not removed completely; `mimlys task prune` lists what remains: {}",
        left.len(),
        parts.join("; ")
    )
}
/// `task prune`: all done.
pub fn prune_done(tasks: usize, no_task: usize, freed: u64) -> String {
    if no_task == 0 {
        format!("pruned {tasks} task(s), {} freed", bytes_text(freed))
    } else {
        format!(
            "pruned {tasks} task(s) and {no_task} workspace(s) with no task, {} freed",
            bytes_text(freed)
        )
    }
}
/// `task prune`: one task deleted with files left: its first paths (all of them are in the JSON
/// report), then how many more.
pub fn prune_task_left(task_id: &str, paths: &[String]) -> String {
    mimlys_core::spi::not_removed_text(task_id, paths)
}
/// `task prune`: tasks deleted with files left, which the next start removes (the run went on).
pub fn prune_left(left: &[(String, Vec<String>)]) -> String {
    const SHOWN: usize = 10;
    let parts: Vec<String> = left
        .iter()
        .map(|(task, paths)| {
            let shown: Vec<&str> = paths.iter().take(SHOWN).map(String::as_str).collect();
            let more = paths.len().saturating_sub(SHOWN);
            format!(
                "{task} ({} path(s): {}{})",
                paths.len(),
                shown.join(", "),
                if more > 0 {
                    format!(" and {more} more")
                } else {
                    String::new()
                }
            )
        })
        .collect();
    format!(
        "{} deleted task(s) have files left that the next start removes: {}",
        left.len(),
        parts.join("; ")
    )
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
/// The index of `task output` (handoff 004d): what the action has, for the result and each stream,
/// with its size, whether the model read a cut of it, and the flag that shows it.
pub fn output_index(
    task_id: &str,
    action_id: &str,
    tool: &str,
    attempt: u32,
    status: &str,
    attempts: &[crate::commands::task_output::Attempt],
    copies: &[crate::commands::task_output::Copy],
) -> Vec<String> {
    use crate::commands::task_output::Unavailable;
    let mut lines = vec![format!(
        "task {task_id}, action {action_id}: {tool}, attempt {attempt}, {status}"
    )];
    for copy in copies {
        let label = format!("{:<17}", copy.which.words());
        let what = match &copy.state {
            Ok(held) if held.cut => format!(
                "{} bytes; the model read a cut of it; {} shows the whole",
                held.bytes,
                copy.which.flag()
            ),
            Ok(held) => format!(
                "{} bytes; the model read it whole; {} shows it",
                held.bytes,
                copy.which.flag()
            ),
            Err(Unavailable::NoStreams) => "none: an external tool has no streams".to_owned(),
            Err(Unavailable::NotKept) => "the model read a cut of it, and no whole copy was kept (the action ran before mimlys 0.12 kept them, or the copy could not be stored)".to_owned(),
            Err(Unavailable::Removed) => "the model read a cut of it, and its whole copy was removed with the task's outputs by `task delete`".to_owned(),
        };
        let what = match copy.captured_bytes {
            Some(kept) => format!(
                "{what}; the tool printed more than mimlys keeps, so what is kept is the first {kept} bytes it printed"
            ),
            None => what,
        };
        // The stored copy was itself cut at its bound: only a redaction that lengthens the text
        // past it can do that. A copy is never shown as whole when it is not.
        let what = match (&copy.state, copy.copy_cut) {
            (Ok(held), true) => format!(
                "{what}; the stored copy is itself cut, at its first {} bytes: a redaction made the text longer than mimlys stores",
                held.bytes
            ),
            (_, true) => format!(
                "{what}; the stored copy was itself cut: a redaction made the text longer than mimlys stores"
            ),
            (_, false) => what,
        };
        lines.push(format!("  {label}{what}"));
    }
    // The attempts the action has, each as it ended, so that "what did it print when it failed?"
    // has an answer: `--attempt N` shows that attempt's copies. A line that offers a choice the
    // user does not have is noise, so it is there only when there is more than one attempt (the
    // JSON always lists them).
    if attempts.len() > 1 {
        let list = attempts
            .iter()
            .map(|a| format!("{} ({})", a.attempt, a.status.name()))
            .collect::<Vec<_>>()
            .join(", ");
        lines.push(format!(
            "  {:<17}{list}; --attempt N shows that attempt's copies",
            "attempts"
        ));
    }
    lines
}

/// `task output --attempt N` named an attempt the action does not have.
pub fn action_has_no_attempt(action_id: &str, asked: u32, has: &[u32]) -> String {
    let has = has
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    format!("action {action_id} has no attempt {asked}; its attempts are {has}")
}

/// `task output` named an action that never ran.
pub fn action_never_ran(action_id: &str, tool: &str, status: &str) -> String {
    format!(
        "action {action_id} ({tool}) never ran (its status is {status}): it has no execution and no output"
    )
}

/// `task output` without an action, for a task none of whose actions ran.
pub fn task_has_no_action_that_ran() -> String {
    "the task has no action that ran, so there is no output to show".to_owned()
}

/// `task output --stdout` or `--stderr` of an external tool.
pub fn external_tool_has_no_streams(tool: &str) -> String {
    format!(
        "{tool} is an external tool: it has no standard output or standard error; --result shows its result"
    )
}

/// A stream the model read a cut of, with no whole copy.
pub fn no_whole_copy_kept(words: &str) -> String {
    format!(
        "no whole copy of the {words} was kept: the model read a cut of it, and the action ran before mimlys 0.12 kept them, or the copy could not be stored"
    )
}

/// A stream whose whole copy `task delete` removed.
pub fn whole_copy_removed(words: &str) -> String {
    format!(
        "the whole copy of the {words} was removed with the task's outputs by `task delete`; the model's cut of it is all that is left"
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
        format!(
            "state     {}",
            state_text_after(&t.state, view.rollback_failed)
        ),
        format!(
            "goal      {}",
            serde_json::to_string(&t.goal).unwrap_or_default()
        ),
        format!("scope     {}", t.scope.root),
        format!("profile   {}", t.profile),
        format!("steps     {} of {}", t.counters.steps, t.budgets.steps),
        format!("deadline  {}", t.deadline.to_rfc3339()),
    ];
    if let Some(waiting) = &view.waiting {
        lines.push(waiting_line(waiting));
    }
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
    if let Some(path) = &view.unreadable {
        lines.push(unreadable_line(path, view.unreadable_kind.as_deref()));
    }
    for a in &view.unknown_actions {
        lines.push(unknown_action_line(
            &a.action_id.to_string(),
            &a.tool,
            a.effect,
        ));
    }
    for s in &view.snapshots {
        lines.push(format!(
            "snapshot  {}  {}  {} bytes, {} files{}",
            s.snapshot_id,
            s.reason.name(),
            s.bytes,
            s.files,
            if s.reason == mimlys_core::snapshot::SnapshotReason::Backup {
                " (a promotion's backup: not a rollback target; use rollback --promotion)"
            } else {
                ""
            }
        ));
    }
    for p in &view.promotions {
        lines.push(format!(
            "promotion {}  {} path(s) at {}{}",
            p.promotion_id,
            p.files.len(),
            p.applied_at.to_rfc3339(),
            if p.reversed_at.is_some() {
                " (reversed)".to_owned()
            } else if p.state != mimlys_core::snapshot::PromotionState::Applied {
                format!(
                    " ({}{})",
                    p.state.name(),
                    p.reason
                        .as_deref()
                        .map(|r| format!(": {r}"))
                        .unwrap_or_default()
                )
            } else {
                String::new()
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

/// `model probe <name>` of a remote backend when `policy.remote_backend_allowed` is not set
/// (REQ-MDL-011, REQ-SEC-010): nothing is sent.
pub fn probe_remote_not_allowed(name: &str) -> String {
    format!(
        "backend `{name}` is remote and `policy.remote_backend_allowed` is not set: it is not probed, and nothing was sent to it"
    )
}
/// `model probe <name>` of a remote backend, said first (REQ-MDL-011, THR-40): what the probe
/// sends, and what it never does.
pub fn probe_remote_notice(name: &str, host: Option<&str>) -> String {
    let at = host.map(|h| format!(" ({h})")).unwrap_or_default();
    format!(
        "backend `{name}` is remote{at}: the probe sends it one fixed mini-task (a made-up file of one line and a fixed goal to edit it) and nothing of yours: no file, goal, task or setting of your own"
    )
}
/// `model probe` begins on a backend (a probe takes up to the time of LIM-E3).
pub fn probe_start(name: &str, kind: &str, model: Option<&str>, locality: &str) -> String {
    let what = match model {
        Some(m) => format!("{kind} {m}"),
        None => kind.to_owned(),
    };
    format!(
        "probing backend `{name}`: {what} ({locality}); at most {} steps and {} seconds",
        mimlys_core::limits::LIM_E2_PROBE_STEPS,
        mimlys_core::limits::LIM_E3_PROBE_MS / 1000
    )
}
/// One backend's result (REQ-MDL-011): the class and how it was measured.
pub fn probe_result(name: &str, last: &crate::probe_results::LastProbe) -> String {
    let class = match &last.detail {
        Some(detail) => format!("{} ({detail})", last.class),
        None => last.class.clone(),
    };
    let mut parts = vec![format!(
        "{} {}",
        last.steps,
        if last.steps == 1 { "step" } else { "steps" }
    )];
    if let Some(ms) = last.model_ms_per_step {
        parts.push(format!("{ms} ms of model time per step"));
    }
    if let Some(transport) = &last.transport {
        parts.push(format!("transport {transport}"));
    }
    if let Some(model) = &last.model {
        parts.push(format!("model {model}"));
    }
    format!(
        "backend `{name}`: {class}; {}; measured on this machine on {}",
        parts.join(", "),
        last.date()
    )
}
/// What each class says, for a user who does not know the words (REQ-MDL-011): one line under the
/// result.
pub fn probe_class_meaning(class: &str, detail: Option<&str>) -> &'static str {
    match (class, detail) {
        ("acts", _) => "it made the edit, completed the task, and the file was found as asked",
        ("incomplete", Some("asked_user")) => {
            "it proposed an action, then asked the user, whom nobody answers in a probe"
        }
        ("incomplete", Some("gave_up")) => "it proposed an action, then gave up",
        ("incomplete", Some("not_verified")) => {
            "it proposed an action and said it was done, but the file was not as asked"
        }
        ("incomplete", _) => {
            "it proposed an action, but did not finish within the steps and the time of the probe"
        }
        ("plans_only", Some("asked_user")) => {
            "it never proposed an action: it asked the user, whom nobody answers in a probe"
        }
        ("plans_only", Some("gave_up")) => "it never proposed an action: it gave up",
        ("plans_only", Some("not_verified")) => {
            "it never proposed an action, and said it was done, but the file was not as asked"
        }
        ("plans_only", _) => {
            "it never proposed an action: it planned until the steps or the time of the probe ran out"
        }
        ("no_structured_call", _) => {
            "every reply was malformed: it does not return the structured call the runtime reads"
        }
        _ => "",
    }
}
/// `model probe`: no reply came from the backend (REQ-MDL-011). It is said, and nothing is
/// recorded, so that the last measurement of the backend stays; the command exits 1.
pub fn probe_unreachable(name: &str) -> String {
    format!(
        "backend `{name}`: unreachable: no reply came from the backend, so nothing was measured and nothing is recorded (the last measurement of this backend, if any, stays); check that the engine is running and the url is right (`mimlys status` shows it live), then probe again"
    )
}
/// `model probe` with no name left remote backends out (REQ-MDL-011).
pub fn probe_left_out(left: &[crate::probe::LeftOut]) -> Vec<String> {
    left.iter()
        .map(|l| {
            let at = l
                .host
                .as_deref()
                .map(|h| format!(" ({h})"))
                .unwrap_or_default();
            format!(
                "left out: backend `{0}` is remote{at}; a probe sends it a fixed mini-task, so it is probed only when you name it: `mimlys model probe {0}`",
                l.backend
            )
        })
        .collect()
}
/// `model probe` with no name and no local backend.
pub fn probe_no_local_backend() -> &'static str {
    "no local backend is configured, so there is nothing to probe without a name"
}
/// A stop signal ended the probe (REQ-MDL-011): what was made is deleted, and nothing is recorded
/// for the backend it ended on.
pub fn probe_stopped(name: &str, not_probed: &[String]) -> String {
    let rest = if not_probed.is_empty() {
        String::new()
    } else {
        format!(
            "; not probed: {}",
            not_probed
                .iter()
                .map(|n| format!("`{n}`"))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    format!(
        "stopped: what the probe of backend `{name}` made is deleted and no class was recorded for it{rest}"
    )
}
/// The probe measured nothing of the model, because the engine's gate stayed held by another
/// runtime of the user's for the whole time (LIM-E3): nothing is recorded.
pub fn probe_not_measured_gate(name: &str) -> String {
    format!(
        "backend `{name}` was not measured: no request could be sent within the time of the probe, because another runtime of yours kept the engine's gate (`mimlys status` shows who); nothing was recorded"
    )
}
/// The mini-task ended in a state a probe does not reach.
pub fn probe_fault(name: &str, end: &str) -> String {
    format!(
        "backend `{name}`: the probe's task ended in a state a probe does not reach ({end}) without a reply from the model; nothing was recorded"
    )
}
/// What a probe could not remove (REQ-MDL-011): the next start that writes removes it.
pub fn probe_left_paths(paths: &[String]) -> String {
    format!(
        "warning: {} path(s) of the probe could not be removed and will be removed by the next command that writes: {}",
        paths.len(),
        paths.join(", ")
    )
}
/// A command that writes, while another process holds the store (LIM-O3): who, and what to do.
pub fn store_in_use(holder: Option<(u32, &str)>) -> String {
    match holder {
        Some((pid, since)) => format!(
            "the store is in use by runtime {pid} since {since}: a command that writes (a task, a scope, a record) does not run beside another runtime of this data directory; run it when that runtime has ended"
        ),
        None => "the store is in use by another runtime: a command that writes (a task, a scope, a record) does not run beside another runtime of this data directory; run it when that runtime has ended".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mimlys_core::task::HaltReason;

    /// REQ-WS-004, REQ-WS-006: the halt lines say what happened. After a rollback that failed,
    /// both lines say that the workspace was not rolled back, and `unknown_outcome` no longer says
    /// that an action *that cannot be undone* did not finish. A tree unreadable for a reason other
    /// than its permissions is advised nothing about permissions, in the line of the halt and in
    /// the line of the path; a permission problem is.
    #[test]
    fn req_ws_004_the_halt_lines_say_what_happened() {
        let clause = "the workspace was not rolled back";
        for reason in [HaltReason::UnknownOutcome, HaltReason::WorkspaceUnreadable] {
            let after = halt_line(reason, true);
            assert!(after.contains(clause), "{reason:?}: {after}");
            assert!(
                !halt_line(reason, false).contains(clause),
                "{reason:?} without a failed rollback"
            );
        }
        assert!(
            !halt_line(HaltReason::UnknownOutcome, true).contains("cannot be undone did not"),
            "a reversible action is not said to be irreversible"
        );
        assert_eq!(
            halt_line(HaltReason::UnknownOutcome, false),
            halt_text(HaltReason::UnknownOutcome),
            "an action that cannot be undone keeps its line"
        );
        // The other reasons do not change.
        assert_eq!(
            halt_line(HaltReason::BudgetSteps, true),
            halt_text(HaltReason::BudgetSteps)
        );
        // Neutral: no advice the cause may not allow.
        assert!(!halt_text(HaltReason::WorkspaceUnreadable).contains("permission"));
        assert!(!halt_line(HaltReason::WorkspaceUnreadable, true).contains("permission"));
        // The path's line: the permissions only for a permission problem.
        assert_eq!(
            unreadable_line("src", None),
            "workspace path that could not be read: src; restore its permissions, then resume"
        );
        let other = unreadable_line("src", Some("entity not found"));
        assert!(
            other.contains("src") && other.contains("entity not found"),
            "{other}"
        );
        assert!(!other.contains("permission"), "{other}");
        // The state line carries the clause when the view says so.
        let halted = TaskState::Halted {
            halt_reason: HaltReason::WorkspaceUnreadable,
            halt_detail: None,
        };
        assert!(task_state_after("t", &halted, true).contains(clause));
        assert!(!task_state_after("t", &halted, false).contains(clause));
        assert_eq!(
            task_state("t", &halted),
            task_state_after("t", &halted, false)
        );
    }
}
