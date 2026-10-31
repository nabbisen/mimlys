use super::permissive::PermissiveHooks;
use super::*;
use crate::action::EffectKind;
use crate::rejection::MalformedProblem;
use crate::tool::{Param, ParamSchema, ParamType, Provider, ResourceProfile};

fn tools() -> Vec<ToolDefinition> {
    let effect = |kind| EffectView {
        kind,
        reversible: true,
        idempotent: true,
    };
    vec![
        ToolDefinition {
            name: "read_file".into(),
            description: "Read a file.".into(),
            input: ParamSchema {
                params: vec![
                    Param {
                        name: "path".into(),
                        ty: ParamType::WorkspacePath,
                        optional: false,
                        description: "file".into(),
                    },
                    Param {
                        name: "max_chars".into(),
                        ty: ParamType::Integer { min: 1, max: 1000 },
                        optional: true,
                        description: "size".into(),
                    },
                ],
            },
            output: None,
            effect: effect(EffectKind::WorkspaceRead),
            deadline_ms: 30_000,
            profile: ResourceProfile::Builtin,
            destination_arguments: vec![],
            provider: Provider::Builtin,
        },
        ToolDefinition {
            name: "git_push".into(),
            description: "Push.".into(),
            input: ParamSchema::default(),
            output: None,
            effect: EffectView {
                kind: EffectKind::ExternalWrite,
                reversible: false,
                idempotent: false,
            },
            deadline_ms: 60_000,
            profile: ResourceProfile::Host,
            destination_arguments: vec![],
            provider: Provider::Host,
        },
    ]
}

struct Ctx {
    tools: Vec<ToolDefinition>,
    hooks: PermissiveHooks,
}

impl Ctx {
    fn new() -> Self {
        Self {
            tools: tools(),
            hooks: PermissiveHooks,
        }
    }

    fn ctx(&self) -> ValidationContext<'_> {
        ValidationContext {
            task_running: true,
            pending_user_item: false,
            tools: &self.tools,
            rules: PlatformRules::Posix,
            grants: &self.hooks,
            effects: &self.hooks,
            budgets: &self.hooks,
        }
    }
}

fn code(outcome: &Outcome) -> Option<RejectionCode> {
    match outcome {
        Outcome::Rejected { rejection, .. } => Some(rejection.code),
        _ => None,
    }
}

