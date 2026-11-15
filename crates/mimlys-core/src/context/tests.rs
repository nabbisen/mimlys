use super::*;
use serde_json::json;

/// REQ-CTX-002: a segment has marker lines with quoted attributes and a one-line JSON body.
#[test]
fn req_ctx_002_segment_shape() {
    let s = write_segment(
        SegmentLabel::Observation,
        &[("tool", "run_script"), ("status", "failed")],
        &json!({"stdout": "x"}),
    );
    assert_eq!(
        s,
        "<<observation tool=\"run_script\" status=\"failed\">>\n{\"stdout\":\"x\"}\n<</observation>>\n"
    );
}

/// REQ-CTX-004 rule 2: runs of identical lines collapse to one line and a
/// marker; runs of two stay as they are.
#[test]
fn req_ctx_004_repeated_lines_collapse() {
    let text = "a\nb\nb\nb\nb\nc\nc\nd";
    assert_eq!(
        collapse_repeated_lines(text),
        "a\nb\n[… repeated 4 times …]\nc\nc\nd"
    );
    assert_eq!(collapse_repeated_lines(""), "");
    assert_eq!(collapse_repeated_lines("x\nx"), "x\nx");
    let many: String = std::iter::repeat_n("warning: x", 40)
        .collect::<Vec<_>>()
        .join("\n");
    let collapsed = collapse_repeated_lines(&many);
    assert_eq!(collapsed, "warning: x\n[… repeated 40 times …]");
}

/// LIM-C4: a summary renders within the limit, dropping the first line
/// before cutting.
#[test]
fn lim_c4_summary_within_limit() {
    let summary = StepSummary {
        step: 7,
        decision: "act".into(),
        tool: Some("run_script".into()),
        path: None,
        status: Some("failed".into()),
        cause: Some("tool_error".into()),
        rejection: None,
        outcome: None,
        notices: vec!["output_truncated".into()],
        first_line: Some("x".repeat(400)),
    };
    let rendered = summary.render(300);
    assert!(rendered.chars().count() <= 300);
    assert!(!rendered.contains("first_line"), "{rendered}");
    let parsed: StepSummary = serde_json::from_str(&rendered).unwrap();
    assert_eq!(parsed.step, 7);
    assert_eq!(parsed.cause.as_deref(), Some("tool_error"));
    let short = StepSummary {
        first_line: Some("ok".into()),
        ..summary
    };
    assert!(short.render(300).contains("\"first_line\":\"ok\""));
    assert_eq!(short.render(10).chars().count(), 10);
}

/// REQ-CTX-007: values and their JSON-escaped forms are replaced by the
/// name marker; short values are not scanned.
#[test]
fn req_ctx_007_redactor() {
    let r = Redactor::new(&[
        ("backend.api_key".into(), "pa\"ss\\word-123".into()),
        ("short".into(), "abc".into()),
    ]);
    let (text, names) = r.redact("token=pa\"ss\\word-123 and {\"t\":\"pa\\\"ss\\\\word-123\"} abc");
    assert_eq!(
        text,
        "token=[redacted:backend.api_key] and {\"t\":\"[redacted:backend.api_key]\"} abc"
    );
    assert_eq!(names, vec!["backend.api_key".to_owned()]);
    let mut value = json!({"a": ["pa\"ss\\word-123", 1], "b": {"c": "clean"}});
    let found = r.redact_value(&mut value);
    assert_eq!(found, vec!["backend.api_key".to_owned()]);
    assert_eq!(value["a"][0], "[redacted:backend.api_key]");
    assert_eq!(value["b"]["c"], "clean");
    assert!(Redactor::new(&[("x".into(), "1234567".into())]).is_empty());
    assert!(!Redactor::new(&[("x".into(), "12345678".into())]).is_empty());
}

/// REQ-CTX-010: the prefix measure counts the shared serialized head.
#[test]
fn req_ctx_010_common_prefix() {
    let m = |role: Role, s: &str| Message::text(role, s);
    let a = vec![
        m(Role::System, "sys"),
        m(Role::User, "goal"),
        m(Role::User, "x"),
    ];
    let b = vec![
        m(Role::System, "sys"),
        m(Role::User, "goal"),
        m(Role::User, "y"),
    ];
    // "system\nsys\nuser\ngoal\nuser\n" is shared: 26 characters.
    assert_eq!(common_prefix_chars(&a, &b), 26);
    assert_eq!(common_prefix_chars(&a, &a), 28);
    assert_eq!(content_chars(&a), 8);
    assert_eq!(common_prefix_chars(&[], &a), 0);
}

/// REQ-CTX-007 (handoff 009g, item 5): the keys of an object are text too. A key
/// that holds a secret is redacted and keeps its value; two keys that come to
/// the same text keep both values; the failure cause of a tool is redacted.
#[test]
fn req_ctx_007_redacts_keys_and_the_cause() {
    let r = Redactor::new(&[("servers.x.env.TOKEN".into(), "tok-9f8e7d6c5b4a".into())]);
    let mut value = json!({
        "tok-9f8e7d6c5b4a": 1,
        "[redacted:servers.x.env.TOKEN]": 2,
        "plain": {"tok-9f8e7d6c5b4a-inner": "tok-9f8e7d6c5b4a"},
    });
    let found = r.redact_value(&mut value);
    assert_eq!(found, ["servers.x.env.TOKEN"]);
    let text = value.to_string();
    assert!(!text.contains("tok-9f8e7d6c5b4a"), "{text}");
    let object = value.as_object().unwrap();
    assert_eq!(object.len(), 3, "{text}");
    let mut numbers: Vec<i64> = object.values().filter_map(Value::as_i64).collect();
    numbers.sort();
    assert_eq!(numbers, [1, 2], "both values are kept");
    assert_eq!(
        value["plain"]["[redacted:servers.x.env.TOKEN]-inner"],
        "[redacted:servers.x.env.TOKEN]"
    );
    let mut cause = crate::action::FailureCause::ToolError {
        detail: "denied: token tok-9f8e7d6c5b4a is not accepted".into(),
    };
    assert_eq!(r.redact_cause(&mut cause), ["servers.x.env.TOKEN"]);
    assert_eq!(
        cause,
        crate::action::FailureCause::ToolError {
            detail: "denied: token [redacted:servers.x.env.TOKEN] is not accepted".into()
        }
    );
    let mut timeout = crate::action::FailureCause::Timeout;
    assert!(r.redact_cause(&mut timeout).is_empty());
}
