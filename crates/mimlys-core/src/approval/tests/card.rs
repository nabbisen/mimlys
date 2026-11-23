//! REQ-APR-002: every argument is named, each value is bounded on its own
//! and says when it is cut, and the whole request is within LIM-I6.

use super::*;
use crate::limits::LIM_I3_TOOL_PARAMETERS;
use crate::tool::{ParamSchema, ParamType};
use serde_json::json;

fn string_param(name: &str, max_chars: usize) -> Param {
    Param {
        name: name.into(),
        ty: ParamType::String {
            min_chars: 0,
            max_chars,
            max_bytes: None,
        },
        optional: false,
        description: "p".into(),
    }
}

/// An external tool with the given parameters and destination arguments.
fn external_tool(name: &str, params: Vec<Param>, destinations: &[&str]) -> ToolDefinition {
    ToolDefinition {
        name: name.into(),
        description: "d".into(),
        input: ParamSchema { params },
        output: None,
        effect: crate::action::EffectView {
            kind: EffectKind::ExternalWrite,
            reversible: false,
            idempotent: false,
        },
        deadline_ms: 30_000,
        profile: ResourceProfile::External,
        destination_arguments: destinations.iter().map(|d| (*d).into()).collect(),
        provider: Provider::Mcp {
            server: "mail".into(),
        },
    }
}

fn send_message() -> ToolDefinition {
    external_tool(
        "mail.send_message",
        vec![
            string_param("body", 20_000),
            string_param("subject", 200),
            string_param("to", 200),
        ],
        &[],
    )
}

fn authorize(tool: &ToolDefinition, arguments: &Value, reason: Option<&str>) -> AuthorizedAction {
    let tools = vec![tool.clone()];
    let hooks = PermissiveHooks;
    let ctx = ValidationContext {
        task_running: true,
        pending_user_item: false,
        tools: &tools,
        rules: PlatformRules::Posix,
        grants: &hooks,
        effects: &hooks,
        budgets: &hooks,
        plan_required: false,
    };
    let mut decision = json!({"decision": "act", "tool": tool.name, "arguments": arguments});
    if let Some(reason) = reason {
        decision["reason"] = json!(reason);
    }
    match validate(&ctx, decision.to_string().as_bytes()) {
        Outcome::Authorized(a) => a,
        other => panic!("{other:?}"),
    }
}

const ROOT: &str = "/data/workspaces/t1/tree";

/// The request the runtime makes for the action: the rendering and the arguments in full.
fn card(tool: &ToolDefinition, action: &AuthorizedAction) -> ApprovalRequest {
    let ids = ScriptedIds::default();
    let clock = FakeClock::default();
    let rendered =
        render(action, tool, ROOT, &[], None, None, action.reason()).expect("the arguments fit");
    let views = argument_views(
        action.arguments(),
        &tool.destination_arguments,
        &tool.input.path_params().collect::<Vec<_>>(),
        Some(ROOT),
    );
    ApprovalRequest::new(
        ApprovalId::new(&ids),
        TaskId::new(&ids),
        ActionId::new(&ids),
        hash_bytes(b"action"),
        hash_bytes(b"grants"),
        rendered,
        action.reason(),
        clock.now(),
        None,
    )
    .with_arguments(views)
}

/// The Arguments line of a request.
fn arguments_line(request: &ApprovalRequest) -> &str {
    &request
        .rendered
        .lines
        .iter()
        .find(|l| l.label == "Arguments")
        .expect("an arguments line")
        .value
}

/// The characters a marker says are left out, in `line` from `from` on.
fn omitted_after(line: &str, from: usize) -> usize {
    let marker = line[from..]
        .split("… (")
        .nth(1)
        .and_then(|m| m.split(" more characters)").next())
        .unwrap_or_else(|| panic!("no marker in {line}"));
    marker.replace(' ', "").parse().expect(marker)
}

/// REQ-APR-002: a long `body` does not push `to` and `subject` off the card:
/// they come first, whole, and `body` says how much of it is cut.
#[test]
fn req_apr_002_every_argument_is_named_whatever_the_length_of_one() {
    let tool = send_message();
    let action = authorize(
        &tool,
        &json!({"body": "b".repeat(5_800), "subject": "Quarterly numbers", "to": "ada@example.org"}),
        Some("send the report"),
    );
    let request = card(&tool, &action);
    let line = arguments_line(&request);
    let to = line.find("to = \"ada@example.org\"").expect(line);
    let subject = line.find("subject = \"Quarterly numbers\"").expect(line);
    let body = line.find("body = \"").expect(line);
    assert!(to < body && subject < body, "{line}");
    // The marker counts exactly what the line leaves out of the body.
    let kept = line[body + "body = \"".len()..]
        .chars()
        .take_while(|c| *c == 'b')
        .count();
    assert_eq!(kept + omitted_after(line, body), 5_800, "{line}");
    assert!(kept < 5_800 && kept > 1_000, "{line}");
    assert!(request.display_text().chars().count() <= LIM_I6_APPROVAL_CHARS);
}

