//! The approval request in the loop (REQ-APR-002): it names every argument and
//! holds them all in full, and an action whose arguments cannot all be named
//! is refused before it is recorded.

use super::*;
use crate::limits::{LIM_I3_TOOL_PARAMETERS, LIM_I6_APPROVAL_CHARS};
use crate::rejection::RejectionCode;
use crate::spi::Persistence;

fn external_tool(name: &str, params: &[&str]) -> ToolDefinition {
    ToolDefinition {
        name: name.into(),
        description: "Sends a message.".into(),
        input: ParamSchema {
            params: params
                .iter()
                .map(|p| Param {
                    name: (*p).into(),
                    ty: ParamType::String {
                        min_chars: 0,
                        max_chars: 20_000,
                        max_bytes: None,
                    },
                    optional: false,
                    description: "a value".into(),
                })
                .collect(),
        },
        output: None,
        effect: EffectView {
            kind: EffectKind::ExternalWrite,
            reversible: false,
            idempotent: false,
        },
        deadline_ms: 30_000,
        profile: ResourceProfile::External,
        destination_arguments: vec![],
        provider: Provider::Mcp {
            server: "mail".into(),
        },
    }
}

fn harness(tool: ToolDefinition, arguments: Value) -> Harness {
    let mut policy = Policy::shipped();
    policy.ceiling.tools.push(tool.name.clone());
    policy
        .profiles
        .get_mut("workspace")
        .unwrap()
        .tools
        .push(tool.name.clone());
    let decision = json!({
        "decision": "act", "tool": tool.name, "arguments": arguments, "reason": "send the report"
    });
    Harness::with_backend_and_tools(
        Arc::new(ScriptedBackend::with_decisions(vec![decision])),
        policy,
        vec![],
        crate::limits::LIM_C1_CONTEXT_CHARS,
        vec![tool],
    )
}

/// REQ-APR-002: the request that waits for the user names every argument within
/// LIM-I6, and the task view holds every argument in full.
#[test]
fn req_apr_002_the_pending_request_holds_every_argument() {
    let tool = external_tool("mail.send_message", &["body", "subject", "to"]);
    let body = "b".repeat(5_800);
    let h = harness(
        tool,
        json!({"body": body, "subject": "Quarterly numbers", "to": "ada@example.org"}),
    );
    let id = h.create();
    assert_eq!(h.step(id).outcome, StepOutcome::Waited);
    let approval = h
        .runtime
        .get_task(id)
        .unwrap()
        .pending_approval
        .expect("a pending request");
    let text = approval.display_text();
    assert!(text.chars().count() <= LIM_I6_APPROVAL_CHARS, "{text}");
    assert!(text.contains("to = \"ada@example.org\""), "{text}");
    assert!(text.contains("subject = \"Quarterly numbers\""), "{text}");
    assert!(text.contains("more characters)"), "{text}");
    let arguments = approval.arguments.expect("every argument");
    let names: Vec<&str> = arguments.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(names, ["to", "subject", "body"]);
    assert_eq!(arguments[2].value, format!("\"{body}\""), "in full");
    // The JSON of the task view carries the field.
    let json = serde_json::to_value(h.runtime.get_task(id).unwrap()).unwrap();
    assert_eq!(
        json["pending_approval"]["arguments"][2]["value"],
        format!("\"{body}\"")
    );
}

/// REQ-APR-002: a request recorded without the arguments (one that waited
/// across an upgrade) shows them from the action.
#[test]
fn req_apr_002_a_request_without_recorded_arguments_shows_them_from_the_action() {
    let tool = external_tool("mail.send_message", &["body", "subject", "to"]);
    let h = harness(
        tool,
        json!({"body": "b".repeat(5_800), "subject": "s", "to": "ada@example.org"}),
    );
    let id = h.create();
    assert_eq!(h.step(id).outcome, StepOutcome::Waited);
    let task = h.runtime.get_task(id).unwrap();
    let mut approval = task.pending_approval.clone().unwrap();
    approval.arguments = None;
    let event = h
        .persistence
        .events_since(0, Some(id), 1000)
        .unwrap()
        .pop()
        .unwrap()
        .record;
    h.persistence
        .record_approval_decision(&task.task, &approval, &event)
        .unwrap();
    let shown = h.runtime.get_task(id).unwrap().pending_approval.unwrap();
    let arguments = shown.arguments.expect("filled in from the action");
    assert_eq!(arguments.len(), 3);
    assert_eq!(arguments[2].name, "body");
    assert_eq!(arguments[2].value.chars().count(), 5_802);
}

/// REQ-APR-002: a tool whose argument names cannot all be named within LIM-I6
/// is refused before the action is recorded: nothing waits, nothing runs.
#[test]
fn req_apr_002_arguments_that_cannot_all_be_named_are_refused() {
    let names: Vec<String> = (0..LIM_I3_TOOL_PARAMETERS)
        .map(|i| format!("{i}{}", "k".repeat(600)))
        .collect();
    let params: Vec<&str> = names.iter().map(String::as_str).collect();
    let tool = external_tool("mail.wide", &params);
    let arguments: serde_json::Map<String, Value> =
        names.iter().map(|n| (n.clone(), json!("v"))).collect();
    let h = harness(tool, Value::Object(arguments));
    let id = h.create();
    assert_eq!(h.step(id).outcome, StepOutcome::Rejected);
    assert_eq!(h.state(id), TaskState::Running, "no request waits");
    let step = &h.persistence.load_steps(id).unwrap()[0];
    assert_eq!(step.rejection, Some(RejectionCode::Denied));
    assert_eq!(
        step.rejection_detail.as_ref().unwrap().reason_name(),
        Some("arguments_do_not_fit")
    );
    assert!(
        h.persistence.load_actions(id).unwrap().is_empty(),
        "no action was recorded"
    );
    assert!(h.runtime.get_task(id).unwrap().pending_approval.is_none());
    assert!(h.tools.calls().is_empty());
}
