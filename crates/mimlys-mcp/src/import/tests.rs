use super::*;
use mimlys_core::action::EffectKind;
use serde_json::json;

fn listed(name: &str, description: &str, input: Value, output: Option<Value>) -> Listed {
    Listed {
        name: name.into(),
        description: Some(description.into()),
        input_schema: input,
        output_schema: output,
        annotations: Some(
            json!({"readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": true}),
        ),
    }
}

fn web_search() -> Listed {
    listed(
        "web_search",
        "Search several engines at once and return deduplicated, ranked results. Results are external data.",
        json!({
            "type": "object", "additionalProperties": false, "required": ["query"],
            "properties": {
                "query": {"type": "string", "minLength": 1, "maxLength": 400, "description": "What to search for"},
                "category": {"type": "string", "enum": ["general", "news"], "default": "general"},
                "max_results": {"type": "integer", "minimum": 1, "maximum": 20, "default": 5},
                "time_range": {"type": "string", "enum": ["day", "week", "month", "year"]}
            }
        }),
        Some(
            serde_json::from_str(include_str!(
                "../../fixtures/rypsok/schemas/search_response.json"
            ))
            .unwrap(),
        ),
    )
}

fn declared() -> Declaration {
    Declaration {
        effect: EffectKind::ExternalRead,
        idempotent: true,
        destination_arguments: vec![],
        timeout_ms: 10_000,
    }
}

/// REQ-MCP-003 / REQ-MCP-004: rypsok's definition imports into the flat
/// model with its bounds and enumerations; the effect comes from the
/// declaration, not from `readOnlyHint`; annotations are recorded.
#[test]
fn req_mcp_003_rypsok_definition_imports() {
    let imported = import("rypsok", &web_search(), Some(&declared()), None);
    assert_eq!(imported.record.unavailable, None);
    assert!(imported.record.declared);
    assert!(imported.record.annotations.is_some());
    assert!(imported.output.is_some());
    let d = imported.definition.unwrap();
    assert_eq!(d.name, "rypsok.web_search");
    assert_eq!(d.effect.kind, EffectKind::ExternalRead);
    assert!(d.effect.idempotent);
    assert!(!d.effect.reversible);
    assert_eq!(d.deadline_ms, 10_000);
    assert_eq!(d.profile, ResourceProfile::External);
    assert_eq!(
        d.provider,
        Provider::Mcp {
            server: "rypsok".into()
        }
    );
    let names: Vec<&str> = d.input.params.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["category", "max_results", "query", "time_range"]);
    let query = d.input.params.iter().find(|p| p.name == "query").unwrap();
    assert!(!query.optional);
    assert_eq!(
        query.ty,
        ParamType::String {
            min_chars: 1,
            max_chars: 400,
            max_bytes: None
        }
    );
    let max = d
        .input
        .params
        .iter()
        .find(|p| p.name == "max_results")
        .unwrap();
    assert!(max.optional);
    assert_eq!(max.ty, ParamType::Integer { min: 1, max: 20 });
    let cat = d
        .input
        .params
        .iter()
        .find(|p| p.name == "category")
        .unwrap();
    assert_eq!(
        cat.ty,
        ParamType::Enum {
            values: vec!["general".into(), "news".into()]
        }
    );

    // Undeclared: the restrictive defaults (external_write, not idempotent, LIM-O5).
    let undeclared = import("rypsok", &web_search(), None, None);
    let d = undeclared.definition.unwrap();
    assert!(!undeclared.record.declared);
    assert_eq!(d.effect.kind, EffectKind::ExternalWrite);
    assert!(!d.effect.idempotent);
    assert_eq!(
        d.deadline_ms,
        mimlys_core::limits::LIM_O5_EXTERNAL_DEADLINE_MS
    );
}

