//! `mode` derives the effect of `run_script` and `run_wasm` (RFC 005 §5,
//! REQ-TOOL-002; handoff 002d item 13), seen through the loop: a script in
//! `read_only` mode reads, so it takes no snapshot, is not rolled back and shows
//! `workspace_read` to the user; one in `read_write` mode (the default) writes
//! and takes the snapshots of REQ-WS-004. Approval by the profile's
//! `approval_tools` does not depend on the mode.

use super::*;
use crate::action::ActionStatus;

/// The definitions as `mimlys-exec` ships them: the effect of the default mode
/// (`workspace_write`), non-idempotent, and the `mode` enumeration.
fn script_tools() -> Vec<ToolDefinition> {
    let tool = |name: &str, subject: Param| ToolDefinition {
        name: name.into(),
        description: format!("{name}."),
        input: ParamSchema {
            params: vec![
                subject,
                Param {
                    name: "mode".into(),
                    ty: ParamType::Enum {
                        values: vec!["read_only".into(), "read_write".into()],
                    },
                    optional: true,
                    description: "read_only sees the workspace; read_write may change it.".into(),
                },
            ],
        },
        output: None,
        effect: EffectView {
            kind: EffectKind::WorkspaceWrite,
            reversible: true,
            idempotent: false,
        },
        deadline_ms: 30_000,
        profile: ResourceProfile::Sandbox,
        destination_arguments: vec![],
        provider: Provider::Builtin,
    };
    vec![
        tool(
            "run_script",
            Param {
                name: "source".into(),
                ty: ParamType::String {
                    min_chars: 1,
                    max_chars: 1000,
                    max_bytes: Some(1000),
                },
                optional: false,
                description: "The program.".into(),
            },
        ),
        tool(
            "run_wasm",
            Param {
                name: "module".into(),
                ty: ParamType::WorkspacePath,
                optional: false,
                description: "The module.".into(),
            },
        ),
    ]
}

fn script(tool: &str, mode: Option<&str>) -> Value {
    let mut arguments = match tool {
        "run_script" => json!({"source": "1"}),
        _ => json!({"module": "m.wasm"}),
    };
    if let Some(mode) = mode {
        arguments["mode"] = json!(mode);
    }
    json!({"decision": "act", "tool": tool, "arguments": arguments})
}

fn harness(decisions: Vec<Value>, policy: Policy) -> Harness {
    Harness::with_backend_and_tools(
        Arc::new(ScriptedBackend::with_decisions(decisions)),
        policy,
        vec![],
        crate::limits::LIM_C1_CONTEXT_CHARS,
        script_tools(),
    )
}

/// What the loop recorded of the n-th action: its effect, whether it took a
/// snapshot before and after, and whether its observation says `rolled_back`.
struct Seen {
    effect: EffectKind,
    before: bool,
    after: bool,
    rolled_back: bool,
    status: ActionStatus,
}

fn seen(h: &Harness, task: TaskId, n: usize) -> Seen {
    let action = h.persistence.load_actions(task).unwrap().remove(n);
    let executions = h.persistence.load_executions(action.action_id).unwrap();
    Seen {
        effect: action.content().effect().kind,
        before: action.snapshot_before.is_some(),
        after: executions.iter().any(|e| e.snapshot_after.is_some()),
        rolled_back: h.persistence.load_observations(task).unwrap()[n].rolled_back,
        status: action.status,
    }
}

/// REQ-TOOL-002, REQ-WS-004: a script in `read_only` mode takes no snapshot,
/// before or after; one in `read_write` mode, or with no `mode`, takes both.
/// Same for `run_wasm`.
#[test]
fn req_tool_002_a_read_only_script_takes_no_snapshot_and_a_read_write_one_does() {
    let calls = [
        ("run_script", Some("read_only")),
        ("run_script", Some("read_write")),
        ("run_script", None),
        ("run_wasm", Some("read_only")),
        ("run_wasm", Some("read_write")),
        ("run_wasm", None),
    ];
    let h = harness(
        calls.iter().map(|(t, m)| script(t, *m)).collect(),
        Policy::shipped(),
    );
    for (tool, _) in calls {
        h.tools
            .script(tool, ExecutionOutcome::succeeded(json!({}), 1));
    }
    let id = h.create();
    for (n, (tool, mode)) in calls.iter().enumerate() {
        assert_eq!(
            h.step(id).outcome,
            StepOutcome::Continued,
            "{tool} {mode:?}"
        );
        let s = seen(&h, id, n);
        let reads = *mode == Some("read_only");
        let label = format!("{tool} {mode:?}");
        assert_eq!(
            s.effect,
            if reads {
                EffectKind::WorkspaceRead
            } else {
                EffectKind::WorkspaceWrite
            },
            "{label}: the effect in force"
        );
        assert_eq!(s.before, !reads, "{label}: snapshot before");
        assert_eq!(s.after, !reads, "{label}: snapshot after");
        assert_eq!(s.status, ActionStatus::Succeeded, "{label}");
    }
}

