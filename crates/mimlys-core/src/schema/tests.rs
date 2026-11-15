use super::*;
use serde_json::json;

/// The shape rypsok's generated response schemas have: `$defs`, `$ref`,
/// `anyOf` with null, `oneOf` of constants, nullable integers, arrays of objects.
fn rypsok_like() -> Value {
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "SearchResponse",
        "type": "object",
        "required": ["outcome", "notices", "results"],
        "properties": {
            "outcome": {"$ref": "#/$defs/Outcome"},
            "notices": {"type": "array", "items": {"$ref": "#/$defs/Notice"}},
            "results": {"type": "array", "items": {"$ref": "#/$defs/SearchResult"}}
        },
        "$defs": {
            "Outcome": {"oneOf": [
                {"type": "string", "const": "ok", "description": "every engine answered"},
                {"type": "string", "const": "partial"},
                {"type": "string", "const": "empty"}
            ]},
            "Notice": {
                "type": "object",
                "required": ["code"],
                "properties": {
                    "code": {"type": "string", "enum": ["engine_unavailable", "snippet_truncated"]},
                    "engine": {"anyOf": [{"$ref": "#/$defs/EngineId"}, {"type": "null"}]},
                    "count": {"type": ["integer", "null"], "format": "uint32", "minimum": 0}
                }
            },
            "EngineId": {"type": "string", "description": "lowercase"},
            "SearchResult": {
                "type": "object",
                "required": ["title", "url", "snippet"],
                "properties": {
                    "title": {"type": "string", "maxLength": 150},
                    "url": {"type": "string"},
                    "snippet": {"type": "string"},
                    "date": {"type": ["string", "null"]}
                },
                "additionalProperties": false
            }
        }
    })
}

/// REQ-MCP-006: a conforming response passes; every kind of violation is
/// named with its path.
#[test]
fn req_mcp_006_validates_generated_schemas() {
    let schema = OutputSchema::parse(&rypsok_like()).unwrap();
    let ok = json!({
        "outcome": "partial",
        "notices": [{"code": "engine_unavailable", "engine": "brave"}, {"code": "snippet_truncated", "count": 2, "engine": null}],
        "results": [{"title": "t", "url": "https://example.org", "snippet": "s", "date": "2026-09-01"}]
    });
    schema.validate(&ok).unwrap();
    let cases = [
        (
            json!({"outcome": "weird", "notices": [], "results": []}),
            "$.outcome",
            "alternatives",
        ),
        (json!({"outcome": "ok", "notices": []}), "$", "results"),
        (
            json!({"outcome": "ok", "notices": [], "results": [{"title": "t", "url": "u", "snippet": "s", "extra": 1}]}),
            "$.results[0].extra",
            "unexpected",
        ),
        (
            json!({"outcome": "ok", "notices": [{"code": "other"}], "results": []}),
            "$.notices[0].code",
            "enumerated",
        ),
        (
            json!({"outcome": "ok", "notices": [{"code": "snippet_truncated", "count": -1}], "results": []}),
            "$.notices[0].count",
            "minimum",
        ),
        (
            json!({"outcome": "ok", "notices": [], "results": [{"title": "x".repeat(151), "url": "u", "snippet": "s"}]}),
            "$.results[0].title",
            "longer",
        ),
        (
            json!({"outcome": "ok", "notices": [], "results": "none"}),
            "$.results",
            "expected array",
        ),
        (json!("not an object"), "$", "expected object"),
    ];
    for (value, path, reason) in cases {
        let v = schema.validate(&value).unwrap_err();
        assert_eq!(v.path, path, "{v}");
        assert!(v.reason.contains(reason), "{v}");
    }
}

