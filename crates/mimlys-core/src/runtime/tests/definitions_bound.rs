//! The definitions never fail a step (handoff 009i, item 2; audit A-32;
//! REQ-CTX-005): a grant that would make the visible definitions exceed LIM-I1
//! is refused, naming the tool and the sizes, whether it comes from the command
//! line or from a profile at task creation; the last guard, for a set that
//! exceeds the limit all the same at a step boundary, halts the task once with a
//! stated reason; and a built-in step is never denied for another tool's size.

use super::*;
use crate::grant::Grant;
use crate::ids::GrantId;
use crate::spi::ToolProvider;
use crate::tool::rendered_chars;

/// A tool whose definition renders to a little over `chars` characters.
fn big(name: &str, chars: usize) -> ToolDefinition {
    ToolDefinition {
        description: "d".repeat(chars),
        ..tool(name, EffectKind::WorkspaceRead)
    }
}

fn bigs() -> Vec<ToolDefinition> {
    ["big_a", "big_b", "big_c"]
        .iter()
        .map(|n| big(n, 3000))
        .collect()
}

/// The policy with the big tools under the ceiling.
fn policy() -> Policy {
    let mut policy = Policy::shipped();
    for t in bigs() {
        policy.ceiling.tools.push(t.name);
    }
    policy
}

fn harness(policy: Policy, decisions: Vec<Value>) -> Harness {
    Harness::with_backend_and_tools(
        Arc::new(ScriptedBackend::with_decisions(decisions)),
        policy,
        vec![],
        crate::limits::LIM_C1_CONTEXT_CHARS,
        bigs(),
    )
}

fn grant_tool(h: &Harness, id: TaskId, name: &str) -> Result<crate::grant::Grant, RuntimeError> {
    h.runtime.grant_capability(
        &h.session,
        id,
        GrantKind::Tool {
            name: name.to_owned(),
        },
    )
}

/// The definitions of the named tools, as the model would be shown them.
fn rendered(h: &Harness, names: &[&str]) -> usize {
    let defs: Vec<ToolDefinition> = h
        .tools
        .definitions()
        .into_iter()
        .filter(|d| names.contains(&d.name.as_str()))
        .collect();
    assert_eq!(defs.len(), names.len());
    rendered_chars(&defs)
}

const SIX: [&str; 6] = [
    "read_file",
    "write_file",
    "list_dir",
    "search_files",
    "replace_in_file",
    "delete_path",
];

/// REQ-CTX-005 (handoff 009i, item 2b): a `tool` grant that would make the visible
/// definitions exceed LIM-I1 is refused, `denied`, with the tool's name and the
/// sizes (its own, those before it, the total and the limit), and nothing is
/// recorded: the grants are what they were. A grant that fits is made.
#[test]
fn req_ctx_005_a_grant_that_would_exceed_lim_i1_is_refused_naming_the_tool_and_the_sizes() {
    let h = harness(policy(), vec![]);
    let id = h.create();
    let before = h.persistence.load_grants(id).unwrap().len();
    // The six tools, and one big one: about 3 700 characters.
    grant_tool(&h, id, "big_a").unwrap();
    grant_tool(&h, id, "big_b").unwrap();
    let granted = h.persistence.load_grants(id).unwrap().len();
    assert_eq!(granted, before + 2);
    let refused = grant_tool(&h, id, "big_c").unwrap_err();
    let RuntimeError::Denied { detail } = &refused else {
        panic!("not denied: {refused:?}");
    };
    let alone = rendered(&h, &["big_c"]);
    let others = rendered(&h, &[&SIX[..], &["big_a", "big_b"]].concat());
    let total = rendered(&h, &[&SIX[..], &["big_a", "big_b", "big_c"]].concat());
    assert!(total > 8000 && others <= 8000, "{others} {total}");
    assert!(detail.contains("big_c"), "{detail}");
    for number in [alone, others, total, 8000] {
        assert!(
            detail.contains(&number.to_string()),
            "{number} is not in: {detail}"
        );
    }
    assert!(detail.contains("LIM-I1"), "{detail}");
    assert_eq!(h.persistence.load_grants(id).unwrap().len(), granted);
    // A grant of a tool that is visible already changes nothing, and fits.
    grant_tool(&h, id, "big_a").unwrap();
    // After a revocation the refused grant fits.
    let big_b = h
        .persistence
        .load_grants(id)
        .unwrap()
        .into_iter()
        .find(|g| matches!(g.kind(), GrantKind::Tool { name } if name == "big_b"))
        .unwrap();
    h.runtime
        .revoke_capability(&h.session, id, big_b.grant_id)
        .unwrap();
    // `big_a` was granted twice: one of its grants is still active.
    grant_tool(&h, id, "big_c").unwrap();
}