/// REQ-TOOL-010, REQ-WS-010: a failed or cancelled script in `read_only` mode
/// is not restored (it wrote nothing) and its observation does not say
/// `rolled_back`; the same in `read_write` mode is restored.
#[test]
fn req_tool_010_a_failed_or_cancelled_read_only_script_is_not_rolled_back() {
    let outcomes = [
        ("failed", ExecutionOutcome::tool_error("exit", json!({}), 1)),
        ("cancelled", ExecutionOutcome::cancelled(1)),
    ];
    for (how, outcome) in outcomes {
        for mode in ["read_only", "read_write"] {
            let h = harness(vec![script("run_script", Some(mode))], Policy::shipped());
            h.tools.script("run_script", outcome.clone());
            let id = h.create();
            h.step(id);
            let s = seen(&h, id, 0);
            let writes = mode == "read_write";
            assert_eq!(s.before, writes, "{how} {mode}");
            assert_eq!(s.rolled_back, writes, "{how} {mode}: rolled_back");
            assert_eq!(
                h.workspaces.restores().len(),
                usize::from(writes),
                "{how} {mode}: restores (on_cancel = rollback by default)"
            );
        }
    }
}

/// REQ-APR-001: approval by the profile's `approval_tools` names the tool and
/// does not depend on the mode: a read-only script waits as a read-write one
/// does. What the user reads follows the mode: `workspace_read`, or
/// `workspace_write`. After approval the read-only one runs without a snapshot.
#[test]
fn req_apr_001_approval_by_tool_name_is_the_same_in_either_mode_and_shows_the_effect() {
    let mut policy = Policy::shipped();
    policy.profiles.get_mut("workspace").unwrap().approval_tools = vec!["run_script".into()];
    let h = harness(
        vec![
            script("run_script", Some("read_only")),
            script("run_script", Some("read_write")),
        ],
        policy,
    );
    h.tools
        .script("run_script", ExecutionOutcome::succeeded(json!({}), 1));
    h.tools
        .script("run_script", ExecutionOutcome::succeeded(json!({}), 1));
    let id = h.create();
    for (n, (mode, effect, snapshots)) in [
        ("read_only", "workspace_read", false),
        ("read_write", "workspace_write", true),
    ]
    .into_iter()
    .enumerate()
    {
        let wait = h.step(id);
        assert_eq!(wait.outcome, StepOutcome::Waited, "{mode}");
        let approval = h.runtime.get_task(id).unwrap().pending_approval.unwrap();
        assert!(
            approval.display_text().contains(&format!("{effect} ·")),
            "{mode}: {}",
            approval.display_text()
        );
        h.runtime.approve_action(&h.session, id).unwrap();
        assert_eq!(h.step(id).outcome, StepOutcome::Continued, "{mode}");
        let s = seen(&h, id, n);
        assert_eq!(s.before, snapshots, "{mode}: snapshot before");
        assert_eq!(s.after, snapshots, "{mode}: snapshot after");
    }
}

/// REQ-DEC-006 (stage 5): a `mode` that is neither value is refused before
/// anything is derived, so no spelling of it reads. Nothing runs.
#[test]
fn req_tool_002_a_mode_that_is_neither_value_is_refused_not_taken_for_a_read() {
    for mode in ["READ_ONLY", "read-only", ""] {
        let h = harness(vec![script("run_script", Some(mode))], Policy::shipped());
        let id = h.create();
        assert_eq!(h.step(id).outcome, StepOutcome::Rejected, "{mode:?}");
        assert!(h.tools.calls().is_empty(), "{mode:?}");
        assert!(h.persistence.load_actions(id).unwrap().is_empty());
    }
}
