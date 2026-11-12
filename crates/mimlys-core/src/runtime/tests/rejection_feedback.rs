//! A rejection reaches the model with its own hint and its detail, is stored
//! with the step, and is counted (handoff 002c; REQ-DEC-008, REQ-DEC-009,
//! REQ-DEC-007, REQ-DEC-010, REQ-OBS-008, REQ-TASK-004), through real steps over
//! the fakes.

use super::*;
use crate::native::DecisionTransport;
use crate::rejection::{Detail, RejectionCode};
use crate::spi::Message;

/// How a rejection's segment opens (the history segment is feedback too).
const REJECTION: &str = "<<feedback kind=\"rejection\"";

/// The bodies of the rejection segments of a request, in order.
fn feedback(messages: &[Message]) -> Vec<Value> {
    messages
        .iter()
        .filter(|m| m.content.starts_with(REJECTION))
        .map(|m| segment_body(&m.content))
        .collect()
}

/// The JSON body of one segment (`<<label …>>`, the body on one line, `<</label>>`).
fn segment_body(segment: &str) -> Value {
    let body = segment.lines().nth(1).expect("a segment has a body line");
    serde_json::from_str(body).expect("the body is JSON")
}

/// The last feedback segment of the request the model got after `steps` steps.
fn last_feedback(h: &Harness, request: usize) -> Value {
    feedback(&h.backend.requests()[request].messages)
        .pop()
        .expect("the request holds a feedback segment")
}

/// The words in backticks of a hint.
fn ticked(hint: &str) -> Vec<&str> {
    hint.split('`').skip(1).step_by(2).collect()
}

/// Every key a detail may carry (external design §6.4).
const DETAIL_KEYS: [&str; 12] = [
    "problem",
    "limit",
    "path",
    "expected",
    "allowed",
    "tool",
    "visible_tools",
    "min",
    "max",
    "reason",
    "grant_kind",
    "argument",
];

/// CLR-5, REQ-DEC-008: a hint names a key of the detail only when the segment carries it.
fn assert_hint_names_carried_keys(body: &Value) {
    let hint = body["hint"].as_str().expect("a hint");
    let detail = body.get("detail").and_then(Value::as_object);
    for word in ticked(hint) {
        if DETAIL_KEYS.contains(&word) {
            assert!(
                detail.is_some_and(|d| d.contains_key(word)),
                "the hint names `{word}`, which the segment does not carry: {body}"
            );
        }
    }
}

fn fetch_tool() -> ToolDefinition {
    ToolDefinition {
        name: "fetch_url".into(),
        description: "Fetch a page.".into(),
        input: ParamSchema {
            params: vec![Param {
                name: "url".into(),
                ty: ParamType::String {
                    min_chars: 1,
                    max_chars: 2048,
                    max_bytes: None,
                },
                optional: false,
                description: "the address".into(),
            }],
        },
        output: None,
        effect: EffectView {
            kind: EffectKind::ExternalRead,
            reversible: true,
            idempotent: true,
        },
        deadline_ms: 30_000,
        profile: ResourceProfile::Builtin,
        destination_arguments: vec!["url".into()],
        provider: Provider::Builtin,
    }
}

fn harness_with(decisions: Vec<Value>, tool_calls: bool) -> Harness {
    let backend = ScriptedBackend::with_decisions(decisions);
    let backend = if tool_calls {
        backend.tool_calls()
    } else {
        backend
    };
    let mut policy = Policy::shipped();
    policy.ceiling.tools.push("fetch_url".into());
    policy
        .profiles
        .get_mut("workspace")
        .unwrap()
        .tools
        .push("fetch_url".into());
    Harness::with_backend_and_tools(
        Arc::new(backend),
        policy,
        vec![],
        crate::limits::LIM_C1_CONTEXT_CHARS,
        vec![fetch_tool()],
    )
}