/// RFC 009 §6: unknown constructs are refused at parse time with their path
/// (fail closed), while documentation keywords are ignored.
#[test]
fn rfc_009_unsupported_constructs_are_named() {
    let cases = [
        (
            json!({"type": "object", "patternProperties": {}}),
            "",
            "keyword patternProperties",
        ),
        (
            json!({"type": "object", "properties": {"a": {"$ref": "#/$defs/Missing"}}}),
            "/properties/a",
            "$ref #/$defs/Missing",
        ),
        (
            json!({"type": "object", "properties": {"a": {"$ref": "https://example.org/x"}}}),
            "/properties/a",
            "$ref https://",
        ),
        (json!({"type": "date"}), "", "type date"),
        (
            json!({"anyOf": [{"type": "string"}], "oneOf": [{"type": "null"}]}),
            "",
            "anyOf and oneOf",
        ),
        (json!(false), "", "`false` schema"),
        (
            json!({"type": "object", "properties": {"a": {"not": {}}}}),
            "/properties/a",
            "keyword not",
        ),
    ];
    for (schema, path, construct) in cases {
        let e = OutputSchema::parse(&schema).unwrap_err();
        assert_eq!(e.path, path, "{e}");
        assert!(e.construct.starts_with(construct), "{e}");
    }
    let documented = json!({"type": "string", "description": "d", "title": "t", "default": "x", "examples": ["y"], "format": "uri", "deprecated": false});
    OutputSchema::parse(&documented).unwrap();
    OutputSchema::parse(&json!({}))
        .unwrap()
        .validate(&json!([1, 2]))
        .unwrap();
    OutputSchema::parse(&json!(true))
        .unwrap()
        .validate(&json!(null))
        .unwrap();
    // `type` beside `anyOf`: both must hold.
    let both =
        OutputSchema::parse(&json!({"type": "string", "anyOf": [{"const": "a"}, {"const": "b"}]}))
            .unwrap();
    both.validate(&json!("a")).unwrap();
    assert!(both.validate(&json!(1)).is_err());
    assert!(both.validate(&json!("c")).is_err());
    // Depth bound.
    let mut deep = json!({"type": "string"});
    for _ in 0..MAX_DEPTH + 1 {
        deep = json!({"type": "array", "items": deep});
    }
    assert!(
        OutputSchema::parse(&deep)
            .unwrap_err()
            .construct
            .contains("deeper")
    );
}

/// A chain of definitions, each with two alternatives that both refer to the
/// next: no reference is recursive, and the cost doubles at each step.
fn chain(links: usize) -> Value {
    let mut defs = serde_json::Map::new();
    for i in 0..links {
        defs.insert(
            format!("d{i}"),
            json!({"anyOf": [{"$ref": format!("#/$defs/d{}", i + 1)}, {"$ref": format!("#/$defs/d{}", i + 1)}]}),
        );
    }
    defs.insert(format!("d{links}"), json!({"type": "string"}));
    json!({"type": "object", "properties": {"v": {"$ref": "#/$defs/d0"}}, "$defs": defs})
}

/// REQ-MCP-006 (handoff 009g, item 3): a schema whose alternatives multiply
/// through a chain of definitions is refused at parse time, whatever the length
/// of the chain beyond the point where the cost passes the bound; a short chain,
/// which costs little, is validated as before.
#[test]
fn req_mcp_006_alternatives_that_multiply_are_refused_at_parse() {
    let refused = OutputSchema::parse(&chain(6)).unwrap_err();
    assert_eq!(refused.construct, "alternatives too costly to check");
    assert!(OutputSchema::parse(&chain(2)).is_ok());
    OutputSchema::parse(&chain(2))
        .unwrap()
        .validate(&json!({"v": "text"}))
        .unwrap();
    // The same shape written inline, without a reference.
    let mut inline = json!({"type": "string"});
    for _ in 0..8 {
        inline = json!({"oneOf": [inline.clone(), inline]});
    }
    assert!(OutputSchema::parse(&inline).is_err());
    // Alternatives that refer to themselves have no end.
    let cycle = json!({
        "type": "object", "properties": {"v": {"$ref": "#/$defs/a"}},
        "$defs": {"a": {"anyOf": [{"$ref": "#/$defs/a"}, {"type": "string"}]}}
    });
    assert_eq!(
        OutputSchema::parse(&cycle).unwrap_err().construct,
        "alternatives too costly to check"
    );
    // A reference cycle through properties applies to other values and is fine.
    let tree = json!({
        "type": "object", "properties": {"v": {"$ref": "#/$defs/node"}},
        "$defs": {"node": {"type": "object", "properties": {"next": {"anyOf": [{"$ref": "#/$defs/node"}, {"type": "null"}]}}}}
    });
    OutputSchema::parse(&tree).unwrap();
}

