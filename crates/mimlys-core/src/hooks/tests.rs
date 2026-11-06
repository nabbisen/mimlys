use super::*;
use crate::action::EffectKind;
use crate::clock::{Clock, FakeClock};
use crate::event::Actor;
use crate::grant::{Grant, GrantKind};
use crate::ids::{GrantId, ScriptedIds, TaskId};
use crate::path::PlatformRules;
use crate::policy::Policy;
use crate::rejection::{Detail, Rejection, RejectionCode};
use crate::tool::{Param, ParamSchema, Provider, ResourceProfile};
use crate::validate::{Outcome, ValidationContext, validate};

fn tools() -> Vec<ToolDefinition> {
    let read = EffectView {
        kind: EffectKind::WorkspaceRead,
        reversible: true,
        idempotent: true,
    };
    let write = EffectView {
        kind: EffectKind::WorkspaceWrite,
        reversible: true,
        idempotent: true,
    };
    let fetch = EffectView {
        kind: EffectKind::ExternalRead,
        reversible: true,
        idempotent: true,
    };
    vec![
        ToolDefinition {
            name: "read_file".into(),
            description: "r".into(),
            input: ParamSchema {
                params: vec![Param {
                    name: "path".into(),
                    ty: ParamType::WorkspacePath,
                    optional: false,
                    description: "p".into(),
                }],
            },
            output: None,
            effect: read,
            deadline_ms: 30_000,
            profile: ResourceProfile::Builtin,
            destination_arguments: vec![],
            provider: Provider::Builtin,
        },
        ToolDefinition {
            name: "write_file".into(),
            description: "w".into(),
            input: ParamSchema {
                params: vec![
                    Param {
                        name: "path".into(),
                        ty: ParamType::WorkspacePath,
                        optional: false,
                        description: "p".into(),
                    },
                    Param {
                        name: "content".into(),
                        ty: ParamType::String {
                            min_chars: 0,
                            max_chars: 1_000_000,
                            max_bytes: Some(1_000_000),
                        },
                        optional: false,
                        description: "c".into(),
                    },
                ],
            },
            output: None,
            effect: write,
            deadline_ms: 30_000,
            profile: ResourceProfile::Builtin,
            destination_arguments: vec![],
            provider: Provider::Builtin,
        },
        ToolDefinition {
            name: "rypsok.web_fetch".into(),
            description: "f".into(),
            input: ParamSchema {
                params: vec![Param {
                    name: "url".into(),
                    ty: ParamType::String {
                        min_chars: 1,
                        max_chars: 2048,
                        max_bytes: None,
                    },
                    optional: false,
                    description: "u".into(),
                }],
            },
            output: None,
            effect: fetch,
            deadline_ms: 15_000,
            profile: ResourceProfile::Builtin,
            destination_arguments: vec!["url".into()],
            provider: Provider::Mcp {
                server: "rypsok".into(),
            },
        },
    ]
}

use crate::action::EffectView;

struct Fixture {
    tools: Vec<ToolDefinition>,
    policy: Policy,
    grants: GrantSet,
    exclusions: ExclusionMatcher,
    clock: FakeClock,
}

impl Fixture {
    fn new(network: bool) -> Self {
        let policy = Policy::shipped();
        let ids = ScriptedIds::default();
        let clock = FakeClock::default();
        let task = TaskId::new(&ids);
        let mut grants = GrantSet::new();
        for t in ["read_file", "write_file", "rypsok.web_fetch"] {
            grants.add(Grant::issue(
                GrantId::new(&ids),
                task,
                GrantKind::Tool { name: t.into() },
                Actor::Runtime,
                clock.now(),
                None,
            ));
        }
        if network {
            grants.add(Grant::issue(
                GrantId::new(&ids),
                task,
                GrantKind::Network {
                    domains: vec!["example.org".into()],
                    ports: vec![],
                },
                Actor::Runtime,
                clock.now(),
                None,
            ));
        }
        let all: Vec<String> = policy
            .exclusions
            .materialize
            .iter()
            .chain(policy.exclusions.secrets.iter())
            .cloned()
            .collect();
        let exclusions = ExclusionMatcher::new(&all, CaseMode::Sensitive).unwrap();
        Self {
            tools: tools(),
            policy,
            grants,
            exclusions,
            clock,
        }
    }