/// REQ-APR-002: a value that fits is shown whole and unmarked, however many
/// arguments there are.
#[test]
fn req_apr_002_values_that_fit_are_shown_whole() {
    let tool = send_message();
    let action = authorize(
        &tool,
        &json!({"body": "short", "subject": "s", "to": "ada@example.org"}),
        None,
    );
    let request = card(&tool, &action);
    assert_eq!(
        arguments_line(&request),
        "subject = \"s\" · body = \"short\" · to = \"ada@example.org\""
    );
}

/// REQ-APR-002: the arguments in full are kept sanitized, in the order of the card.
#[test]
fn req_apr_002_the_arguments_in_full_are_kept_in_the_order_of_the_card() {
    let tool = send_message();
    let body = format!("{}\u{202e}end", "b".repeat(5_800));
    let action = authorize(
        &tool,
        &json!({"body": body, "subject": "s", "to": "ada@example.org"}),
        None,
    );
    let request = card(&tool, &action);
    let views = request.arguments.as_ref().expect("recorded");
    let names: Vec<&str> = views.iter().map(|v| v.name.as_str()).collect();
    assert_eq!(names, ["subject", "to", "body"]);
    assert_eq!(
        views[2].value,
        format!("\"{}end\"", "b".repeat(5_800)),
        "the whole value, without the bidirectional control"
    );
    assert_eq!(arguments_text(views).lines().count(), 3);
}

/// REQ-APR-002: destinations and paths come first whatever the names are; the
/// others follow, the shortest value first.
#[test]
fn req_apr_002_destinations_and_paths_come_first() {
    let tool = external_tool(
        "mail.relay",
        vec![
            string_param("a_note", 100),
            string_param("z_url", 100),
            string_param("m_text", 100),
            Param {
                name: "y_file".into(),
                ty: ParamType::WorkspacePath,
                optional: false,
                description: "p".into(),
            },
        ],
        &["z_url"],
    );
    let action = authorize(
        &tool,
        &json!({"a_note": "a longer note", "z_url": "https://x.example/", "m_text": "m", "y_file": "dir/f.txt"}),
        None,
    );
    let request = card(&tool, &action);
    let names: Vec<&str> = request
        .arguments
        .as_ref()
        .unwrap()
        .iter()
        .map(|v| v.name.as_str())
        .collect();
    assert_eq!(names, ["z_url", "y_file", "m_text", "a_note"]);
    assert!(
        arguments_line(&request)
            .contains("y_file = \"dir/f.txt\" (resolved: \"/data/workspaces/t1/tree/dir/f.txt\")")
    );
}

/// REQ-APR-002: whatever the values and the reason, the request with its
/// reason is within LIM-I6, and every argument is named.
#[test]
fn req_apr_002_the_request_with_its_reason_is_within_lim_i6() {
    let tool = external_tool(
        "mail.bulk",
        (0..8)
            .map(|i| Param {
                optional: true,
                ..string_param(&format!("field_{i}"), 20_000)
            })
            .collect(),
        &["field_3"],
    );
    // A reason of quotes is the longest once quoted: every character takes two.
    let quoted_reason = "\"".repeat(LIM_D2_REASON_CHARS);
    for length in [0, 1, 40, 300, 480, 481, 700, 5_000, 20_000 - 60] {
        for count in 1..=8 {
            for reason in [None, Some("because"), Some(quoted_reason.as_str())] {
                let arguments: serde_json::Map<String, Value> = (0..count)
                    .map(|i| (format!("field_{i}"), json!("v".repeat(length + i * 7))))
                    .collect();
                let action = authorize(&tool, &Value::Object(arguments), reason);
                let request = card(&tool, &action);
                let chars = request.display_text().chars().count();
                assert!(
                    chars <= LIM_I6_APPROVAL_CHARS,
                    "{chars} characters for {count} values of {length}"
                );
                let line = arguments_line(&request);
                for i in 0..count {
                    assert!(line.contains(&format!("field_{i} = ")), "{line}");
                }
            }
        }
    }
}