/// A runtime over the same stores, as a restarted process builds it.
fn restarted(h: &Harness) -> Runtime {
    Runtime::new(RuntimeDeps {
        persistence: h.persistence.clone(),
        backend: h.backend.clone(),
        workspaces: h.workspaces.clone(),
        policy: Policy::shipped(),
        ids: Arc::new(ScriptedIds::starting_at(1_000_000)),
        clock: h.clock.clone(),
        runtime_version: "0.1.0-test".into(),
        rules: PlatformRules::Posix,
        probe: crate::confine::Probe::unavailable("no backend in tests"),
        secrets: vec![],
        context_chars: crate::limits::LIM_C1_CONTEXT_CHARS,
    })
    .unwrap()
}

/// REQ-DEC-008, REQ-DEC-009, CLR-5: a path outside the workspace is told to
/// use a relative path (not to ask for a grant), with the reason as detail;
/// the rejected path is not repeated.
#[test]
fn req_dec_008_a_path_outside_the_workspace_gets_the_relative_path_hint() {
    let h = harness_with(
        vec![act("read_file", "../secret/x"), act("read_file", "a")],
        false,
    );
    let id = h.create();
    assert_eq!(h.step(id).outcome, StepOutcome::Rejected);
    h.tools
        .script("read_file", ExecutionOutcome::succeeded(json!({}), 1));
    h.step(id);
    let body = last_feedback(&h, 1);
    assert_eq!(body["code"], "denied");
    assert_eq!(
        body["hint"],
        crate::rejection::DeniedReason::PathOutsideWorkspace.hint()
    );
    assert!(
        body["hint"].as_str().unwrap().contains("relative"),
        "{body}"
    );
    assert!(
        !body["hint"].as_str().unwrap().contains("ask_user"),
        "{body}"
    );
    assert_eq!(body["detail"], json!({"reason": "path_outside_workspace"}));
    assert_hint_names_carried_keys(&body);
    let request: String = h.backend.requests()[1]
        .messages
        .iter()
        .map(|m| m.content.as_str())
        .collect();
    assert!(
        !request.contains("secret/x"),
        "the rejected decision is not repeated to the model"
    );
}

/// REQ-DEC-007, INV-03: the denial of a path outside the workspace is the same
/// bytes whether or not the path exists, and names no path.
#[test]
fn req_dec_007_the_denial_is_identical_whether_or_not_the_path_exists() {
    let segment = |path: &str| {
        let h = harness_with(vec![act("read_file", path), act("read_file", "a")], false);
        let id = h.create();
        h.step(id);
        h.tools
            .script("read_file", ExecutionOutcome::succeeded(json!({}), 1));
        h.step(id);
        h.backend.requests()[1]
            .messages
            .iter()
            .find(|m| m.content.starts_with(REJECTION))
            .expect("a feedback segment")
            .content
            .clone()
    };
    // `/etc/passwd` exists on a host; the other does not. Neither is read: the verdict is lexical.
    let existing = segment("/etc/passwd");
    let missing = segment("/etc/this-name-is-not-there");
    let escaping = segment("../../does-not-exist");
    assert_eq!(existing, missing);
    assert_eq!(existing, escaping);
    assert!(!existing.contains("passwd") && !existing.contains("not-there"));
}

/// REQ-DEC-008, REQ-MCP-005, CLR-5: an `http` address is told to use `https`.
#[test]
fn req_dec_008_an_http_url_gets_the_https_hint() {
    let h = harness_with(
        vec![
            json!({"decision": "act", "tool": "fetch_url", "arguments": {"url": "http://example.org/a"}}),
            act("read_file", "a"),
        ],
        false,
    );
    let id = h.create();
    assert_eq!(h.step(id).outcome, StepOutcome::Rejected);
    h.tools
        .script("read_file", ExecutionOutcome::succeeded(json!({}), 1));
    h.step(id);
    let body = last_feedback(&h, 1);
    assert_eq!(body["code"], "denied");
    assert_eq!(
        body["hint"],
        crate::rejection::DeniedReason::SchemeNotHttps.hint()
    );
    assert!(body["hint"].as_str().unwrap().contains("https"), "{body}");
    assert_eq!(body["detail"], json!({"reason": "scheme_not_https"}));
    assert_hint_names_carried_keys(&body);
}

