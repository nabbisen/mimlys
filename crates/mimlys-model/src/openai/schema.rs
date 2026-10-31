//! The decision envelope as a JSON schema for `response_format`
//! (REQ-MDL-004): the five decision kinds and their fields; tool arguments
//! stay an open object, validated by the runtime's twelve stages
//! (REQ-DEC-004). Hand-written and small: it mirrors `mimlys_core::decision`.

use serde_json::{Value, json};

/// The schema's name in the request.
pub const SCHEMA_NAME: &str = "mimlys_decision";

/// The envelope schema (draft 2020-12 vocabulary that grammar converters understand).
pub fn decision_schema() -> Value {
    json!({
        "type": "object",
        "anyOf": [
            {
                "type": "object",
                "properties": {
                    "decision": {"type": "string", "enum": ["act"]},
                    "tool": {"type": "string"},
                    "arguments": {"type": "object"},
                    "reason": {"type": ["string", "null"]}
                },
                "required": ["decision", "tool", "arguments"],
                "additionalProperties": false
            },
            {
                "type": "object",
                "properties": {
                    "decision": {"type": "string", "enum": ["plan"]},
                    "steps": {"type": "array", "items": {"type": "string"}}
                },
                "required": ["decision", "steps"],
                "additionalProperties": false
            },
            {
                "type": "object",
                "properties": {
                    "decision": {"type": "string", "enum": ["ask_user"]},
                    "question": {"type": "string"}
                },
                "required": ["decision", "question"],
                "additionalProperties": false
            },
            {
                "type": "object",
                "properties": {
                    "decision": {"type": "string", "enum": ["complete"]},
                    "summary": {"type": "string"}
                },
                "required": ["decision", "summary"],
                "additionalProperties": false
            },
            {
                "type": "object",
                "properties": {
                    "decision": {"type": "string", "enum": ["give_up"]},
                    "reason": {"type": "string"}
                },
                "required": ["decision", "reason"],
                "additionalProperties": false
            }
        ]
    })
}