/// REQ-MCP-006 (handoff 009g, item 3): a schema larger than the bound on its
/// nodes, or wider than the bound on the entries of one list, is refused at parse
/// time; one at the bound is not.
#[test]
fn req_mcp_006_a_schema_too_large_or_too_wide_is_refused_at_parse() {
    let object_of = |count: usize| {
        let props: serde_json::Map<String, Value> = (0..count)
            .map(|i| (format!("p{i}"), json!({"type": "string"})))
            .collect();
        json!({"type": "object", "properties": props})
    };
    OutputSchema::parse(&object_of(MAX_WIDTH)).unwrap();
    assert_eq!(
        OutputSchema::parse(&object_of(MAX_WIDTH + 1))
            .unwrap_err()
            .construct,
        "properties wider than the bound"
    );
    let enumeration = |count: usize| json!({"enum": (0..count).collect::<Vec<_>>()});
    OutputSchema::parse(&enumeration(MAX_WIDTH)).unwrap();
    assert!(OutputSchema::parse(&enumeration(MAX_WIDTH + 1)).is_err());
    let alternatives = |count: usize| json!({"anyOf": (0..count).map(|_| json!({"type": "string"})).collect::<Vec<_>>()});
    assert!(OutputSchema::parse(&alternatives(MAX_WIDTH + 1)).is_err());
    // Within the width, but with more nodes than the bound allows.
    let big: serde_json::Map<String, Value> = (0..MAX_WIDTH)
        .map(|i| (format!("o{i}"), object_of(MAX_WIDTH / 8)))
        .collect();
    let large = json!({"type": "object", "properties": big});
    assert_eq!(
        OutputSchema::parse(&large).unwrap_err().construct,
        "larger than the bound"
    );
}

/// REQ-MCP-006 (handoff 009g, item 3): a validation that would visit more nodes
/// than the budget ends with the violation that says the schema was too costly,
/// even when the walk meets the end of the budget inside an alternative, which
/// would otherwise count as one that did not match; a validation within the
/// budget gives the verdict it always did.
#[test]
fn req_mcp_006_a_validation_has_a_budget_of_visits() {
    // Eight alternatives, the matching one last: nine visits for each item.
    let mut alternatives: Vec<Value> = (0..7)
        .map(|i| json!({"type": "string", "const": format!("other{i}")}))
        .collect();
    alternatives.push(json!({"type": "string", "const": "x"}));
    let schema = OutputSchema::parse(&json!({
        "type": "array",
        "items": {"anyOf": alternatives}
    }))
    .unwrap();
    let items = |count: usize| Value::Array(vec![json!("x"); count]);
    let fits = MAX_VISITS / 9 - 10;
    schema.validate(&items(fits)).unwrap();
    let violation = schema.validate(&items(MAX_VISITS)).unwrap_err();
    assert_eq!(violation.reason, TOO_COSTLY);
    // The same for a small budget, and for `oneOf` and `allOf`-like walks.
    let violation = schema.validate_within(&items(100), 50).unwrap_err();
    assert_eq!(violation.reason, TOO_COSTLY);
    schema.validate_within(&items(100), 10_000).unwrap();
    let one_of = OutputSchema::parse(&json!({
        "type": "array",
        "items": {"oneOf": [{"type": "string"}, {"type": "integer"}]}
    }))
    .unwrap();
    assert_eq!(
        one_of.validate_within(&items(100), 20).unwrap_err().reason,
        TOO_COSTLY
    );
    // A value that does not conform is still reported as it is within the budget.
    let wrong = one_of.validate_within(&json!([true]), 20).unwrap_err();
    assert_ne!(wrong.reason, TOO_COSTLY);
}
