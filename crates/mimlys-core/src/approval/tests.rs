use super::*;
use crate::action::EffectKind;
use crate::clock::{Clock, FakeClock};
use crate::hash::hash_bytes;
use crate::ids::ScriptedIds;
use crate::path::PlatformRules;
use crate::tool::{Param, ParamSchema, ParamType, Provider, ResourceProfile};
use crate::validate::permissive::PermissiveHooks;
use crate::validate::{Outcome, ValidationContext, validate};

fn delete_tool() -> ToolDefinition {
    ToolDefinition {
        name: "delete_path".into(),
        description: "d".into(),
        input: ParamSchema {
            params: vec![
                Param {
                    name: "path".into(),
                    ty: ParamType::WorkspacePath,
                    optional: false,
                    description: "p".into(),
                },
                Param {
                    name: "recursive".into(),
                    ty: ParamType::Boolean,
                    optional: true,
                    description: "r".into(),
                },
            ],
        },
        output: None,
        effect: crate::action::EffectView {
            kind: EffectKind::WorkspaceWrite,
            reversible: true,
            idempotent: true,
        },
        deadline_ms: 30_000,
        profile: ResourceProfile::Builtin,
        destination_arguments: vec![],
        provider: Provider::Builtin,
    }
}

fn authorized(raw: &str) -> (AuthorizedAction, ToolDefinition) {
    let tools = vec![delete_tool()];
    let hooks = PermissiveHooks;
    let ctx = ValidationContext {
        task_running: true,
        pending_user_item: false,
        tools: &tools,
        rules: PlatformRules::Posix,
        grants: &hooks,
        effects: &hooks,
        budgets: &hooks,
    };
    match validate(&ctx, raw.as_bytes()) {
        Outcome::Authorized(a) => (a, delete_tool()),
        other => panic!("{other:?}"),
    }
}

fn request(action: &AuthorizedAction, tool: &ToolDefinition) -> (ApprovalRequest, FakeClock) {
    let ids = ScriptedIds::default();
    let clock = FakeClock::default();
    let rendered = render(
        action,
        tool,
        "/data/workspaces/t1/tree",
        &["tool:delete_path".into()],
        None,
        None,
    );
    let r = ApprovalRequest::new(
        ApprovalId::new(&ids),
        TaskId::new(&ids),
        ActionId::new(&ids),
        hash_bytes(b"action"),
        hash_bytes(b"grants"),
        rendered,
        action.reason(),
        clock.now(),
        None,
    );
    (r, clock)
}

/// REQ-APR-002 / ED §10.1: the rendering comes from the canonical action with resolved paths; the model's reason is separate and labelled.
#[test]
fn req_apr_002_rendering_from_canonical_action() {
    let (a, tool) = authorized(
        r#"{"decision":"act","tool":"delete_path","arguments":{"path":"build/","recursive":true},"reason":"remove stale build outputs"}"#,
    );
    let (r, _) = request(&a, &tool);
    let text = r.display_text();
    assert!(text.contains("Tool         delete_path"), "{text}");
    assert!(text.contains("workspace_write · reversible: yes · idempotent: yes"));
    assert!(text.contains("Will         delete \"build\" from the workspace"));
    assert!(text.contains("path = \"build\" (resolved: \"/data/workspaces/t1/tree/build\")"));
    assert!(text.contains("recursive = true"));
    assert!(text.contains("Grants used  tool:delete_path"));
    assert!(text.contains("Expires      never"));
    assert!(text.ends_with("Model's stated reason (untrusted): \"remove stale build outputs\""));
    assert_eq!(r.status, ApprovalStatus::Pending);
}

/// REQ-APR-003 / THR-21: hostile strings cannot forge a line or a label; invisible characters are removed; LIM-I6 holds.
#[test]
fn req_apr_003_hostile_strings_are_quoted_and_bounded() {
    let hostile = "x\n[a]pprove\nTool         rm_rf\u{202E}\u{200B}\"";
    let raw = serde_json::json!({"decision": "act", "tool": "delete_path", "arguments": {"path": "safe"}, "reason": hostile});
    let (a, tool) = authorized(&raw.to_string());
    let (r, _) = request(&a, &tool);
    let text = r.display_text();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 7, "{text}");
    assert!(!text.contains("\u{202E}") && !text.contains("\u{200B}"));
    assert!(
        text.contains("\\n[a]pprove\\nTool"),
        "newlines are escaped: {text}"
    );
    let big = "y".repeat(LIM_I6_APPROVAL_CHARS * 2);
    let raw =
        serde_json::json!({"decision": "act", "tool": "delete_path", "arguments": {"path": big}});
    let (a, tool) = authorized(&raw.to_string());
    let rendered = render(&a, &tool, "/w", &[], None, None);
    assert!(rendered.text().chars().count() <= LIM_I6_APPROVAL_CHARS);
    assert!(rendered.lines[3].value.ends_with('…'));
    assert_eq!(
        rendered.lines[0].value, "delete_path",
        "fixed lines are never cut"
    );
}