/// ED §6.3: a valid action passes all stages and yields an authorized action with canonical arguments.
#[test]
fn ed_6_3_valid_action_is_authorized() {
    let c = Ctx::new();
    let out = validate(&c.ctx(), br#"{"decision":"act","tool":"read_file","arguments":{"path":"src/./lib.rs","max_chars":10},"reason":"look"}"#);
    let Outcome::Authorized(a) = out else {
        panic!("{out:?}")
    };
    assert_eq!(a.tool(), "read_file");
    assert_eq!(a.arguments()["path"], "src/lib.rs");
    assert_eq!(a.effect().kind, EffectKind::WorkspaceRead);
    assert!(!a.approval_required());
    assert_eq!(a.deadline_ms(), 30_000);
    assert_eq!(a.reason(), Some("look"));
}

/// ED §6.3 stage 1: the state is checked before anything is parsed.
#[test]
fn ed_6_3_stage_1_state() {
    let c = Ctx::new();
    let mut ctx = c.ctx();
    ctx.task_running = false;
    let out = validate(&ctx, b"garbage");
    assert_eq!(code(&out), Some(RejectionCode::State));
    assert!(out.decision_hash().is_none());
    let mut ctx = c.ctx();
    ctx.pending_user_item = true;
    let out = validate(&ctx, br#"{"decision":"complete","summary":"x"}"#);
    assert!(matches!(
        out,
        Outcome::Rejected {
            rejection: Rejection {
                detail: Detail::State {
                    reason: StateReason::PendingUserItem
                },
                ..
            },
            ..
        }
    ));
}

/// ED §6.3 stages 2–3: malformed and schema failures stop the pipeline before tool lookup.
#[test]
fn ed_6_3_stages_2_3_parse_and_schema() {
    let c = Ctx::new();
    let out = validate(&c.ctx(), b"not json");
    assert!(matches!(
        out,
        Outcome::Rejected {
            rejection: Rejection {
                detail: Detail::Malformed {
                    problem: MalformedProblem::NotObject,
                    ..
                },
                ..
            },
            decision_hash: None
        }
    ));
    let out = validate(&c.ctx(), br#"{"decision":"act","tool":"nope"}"#);
    assert_eq!(code(&out), Some(RejectionCode::Schema));
}

/// REQ-DEC-005 (stage 4): an invisible tool is `unknown_tool` with the visible list; invisible characters do not match.
#[test]
fn req_dec_005_unknown_tool() {
    let c = Ctx::new();
    let out = validate(
        &c.ctx(),
        "{\"decision\":\"act\",\"tool\":\"read\u{200B}_file\",\"arguments\":{}}".as_bytes(),
    );
    let Outcome::Rejected {
        rejection,
        decision_hash,
    } = out
    else {
        panic!()
    };
    assert_eq!(rejection.code, RejectionCode::UnknownTool);
    assert!(decision_hash.is_some());
    assert!(
        matches!(rejection.detail, Detail::UnknownTool { ref visible_tools, .. } if visible_tools == &vec!["read_file".to_owned(), "git_push".to_owned()])
    );
}

/// REQ-DEC-007 / INV-03 (stage 7): out-of-scope paths are denied byte-identically whether or not they exist.
#[test]
fn req_dec_007_out_of_scope_denied_identically() {
    let c = Ctx::new();
    let a = validate(
        &c.ctx(),
        br#"{"decision":"act","tool":"read_file","arguments":{"path":"../../etc/shadow"}}"#,
    );
    let b = validate(&c.ctx(), br#"{"decision":"act","tool":"read_file","arguments":{"path":"../../does-not-exist-anywhere"}}"#);
    let absolute = validate(
        &c.ctx(),
        br#"{"decision":"act","tool":"read_file","arguments":{"path":"/etc/shadow"}}"#,
    );
    let render = |o: &Outcome| match o {
        Outcome::Rejected { rejection, .. } => serde_json::to_string(rejection).unwrap(),
        other => panic!("{other:?}"),
    };
    assert_eq!(render(&a), render(&b));
    assert_eq!(render(&a), render(&absolute));
    assert!(render(&a).contains("path_outside_workspace"));
    assert!(
        !render(&a).contains("shadow"),
        "no path content in a denial"
    );
}

/// ED §6.3 stage 6: an invalid name is `invalid_argument`; stage 5 unknown property is `schema`.
#[test]
fn ed_6_3_stages_5_6_shape_and_names() {
    let c = Ctx::new();
    let out = validate(
        &c.ctx(),
        br#"{"decision":"act","tool":"read_file","arguments":{"path":"a","extra":1}}"#,
    );
    assert_eq!(code(&out), Some(RejectionCode::Schema));
    let out = validate(
        &c.ctx(),
        b"{\"decision\":\"act\",\"tool\":\"read_file\",\"arguments\":{\"path\":\"a\\u0000b\"}}",
    );
    assert_eq!(code(&out), Some(RejectionCode::InvalidArgument));
}

/// REQ-DEC-004: stage order — a decision failing both stage 7 (escape) and stage 8 (bounds) reports stage 7.
#[test]
fn req_dec_004_stage_order_7_before_8() {
    let c = Ctx::new();
    let out = validate(
        &c.ctx(),
        br#"{"decision":"act","tool":"read_file","arguments":{"path":"../x","max_chars":99999}}"#,
    );
    assert_eq!(code(&out), Some(RejectionCode::Denied));
    let out = validate(
        &c.ctx(),
        br#"{"decision":"act","tool":"read_file","arguments":{"path":"x","max_chars":99999}}"#,
    );
    assert!(
        matches!(out, Outcome::Rejected { rejection: Rejection { detail: Detail::InvalidArgument { ref path, max: Some(1000), .. }, .. }, .. } if path == "$.max_chars")
    );
}

/// ED §6.3 stages 7, 9, 10: the hooks deny, derive and reserve; a budget failure is `budget`.
#[test]
fn ed_6_3_hooks() {
    struct Deny;
    impl GrantCheck for Deny {
        fn check(&self, _t: &ToolDefinition, _a: &Map<String, Value>) -> Result<(), Denial> {
            Err(Denial::grant(DeniedReason::GrantMissing, "tool"))
        }
    }
    struct NoBudget;
    impl BudgetReservation for NoBudget {
        fn reserve(
            &self,
            _t: &ToolDefinition,
            _a: &Map<String, Value>,
            _d: &Derived,
        ) -> Result<i64, BudgetKind> {
            Err(BudgetKind::Deadline)
        }
    }
    let c = Ctx::new();
    let mut ctx = c.ctx();
    ctx.grants = &Deny;
    let out = validate(
        &ctx,
        br#"{"decision":"act","tool":"read_file","arguments":{"path":"a"}}"#,
    );
    assert!(
        matches!(out, Outcome::Rejected { rejection: Rejection { detail: Detail::Denied { reason: DeniedReason::GrantMissing, grant_kind: Some(ref k), .. }, .. }, .. } if k == "tool")
    );
    let mut ctx = c.ctx();
    ctx.budgets = &NoBudget;
    let out = validate(
        &ctx,
        br#"{"decision":"act","tool":"read_file","arguments":{"path":"a"}}"#,
    );
    assert!(matches!(
        out,
        Outcome::Rejected {
            rejection: Rejection {
                detail: Detail::Budget {
                    budget: BudgetKind::Deadline
                },
                ..
            },
            ..
        }
    ));
    // Stage 9 with the permissive hooks: an external write requires approval.
    let out = validate(
        &c.ctx(),
        br#"{"decision":"act","tool":"git_push","arguments":{}}"#,
    );
    let Outcome::Authorized(a) = out else {
        panic!()
    };
    assert!(a.approval_required());
}

/// ED §6.2: control decisions bypass the tool stages and carry their hash.
#[test]
fn ed_6_2_control_decisions() {
    let c = Ctx::new();
    let out = validate(&c.ctx(), br#"{"decision":"plan","steps":["a"]}"#);
    assert!(matches!(
        out,
        Outcome::Control {
            decision: ControlDecision::Plan { .. },
            ..
        }
    ));
    assert!(out.decision_hash().is_some());
    let out = validate(&c.ctx(), br#"{"decision":"give_up","reason":"no"}"#);
    assert!(matches!(
        out,
        Outcome::Control {
            decision: ControlDecision::GiveUp { .. },
            ..
        }
    ));
}

/// REQ-ARCH-005 / INV-01: `AuthorizedAction` has no public constructor and no public mutable field.
#[test]
fn req_arch_005_authorized_action_is_sealed() {
    let source = include_str!("../validate.rs");
    let struct_block = source
        .split("pub struct AuthorizedAction {")
        .nth(1)
        .and_then(|s| s.split('}').next())
        .unwrap();
    assert!(
        !struct_block.contains("pub "),
        "fields must stay private: {struct_block}"
    );
    assert!(source.contains("_sealed: Sealed"));
    assert!(
        !source.contains("pub fn new(")
            || !source.contains("impl AuthorizedAction {\n    pub fn new(")
    );
}
