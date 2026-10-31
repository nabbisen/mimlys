use super::*;
use crate::action::EffectKind;
use serde_json::json;

fn string(name: &str, max: usize) -> Param {
    Param {
        name: name.into(),
        ty: ParamType::String {
            min_chars: 1,
            max_chars: max,
            max_bytes: None,
        },
        optional: false,
        description: "a string".into(),
    }
}

fn sample() -> ToolDefinition {
    ToolDefinition {
        name: "read_file".into(),
        description: "Read a file.".into(),
        input: ParamSchema {
            params: vec![
                Param {
                    name: "path".into(),
                    ty: ParamType::WorkspacePath,
                    optional: false,
                    description: "the file".into(),
                },
                Param {
                    name: "start_line".into(),
                    ty: ParamType::Integer {
                        min: 1,
                        max: 1_000_000,
                    },
                    optional: true,
                    description: "first line".into(),
                },
                Param {
                    name: "mode".into(),
                    ty: ParamType::Enum {
                        values: vec!["read_only".into(), "read_write".into()],
                    },
                    optional: true,
                    description: "mode".into(),
                },
            ],
        },
        output: None,
        effect: EffectView {
            kind: EffectKind::WorkspaceRead,
            reversible: true,
            idempotent: true,
        },
        deadline_ms: 30_000,
        profile: ResourceProfile::Builtin,
        destination_arguments: vec![],
        provider: Provider::Builtin,
    }
}

/// REQ-TOOL-001: a definition's name, description, parameter count, depth and destination names are checked.
#[test]
fn req_tool_001_definition_bounds() {
    assert!(sample().check().is_ok());
    let mut t = sample();
    t.name = "Read-File".into();
    assert!(matches!(t.check(), Err(DefinitionError::Name(_))));
    let mut t = sample();
    t.name = "rypsok.web_search".into();
    assert!(t.check().is_ok());
    let mut t = sample();
    t.name = "a.b.c".into();
    assert!(t.check().is_err());
    let mut t = sample();
    t.description = "x".repeat(LIM_I2_TOOL_DESCRIPTION_CHARS + 1);
    assert!(matches!(t.check(), Err(DefinitionError::Description(_))));
    let mut t = sample();
    t.input.params = (0..=LIM_I3_TOOL_PARAMETERS)
        .map(|i| string(&format!("p{i}"), 10))
        .collect();
    assert!(matches!(
        t.check(),
        Err(DefinitionError::TooManyParameters(_))
    ));
    let mut t = sample();
    t.input.params.push(string("path", 10));
    assert!(matches!(
        t.check(),
        Err(DefinitionError::DuplicateParameter(..))
    ));
    let mut t = sample();
    t.destination_arguments = vec!["url".into()];
    assert!(matches!(
        t.check(),
        Err(DefinitionError::UnknownDestination(..))
    ));
    // Depth: object inside object inside the top level is 3 > LIM-I4.
    let mut t = sample();
    t.input.params = vec![Param {
        name: "o".into(),
        ty: ParamType::Object {
            schema: ParamSchema {
                params: vec![Param {
                    name: "p".into(),
                    ty: ParamType::Object {
                        schema: ParamSchema {
                            params: vec![string("q", 5)],
                        },
                    },
                    optional: false,
                    description: "inner".into(),
                }],
            },
        },
        optional: false,
        description: "outer".into(),
    }];
    assert!(matches!(t.check(), Err(DefinitionError::TooDeep(_))));
}

/// REQ-DEC-006 (stage 5): shape validation refuses unknown properties, missing required ones and wrong types, with paths.
#[test]
fn req_dec_006_shape_validation() {
    let s = sample().input;
    assert!(
        s.validate_shape(json!({"path": "a"}).as_object().unwrap())
            .is_ok()
    );
    let e = s
        .validate_shape(json!({"path": "a", "extra": 1}).as_object().unwrap())
        .unwrap_err();
    assert_eq!(e.path, "$.extra");
    let e = s
        .validate_shape(json!({}).as_object().unwrap())
        .unwrap_err();
    assert_eq!(
        (e.path.as_str(), e.expected.as_str()),
        ("$.path", "a required parameter")
    );
    let e = s
        .validate_shape(json!({"path": 3}).as_object().unwrap())
        .unwrap_err();
    assert_eq!(e.path, "$.path");
    let e = s
        .validate_shape(json!({"path": "a", "start_line": 1.5}).as_object().unwrap())
        .unwrap_err();
    assert_eq!(
        (e.path.as_str(), e.expected.as_str()),
        ("$.start_line", "integer")
    );
    let e = s
        .validate_shape(json!({"path": "a", "mode": 1}).as_object().unwrap())
        .unwrap_err();
    assert_eq!(e.path, "$.mode");
}