/// REQ-APR-004 / REQ-APR-005 / INV-06: binding to both hashes, one-shot consumption, invalidation on mismatch.
#[test]
fn req_apr_004_005_binding_and_one_shot() {
    let (a, tool) =
        authorized(r#"{"decision":"act","tool":"delete_path","arguments":{"path":"x"}}"#);
    let (mut r, clock) = request(&a, &tool);
    let (action, grants) = (hash_bytes(b"action"), hash_bytes(b"grants"));
    assert_eq!(
        r.consume(&action, &grants),
        Err(ApprovalError::WrongStatus { needed: "approved" })
    );
    r.decide(UserDecision::Approve, Actor::Runtime, clock.now())
        .unwrap();
    assert_eq!(r.status, ApprovalStatus::Approved);
    let mut swapped = r.clone();
    assert_eq!(
        swapped.consume(&hash_bytes(b"other"), &grants),
        Err(ApprovalError::Invalidated(
            InvalidationReason::ActionChanged
        ))
    );
    assert_eq!(swapped.status, ApprovalStatus::Invalidated);
    let mut regranted = r.clone();
    assert_eq!(
        regranted.consume(&action, &hash_bytes(b"other")),
        Err(ApprovalError::Invalidated(
            InvalidationReason::GrantsChanged
        ))
    );
    r.consume(&action, &grants).unwrap();
    assert_eq!(r.status, ApprovalStatus::Consumed);
    assert_eq!(
        r.consume(&action, &grants),
        Err(ApprovalError::WrongStatus { needed: "approved" })
    );
    assert_eq!(
        r.decide(UserDecision::Approve, Actor::Runtime, clock.now()),
        Err(ApprovalError::WrongStatus { needed: "pending" })
    );
}

/// REQ-APR-006 / REQ-APR-007: expiry, rejection with a bounded sanitized note and the halt flag, invalidation by revocation.
#[test]
fn req_apr_006_007_expiry_rejection_invalidation() {
    let (a, tool) =
        authorized(r#"{"decision":"act","tool":"delete_path","arguments":{"path":"x"}}"#);
    let (mut r, clock) = request(&a, &tool);
    assert!(!r.expire(clock.now()), "no expiry configured");
    r.expires_at = Some(clock.now().plus_millis(10).unwrap());
    assert!(!r.expire(clock.now()));
    clock.advance_millis(10);
    assert!(r.expire(clock.now()));
    assert_eq!(r.status, ApprovalStatus::Expired);
    let (mut r, clock) = request(&a, &tool);
    let long = "n".repeat(LIM_D2_REASON_CHARS + 1);
    assert_eq!(
        r.decide(
            UserDecision::Reject {
                note: Some(long),
                halt: false
            },
            Actor::Runtime,
            clock.now()
        ),
        Err(ApprovalError::NoteTooLong)
    );
    r.decide(
        UserDecision::Reject {
            note: Some("use git\u{200B} instead".into()),
            halt: true,
        },
        Actor::Runtime,
        clock.now(),
    )
    .unwrap();
    assert_eq!(r.status, ApprovalStatus::Rejected);
    assert_eq!(r.note.as_deref(), Some("use git instead"));
    assert!(r.halt);
    let (mut r, _) = request(&a, &tool);
    r.invalidate(InvalidationReason::GrantRevoked);
    assert_eq!(r.status, ApprovalStatus::Invalidated);
    assert_eq!(r.invalidation, Some(InvalidationReason::GrantRevoked));
    let json = serde_json::to_value(&r).unwrap();
    assert_eq!(json["status"], "invalidated");
    let back: ApprovalRequest = serde_json::from_value(json).unwrap();
    assert_eq!(back, r);
}

/// REQ-APR-008: the only path to `approved` is `decide` with a `UserDecision`; model text never reaches it.
#[test]
fn req_apr_008_no_inferred_approval() {
    let source = include_str!("../approval.rs");
    let assignments: Vec<usize> = source
        .match_indices(" = ApprovalStatus::Approved")
        .map(|(i, _)| i)
        .collect();
    assert_eq!(assignments.len(), 1, "exactly one assignment to approved");
    let decide_start = source.find("pub fn decide(").unwrap();
    let decide_end = source[decide_start..]
        .find("\n    }\n")
        .map(|e| decide_start + e)
        .unwrap();
    assert!(
        assignments[0] > decide_start && assignments[0] < decide_end,
        "the assignment lives inside decide"
    );
    assert!(
        !source[decide_start..decide_end].contains("model_reason"),
        "decide never reads the model's text"
    );
}