/// REQ-APR-002: with the most parameters the limits allow, named at the length
/// of a tool name, and values too long for any share, every name is on the card.
#[test]
fn req_apr_002_every_key_appears_for_the_most_parameters_the_limits_allow() {
    let names: Vec<String> = (0..LIM_I3_TOOL_PARAMETERS)
        .map(|i| format!("{i}{}", "k".repeat(63)))
        .collect();
    let tool = external_tool(
        "mail.wide",
        names.iter().map(|n| string_param(n, 20_000)).collect(),
        &[],
    );
    tool.check().expect("a definition within the limits");
    let arguments: serde_json::Map<String, Value> = names
        .iter()
        .map(|n| (n.clone(), json!("v".repeat(20_000))))
        .collect();
    let action = authorize(&tool, &Value::Object(arguments), Some(&"r".repeat(500)));
    let request = card(&tool, &action);
    let line = arguments_line(&request);
    for name in &names {
        assert!(line.contains(&format!("{name} = \"")), "{name} is missing");
    }
    assert!(request.display_text().chars().count() <= LIM_I6_APPROVAL_CHARS);
}

/// REQ-APR-002: where the names cannot all be named, there is no request.
#[test]
fn req_apr_002_names_that_cannot_all_be_named_make_no_request() {
    let names: Vec<String> = (0..LIM_I3_TOOL_PARAMETERS)
        .map(|i| format!("{i}{}", "k".repeat(600)))
        .collect();
    let tool = external_tool(
        "mail.wide",
        names.iter().map(|n| string_param(n, 20_000)).collect(),
        &[],
    );
    let arguments: serde_json::Map<String, Value> =
        names.iter().map(|n| (n.clone(), json!("v"))).collect();
    let action = authorize(&tool, &Value::Object(arguments), None);
    let refused = render(&action, &tool, ROOT, &[], None, None, None).unwrap_err();
    assert!(refused.needed > refused.available, "{refused}");
}

/// REQ-APR-002, REQ-APR-003: a name is shown as it is only when it is plain;
/// else it is sanitized and quoted, so it cannot pass for a value or a separator.
#[test]
fn req_apr_002_a_name_cannot_pass_for_a_value_or_a_separator() {
    let odd = "x = \"1\" · y\u{202e}";
    let tool = external_tool(
        "mail.odd",
        vec![string_param("ok_name", 50), string_param(odd, 50)],
        &[],
    );
    let action = authorize(&tool, &json!({"ok_name": "v", odd: "w"}), None);
    let request = card(&tool, &action);
    let line = arguments_line(&request);
    assert!(line.contains("ok_name = \"v\""), "{line}");
    assert!(line.contains("\"x = \\\"1\\\" · y\" = \"w\""), "{line}");
    assert!(!line.contains('\u{202e}'));
    assert_eq!(
        request.display_text().lines().count(),
        request.rendered.lines.len(),
        "one line per label"
    );
}

/// REQ-APR-002: a cut stays inside the share it was given, counts what it
/// leaves out, and never splits a backslash sequence.
#[test]
fn req_apr_002_a_cut_counts_what_it_leaves_out() {
    let view = |value: String| ArgumentView {
        name: "b".into(),
        value,
        resolved: None,
    };
    for available in [60, 100, 333, 1_000] {
        // Every quote takes two characters in the line, as a backslash and a quote.
        let value = serde_json::to_string(&"\"".repeat(2_000)).unwrap();
        let line = arguments::fit(&[view(value)], available).unwrap();
        assert!(line.chars().count() <= available, "{line}");
        let kept = line.matches("\\\"").count();
        assert_eq!(kept + omitted_after(&line, 0), 2_000, "{line}");
        assert!(line.contains("\"…"), "the closing quote stays: {line}");
    }
    let line = arguments::fit(&[view(format!("\"{}\"", "x".repeat(5_300)))], 100).unwrap();
    assert_eq!(line.matches('x').count() + omitted_after(&line, 0), 5_300);
    // Thousands are grouped.
    assert_eq!(arguments::marker(5_200), "… (5 200 more characters)");
    assert_eq!(arguments::marker(999), "… (999 more characters)");
    assert_eq!(
        arguments::marker(1_234_567),
        "… (1 234 567 more characters)"
    );
    // Where not even the marker fits, there is nothing to show.
    let refused = arguments::fit(&[view(format!("\"{}\"", "x".repeat(6_000)))], 20).unwrap_err();
    assert_eq!(refused.available, 20);
}