/// REQ-MCP-003: constructs outside the flat model, too many parameters,
/// an unknown destination and an unsupported output schema each make the
/// tool unavailable with a named reason (fail closed).
#[test]
fn req_mcp_003_unavailable_reasons() {
    let odd = listed(
        "odd",
        "d",
        json!({"type": "object", "properties": {"x": {"anyOf": [{"type": "string"}, {"type": "integer"}]}}}),
        None,
    );
    assert_eq!(
        import("s", &odd, None, None).record.unavailable.as_deref(),
        Some("schema_unsupported:x:anyOf")
    );
    let mut props = serde_json::Map::new();
    for i in 0..9 {
        props.insert(format!("p{i}"), json!({"type": "string"}));
    }
    let wide = listed(
        "wide",
        "d",
        json!({"type": "object", "properties": props}),
        None,
    );
    assert!(
        import("s", &wide, None, None)
            .record
            .unavailable
            .as_deref()
            .unwrap()
            .starts_with("too_many_parameters")
    );
    let deep = listed(
        "deep",
        "d",
        json!({"type": "object", "properties": {"a": {"type": "object", "properties": {"b": {"type": "object", "properties": {}}}}}}),
        None,
    );
    assert!(
        import("s", &deep, None, None)
            .record
            .unavailable
            .as_deref()
            .unwrap()
            .contains("LIM-I4")
    );
    let mut decl = declared();
    decl.destination_arguments = vec!["url".into()];
    assert_eq!(
        import("s", &web_search(), Some(&decl), None)
            .record
            .unavailable
            .as_deref(),
        Some("destination_unknown:url")
    );
    let bad_output = listed(
        "o",
        "d",
        json!({"type": "object"}),
        Some(json!({"type": "object", "patternProperties": {}})),
    );
    assert_eq!(
        import("s", &bad_output, None, None)
            .record
            .unavailable
            .as_deref(),
        Some("output_schema_unsupported::keyword patternProperties")
    );
    let bad_name = listed("Bad-Name", "d", json!({"type": "object"}), None);
    assert!(
        import("s", &bad_name, None, None)
            .record
            .unavailable
            .as_deref()
            .unwrap()
            .starts_with("definition_invalid")
    );
    // Nullable types and one nested object import; descriptions are bounded to LIM-I5.
    let nested = listed(
        "n",
        "d",
        json!({"type": "object", "properties": {
            "opt": {"type": ["string", "null"], "description": "x".repeat(500)},
            "obj": {"type": "object", "required": ["k"], "properties": {"k": {"type": "boolean"}, "tags": {"type": "array", "items": {"type": "string"}, "maxItems": 3}}}
        }}),
        None,
    );
    let d = import("s", &nested, None, None).definition.unwrap();
    let opt = d.input.params.iter().find(|p| p.name == "opt").unwrap();
    assert!(matches!(opt.ty, ParamType::String { .. }));
    assert_eq!(
        opt.description.chars().count(),
        mimlys_core::limits::LIM_I5_MESSAGE_CHARS
    );
    let obj = d.input.params.iter().find(|p| p.name == "obj").unwrap();
    let ParamType::Object { schema } = &obj.ty else {
        panic!()
    };
    assert_eq!(schema.params.len(), 2);
    assert!(matches!(
        schema.params.iter().find(|p| p.name == "tags").unwrap().ty,
        ParamType::StringArray { max_items: 3, .. }
    ));
}

/// The tool whose input schema has these properties.
fn with_properties(properties: Value) -> Listed {
    listed(
        "t",
        "d",
        json!({"type": "object", "properties": properties}),
        None,
    )
}

