//! A result bounded as a whole (handoff 009i, item 1; audit A-26).

use super::*;
use serde_json::Map;

fn bound_chars() -> usize {
    mimlys_core::limits::LIM_I19_RESULT_TOTAL_CHARS
}

fn size_of(value: &Value) -> usize {
    serde_json::to_string(value).unwrap().chars().count()
}

fn count_of<F: Fn(&Notice) -> bool>(outcome: &ExecutionOutcome, f: F) -> usize {
    outcome.notices.iter().filter(|n| f(n)).count()
}

fn truncated(n: &Notice) -> bool {
    matches!(n, Notice::ContentTruncated { .. })
}

fn removed(n: &Notice) -> bool {
    matches!(n, Notice::CharactersRemoved { .. })
}

/// REQ-MCP-006, REQ-CTX-006, REQ-CTX-009 (handoff 009i, item 1; audit A-26): a
/// result of many medium strings and many keys is bounded in total, as valid
/// JSON, and each kind of change is said once with its total, however many
/// strings and keys it came from; the short string stays; the user's copy is the
/// whole result, cleaned.
#[test]
fn req_mcp_006_a_result_is_bounded_in_total_and_each_change_is_said_once() {
    let items: Vec<String> = (0..40)
        .map(|i| format!("{i:03}\u{200B}{}", "m".repeat(1500)))
        .collect();
    let keys: Map<String, Value> = (0..30)
        .map(|i| (format!("{i:02}\u{202E}{}", "k".repeat(58)), json!(i)))
        .collect();
    let value = json!({"items": items, "keys": keys, "short": "keep me"});
    let outcome = maps(&provider(), "t.x", result_of(value));
    assert_eq!(outcome.failure, None);
    assert!(size_of(&outcome.payload) <= bound_chars());
    // Valid JSON: it reads back as itself.
    let text = serde_json::to_string(&outcome.payload).unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&text).unwrap(),
        outcome.payload
    );
    assert_eq!(outcome.payload["short"], "keep me");
    assert_eq!(outcome.payload["items"].as_array().unwrap().len(), 40);
    assert_eq!(outcome.payload["keys"].as_object().unwrap().len(), 30);
    // One notice of each kind, with its total.
    assert_eq!(count_of(&outcome, removed), 1, "{:?}", outcome.notices);
    assert_eq!(count_of(&outcome, truncated), 1, "{:?}", outcome.notices);
    assert!(
        outcome
            .notices
            .contains(&Notice::CharactersRemoved { count: 70 })
    );
    assert!(outcome.notices.contains(&Notice::ContentTruncated {
        total_chars: 40 * 1503
    }));
    // The user's copy is whole.
    let full = outcome.full_payload.as_ref().expect("the whole result");
    let first = full["items"][0].as_str().unwrap();
    assert_eq!(first.chars().count(), 1503);
    assert!(first.starts_with("000m"));
    assert_eq!(full["keys"].as_object().unwrap().len(), 30);
}

/// REQ-MCP-006 (handoff 009i, item 1): a result within the bound is the whole
/// result: no cut, no notice, and no second copy.
#[test]
fn req_mcp_006_a_result_within_the_bound_has_no_cut_and_no_second_copy() {
    let outcome = maps(
        &provider(),
        "t.x",
        result_of(json!({"a": "x".repeat(3000)})),
    );
    assert_eq!(outcome.payload["a"].as_str().unwrap().len(), 3000);
    assert_eq!(outcome.full_payload, None);
    assert!(outcome.notices.is_empty());
}

/// REQ-MCP-006, REQ-TOOL-005 (handoff 009i, item 1): the bound is applied after
/// the output schema is checked. A string that conforms (`minLength`) and a key
/// that is required, both longer than the cut leaves, are not `output_invalid`
/// after it: the call succeeds, and the cut result is what the model reads.
#[test]
fn req_mcp_006_a_result_that_conforms_before_the_cut_is_not_invalid_after_it() {
    let long_key = "k".repeat(300);
    let schema = json!({
        "type": "object",
        "required": ["text", long_key.clone()],
        "properties": {"text": {"type": "string", "minLength": 20_000}},
    });
    let provider = provider_with_output(schema);
    let outcome = maps(
        &provider,
        "s.t",
        result_of(json!({"text": "t".repeat(30_000), long_key.clone(): 1})),
    );
    assert_eq!(outcome.failure, None, "{:?}", outcome.payload);
    assert!(size_of(&outcome.payload) <= bound_chars());
    assert!(outcome.payload["text"].as_str().unwrap().len() < 20_000);
    assert!(outcome.payload.get(&long_key).is_none(), "the key is cut");
    assert_eq!(count_of(&outcome, truncated), 1);
    // The whole result is still the one that conforms.
    let full = outcome.full_payload.as_ref().unwrap();
    assert_eq!(full["text"].as_str().unwrap().len(), 30_000);
    assert_eq!(full[&long_key], 1);
}

/// REQ-MCP-006 (handoff 009i, item 1): a failure's payload is bounded too: a
/// server's error with a huge object, and an output that is not valid with a huge
/// key in its path, never reach the model whole.
#[test]
fn req_mcp_006_the_payload_of_a_failure_is_bounded_too() {
    let error = maps(
        &provider(),
        "t.x",
        CallOutcome::Result {
            content: vec![],
            structured: Some(json!({
                "error": {"kind": "denied", "message": "m".repeat(100_000)},
                "more": vec!["z".repeat(50_000); 4],
            })),
            is_error: true,
        },
    );
    assert!(matches!(
        error.failure,
        Some(FailureCause::ToolError { .. })
    ));
    assert!(size_of(&error.payload) <= bound_chars());
    assert_eq!(count_of(&error, truncated), 1);
    assert!(error.full_payload.is_some());
    let provider = provider_with_output(json!({
        "type": "object", "additionalProperties": false, "properties": {}
    }));
    let invalid = maps(&provider, "s.t", result_of(json!({"k".repeat(200_000): 1})));
    assert_eq!(invalid.failure, Some(FailureCause::OutputInvalid));
    assert!(size_of(&invalid.payload) <= bound_chars());
}

/// REQ-MCP-006 (handoff 009i, item 1): what is only structure past the bound is
/// replaced by an object that says its size, said once.
#[test]
fn req_mcp_006_a_result_that_is_all_structure_is_replaced_by_its_size() {
    let value: Value = (0..4000)
        .map(|i| (format!("key{i}"), json!(i)))
        .collect::<Map<_, _>>()
        .into();
    let whole = size_of(&value) as u64;
    let outcome = maps(&provider(), "t.x", result_of(value));
    assert!(size_of(&outcome.payload) <= bound_chars());
    assert_eq!(outcome.payload["total_chars"], whole);
    assert!(
        outcome
            .notices
            .contains(&Notice::ContentTruncated { total_chars: whole })
    );
    assert_eq!(
        outcome
            .full_payload
            .as_ref()
            .unwrap()
            .as_object()
            .unwrap()
            .len(),
        4000
    );
}
