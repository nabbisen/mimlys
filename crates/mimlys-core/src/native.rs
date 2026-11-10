//! The `tool_calls` decision transport, its pure half (REQ-MDL-009,
//! REQ-DEC-003; RFC 012; external design §15.4): the functions offered to
//! the model, generated deterministically from the pinned tool definitions,
//! and the mapping of a reply's one structured call onto the decision
//! envelope. Nothing here performs I/O; the HTTP half lives in the adapter.
//!
//! The pinned definitions and their hashes do not change: the functions are a
//! view of them, rebuilt identically from the same list.

use crate::decision::parse_strict_object;
use crate::tool::{ParamSchema, ParamType, ToolDefinition};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

/// How a decision reaches the runtime (REQ-MDL-009).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionTransport {
    /// The model's structured `tool_calls`, mapped onto the envelope.
    ToolCalls,
    /// The decision envelope written as text (the default of events recorded before 0.9).
    #[default]
    Envelope,
}

impl DecisionTransport {
    /// The name on the wire, in events and in `status`.
    pub fn as_str(self) -> &'static str {
        match self {
            DecisionTransport::ToolCalls => "tool_calls",
            DecisionTransport::Envelope => "envelope",
        }
    }
}

/// The four decision kinds that are not tools, offered as functions of the
/// same name, with the one field each carries.
pub const RESERVED_FUNCTIONS: [(&str, &str); 4] = [
    ("plan", "steps"),
    ("ask_user", "question"),
    ("complete", "summary"),
    ("give_up", "reason"),
];

/// Why a tool is not offered as a function (the fixed vocabulary of `tool_unavailable`).
pub const UNAVAILABLE_NAME_INVALID: &str = "function_name_invalid";
/// The function name equals a reserved one.
pub const UNAVAILABLE_NAME_RESERVED: &str = "function_name_reserved";
/// The function name equals another tool's.
pub const UNAVAILABLE_NAME_COLLISION: &str = "function_name_collision";

/// What the strict parser sees for a reply that carries no valid decision:
/// the empty input, which `parse_strict` rejects as `malformed`. An adapter
/// returns it instead of any part of the reply, so that nothing the model
/// wrote outside one structured call can be read as a decision.
pub const NO_DECISION: &[u8] = b"";

/// One function offered to the model.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Function {
    /// The function's name: the tool's with `.` written `__`, or a reserved name.
    pub name: String,
    /// The tool it stands for; `None` for the four reserved functions.
    pub tool: Option<String>,
    /// One sentence for the model.
    pub description: String,
    /// A JSON-schema object for the arguments.
    pub parameters: Value,
}

/// The functions of one step and the tools left out.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Offer {
    /// Tools first, in the pinned order, then the four reserved functions.
    pub functions: Vec<Function>,
    /// Tools not offered, with the reason (`function_name_*`).
    pub unavailable: Vec<(String, String)>,
}

/// A tool's name as a function name: `.` is written `__`.
pub fn function_name(tool: &str) -> String {
    tool.replace('.', "__")
}

/// Whether `name` matches `^[A-Za-z0-9_-]{1,64}$`.
pub fn valid_function_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Builds the functions for `tools`: one per tool whose function name is
/// valid and unique, plus the four reserved ones. A tool whose name is
/// invalid, equals a reserved name, or equals another tool's is left out, and
/// so is the other tool of a collision, so that no tool shadows another
/// (REQ-MDL-009). Deterministic: the same list gives the same bytes.
pub fn offer(tools: &[ToolDefinition]) -> Offer {
    let names: Vec<String> = tools.iter().map(|t| function_name(&t.name)).collect();
    let mut offer = Offer::default();
    for (tool, name) in tools.iter().zip(&names) {
        let reason = if !valid_function_name(name) {
            Some(UNAVAILABLE_NAME_INVALID)
        } else if RESERVED_FUNCTIONS.iter().any(|(r, _)| r == name) {
            Some(UNAVAILABLE_NAME_RESERVED)
        } else if names.iter().filter(|n| *n == name).count() > 1 {
            Some(UNAVAILABLE_NAME_COLLISION)
        } else {
            None
        };
        match reason {
            Some(reason) => offer
                .unavailable
                .push((tool.name.clone(), reason.to_owned())),
            None => offer.functions.push(Function {
                name: name.clone(),
                tool: Some(tool.name.clone()),
                description: tool.description.clone(),
                parameters: object_schema(&tool.input),
            }),
        }
    }
    offer.functions.extend(reserved_functions());
    offer
}

fn reserved_functions() -> Vec<Function> {
    let function = |name: &str, description: &str, field: &str, schema: Value| Function {
        name: name.to_owned(),
        tool: None,
        description: description.to_owned(),
        parameters: json!({
            "type": "object",
            "properties": { field: schema },
            "required": [field],
            "additionalProperties": false,
        }),
    };
    vec![
        function(
            "plan",
            "Record your intended steps.",
            "steps",
            json!({"type": "array", "items": {"type": "string"}}),
        ),
        function(
            "ask_user",
            "Ask the user a question.",
            "question",
            json!({"type": "string"}),
        ),
        function(
            "complete",
            "The goal is reached; give a summary.",
            "summary",
            json!({"type": "string"}),
        ),
        function(
            "give_up",
            "The goal cannot be reached; give the reason.",
            "reason",
            json!({"type": "string"}),
        ),
    ]
}