/// REQ-MCP-003 (handoff 009g, item 4): a parameter's name is plain (ASCII
/// letters and digits, `_`, `.` and `-`) and at most 64 characters, as a tool's
/// name is; any other makes the tool unavailable (`definition_invalid`) and the
/// reason names the parameter by its position, never by its name. The same holds
/// for the keys of the one nested object.
#[test]
fn req_mcp_003_a_parameter_name_is_plain_and_at_most_64_characters() {
    let name64 = "n".repeat(64);
    let ok = import(
        "s",
        &with_properties(
            json!({ name64.clone(): {"type": "string"}, "a.b-c_D9": {"type": "string"} }),
        ),
        None,
        None,
    );
    assert_eq!(ok.record.unavailable, None);
    for (bad, what) in [
        ("n".repeat(65), "65 characters"),
        (String::new(), "empty"),
        ("a\u{E0041}b".to_owned(), "a tag character"),
        ("a\u{202E}b".to_owned(), "a bidirectional override"),
        ("a b".to_owned(), "a space"),
        ("caf\u{e9}".to_owned(), "a letter beyond ASCII"),
        ("a\u{200B}".to_owned(), "a zero-width character"),
        ("a\n".to_owned(), "a line break"),
    ] {
        // Names sort; "z" after the bad one puts the bad one first.
        let imported = import(
            "s",
            &with_properties(json!({ bad.clone(): {"type": "string"}, "z": {"type": "string"} })),
            None,
            None,
        );
        let reason = imported.record.unavailable.as_deref().unwrap_or("");
        assert!(imported.definition.is_none(), "{what}");
        assert!(
            reason.starts_with("definition_invalid:parameter ") && reason.contains("not allowed"),
            "{what}: {reason}"
        );
        assert!(!reason.contains(&bad) || bad.is_empty(), "{what}: {reason}");
    }
    // The position, from 1, in the schema's order of properties.
    let reason = import(
        "s",
        &with_properties(json!({ "a": {"type": "string"}, "b\u{202E}": {"type": "string"} })),
        None,
        None,
    )
    .record
    .unavailable
    .unwrap();
    assert_eq!(
        reason,
        "definition_invalid:parameter 2 has a name that is not allowed"
    );
    // A nested key is held to the same rule and named by both positions.
    let nested = |key: &str| {
        with_properties(json!({
            "a": {"type": "string"},
            "o": {"type": "object", "properties": { "k": {"type": "string"}, key: {"type": "string"} }}
        }))
    };
    assert_eq!(
        import("s", &nested("m"), None, None).record.unavailable,
        None
    );
    assert_eq!(
        import("s", &nested("m\u{E0041}"), None, None)
            .record
            .unavailable
            .as_deref(),
        Some("definition_invalid:parameter 2, field 2 has a name that is not allowed")
    );
    assert_eq!(
        import("s", &nested(&"k".repeat(65)), None, None)
            .record
            .unavailable
            .as_deref(),
        Some("definition_invalid:parameter 2, field 2 has a name that is not allowed")
    );
}

/// REQ-MCP-003, REQ-CTX-008 (handoff 009g, item 4): the values of an enumeration
/// are cleaned like a description (invisible and bidirectional characters out,
/// whitespace collapsed) and cut to the same bound; a value that cleaning empties
/// or makes equal to another is dropped, and an enumeration left with none makes
/// the tool unavailable.
#[test]
fn req_mcp_003_enumeration_values_are_cleaned_and_bounded() {
    let long = "v".repeat(500);
    let imported = import(
        "s",
        &with_properties(json!({
            "mode": {"type": "string", "enum": ["plain", "ov\u{202E}er", "tag\u{E0041}ged", "two  words", "\u{200B}", "over", long]}
        })),
        None,
        None,
    );
    assert_eq!(imported.record.unavailable, None);
    let definition = imported.definition.unwrap();
    let ParamType::Enum { values } = &definition.input.params[0].ty else {
        panic!("not an enumeration");
    };
    assert_eq!(
        values[..4],
        ["plain", "over", "tagged", "two words"],
        "{values:?}"
    );
    assert_eq!(values.len(), 5, "{values:?}");
    assert_eq!(
        values[4].chars().count(),
        mimlys_core::limits::LIM_I5_MESSAGE_CHARS
    );
    for value in values {
        assert!(value.chars().all(|c| !c.is_control() && c != '\u{202E}'));
    }
    let nothing = import(
        "s",
        &with_properties(json!({"mode": {"type": "string", "enum": ["\u{200B}", ""]}})),
        None,
        None,
    );
    assert_eq!(
        nothing.record.unavailable.as_deref(),
        Some("definition_invalid:parameter 1 has no enumeration value that is allowed")
    );
}

