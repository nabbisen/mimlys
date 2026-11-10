//! The decision schema v1 and its strict parser (external design §6.2,
//! §6.3 stages 2–3; REQ-DEC-002, REQ-DEC-003, REQ-SEC-009). Exactly one
//! JSON object, no duplicate keys, no trailing text, unknown fields
//! refused, bounds from the limits register; the model's bytes are never
//! altered to make them parse (CLR-2).

use crate::hash::{Hash, hash_canonical};
use crate::limits::{
    LIM_D1_DECISION_BYTES, LIM_D2_REASON_CHARS, LIM_D3_PLAN_ITEM_CHARS, LIM_D3_PLAN_ITEMS,
    LIM_D4_TEXT_CHARS,
};
use crate::rejection::{MalformedProblem, Rejection};
use serde::de::{self, Deserializer, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::fmt;

/// The decision schema version this parser implements (REQ-MAIN-002).
pub const DECISION_SCHEMA_VERSION: u32 = 1;

/// One decision of the model (external design §6.2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum Decision {
    /// Run a tool.
    Act {
        /// The tool's name.
        tool: String,
        /// Arguments, validated later against the tool's schema.
        arguments: Map<String, Value>,
        /// The model's stated reason; display-only, untrusted.
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    /// Replace the plan.
    Plan {
        /// Intended steps.
        steps: Vec<String>,
    },
    /// Ask the user.
    AskUser {
        /// The question.
        question: String,
    },
    /// Declare completion.
    Complete {
        /// A summary for the user.
        summary: String,
    },
    /// Give up.
    GiveUp {
        /// Why.
        reason: String,
    },
}

impl Decision {
    /// The kind's name on the wire.
    pub fn kind(&self) -> &'static str {
        match self {
            Decision::Act { .. } => "act",
            Decision::Plan { .. } => "plan",
            Decision::AskUser { .. } => "ask_user",
            Decision::Complete { .. } => "complete",
            Decision::GiveUp { .. } => "give_up",
        }
    }

    /// The canonical hash of the decision (REQ-DEC-012).
    pub fn hash(&self) -> Hash {
        hash_canonical(&serde_json::to_value(self).expect("a decision always serializes"))
    }
}

/// Parses the model's bytes strictly (REQ-DEC-003).
pub fn parse_strict(bytes: &[u8]) -> Result<Decision, Rejection> {
    decode(parse_strict_object(bytes)?)
}

/// Parses bytes that must be exactly one JSON object, within LIM-D1, with no
/// duplicate keys and nothing before or after it (REQ-DEC-003). The first
/// half of [`parse_strict`]; the `tool_calls` transport applies it to a
/// call's `arguments` string (REQ-MDL-009).
pub fn parse_strict_object(bytes: &[u8]) -> Result<Map<String, Value>, Rejection> {
    if bytes.len() > LIM_D1_DECISION_BYTES {
        return Err(Rejection::malformed(
            MalformedProblem::TooLarge,
            Some(LIM_D1_DECISION_BYTES),
        ));
    }
    let mut stream = serde_json::Deserializer::from_slice(bytes).into_iter::<StrictValue>();
    let first = match stream.next() {
        Some(Ok(value)) => value,
        Some(Err(err)) => {
            let problem = if err.to_string().contains(DUPLICATE_KEY_MARKER) {
                MalformedProblem::DuplicateKey
            } else {
                MalformedProblem::NotObject
            };
            return Err(Rejection::malformed(problem, None));
        }
        None => return Err(Rejection::malformed(MalformedProblem::NotObject, None)),
    };
    let rest = &bytes[stream.byte_offset()..];
    if !rest
        .iter()
        .all(|b| matches!(b, b' ' | b'\t' | b'\n' | b'\r'))
    {
        let problem = match stream.next() {
            Some(Ok(_)) => MalformedProblem::MultipleValues,
            _ => MalformedProblem::TrailingText,
        };
        return Err(Rejection::malformed(problem, None));
    }
    let Value::Object(object) = first.0 else {
        return Err(Rejection::malformed(MalformedProblem::NotObject, None));
    };
    Ok(object)
}