/// The largest integer a JSON number carries exactly in every reader.
const SAFE_INTEGER: i64 = (1 << 53) - 1;

/// A flat parameter schema as a JSON-schema object. Bounds other than an
/// integer's are not stated: stage 8 enforces them and names the bound in
/// its rejection.
fn object_schema(schema: &ParamSchema) -> Value {
    let mut properties = Map::new();
    let mut required = Vec::new();
    for p in &schema.params {
        let mut property = match &p.ty {
            ParamType::String { .. } | ParamType::WorkspacePath => json!({"type": "string"}),
            ParamType::Integer { min, max } => {
                let mut v = json!({"type": "integer"});
                if min.abs() <= SAFE_INTEGER {
                    v["minimum"] = (*min).into();
                }
                if max.abs() <= SAFE_INTEGER {
                    v["maximum"] = (*max).into();
                }
                v
            }
            ParamType::Boolean => json!({"type": "boolean"}),
            ParamType::Enum { values } => json!({"type": "string", "enum": values}),
            ParamType::StringArray { .. } => json!({"type": "array", "items": {"type": "string"}}),
            ParamType::Object { schema } => object_schema(schema),
        };
        if !p.description.is_empty() {
            property["description"] = Value::String(p.description.clone());
        }
        properties.insert(p.name.clone(), property);
        if !p.optional {
            required.push(Value::String(p.name.clone()));
        }
    }
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    })
}

/// The request's `tools` array, exactly as it is sent.
pub fn tools_json(functions: &[Function]) -> Value {
    Value::Array(
        functions
            .iter()
            .map(|f| {
                json!({
                    "type": "function",
                    "function": {
                        "name": f.name,
                        "description": f.description,
                        "parameters": f.parameters,
                    },
                })
            })
            .collect(),
    )
}

/// The characters of the `tools` array as sent: what REQ-CTX-003 counts
/// beside the messages.
pub fn tools_chars(functions: &[Function]) -> usize {
    if functions.is_empty() {
        return 0;
    }
    tools_json(functions).to_string().chars().count()
}

/// One entry of the reply's `tool_calls`, as the adapter read it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplyCall {
    /// The function's name.
    pub name: String,
    /// The `arguments`, when the engine sent a string; `None` for any other JSON type.
    pub arguments: Option<String>,
}

/// Why a reply carries no valid decision. The runtime sees `malformed` for
/// all of them; the problem is for the log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReplyProblem {
    /// No structured call (a call written into the text is never read).
    NoCall,
    /// More than one structured call.
    SeveralCalls,
    /// The name is not an offered function.
    UnknownFunction,
    /// `arguments` was not a string.
    ArgumentsNotText,
    /// `arguments` is not exactly one JSON object within LIM-D1.
    ArgumentsNotObject,
    /// A reserved function's arguments carry a `decision` field.
    DecisionField,
}

impl ReplyProblem {
    /// A fixed name for the log.
    pub fn as_str(self) -> &'static str {
        match self {
            ReplyProblem::NoCall => "no_call",
            ReplyProblem::SeveralCalls => "several_calls",
            ReplyProblem::UnknownFunction => "unknown_function",
            ReplyProblem::ArgumentsNotText => "arguments_not_text",
            ReplyProblem::ArgumentsNotObject => "arguments_not_object",
            ReplyProblem::DecisionField => "decision_field",
        }
    }
}

/// Maps a reply's structured calls onto the decision envelope's bytes
/// (REQ-DEC-003, amended): exactly one call, naming an offered function,
/// whose `arguments` is exactly one JSON object within LIM-D1. The name is
/// looked up in `functions`; it is never rewritten. The bytes then pass the
/// twelve stages from stage 1; nothing is repaired.
pub fn map_reply(functions: &[Function], calls: &[ReplyCall]) -> Result<Vec<u8>, ReplyProblem> {
    let call = match calls {
        [] => return Err(ReplyProblem::NoCall),
        [one] => one,
        _ => return Err(ReplyProblem::SeveralCalls),
    };
    let function = functions
        .iter()
        .find(|f| f.name == call.name)
        .ok_or(ReplyProblem::UnknownFunction)?;
    let text = call
        .arguments
        .as_deref()
        .ok_or(ReplyProblem::ArgumentsNotText)?;
    let arguments =
        parse_strict_object(text.as_bytes()).map_err(|_| ReplyProblem::ArgumentsNotObject)?;
    let envelope = match &function.tool {
        Some(tool) => json!({"decision": "act", "tool": tool, "arguments": arguments}),
        None => {
            if arguments.contains_key("decision") {
                return Err(ReplyProblem::DecisionField);
            }
            let mut object = arguments;
            object.insert("decision".into(), Value::String(function.name.clone()));
            Value::Object(object)
        }
    };
    Ok(serde_json::to_vec(&envelope).expect("an envelope serializes"))
}

#[cfg(test)]
mod tests;