/// ED §6.3 stage 8: bounds validation reports min, max or allowed values.
#[test]
fn ed_6_3_stage_8_bounds_validation() {
    let s = sample().input;
    let e = s
        .validate_bounds(json!({"path": "a", "start_line": 0}).as_object().unwrap())
        .unwrap_err();
    assert_eq!(
        (e.path.as_str(), e.min, e.max),
        ("$.start_line", Some(1), Some(1_000_000))
    );
    let e = s
        .validate_bounds(json!({"path": "a", "mode": "fast"}).as_object().unwrap())
        .unwrap_err();
    assert_eq!(e.allowed.unwrap(), ["read_only", "read_write"]);
    let bytes = ParamSchema {
        params: vec![Param {
            name: "content".into(),
            ty: ParamType::String {
                min_chars: 0,
                max_chars: 100,
                max_bytes: Some(4),
            },
            optional: false,
            description: "c".into(),
        }],
    };
    assert!(
        bytes
            .validate_bounds(json!({"content": "ab"}).as_object().unwrap())
            .is_ok()
    );
    let e = bytes
        .validate_bounds(json!({"content": "ééé"}).as_object().unwrap())
        .unwrap_err();
    assert_eq!(e.max, Some(4));
    let arr = ParamSchema {
        params: vec![Param {
            name: "args".into(),
            ty: ParamType::StringArray {
                max_items: 2,
                max_chars: 3,
            },
            optional: true,
            description: "a".into(),
        }],
    };
    assert!(
        arr.validate_bounds(json!({"args": ["a", "bb"]}).as_object().unwrap())
            .is_ok()
    );
    assert_eq!(
        arr.validate_bounds(json!({"args": ["a", "b", "c"]}).as_object().unwrap())
            .unwrap_err()
            .path,
        "$.args"
    );
    assert_eq!(
        arr.validate_bounds(json!({"args": ["abcd"]}).as_object().unwrap())
            .unwrap_err()
            .path,
        "$.args[0]"
    );
}

/// ED §6.3 stage 6: workspace-path parameters are canonicalized in place; escapes are reported with their path.
#[test]
fn ed_6_3_stage_6_canonicalizes_path_parameters() {
    let s = sample().input;
    let mut args = json!({"path": "src/./lib.rs"}).as_object().unwrap().clone();
    s.canonicalize_paths(&mut args, PlatformRules::Posix)
        .unwrap();
    assert_eq!(args["path"], "src/lib.rs");
    let mut args = json!({"path": "../x"}).as_object().unwrap().clone();
    let (path, err) = s
        .canonicalize_paths(&mut args, PlatformRules::Posix)
        .unwrap_err();
    assert_eq!((path.as_str(), err), ("$.path", PathError::Escapes));
    assert_eq!(s.path_params().collect::<Vec<_>>(), ["path"]);
}

/// REQ-CTX-005 / LIM-I1: definitions render compactly within the budget, and an oversize set is refused.
#[test]
fn req_ctx_005_render_definitions_within_lim_i1() {
    let text = render_definitions(&[sample()]).unwrap();
    assert!(text.contains("\"name\":\"read_file\""));
    assert!(text.contains("\"values\":[\"read_only\",\"read_write\"]"));
    assert!(
        !text.contains("effect"),
        "effects are not shown to the model"
    );
    let mut big = sample();
    big.description = "d".repeat(LIM_I2_TOOL_DESCRIPTION_CHARS);
    let many: Vec<ToolDefinition> = (0..40)
        .map(|i| {
            let mut t = big.clone();
            t.name = format!("tool_{i}");
            t
        })
        .collect();
    let err = render_definitions(&many).unwrap_err();
    assert_eq!(err.limit, LIM_I1_TOOL_DEFINITIONS_CHARS);
    assert!(err.chars > err.limit);
}

/// REQ-TOOL-001: definitions round-trip through serde with the provider flattened.
#[test]
fn req_tool_001_definition_round_trip() {
    let mut t = sample();
    t.provider = Provider::Mcp {
        server: "rypsok".into(),
    };
    let json = serde_json::to_value(&t).unwrap();
    assert_eq!(json["provider"], "mcp");
    assert_eq!(json["server"], "rypsok");
    assert_eq!(json["input"]["params"][0]["type"], "workspace_path");
    let back: ToolDefinition = serde_json::from_value(json).unwrap();
    assert_eq!(back, t);
}