fn decode(object: Map<String, Value>) -> Result<Decision, Rejection> {
    let kind = match object.get("decision") {
        Some(Value::String(s)) => s.as_str(),
        Some(_) => return Err(Rejection::schema("$.decision", "string", Some(kinds()))),
        None => {
            return Err(Rejection::schema(
                "$.decision",
                "one of the decision kinds",
                Some(kinds()),
            ));
        }
    };
    let allowed: &[&str] = match kind {
        "act" => &["decision", "tool", "arguments", "reason"],
        "plan" => &["decision", "steps"],
        "ask_user" => &["decision", "question"],
        "complete" => &["decision", "summary"],
        "give_up" => &["decision", "reason"],
        _ => {
            return Err(Rejection::schema(
                "$.decision",
                "one of the decision kinds",
                Some(kinds()),
            ));
        }
    };
    for key in object.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(Rejection::schema(
                format!("$.{key}"),
                format!("no such field for decision `{kind}`"),
                Some(allowed.iter().map(|s| (*s).to_owned()).collect()),
            ));
        }
    }
    match kind {
        "act" => {
            let tool = required_string(&object, "tool", 1, 64)?;
            let arguments = match object.get("arguments") {
                Some(Value::Object(map)) => map.clone(),
                Some(_) => return Err(Rejection::schema("$.arguments", "object", None)),
                None => return Err(Rejection::schema("$.arguments", "object", None)),
            };
            let reason = optional_string(&object, "reason", LIM_D2_REASON_CHARS)?;
            Ok(Decision::Act {
                tool,
                arguments,
                reason,
            })
        }
        "plan" => {
            let steps = match object.get("steps") {
                Some(Value::Array(items)) => items,
                _ => return Err(Rejection::schema("$.steps", "array of strings", None)),
            };
            if steps.is_empty() || steps.len() > LIM_D3_PLAN_ITEMS {
                return Err(Rejection::schema(
                    "$.steps",
                    format!("between 1 and {LIM_D3_PLAN_ITEMS} items"),
                    None,
                ));
            }
            let mut out = Vec::with_capacity(steps.len());
            for (i, item) in steps.iter().enumerate() {
                match item {
                    Value::String(s)
                        if (1..=LIM_D3_PLAN_ITEM_CHARS).contains(&s.chars().count()) =>
                    {
                        out.push(s.clone());
                    }
                    _ => {
                        return Err(Rejection::schema(
                            format!("$.steps[{i}]"),
                            format!("string of 1 to {LIM_D3_PLAN_ITEM_CHARS} characters"),
                            None,
                        ));
                    }
                }
            }
            Ok(Decision::Plan { steps: out })
        }
        "ask_user" => Ok(Decision::AskUser {
            question: required_string(&object, "question", 1, LIM_D4_TEXT_CHARS)?,
        }),
        "complete" => Ok(Decision::Complete {
            summary: required_string(&object, "summary", 1, LIM_D4_TEXT_CHARS)?,
        }),
        _ => Ok(Decision::GiveUp {
            reason: required_string(&object, "reason", 1, LIM_D2_REASON_CHARS)?,
        }),
    }
}

fn kinds() -> Vec<String> {
    ["act", "plan", "ask_user", "complete", "give_up"]
        .iter()
        .map(|s| (*s).to_owned())
        .collect()
}

fn required_string(
    object: &Map<String, Value>,
    key: &str,
    min: usize,
    max: usize,
) -> Result<String, Rejection> {
    match object.get(key) {
        Some(Value::String(s)) if (min..=max).contains(&s.chars().count()) => Ok(s.clone()),
        _ => Err(Rejection::schema(
            format!("$.{key}"),
            format!("string of {min} to {max} characters"),
            None,
        )),
    }
}

fn optional_string(
    object: &Map<String, Value>,
    key: &str,
    max: usize,
) -> Result<Option<String>, Rejection> {
    match object.get(key) {
        None => Ok(None),
        Some(Value::String(s)) if s.chars().count() <= max => Ok(Some(s.clone())),
        Some(_) => Err(Rejection::schema(
            format!("$.{key}"),
            format!("string of at most {max} characters"),
            None,
        )),
    }
}

const DUPLICATE_KEY_MARKER: &str = "duplicate key";

/// A JSON value whose objects refuse duplicate keys (`serde_json::Value`
/// silently keeps the last one).
struct StrictValue(Value);

impl<'de> Deserialize<'de> for StrictValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(StrictVisitor)
    }
}

struct StrictVisitor;

impl<'de> Visitor<'de> for StrictVisitor {
    type Value = StrictValue;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("any JSON value without duplicate keys")
    }

    fn visit_bool<E: de::Error>(self, v: bool) -> Result<StrictValue, E> {
        Ok(StrictValue(Value::Bool(v)))
    }

    fn visit_i64<E: de::Error>(self, v: i64) -> Result<StrictValue, E> {
        Ok(StrictValue(Value::from(v)))
    }

    fn visit_u64<E: de::Error>(self, v: u64) -> Result<StrictValue, E> {
        Ok(StrictValue(Value::from(v)))
    }

    fn visit_f64<E: de::Error>(self, v: f64) -> Result<StrictValue, E> {
        Ok(StrictValue(
            serde_json::Number::from_f64(v)
                .map(Value::Number)
                .unwrap_or(Value::Null),
        ))
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<StrictValue, E> {
        Ok(StrictValue(Value::String(v.to_owned())))
    }

    fn visit_string<E: de::Error>(self, v: String) -> Result<StrictValue, E> {
        Ok(StrictValue(Value::String(v)))
    }

    fn visit_none<E: de::Error>(self) -> Result<StrictValue, E> {
        Ok(StrictValue(Value::Null))
    }

    fn visit_unit<E: de::Error>(self) -> Result<StrictValue, E> {
        Ok(StrictValue(Value::Null))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<StrictValue, A::Error> {
        let mut items = Vec::new();
        while let Some(StrictValue(item)) = seq.next_element::<StrictValue>()? {
            items.push(item);
        }
        Ok(StrictValue(Value::Array(items)))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<StrictValue, A::Error> {
        let mut object = Map::new();
        while let Some(key) = map.next_key::<String>()? {
            let StrictValue(value) = map.next_value::<StrictValue>()?;
            if object.insert(key.clone(), value).is_some() {
                return Err(de::Error::custom(format!("{DUPLICATE_KEY_MARKER}: {key}")));
            }
        }
        Ok(StrictValue(Value::Object(object)))
    }
}

#[cfg(test)]
mod tests;