    fn run(&self, raw: &str, remaining_ms: i64, workspace_remaining: Option<u64>) -> Outcome {
        let hooks = PolicyHooks {
            grants: &self.grants,
            profile: self.policy.profile("workspace").unwrap(),
            exclusions: &self.exclusions,
            case: CaseMode::Sensitive,
            task: TaskView {
                now: self.clock.now(),
                remaining_ms,
                workspace_remaining_bytes: workspace_remaining,
            },
            host_tools: &self.policy.host_tools,
        };
        let ctx = ValidationContext {
            task_running: true,
            pending_user_item: false,
            tools: &self.tools,
            rules: PlatformRules::Posix,
            grants: &hooks,
            effects: &hooks,
            budgets: &hooks,
        };
        validate(&ctx, raw.as_bytes())
    }
}

fn denied_reason(o: &Outcome) -> DeniedReason {
    match o {
        Outcome::Rejected {
            rejection:
                Rejection {
                    code: RejectionCode::Denied,
                    detail: Detail::Denied { reason, .. },
                    ..
                },
            ..
        } => *reason,
        other => panic!("expected denied: {other:?}"),
    }
}

/// REQ-TOOL-006 / RFC 011 §6 (stage 10): a host tool's deadline is its
/// declaration's `timeout_ms`, bounded by the task's remaining time; the
/// gateway definition's default applies to no declared tool.
#[test]
fn req_tool_006_host_tool_deadline_is_the_declared_timeout() {
    let mut f = Fixture::new(false);
    let ids = ScriptedIds::default();
    let task = TaskId::new(&ids);
    f.grants.add(Grant::issue(
        GrantId::new(&ids),
        task,
        GrantKind::HostTool {
            name: "probe".into(),
        },
        Actor::Runtime,
        f.clock.now(),
        None,
    ));
    f.policy.host_tools.insert(
        "probe".into(),
        crate::host_tool::HostToolDeclaration {
            name: "probe".into(),
            command: "/usr/bin/true".into(),
            args: vec![],
            allowed_args: vec!["ok".into()],
            arg_pattern: None,
            max_args: 1,
            effect: EffectKind::WorkspaceWrite,
            reversible: false,
            timeout_ms: 10_000,
            read_paths: vec![],
            env: Default::default(),
            env_allow: vec![],
            network: false,
        },
    );
    f.tools.push(ToolDefinition {
        name: "run_host_tool".into(),
        description: "h".into(),
        input: ParamSchema {
            params: vec![
                Param {
                    name: "tool".into(),
                    ty: ParamType::Enum {
                        values: vec!["probe".into()],
                    },
                    optional: false,
                    description: "t".into(),
                },
                Param {
                    name: "args".into(),
                    ty: ParamType::StringArray {
                        max_items: 1,
                        max_chars: 400,
                    },
                    optional: true,
                    description: "a".into(),
                },
            ],
        },
        output: None,
        effect: EffectView {
            kind: EffectKind::HostWrite,
            reversible: false,
            idempotent: false,
        },
        deadline_ms: 600_000,
        profile: ResourceProfile::Host,
        destination_arguments: vec![],
        provider: Provider::Builtin,
    });
    let raw =
        r#"{"decision":"act","tool":"run_host_tool","arguments":{"tool":"probe","args":["ok"]}}"#;
    let Outcome::Authorized(a) = f.run(raw, 60_000, None) else {
        panic!()
    };
    assert_eq!(a.deadline_ms(), 10_000, "the declaration's timeout_ms");
    let Outcome::Authorized(a) = f.run(raw, 4_000, None) else {
        panic!()
    };
    assert_eq!(
        a.deadline_ms(),
        4_000,
        "bounded by the task's remaining time"
    );
}