/// REQ-DEC-008, CLR-5: a destination that is not granted names the grant kind to ask for.
#[test]
fn req_dec_008_an_ungranted_destination_names_its_grant_kind() {
    let h = harness_with(
        vec![
            json!({"decision": "act", "tool": "fetch_url", "arguments": {"url": "https://example.org/a"}}),
            act("read_file", "a"),
        ],
        false,
    );
    let id = h.create();
    assert_eq!(h.step(id).outcome, StepOutcome::Rejected);
    h.tools
        .script("read_file", ExecutionOutcome::succeeded(json!({}), 1));
    h.step(id);
    let body = last_feedback(&h, 1);
    assert_eq!(
        body["detail"],
        json!({"reason": "destination_not_granted", "grant_kind": "network"})
    );
    assert_hint_names_carried_keys(&body);
    // The event carries what the counters count (REQ-OBS-008).
    let rejected: Vec<_> = h
        .persistence
        .events()
        .into_iter()
        .filter_map(|e| match e.event {
            crate::event::Event::DecisionRejected {
                code,
                reason,
                grant_kind,
            } => Some((code, reason, grant_kind)),
            _ => None,
        })
        .collect();
    assert_eq!(
        rejected,
        [(
            RejectionCode::Denied,
            Some("destination_not_granted".to_owned()),
            Some("network".to_owned())
        )]
    );
}

/// REQ-DEC-008, CLR-5: a schema error's segment carries the `path` and the `expected` the hint names.
#[test]
fn req_dec_008_a_schema_error_carries_its_path() {
    let h = harness_with(
        vec![
            json!({"decision": "act", "tool": "read_file", "arguments": {"path": 5}}),
            act("read_file", "a"),
        ],
        false,
    );
    let id = h.create();
    assert_eq!(h.step(id).outcome, StepOutcome::Rejected);
    h.tools
        .script("read_file", ExecutionOutcome::succeeded(json!({}), 1));
    h.step(id);
    let body = last_feedback(&h, 1);
    assert_eq!(body["code"], "schema");
    let path = body["detail"]["path"].as_str().expect("a path");
    assert!(path.contains("path"), "{body}");
    assert!(body["detail"]["expected"].is_string(), "{body}");
    assert_hint_names_carried_keys(&body);
    let events: Vec<_> = h
        .persistence
        .events()
        .into_iter()
        .filter_map(|e| match e.event {
            crate::event::Event::DecisionRejected {
                reason, grant_kind, ..
            } => Some((reason, grant_kind)),
            _ => None,
        })
        .collect();
    assert_eq!(events, [(None, None)], "`schema` has no reason");
}

