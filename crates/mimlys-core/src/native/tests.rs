use super::*;
use crate::action::{EffectKind, EffectView};
use crate::decision::{Decision, parse_strict};
use crate::limits::LIM_D1_DECISION_BYTES;
use crate::rejection::RejectionCode;
use crate::tool::{Param, Provider, ResourceProfile};

fn tool(name: &str) -> ToolDefinition {
    ToolDefinition {
        name: name.into(),
        description: format!("The tool {name}."),
        input: ParamSchema {
            params: vec![
                Param {
                    name: "path".into(),
                    ty: ParamType::WorkspacePath,
                    optional: false,
                    description: "The file.".into(),
                },
                Param {
                    name: "limit".into(),
                    ty: ParamType::Integer {
                        min: 1,
                        max: i64::MAX,
                    },
                    optional: true,
                    description: String::new(),
                },
            ],
        },
        output: None,
        effect: EffectView {
            kind: EffectKind::WorkspaceRead,
            reversible: true,
            idempotent: true,
        },
        deadline_ms: 1000,
        profile: ResourceProfile::Builtin,
        destination_arguments: vec![],
        provider: Provider::Builtin,
    }
}

fn names(offer: &Offer) -> Vec<&str> {
    offer.functions.iter().map(|f| f.name.as_str()).collect()
}

fn call(name: &str, arguments: &str) -> ReplyCall {
    ReplyCall {
        name: name.into(),
        arguments: Some(arguments.into()),
    }
}

/// REQ-MDL-009: one function per tool, then the four reserved ones; the
/// same input gives the same bytes; `.` is written `__`.
#[test]
fn req_mdl_009_functions_are_deterministic_and_named_by_the_rule() {
    let tools = vec![tool("read_file"), tool("fs.read_text")];
    let first = offer(&tools);
    let second = offer(&tools);
    assert_eq!(
        serde_json::to_vec(&tools_json(&first.functions)).unwrap(),
        serde_json::to_vec(&tools_json(&second.functions)).unwrap(),
        "the same input, the same bytes"
    );
    assert_eq!(
        names(&first),
        [
            "read_file",
            "fs__read_text",
            "plan",
            "ask_user",
            "complete",
            "give_up"
        ]
    );
    assert_eq!(first.functions[1].tool.as_deref(), Some("fs.read_text"));
    assert!(first.unavailable.is_empty());
    let schema = &first.functions[0].parameters;
    assert_eq!(
        *schema,
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "The file."},
                "limit": {"type": "integer", "minimum": 1},
            },
            "required": ["path"],
            "additionalProperties": false,
        }),
        "an integer bound beyond 2^53 is not stated; an empty description is omitted"
    );
}

/// REQ-MDL-009: a tool whose function name is invalid, equals a reserved one
/// or equals another tool's is unavailable in this transport, with the
/// reason; the other tools are offered unchanged.
#[test]
fn req_mdl_009_invalid_and_colliding_names_are_unavailable_with_the_reason() {
    // 64 characters are valid as a tool name; the server prefix makes 65 as a function name.
    let long = format!("s.{}", "a".repeat(62));
    assert_eq!(long.len(), 64);
    let tools = vec![
        tool("read_file"),
        tool(&long),
        tool("plan"),
        tool("a.b"),
        tool("a__b"),
    ];
    let offered = offer(&tools);
    assert_eq!(
        names(&offered),
        ["read_file", "plan", "ask_user", "complete", "give_up"],
        "only read_file and the four reserved functions are offered"
    );
    assert!(
        offered
            .functions
            .iter()
            .filter(|f| f.name == "plan")
            .all(|f| f.tool.is_none()),
        "the offered `plan` is the reserved one"
    );
    assert_eq!(
        offered.unavailable,
        vec![
            (long.clone(), UNAVAILABLE_NAME_INVALID.to_owned()),
            ("plan".to_owned(), UNAVAILABLE_NAME_RESERVED.to_owned()),
            ("a.b".to_owned(), UNAVAILABLE_NAME_COLLISION.to_owned()),
            ("a__b".to_owned(), UNAVAILABLE_NAME_COLLISION.to_owned()),
        ]
    );
    assert!(
        offered
            .functions
            .iter()
            .all(|f| valid_function_name(&f.name))
    );
    assert!(valid_function_name("a-b_9"));
    assert!(!valid_function_name(""));
    assert!(!valid_function_name("a.b"));
    assert!(!valid_function_name(&"x".repeat(65)));
}

fn mapped(functions: &[Function], calls: &[ReplyCall]) -> Result<Decision, ReplyProblem> {
    let bytes = map_reply(functions, calls)?;
    Ok(parse_strict(&bytes).expect("a mapped call passes stage 1"))
}

