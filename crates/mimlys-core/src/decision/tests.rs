use super::*;
use crate::rejection::RejectionCode;

fn reject(input: &str) -> Rejection {
    parse_strict(input.as_bytes()).unwrap_err()
}

/// REQ-DEC-002: the five kinds parse into their fields.
#[test]
fn req_dec_002_five_decision_kinds_parse() {
    let act = parse_strict(
        br#"{"decision":"act","tool":"read_file","arguments":{"path":"a"},"reason":"look"}"#,
    )
    .unwrap();
    assert_eq!(act.kind(), "act");
    assert!(
        matches!(&act, Decision::Act { tool, reason: Some(r), .. } if tool == "read_file" && r == "look")
    );
    let plan = parse_strict(br#"{"decision":"plan","steps":["a","b"]}"#).unwrap();
    assert!(matches!(plan, Decision::Plan { steps } if steps.len() == 2));
    assert!(matches!(
        parse_strict(br#"{"decision":"ask_user","question":"Which?"}"#).unwrap(),
        Decision::AskUser { .. }
    ));
    assert!(matches!(
        parse_strict(br#"{"decision":"complete","summary":"done"}"#).unwrap(),
        Decision::Complete { .. }
    ));
    assert!(matches!(
        parse_strict(br#"{"decision":"give_up","reason":"stuck"}"#).unwrap(),
        Decision::GiveUp { .. }
    ));
    assert_eq!(DECISION_SCHEMA_VERSION, 1);
}

/// REQ-DEC-003 / REQ-SEC-009: malformed inputs are rejected with the matching problem and never repaired.
/// REQ-SEC-013 (hostile corpus, repository guards §3a), hostile decisions: malformed model output.
#[test]
fn req_dec_003_malformed_inputs() {
    let m = |input: &str| match reject(input).detail {
        crate::rejection::Detail::Malformed { problem, .. } => problem,
        other => panic!("expected malformed, got {other:?}"),
    };
    assert_eq!(m(""), MalformedProblem::NotObject);
    assert_eq!(m("[1]"), MalformedProblem::NotObject);
    assert_eq!(m("\"text\""), MalformedProblem::NotObject);
    assert_eq!(
        m("{\"decision\":\"complete\",\"summary\":\"x\""),
        MalformedProblem::NotObject
    );
    assert_eq!(
        m("Sure! {\"decision\":\"complete\",\"summary\":\"x\"}"),
        MalformedProblem::NotObject
    );
    assert_eq!(
        m("{\"decision\":\"complete\",\"summary\":\"x\"} done"),
        MalformedProblem::TrailingText
    );
    assert_eq!(
        m(
            "{\"decision\":\"complete\",\"summary\":\"x\"}{\"decision\":\"complete\",\"summary\":\"y\"}"
        ),
        MalformedProblem::MultipleValues
    );
    assert_eq!(
        m("{\"decision\":\"complete\",\"decision\":\"give_up\",\"summary\":\"x\"}"),
        MalformedProblem::DuplicateKey
    );
    assert_eq!(
        m("{\"decision\":\"act\",\"tool\":\"t\",\"arguments\":{\"a\":1,\"a\":2}}"),
        MalformedProblem::DuplicateKey
    );
    let big = format!(
        "{{\"decision\":\"complete\",\"summary\":\"{}\"}}",
        "x".repeat(LIM_D1_DECISION_BYTES)
    );
    let r = reject(&big);
    assert_eq!(r.code, RejectionCode::Malformed);
    assert_eq!(m(&big), MalformedProblem::TooLarge);
    // Whitespace around the object is fine.
    assert!(parse_strict(b"  \n{\"decision\":\"complete\",\"summary\":\"x\"}\n ").is_ok());
}

/// REQ-DEC-002: unknown fields, wrong types and bad kinds are `schema` rejections with a path.
#[test]
fn req_dec_002_schema_rejections_carry_paths() {
    let p = |input: &str| {
        let r = reject(input);
        assert_eq!(r.code, RejectionCode::Schema, "{input}");
        match r.detail {
            crate::rejection::Detail::Schema { path, allowed, .. } => (path, allowed),
            other => panic!("{other:?}"),
        }
    };
    assert_eq!(p(r#"{"summary":"x"}"#).0, "$.decision");
    let (path, allowed) = p(r#"{"decision":"think","summary":"x"}"#);
    assert_eq!(path, "$.decision");
    assert_eq!(
        allowed.unwrap(),
        ["act", "plan", "ask_user", "complete", "give_up"]
    );
    assert_eq!(
        p(r#"{"decision":"complete","summary":"x","extra":1}"#).0,
        "$.extra"
    );
    assert_eq!(
        p(r#"{"decision":"act","tool":5,"arguments":{}}"#).0,
        "$.tool"
    );
    assert_eq!(
        p(r#"{"decision":"act","tool":"t","arguments":[]}"#).0,
        "$.arguments"
    );
    assert_eq!(p(r#"{"decision":"act","tool":"t"}"#).0, "$.arguments");
    assert_eq!(p(r#"{"decision":"plan","steps":[]}"#).0, "$.steps");
    assert_eq!(p(r#"{"decision":"plan","steps":["a",2]}"#).0, "$.steps[1]");
    assert_eq!(
        p(r#"{"decision":"ask_user","question":""}"#).0,
        "$.question"
    );
    assert_eq!(p(r#"{"decision":"give_up"}"#).0, "$.reason");
}

/// LIM-D2 / LIM-D3 / LIM-D4: string bounds in characters, not bytes.
#[test]
fn lim_d2_d3_d4_bounds_are_in_characters() {
    let reason_ok = "é".repeat(LIM_D2_REASON_CHARS);
    assert!(
        parse_strict(format!(r#"{{"decision":"give_up","reason":"{reason_ok}"}}"#).as_bytes())
            .is_ok()
    );
    let reason_long = "é".repeat(LIM_D2_REASON_CHARS + 1);
    assert_eq!(
        reject(&format!(
            r#"{{"decision":"give_up","reason":"{reason_long}"}}"#
        ))
        .code,
        RejectionCode::Schema
    );
    let items: Vec<String> = (0..LIM_D3_PLAN_ITEMS + 1)
        .map(|i| format!("\"s{i}\""))
        .collect();
    assert_eq!(
        reject(&format!(
            r#"{{"decision":"plan","steps":[{}]}}"#,
            items.join(",")
        ))
        .code,
        RejectionCode::Schema
    );
    let item_long = "x".repeat(LIM_D3_PLAN_ITEM_CHARS + 1);
    assert_eq!(
        reject(&format!(r#"{{"decision":"plan","steps":["{item_long}"]}}"#)).code,
        RejectionCode::Schema
    );
    let summary_long = "x".repeat(LIM_D4_TEXT_CHARS + 1);
    assert_eq!(
        reject(&format!(
            r#"{{"decision":"complete","summary":"{summary_long}"}}"#
        ))
        .code,
        RejectionCode::Schema
    );
}

/// REQ-DEC-012: the decision hash ignores textual form and key order.
#[test]
fn req_dec_012_decision_hash_is_canonical() {
    let a = parse_strict(br#"{"decision":"act","tool":"t","arguments":{"b":1,"a":2}}"#).unwrap();
    let b = parse_strict(
        b"{ \"arguments\" : {\"a\":2, \"b\":1}, \"tool\":\"t\", \"decision\":\"act\" }",
    )
    .unwrap();
    assert_eq!(a.hash(), b.hash());
    let c = parse_strict(br#"{"decision":"act","tool":"t","arguments":{"a":3,"b":1}}"#).unwrap();
    assert_ne!(a.hash(), c.hash());
}

/// REQ-DEC-002: invisible characters inside the tool name survive parsing unchanged (the validator rejects them at stage 4),
/// proving that nothing in the parser alters the model's text.
/// REQ-SEC-013 (hostile corpus, repository guards §3a), hostile decisions: invisible characters in a name.
#[test]
fn req_dec_003_parser_never_alters_text() {
    let d = parse_strict(
        "{\"decision\":\"act\",\"tool\":\"read\u{200B}_file\",\"arguments\":{}}".as_bytes(),
    )
    .unwrap();
    assert!(matches!(d, Decision::Act { tool, .. } if tool == "read\u{200B}_file"));
}