/// REQ-CTX-005 (handoff 009i, item 2b): a profile whose tools make the
/// definitions exceed LIM-I1 refuses the creation of the task, naming the tool
/// and the sizes; no task is made. Grants of the request are judged with the
/// profile's, as one set.
#[test]
fn req_ctx_005_a_profile_that_would_exceed_lim_i1_refuses_the_task_naming_the_tool() {
    let mut policy = policy();
    policy
        .profiles
        .get_mut("workspace")
        .unwrap()
        .tools
        .extend(bigs().into_iter().map(|t| t.name));
    let h = harness(policy, vec![]);
    let request = |grants: Vec<GrantKind>, profile: Option<&str>| crate::runtime::CreateTask {
        goal: "g".into(),
        scope: ScopeSpec {
            root: "/home/u/project".into(),
            include: vec![],
        },
        profile: profile.map(str::to_owned),
        grants,
        budgets: None,
        deadline_ms: None,
        completion_check: None,
        hold: false,
        from_task: None,
        from_snapshot: None,
    };
    let refused = h
        .runtime
        .create_task(&h.session, request(vec![], None))
        .unwrap_err();
    let RuntimeError::Denied { detail } = &refused else {
        panic!("not denied: {refused:?}");
    };
    assert!(detail.contains("big_c"), "the tool that crosses: {detail}");
    assert!(
        detail.contains("LIM-I1") && detail.contains("8000"),
        "{detail}"
    );
    assert!(
        h.persistence.list_tasks().unwrap().is_empty(),
        "no task is made"
    );
    // The profile that fits, with the request's grants that do not.
    let kinds = |names: &[&str]| -> Vec<GrantKind> {
        names
            .iter()
            .map(|n| GrantKind::Tool {
                name: (*n).to_owned(),
            })
            .collect()
    };
    let refused = h
        .runtime
        .create_task(
            &h.session,
            request(kinds(&["big_a", "big_b", "big_c"]), Some("read_only")),
        )
        .unwrap_err();
    assert!(
        matches!(&refused, RuntimeError::Denied { detail } if detail.contains("big_c")),
        "{refused:?}"
    );
    assert!(h.persistence.list_tasks().unwrap().is_empty());
    h.runtime
        .create_task(
            &h.session,
            request(kinds(&["big_a", "big_b"]), Some("read_only")),
        )
        .unwrap();
}

/// Makes the grants of a task as a task made before the refusal existed has
/// them: written to the store, not through `grant_capability`.
fn grant_around_the_refusal(h: &Harness, id: TaskId, names: &[&str]) {
    for name in names {
        let grant = Grant::issue(
            GrantId::new(&ScriptedIds::default()),
            id,
            GrantKind::Tool {
                name: (*name).to_owned(),
            },
            h.session.actor(),
            crate::clock::Clock::now(h.clock.as_ref()),
            None,
        );
        let event = h.runtime.event(
            h.session.actor(),
            Some(id),
            None,
            None,
            crate::event::Event::GrantIssued {
                grant_id: grant.grant_id,
            },
        );
        h.persistence.record_grant(&grant, &event).unwrap();
    }
}

/// REQ-CTX-005 (handoff 009i, item 2c): the last guard. A task whose visible
/// definitions exceed LIM-I1 at a step boundary (made before the refusal existed)
/// is halted once, with its own reason, and no step is denied: the step is a
/// `Halted` outcome, the model is never asked, and a second step of the halted
/// task is not a denial either. After a revocation the task resumes and runs.
#[test]
fn req_ctx_005_the_last_guard_halts_the_task_once_and_does_not_fail_every_step() {
    let h = harness(
        policy(),
        vec![json!({"decision": "complete", "summary": "done"})],
    );
    let id = h.create();
    grant_around_the_refusal(&h, id, &["big_a", "big_b", "big_c"]);
    let first = h.step(id);
    assert_eq!(first.outcome, StepOutcome::Halted);
    let halted = TaskState::Halted {
        halt_reason: HaltReason::DefinitionsTooLarge,
    };
    assert_eq!(first.state, halted);
    assert_eq!(h.state(id), halted);
    assert!(h.backend.requests().is_empty(), "the model is not asked");
    let changes = |h: &Harness| {
        h.persistence
            .events()
            .iter()
            .filter(|e| matches!(e.event, crate::event::Event::TaskStateChanged { .. }))
            .count()
    };
    let count = changes(&h);
    // Not a denial: the task is halted, and a step of it is a state error.
    let again = futures_block(h.runtime.run_step(id));
    assert!(
        matches!(again, Err(RuntimeError::State(_))),
        "a halted task has no step: {again:?}"
    );
    assert_eq!(changes(&h), count, "the halt is recorded once");
    // The user revokes a grant and resumes: the task runs.
    let big_c = h
        .persistence
        .load_grants(id)
        .unwrap()
        .into_iter()
        .find(|g| matches!(g.kind(), GrantKind::Tool { name } if name == "big_c"))
        .unwrap();
    h.runtime
        .revoke_capability(&h.session, id, big_c.grant_id)
        .unwrap();
    h.runtime.resume_task(&h.session, id, None).unwrap();
    let step = h.step(id);
    assert_eq!(step.outcome, StepOutcome::Completed);
    assert_eq!(h.backend.requests().len(), 1);
}

/// REQ-CTX-005 (handoff 009i, item 2c): a built-in step is never denied for the
/// size of another tool's definition. The provider holds three big tools that
/// the task is not granted; the model reads a file, and the task completes.
#[test]
fn req_ctx_005_a_built_in_step_is_never_denied_for_the_size_of_a_tool_it_is_not_granted() {
    let h = harness(
        policy(),
        vec![
            json!({"decision": "act", "tool": "read_file", "arguments": {"path": "a.rs"}}),
            json!({"decision": "complete", "summary": "done"}),
        ],
    );
    h.tools.script(
        "read_file",
        ExecutionOutcome::succeeded(json!({"content": "x"}), 1),
    );
    let id = h.create();
    assert_eq!(h.step(id).outcome, StepOutcome::Continued);
    assert_eq!(h.step(id).outcome, StepOutcome::Completed);
    // The definitions the model read hold the six tools and none of the big ones.
    let first = &h.backend.requests()[0].messages[0].content;
    assert!(first.contains("read_file") && !first.contains("big_a"));
}