/// REQ-DEC-003 (amended): exactly one call naming an offered function with
/// one JSON object as `arguments` is the decision; the tool's name comes
/// from the offered set.
#[test]
fn req_dec_003_one_call_is_the_mapped_decision() {
    let functions = offer(&[tool("read_file"), tool("fs.read__text")]).functions;
    assert_eq!(
        mapped(&functions, &[call("read_file", r#"{"path":"a.txt"}"#)]),
        Ok(Decision::Act {
            tool: "read_file".into(),
            arguments: serde_json::from_str(r#"{"path":"a.txt"}"#).unwrap(),
            reason: None,
        })
    );
    // The name is looked up, never rewritten: `__` inside a tool's own name stays.
    assert_eq!(
        mapped(&functions, &[call("fs__read__text", "{}")]),
        Ok(Decision::Act {
            tool: "fs.read__text".into(),
            arguments: serde_json::Map::new(),
            reason: None,
        })
    );
    assert_eq!(
        mapped(&functions, &[call("plan", r#"{"steps":["a","b"]}"#)]),
        Ok(Decision::Plan {
            steps: vec!["a".into(), "b".into()]
        })
    );
    assert_eq!(
        mapped(&functions, &[call("ask_user", r#"{"question":"which?"}"#)]),
        Ok(Decision::AskUser {
            question: "which?".into()
        })
    );
    assert_eq!(
        mapped(&functions, &[call("complete", r#"{"summary":"done"}"#)]),
        Ok(Decision::Complete {
            summary: "done".into()
        })
    );
    assert_eq!(
        mapped(&functions, &[call("give_up", r#"{"reason":"no"}"#)]),
        Ok(Decision::GiveUp {
            reason: "no".into()
        })
    );
}

/// REQ-DEC-003 (amended): zero calls, several, an unknown name, `arguments`
/// that is not a string or not exactly one object, an object above LIM-D1,
/// and a `decision` field smuggled into a reserved call are all refused.
#[test]
fn req_dec_003_every_other_shape_is_refused() {
    let functions = offer(&[tool("read_file")]).functions;
    let ok = call("read_file", r#"{"path":"a"}"#);
    let table: Vec<(Vec<ReplyCall>, ReplyProblem)> = vec![
        (vec![], ReplyProblem::NoCall),
        (vec![ok.clone(), ok.clone()], ReplyProblem::SeveralCalls),
        (
            vec![call("read__file", r#"{"path":"a"}"#)],
            ReplyProblem::UnknownFunction,
        ),
        (
            vec![call("delete_path", "{}")],
            ReplyProblem::UnknownFunction,
        ),
        (
            vec![ReplyCall {
                name: "read_file".into(),
                arguments: None,
            }],
            ReplyProblem::ArgumentsNotText,
        ),
        (
            vec![call("read_file", "[1]")],
            ReplyProblem::ArgumentsNotObject,
        ),
        (
            vec![call("read_file", "")],
            ReplyProblem::ArgumentsNotObject,
        ),
        (
            vec![call("read_file", r#"{"path":"a"} trailing"#)],
            ReplyProblem::ArgumentsNotObject,
        ),
        (
            vec![call("read_file", r#"{"path":"a"}{"path":"b"}"#)],
            ReplyProblem::ArgumentsNotObject,
        ),
        (
            vec![call("read_file", r#"{"path":"a","path":"b"}"#)],
            ReplyProblem::ArgumentsNotObject,
        ),
        (
            vec![call("read_file", r#"{"path":"a"#)],
            ReplyProblem::ArgumentsNotObject,
        ),
        (
            vec![call(
                "read_file",
                &format!(r#"{{"path":"{}"}}"#, "a".repeat(LIM_D1_DECISION_BYTES)),
            )],
            ReplyProblem::ArgumentsNotObject,
        ),
        (
            vec![call("plan", r#"{"steps":["a"],"decision":"complete"}"#)],
            ReplyProblem::DecisionField,
        ),
    ];
    for (calls, problem) in table {
        assert_eq!(
            map_reply(&functions, &calls),
            Err(problem),
            "{calls:?} should be {problem:?}"
        );
    }
}

/// REQ-DEC-003: what an adapter returns for a reply without a valid
/// decision is rejected by the strict parser as `malformed`; no text of the
/// reply can stand in for it.
#[test]
fn req_dec_003_no_decision_is_malformed() {
    let rejection = parse_strict(NO_DECISION).unwrap_err();
    assert_eq!(rejection.code, RejectionCode::Malformed);
}

/// REQ-DEC-003, REQ-OBS-001: the hash of the mapped call is the canonical
/// hash of the envelope, the same as the identical decision written as text.
#[test]
fn req_dec_003_the_mapped_call_has_the_envelopes_hash() {
    let functions = offer(&[tool("read_file")]).functions;
    let by_call = mapped(&functions, &[call("read_file", r#"{ "path" : "a.txt" }"#)]).unwrap();
    let by_text =
        parse_strict(br#"{"decision":"act","tool":"read_file","arguments":{"path":"a.txt"}}"#)
            .unwrap();
    assert_eq!(by_call.hash(), by_text.hash());
}