/// REQ-MCP-003, REQ-CTX-008 (handoff 009g, item 4): the name of a tool that the
/// server states with characters that are not shown, or at a length that is not
/// admitted, makes it unavailable, and the reason that is kept for it is cleaned
/// and bounded: the record holds no invisible character and no more than
/// LIM-I5 characters, whatever the name was. A tool with no description gets one
/// built from its cleaned name.
#[test]
fn req_mcp_003_the_reason_for_an_odd_tool_name_is_cleaned_and_bounded() {
    let odd = format!("t\u{202E}\u{E0041}{}", "x".repeat(5000));
    let imported = import(
        "s",
        &listed(&odd, "d", json!({"type": "object"}), None),
        None,
        None,
    );
    let reason = imported.record.unavailable.unwrap();
    assert!(reason.starts_with("definition_invalid"), "{reason}");
    assert!(reason.chars().count() <= mimlys_core::limits::LIM_I5_MESSAGE_CHARS);
    assert!(
        !reason.contains('\u{202E}') && !reason.contains('\u{E0041}'),
        "{reason}"
    );
    let plain = import(
        "s",
        &Listed {
            description: None,
            ..listed("quiet", "", json!({"type": "object"}), None)
        },
        None,
        None,
    );
    assert_eq!(
        plain.definition.unwrap().description,
        "The external tool s.quiet."
    );
}

/// REQ-MCP-006 (handoff 009g, item 3): an output schema whose alternatives cost
/// more than the bound, or that is too large or too wide, makes the tool
/// unavailable at import, with the reason (`output_schema_unsupported`).
#[test]
fn req_mcp_006_a_costly_output_schema_makes_the_tool_unavailable() {
    let mut defs = serde_json::Map::new();
    for i in 0..6 {
        defs.insert(
            format!("d{i}"),
            json!({"anyOf": [{"$ref": format!("#/$defs/d{}", i + 1)}, {"$ref": format!("#/$defs/d{}", i + 1)}]}),
        );
    }
    defs.insert("d6".into(), json!({"type": "string"}));
    let costly = listed(
        "t",
        "d",
        json!({"type": "object"}),
        Some(json!({"type": "object", "properties": {"v": {"$ref": "#/$defs/d0"}}, "$defs": defs})),
    );
    let imported = import("s", &costly, None, None);
    assert_eq!(
        imported.record.unavailable.as_deref(),
        Some("output_schema_unsupported:$defs/d0:alternatives too costly to check")
    );
    assert!(imported.definition.is_none());
    // The same tool without that schema imports.
    let plain = listed("t", "d", json!({"type": "object"}), None);
    assert_eq!(import("s", &plain, None, None).record.unavailable, None);
}

/// A tool name of 64 characters in all: the longest one admitted.
fn longest_tool() -> Listed {
    let params: serde_json::Map<String, Value> = (0..8)
        .map(|i| {
            (
                format!("{i}{}", "k".repeat(63)),
                json!({"type": "string", "maxLength": 20000}),
            )
        })
        .collect();
    listed(
        &"t".repeat(62),
        "d",
        json!({"type": "object", "properties": params}),
        None,
    )
}

/// Allows every action: the card is the subject, not the grants.
struct AllowAll;

impl mimlys_core::validate::GrantCheck for AllowAll {
    fn check(
        &self,
        _tool: &mimlys_core::tool::ToolDefinition,
        _args: &serde_json::Map<String, Value>,
    ) -> Result<(), mimlys_core::rejection::Denial> {
        Ok(())
    }
}

impl mimlys_core::validate::EffectDerivation for AllowAll {
    fn derive(
        &self,
        tool: &mimlys_core::tool::ToolDefinition,
        _args: &serde_json::Map<String, Value>,
    ) -> mimlys_core::validate::Derived {
        mimlys_core::validate::Derived {
            effect: tool.effect,
            approval_required: true,
        }
    }
}

