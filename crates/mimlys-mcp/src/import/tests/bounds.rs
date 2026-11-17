//! One tool's definition and an enumeration's values are bounded at import
//! (handoff 009i, item 2a; audit A-32; REQ-CTX-005).

use super::*;
use mimlys_core::limits::{PROPOSED_ENUM_VALUES, PROPOSED_TOOL_DEFINITION_CHARS};

/// A tool of `n` string parameters, each with a description of 190 characters.
fn wide_tool(n: usize) -> Listed {
    let properties: serde_json::Map<String, Value> = (0..n)
        .map(|i| {
            (
                format!("p{i}"),
                json!({"type": "string", "description": "d".repeat(190)}),
            )
        })
        .collect();
    listed(
        "wide",
        "A tool.",
        json!({"type": "object", "properties": properties}),
        None,
    )
}

fn rendered(tool: &Imported) -> usize {
    rendered_chars(std::slice::from_ref(
        tool.definition.as_ref().expect("available"),
    ))
}

/// REQ-CTX-005 (handoff 009i, item 2a): a tool whose rendering alone is above the
/// per-tool bound is unavailable, with the reason `definition_too_large` and the
/// sizes, and no definition; one just under it is imported whole. The tool one
/// parameter past the last that fits is the one that is refused.
#[test]
fn req_ctx_005_a_tool_over_the_per_tool_bound_is_unavailable_with_the_reason() {
    let mut last_fit = None;
    for n in 1..=LIM_I3_TOOL_PARAMETERS {
        let imported = import("s", &wide_tool(n), Some(&declared()), None);
        if imported.definition.is_some() {
            assert!(rendered(&imported) <= PROPOSED_TOOL_DEFINITION_CHARS);
            last_fit = Some(n);
        } else {
            // From here on every tool is over the bound.
            let reason = imported.record.unavailable.clone().expect("a reason");
            assert!(
                reason.starts_with("definition_too_large:"),
                "{n} parameters: {reason}"
            );
            assert!(
                reason.ends_with(&format!(">{PROPOSED_TOOL_DEFINITION_CHARS}")),
                "{reason}"
            );
        }
    }
    let fit = last_fit.expect("a small tool fits");
    assert!(
        fit < LIM_I3_TOOL_PARAMETERS,
        "the bound is reached before LIM-I3 is: {fit}"
    );
    let over = import("s", &wide_tool(fit + 1), Some(&declared()), None);
    assert!(over.definition.is_none());
    let reason = over.record.unavailable.unwrap();
    // The size it says is the size it has: the definition built unbounded.
    let size: usize = reason
        .trim_start_matches("definition_too_large:")
        .split('>')
        .next()
        .unwrap()
        .parse()
        .unwrap();
    assert!(size > PROPOSED_TOOL_DEFINITION_CHARS && size < PROPOSED_TOOL_DEFINITION_CHARS + 400);
}

/// REQ-CTX-005, REQ-MCP-003 (handoff 009i, item 2a; audit A-32): the case the
/// audit found: eight parameters of eight nested fields, each within every bound
/// of its own, is more than LIM-I1 alone, and is unavailable at import, not a
/// step that fails for it later.
#[test]
fn req_ctx_005_eight_parameters_of_eight_nested_fields_are_unavailable() {
    let field = || json!({"type": "string", "description": "f".repeat(190)});
    let object = || {
        let fields: serde_json::Map<String, Value> =
            (0..8).map(|i| (format!("f{i}"), field())).collect();
        json!({"type": "object", "properties": fields, "description": "o"})
    };
    let properties: serde_json::Map<String, Value> =
        (0..8).map(|i| (format!("p{i}"), object())).collect();
    let tool = listed(
        "nested",
        "A tool.",
        json!({"type": "object", "properties": properties}),
        None,
    );
    let imported = import("s", &tool, Some(&declared()), None);
    assert!(imported.definition.is_none());
    let reason = imported.record.unavailable.unwrap();
    assert!(reason.starts_with("definition_too_large:"), "{reason}");
    // It is more than the whole of LIM-I1 on its own: the case of the audit.
    let size: usize = reason
        .trim_start_matches("definition_too_large:")
        .split('>')
        .next()
        .unwrap()
        .parse()
        .unwrap();
    assert!(
        size > mimlys_core::limits::LIM_I1_TOOL_DEFINITIONS_CHARS,
        "{size}"
    );
}

/// REQ-CTX-005 (handoff 009i, item 2a): an enumeration of more values than the
/// bound is unavailable with the reason, even when its rendering would fit the
/// per-tool bound; one of exactly the bound is imported. The values are counted
/// before they are cleaned, so the work is bounded too.
#[test]
fn req_ctx_005_an_enumeration_over_its_bound_is_unavailable_with_the_reason() {
    let tool = |n: usize| {
        let values: Vec<String> = (0..n).map(|i| format!("v{i}")).collect();
        listed(
            "pick",
            "A tool.",
            json!({"type": "object", "properties": {"kind": {"type": "string", "enum": values}}}),
            None,
        )
    };
    let at = import("s", &tool(PROPOSED_ENUM_VALUES), Some(&declared()), None);
    assert!(at.definition.is_some(), "{:?}", at.record.unavailable);
    assert!(rendered(&at) <= PROPOSED_TOOL_DEFINITION_CHARS);
    let over = import(
        "s",
        &tool(PROPOSED_ENUM_VALUES + 1),
        Some(&declared()),
        None,
    );
    assert!(over.definition.is_none());
    assert_eq!(
        over.record.unavailable.as_deref(),
        Some(
            format!(
                "too_many_enum_values:kind:{}>{PROPOSED_ENUM_VALUES}",
                PROPOSED_ENUM_VALUES + 1
            )
            .as_str()
        )
    );
    // A nested enumeration counts as well, and a huge one is refused without
    // its values being read (one million values).
    let nested = listed(
        "deep",
        "A tool.",
        json!({"type": "object", "properties": {"o": {"type": "object", "properties": {
            "e": {"type": "string", "enum": vec!["x"; 1_000_000]}
        }}}}),
        None,
    );
    let refused = import("s", &nested, Some(&declared()), None);
    assert!(refused.definition.is_none());
    assert!(
        refused
            .record
            .unavailable
            .unwrap()
            .starts_with("too_many_enum_values:o.e:1000000>")
    );
}