/// REQ-DEC-008, REQ-MDL-009: `unknown_tool` carries the tools the model can see, in both transports,
/// and the hint adjusted for the transport names no key the segment lacks.
#[test]
fn req_dec_008_an_unknown_tool_carries_the_visible_tools_in_both_transports() {
    for tool_calls in [false, true] {
        let h = harness_with(
            vec![
                json!({"decision": "act", "tool": "launch_missiles", "arguments": {}}),
                act("read_file", "a"),
            ],
            tool_calls,
        );
        let id = h.create();
        assert_eq!(h.step(id).outcome, StepOutcome::Rejected);
        h.tools
            .script("read_file", ExecutionOutcome::succeeded(json!({}), 1));
        h.step(id);
        let body = last_feedback(&h, 1);
        assert_eq!(body["code"], "unknown_tool", "{body}");
        assert_eq!(body["detail"]["tool"], "launch_missiles");
        let visible: Vec<String> = body["detail"]["visible_tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect();
        let mut seen = visible.clone();
        seen.sort();
        assert_eq!(
            seen,
            [
                "delete_path",
                "fetch_url",
                "list_dir",
                "read_file",
                "replace_in_file",
                "search_files",
                "write_file"
            ],
            "the tools the model already sees: the harness's, granted"
        );
        assert_hint_names_carried_keys(&body);
        if tool_calls {
            assert_eq!(
                body["hint"],
                RejectionCode::UnknownTool.hint_for(DecisionTransport::ToolCalls)
            );
        } else {
            assert_eq!(body["hint"], RejectionCode::UnknownTool.hint());
            assert!(body["hint"].as_str().unwrap().contains("visible_tools"));
        }
    }
}

/// REQ-DEC-008: every string of the detail is bounded by LIM-I9, whatever the model wrote.
#[test]
fn req_dec_008_the_detail_is_bounded() {
    // The model's own words that come back in a detail: an argument name of 5000 characters.
    let long_name = "t".repeat(5_000);
    let h = harness_with(
        vec![
            json!({"decision": "act", "tool": "read_file", "arguments": {"path": "a", long_name: 1}}),
            act("read_file", "a"),
        ],
        false,
    );
    let id = h.create();
    h.step(id);
    h.tools
        .script("read_file", ExecutionOutcome::succeeded(json!({}), 1));
    h.step(id);
    let body = last_feedback(&h, 1);
    assert_eq!(body["code"], "schema", "{body}");
    let path = body["detail"]["path"].as_str().unwrap();
    assert_eq!(path.chars().count(), crate::limits::LIM_I9_DETAIL_CHARS);
    assert!(
        !h.backend.requests()[1]
            .messages
            .iter()
            .any(|m| m.content.contains(&"t".repeat(300)))
    );
}

/// REQ-DEC-009, REQ-PERS-004: the same decision after a restart gets the same segment: the
/// detail is stored with the step and read back.
#[test]
fn req_dec_009_the_same_segment_after_a_restart() {
    let h = harness_with(
        vec![
            act("read_file", "../x"),
            act("read_file", "../x"),
            act("read_file", "a"),
        ],
        false,
    );
    let id = h.create();
    assert_eq!(h.step(id).outcome, StepOutcome::Rejected);
    h.tools
        .script("read_file", ExecutionOutcome::succeeded(json!({}), 1));
    // The next request is assembled by the first process.
    assert_eq!(h.step(id).outcome, StepOutcome::Rejected);
    let live = h.backend.requests()[1].messages.clone();
    // A new process over the same stores assembles the request for the third step.
    let again = restarted(&h);
    futures_block(again.run_step(id)).unwrap();
    let after = h.backend.requests()[2].messages.clone();
    let first = feedback(&live);
    assert_eq!(first.len(), 1);
    let second = feedback(&after);
    assert_eq!(second.len(), 2, "both rejections are in the history");
    assert_eq!(second[0], first[0], "the segment the live run saw");
    assert_eq!(
        second[1], first[0],
        "the same decision gets the same segment"
    );
    assert_eq!(second[0]["detail"]["reason"], "path_outside_workspace");
}

/// REQ-DEC-009: a step stored without a detail (written before schema version 3) has the
/// code's hint and no `detail` key, as it did before.
#[test]
fn req_dec_009_a_step_without_a_detail_reads_with_the_codes_hint() {
    let h = harness_with(vec![act("read_file", "../x"), act("read_file", "a")], false);
    let id = h.create();
    h.step(id);
    // As an older store holds it: the code, no detail.
    h.persistence.forget_rejection_details(id);
    h.tools
        .script("read_file", ExecutionOutcome::succeeded(json!({}), 1));
    h.step(id);
    let body = last_feedback(&h, 1);
    assert_eq!(
        body,
        json!({"code": "denied", "hint": RejectionCode::Denied.hint()})
    );
    let steps = h.persistence.load_steps(id).unwrap();
    assert_eq!(steps[0].rejection_detail, None);
    // And a rejection stored with one keeps it.
    assert_eq!(
        crate::runtime::context::feedback_segment(
            RejectionCode::State,
            Some(&Detail::State {
                reason: crate::rejection::StateReason::PendingUserItem
            })
        )
        .lines()
        .nth(1)
        .map(|b| serde_json::from_str::<Value>(b).unwrap()["detail"].clone()),
        Some(json!({"reason": "pending_user_item"}))
    );
}

/// REQ-DEC-010: the same decision with an unchanged tree halts `convergence` at LIM-T4;
/// with a tree that changed between decisions it does not (through the loop).
#[test]
fn req_dec_010_convergence_follows_the_trees_content_identity() {
    use crate::limits::LIM_T4_IDENTICAL_DECISIONS as T4;
    // A tree that changes before every decision: the same write never converges.
    let h = Harness::new((0..T4 + 2).map(|_| act("write_file", "a")).collect());
    for _ in 0..T4 + 2 {
        h.tools
            .script("write_file", ExecutionOutcome::succeeded(json!({}), 1));
    }
    let id = h.create();
    for round in 0..T4 + 1 {
        h.workspaces.set_tree(&format!("tree {round}"));
        let result = h.step(id);
        assert_eq!(
            result.state,
            TaskState::Running,
            "round {round}: the tree changed since the previous decision"
        );
    }
    let counters = h.runtime.get_task(id).unwrap().task.counters;
    assert_eq!(counters.identical_decisions, 1);

    // A tree that stays: the same decision converges at LIM-T4, however many writes it makes.
    let h = Harness::new((0..T4 + 2).map(|_| act("write_file", "a")).collect());
    for _ in 0..T4 + 2 {
        h.tools
            .script("write_file", ExecutionOutcome::succeeded(json!({}), 1));
    }
    let id = h.create();
    h.workspaces.set_tree("one tree");
    let mut steps = 0;
    loop {
        steps += 1;
        let result = h.step(id);
        if result.state != TaskState::Running {
            assert_eq!(
                result.state,
                TaskState::Halted {
                    halt_reason: HaltReason::Convergence
                }
            );
            break;
        }
        assert!(steps < 10, "the repetition must halt");
    }
    // The first decision saw the base tree, the later ones the written tree: LIM-T4 identical
    // decisions on the unchanged tree make the halt.
    assert_eq!(steps, T4 + 1);
}

/// REQ-TASK-004, REQ-TASK-012: a fork of a halted source onto its own scope is refused,
/// and the refusal names the source and both ways on; a fork elsewhere, or after the source is
/// cancelled, goes ahead.
#[test]
fn req_task_004_the_fork_of_a_halted_source_names_both_ways_on() {
    let h = Harness::new(vec![
        json!("not an object"),
        json!("not an object"),
        json!("not an object"),
    ]);
    let source = h.create();
    for _ in 0..3 {
        h.step(source);
    }
    assert_eq!(
        h.state(source),
        TaskState::Halted {
            halt_reason: HaltReason::BudgetRejections
        }
    );
    let fork = |scope: &str| {
        h.runtime.create_task(
            &h.session,
            CreateTask {
                goal: "retry".into(),
                scope: ScopeSpec {
                    root: scope.into(),
                    include: vec![],
                },
                profile: None,
                grants: vec![],
                budgets: None,
                deadline_ms: None,
                completion_check: None,
                hold: false,
                from_task: Some(source),
                from_snapshot: None,
            },
        )
    };
    let err = fork("/home/u/project").unwrap_err();
    assert_eq!(err.kind(), "conflict");
    let message = err.to_string();
    assert!(message.contains(&source.to_string()), "{message}");
    assert!(message.contains("--scope"), "{message}");
    assert!(
        message.contains(&format!("mimlys task cancel {source}")),
        "{message}"
    );
    // The refusal stays for a task that is not the fork's source.
    let other = h.runtime.create_task(
        &h.session,
        CreateTask {
            goal: "another".into(),
            scope: ScopeSpec {
                root: "/home/u/project".into(),
                include: vec![],
            },
            profile: None,
            grants: vec![],
            budgets: None,
            deadline_ms: None,
            completion_check: None,
            hold: false,
            from_task: None,
            from_snapshot: None,
        },
    );
    let message = other.unwrap_err().to_string();
    assert!(
        !message.contains("mimlys task cancel"),
        "only a fork of the halted task is given the remedies: {message}"
    );
    // The first way on: another scope.
    let elsewhere = fork("/home/u/elsewhere").unwrap();
    assert_eq!(elsewhere.state, TaskState::Running);
    h.runtime
        .cancel_task(&h.session, elsewhere.task_id)
        .unwrap();
    // The second: cancel the source first.
    h.runtime.cancel_task(&h.session, source).unwrap();
    assert_eq!(fork("/home/u/project").unwrap().state, TaskState::Running);
}