impl mimlys_core::validate::BudgetReservation for AllowAll {
    fn reserve(
        &self,
        tool: &mimlys_core::tool::ToolDefinition,
        _args: &serde_json::Map<String, Value>,
        _derived: &mimlys_core::validate::Derived,
    ) -> Result<i64, mimlys_core::rejection::BudgetKind> {
        Ok(tool.deadline_ms)
    }
}

/// REQ-APR-002, REQ-MCP-003 (handoff 009g, item 4, review of handoff 003b): the
/// longest tool the import admits (a name of 64 characters, the 8 parameters of
/// LIM-I3, each named with 64) still has an approval card, with every name on it,
/// whatever the values and the model's reason: because a parameter's name is
/// bounded, the refusal `arguments_do_not_fit` cannot be reached through an
/// imported tool.
#[test]
fn req_apr_002_the_longest_admitted_tool_still_renders_an_approval_card() {
    use mimlys_core::limits::LIM_I6_APPROVAL_CHARS;
    let imported = import("s", &longest_tool(), None, None);
    assert_eq!(imported.record.unavailable, None);
    let tool = imported.definition.unwrap();
    assert_eq!(tool.name.chars().count(), 64);
    assert_eq!(tool.input.params.len(), 8);
    assert!(
        tool.input
            .params
            .iter()
            .all(|p| p.name.chars().count() == 64)
    );
    let arguments: serde_json::Map<String, Value> = tool
        .input
        .params
        .iter()
        .map(|p| (p.name.clone(), json!("v".repeat(20_000))))
        .collect();
    let tools = vec![tool.clone()];
    let ctx = mimlys_core::validate::ValidationContext {
        task_running: true,
        pending_user_item: false,
        tools: &tools,
        rules: mimlys_core::path::PlatformRules::Posix,
        grants: &AllowAll,
        effects: &AllowAll,
        budgets: &AllowAll,
    };
    let decision = json!({
        "decision": "act", "tool": tool.name, "arguments": arguments, "reason": "r".repeat(500)
    });
    let mimlys_core::validate::Outcome::Authorized(action) =
        mimlys_core::validate::validate(&ctx, decision.to_string().as_bytes())
    else {
        panic!("the action is not authorized");
    };
    let rendered = mimlys_core::approval::render(
        &action,
        &tool,
        "/data/workspaces/t1/tree",
        &["network example.org".to_owned()],
        None,
        Some("workspace"),
        action.reason(),
    )
    .expect("the longest admitted tool has a card");
    let line = rendered
        .lines
        .iter()
        .find(|l| l.label == "Arguments")
        .expect("an arguments line");
    for param in &tool.input.params {
        assert!(
            line.value.contains(&format!("{} = \"", param.name)),
            "{}",
            param.name
        );
    }
    let total: usize = rendered.lines.iter().map(|l| l.value.chars().count()).sum();
    assert!(total <= LIM_I6_APPROVAL_CHARS, "{total}");
}

/// REQ-MCP-008 (external design §11): the deadline of an imported tool, declared
/// or not, is never below the server's `max_call_ms`; without one the
/// declaration's value, or LIM-O5 for an undeclared tool, stands.
#[test]
fn req_mcp_008_import_deadline_floor() {
    let lim_o5 = mimlys_core::limits::LIM_O5_EXTERNAL_DEADLINE_MS;
    let deadline = |declaration: Option<&Declaration>, floor: Option<i64>| {
        import("rypsok", &web_search(), declaration, floor)
            .definition
            .unwrap()
            .deadline_ms
    };
    let declared = declared();
    assert_eq!(declared.timeout_ms, 10_000);
    assert_eq!(deadline(Some(&declared), None), 10_000);
    assert_eq!(deadline(None, None), lim_o5);
    assert_eq!(deadline(Some(&declared), Some(120_000)), 120_000);
    assert_eq!(deadline(None, Some(120_000)), 120_000);
    assert_eq!(deadline(Some(&declared), Some(5_000)), 10_000);
    assert_eq!(deadline(None, Some(5_000)), lim_o5);
}