/// REQ-WS-003 (stage 7): excluded paths are denied with `path_excluded`; ordinary paths pass.
#[test]
fn req_ws_003_excluded_paths_denied() {
    let f = Fixture::new(false);
    let o = f.run(
        r#"{"decision":"act","tool":"read_file","arguments":{"path":".env"}}"#,
        1000,
        None,
    );
    assert_eq!(denied_reason(&o), DeniedReason::PathExcluded);
    let o = f.run(
        r#"{"decision":"act","tool":"read_file","arguments":{"path":"src/.git/config"}}"#,
        1000,
        None,
    );
    assert_eq!(denied_reason(&o), DeniedReason::PathExcluded);
    assert!(matches!(
        f.run(
            r#"{"decision":"act","tool":"read_file","arguments":{"path":"src/main.rs"}}"#,
            1000,
            None
        ),
        Outcome::Authorized(_)
    ));
    // Escapes are denied before this hook, as path_outside_workspace.
    let o = f.run(
        r#"{"decision":"act","tool":"read_file","arguments":{"path":"../.env"}}"#,
        1000,
        None,
    );
    assert_eq!(denied_reason(&o), DeniedReason::PathOutsideWorkspace);
}

/// K-02 / REQ-MCP-005 (stage 7): a destination argument needs a matching network grant.
/// INV-13 (external design §21.2), external network only through a matching
/// `network` grant: without a grant the destination is `DestinationNotGranted`;
/// with a grant for one host, a call to another host is too.
#[test]
fn k_02_destination_needs_network_grant() {
    let no_net = Fixture::new(false);
    let o = no_net.run(r#"{"decision":"act","tool":"rypsok.web_fetch","arguments":{"url":"https://example.org/x"}}"#, 1000, None);
    assert_eq!(denied_reason(&o), DeniedReason::DestinationNotGranted);
    let net = Fixture::new(true);
    assert!(matches!(net.run(r#"{"decision":"act","tool":"rypsok.web_fetch","arguments":{"url":"https://example.org/x"}}"#, 1000, None), Outcome::Authorized(_)));
    let o = net.run(
        r#"{"decision":"act","tool":"rypsok.web_fetch","arguments":{"url":"https://other.org/x"}}"#,
        1000,
        None,
    );
    assert_eq!(denied_reason(&o), DeniedReason::DestinationNotGranted);
}

/// REQ-APR-001 (stage 9) and REQ-TOOL-006 / REQ-WS-008 (stage 10): approval from the profile, deadline and quota.
/// INV-04 (external design §21.2): every authorized action carries a deadline
/// (the smaller of the tool's default and the task's remaining time), and an
/// action with no time left is rejected on the deadline budget instead of
/// being given an unbounded one.
#[test]
fn req_apr_001_tool_006_ws_008_effect_and_budget_hooks() {
    let mut f = Fixture::new(false);
    let raw =
        r#"{"decision":"act","tool":"write_file","arguments":{"path":"a.txt","content":"hello"}}"#;
    let Outcome::Authorized(a) = f.run(raw, 500, Some(10)) else {
        panic!()
    };
    assert!(!a.approval_required());
    assert_eq!(
        a.deadline_ms(),
        500,
        "the smaller of the tool default and the remaining time"
    );
    let Outcome::Authorized(a) = f.run(raw, 60_000, Some(10)) else {
        panic!()
    };
    assert_eq!(a.deadline_ms(), 30_000);
    assert!(matches!(
        f.run(raw, 0, None),
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
    assert!(matches!(
        f.run(raw, 1000, Some(4)),
        Outcome::Rejected {
            rejection: Rejection {
                detail: Detail::Budget {
                    budget: BudgetKind::Workspace
                },
                ..
            },
            ..
        }
    ));
    f.policy
        .profiles
        .get_mut("workspace")
        .unwrap()
        .approval_tools = vec!["write_file".into()];
    let Outcome::Authorized(a) = f.run(raw, 1000, None) else {
        panic!()
    };
    assert!(
        a.approval_required(),
        "a tool the profile lists needs approval"
    );
}
