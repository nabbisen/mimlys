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
