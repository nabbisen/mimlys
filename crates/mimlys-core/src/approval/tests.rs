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

/// REQ-APR-004 / REQ-APR-005 / INV-06: binding to both hashes, one-shot consumption, invalidation on mismatch.
/// REQ-TEST-006 (hash binding, grant-set binding, one-shot consumption): a changed action hash and a changed grant-set hash each invalidate on consume, and a consumed approval cannot be consumed or decided again.
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
/// REQ-TEST-006 (expiry, invalidation on revocation): a request expires exactly at its `expires_at` and not before, and `invalidate(GrantRevoked)` makes it invalidated with its reason.
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

/// REQ-TEST-003 (approvals): every legal transition of external design §10.2
/// is taken and every other (status, operation) pair is refused with the
/// request left exactly as it was. The six statuses are tried against the
/// seven operations that can change one: approve, reject, expire,
/// invalidate, consume, and consume with a changed action hash or grant-set
/// hash. The legal ones are `pending → approved | rejected | expired |
/// invalidated` and `approved → consumed | invalidated`; the other 34 pairs
/// (a terminal status takes nothing, `consume` needs `approved`, `decide`
/// and `expire` need `pending`) change nothing.
#[test]
fn req_test_003_every_approval_transition_is_legal_or_refused() {
    use ApprovalStatus::*;
    #[derive(Clone, Copy, Debug)]
    enum Op {
        Approve,
        Reject,
        Expire,
        Invalidate,
        Consume,
        ConsumeOtherAction,
        ConsumeOtherGrants,
    }
    use Op::*;
    let legal = |from: ApprovalStatus, op: Op| -> Option<ApprovalStatus> {
        match (from, op) {
            (Pending, Approve) => Some(Approved),
            (Pending, Reject) => Some(Rejected),
            (Pending, Expire) => Some(Expired),
            (Pending, Invalidate) => Some(Invalidated),
            (Approved, Consume) => Some(Consumed),
            (Approved, ConsumeOtherAction | ConsumeOtherGrants) => Some(Invalidated),
            (Approved, Invalidate) => Some(Invalidated),
            _ => None,
        }
    };
    let (a, tool) =
        authorized(r#"{"decision":"act","tool":"delete_path","arguments":{"path":"x"}}"#);
    let (action, grants) = (hash_bytes(b"action"), hash_bytes(b"grants"));
    let (mut taken, mut refused) = (0, 0);
    for from in [Pending, Approved, Consumed, Rejected, Expired, Invalidated] {
        for op in [
            Approve,
            Reject,
            Expire,
            Invalidate,
            Consume,
            ConsumeOtherAction,
            ConsumeOtherGrants,
        ] {
            let (mut r, clock) = request(&a, &tool);
            r.status = from;
            r.expires_at = Some(clock.now());
            let before = r.clone();
            let expected = legal(from, op);
            let ctx = format!("{from:?} {op:?}");
            let wrong = |needed| Err(ApprovalError::WrongStatus { needed });
            match op {
                Approve => assert_eq!(
                    r.decide(UserDecision::Approve, Actor::Runtime, clock.now()),
                    if expected.is_some() {
                        Ok(())
                    } else {
                        wrong("pending")
                    },
                    "{ctx}"
                ),
                Reject => assert_eq!(
                    r.decide(
                        UserDecision::Reject {
                            note: None,
                            halt: false
                        },
                        Actor::Runtime,
                        clock.now()
                    ),
                    if expected.is_some() {
                        Ok(())
                    } else {
                        wrong("pending")
                    },
                    "{ctx}"
                ),
                Expire => assert_eq!(r.expire(clock.now()), expected.is_some(), "{ctx}"),
                Invalidate => r.invalidate(InvalidationReason::GrantRevoked),
                Consume => assert_eq!(
                    r.consume(&action, &grants),
                    if expected.is_some() {
                        Ok(())
                    } else {
                        wrong("approved")
                    },
                    "{ctx}"
                ),
                ConsumeOtherAction => assert_eq!(
                    r.consume(&hash_bytes(b"other"), &grants),
                    if expected.is_some() {
                        Err(ApprovalError::Invalidated(
                            InvalidationReason::ActionChanged,
                        ))
                    } else {
                        wrong("approved")
                    },
                    "{ctx}"
                ),
                ConsumeOtherGrants => assert_eq!(
                    r.consume(&action, &hash_bytes(b"other")),
                    if expected.is_some() {
                        Err(ApprovalError::Invalidated(
                            InvalidationReason::GrantsChanged,
                        ))
                    } else {
                        wrong("approved")
                    },
                    "{ctx}"
                ),
            }
            match expected {
                Some(to) => {
                    assert_eq!(r.status, to, "{ctx}");
                    taken += 1;
                }
                None => {
                    assert_eq!(r, before, "{ctx}: a refused operation changes nothing");
                    refused += 1;
                }
            }
        }
    }
    assert_eq!((taken, refused), (8, 34));
}

/// REQ-APR-008: the only path to `approved` is `decide` with a `UserDecision`; model text never reaches it.
#[test]
fn req_apr_008_no_inferred_approval() {
    // Line endings as the checkout made them: the positions below are relative.
    let source = include_str!("../approval.rs").replace("\r\n", "\n");
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
