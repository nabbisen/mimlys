//! Definition import (RFC 009 §2, §3; REQ-MCP-002, REQ-MCP-003,
//! REQ-MCP-004): the server's tool becomes `<server>.<tool>` with a
//! sanitized, bounded description, a flat parameter schema, the
//! administrator's effect declaration, and the output schema kept whole
//! for validation. Anything the flat model cannot express makes the tool
//! unavailable with a named reason; annotations are recorded and used for
//! nothing.

use crate::server::{Declaration, Listed};
use mimlys_core::action::EffectView;
use mimlys_core::hash::{Hash, hash_canonical};
use mimlys_core::limits::{
    LIM_D4_TEXT_CHARS, LIM_I2_TOOL_DESCRIPTION_CHARS, LIM_I3_TOOL_PARAMETERS, LIM_I7_ENTRIES,
    LIM_I20_TOOL_DEFINITION_CHARS, LIM_I21_ENUM_VALUES,
};
use mimlys_core::observation::sanitize;
use mimlys_core::schema::OutputSchema;
use mimlys_core::tool::{
    ImportRecord, Param, ParamSchema, ParamType, Provider, ResourceProfile, ToolDefinition,
    rendered_chars,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// One imported tool: its definition (when available) and its record.
#[derive(Clone, Debug)]
pub struct Imported {
    /// The full name `server.tool`.
    pub name: String,
    /// The definition the model may see; `None` when unavailable.
    pub definition: Option<ToolDefinition>,
    /// The parsed output schema, when declared and supported.
    pub output: Option<OutputSchema>,
    /// The record on the task's versions.
    pub record: ImportRecord,
}

/// The hash of each listed tool's definition as the server states it
/// (name, description, schemas, annotations): what pinning compares.
pub fn hashes(server: &str, tools: &[Listed]) -> BTreeMap<String, Hash> {
    tools
        .iter()
        .map(|t| (t.name.clone(), listed_hash(server, t)))
        .collect()
}

fn listed_hash(server: &str, t: &Listed) -> Hash {
    hash_canonical(&json!({
        "server": server,
        "name": t.name,
        "description": t.description,
        "input_schema": t.input_schema,
        "output_schema": t.output_schema,
        "annotations": t.annotations,
    }))
}

/// Imports one listed tool under the declaration (or the undeclared defaults).
/// `max_call_ms` is the server's declared maximum call time: the tool's
/// deadline, declared or defaulted, is never below it (REQ-MCP-008).
pub fn import(
    server: &str,
    listed: &Listed,
    declaration: Option<&Declaration>,
    max_call_ms: Option<i64>,
) -> Imported {
    let name = format!("{server}.{}", listed.name);
    let hash = listed_hash(server, listed);
    let declared = declaration.is_some();
    let undeclared = Declaration::undeclared();
    let declaration = declaration.unwrap_or(&undeclared);
    let mut record = ImportRecord {
        server: server.to_owned(),
        hash,
        annotations: listed.annotations.clone(),
        description_truncated: false,
        declared,
        unavailable: None,
    };
    let unavailable = |record: &mut ImportRecord, reason: String| {
        record.unavailable = Some(clean_reason(&reason));
    };
    // Description: sanitized, bounded.
    let (clean, _) = sanitize(listed.description.as_deref().unwrap_or(""));
    let clean = clean.split_whitespace().collect::<Vec<_>>().join(" ");
    let description = if clean.chars().count() > LIM_I2_TOOL_DESCRIPTION_CHARS {
        record.description_truncated = true;
        clean
            .chars()
            .take(LIM_I2_TOOL_DESCRIPTION_CHARS - 1)
            .collect::<String>()
            + "…"
    } else if clean.is_empty() {
        // The server's name for the tool is cleaned like any text of its own.
        format!("The external tool {}.", sanitize(&name).0)
    } else {
        clean
    };
    // Input schema → the flat model.
    let input = match convert_input(&listed.input_schema) {
        Ok(schema) => schema,
        Err(reason) => {
            unavailable(&mut record, reason);
            return Imported {
                name,
                definition: None,
                output: None,
                record,
            };
        }
    };
    for arg in &declaration.destination_arguments {
        // A destination is a string (REQ-MCP-005): the parameter must exist, and
        // its schema type must be `string` (an enumeration of strings is one).
        let reason = match input.params.iter().find(|p| &p.name == arg) {
            None => Some(format!("destination_unknown:{arg}")),
            Some(p) if !matches!(p.ty, ParamType::String { .. } | ParamType::Enum { .. }) => {
                Some(format!("destination_not_string:{arg}"))
            }
            Some(_) => None,
        };
        if let Some(reason) = reason {
            unavailable(&mut record, reason);
            return Imported {
                name,
                definition: None,
                output: None,
                record,
            };
        }
    }
    // Output schema: kept whole; unsupported constructs make the tool unavailable (fail closed).
    let output = match &listed.output_schema {
        Some(schema) => match OutputSchema::parse(schema) {
            Ok(parsed) => Some(parsed),
            Err(e) => {
                unavailable(
                    &mut record,
                    format!("output_schema_unsupported:{}:{}", e.path, e.construct),
                );
                return Imported {
                    name,
                    definition: None,
                    output: None,
                    record,
                };
            }
        },
        None => None,
    };
    let definition = ToolDefinition {
        name: name.clone(),
        description,
        input,
        output: None,
        effect: EffectView {
            kind: declaration.effect,
            reversible: false,
            idempotent: declaration.idempotent,
        },
        deadline_ms: max_call_ms.map_or(declaration.timeout_ms, |m| declaration.timeout_ms.max(m)),
        profile: ResourceProfile::External,
        destination_arguments: declaration.destination_arguments.clone(),
        provider: Provider::Mcp {
            server: server.to_owned(),
        },
    };
    if let Err(e) = definition.check() {
        unavailable(&mut record, format!("definition_invalid:{e}"));
        return Imported {
            name,
            definition: None,
            output,
            record,
        };
    }
    // One tool's rendering alone is bounded (handoff 009i, A-32): a tool that
    // would take more than its share of LIM-I1 is not offered, so that it can
    // never be the reason a step fails for the size of the definitions.
    let chars = rendered_chars(std::slice::from_ref(&definition));
    if chars > LIM_I20_TOOL_DEFINITION_CHARS {
        unavailable(
            &mut record,
            format!("definition_too_large:{chars}>{LIM_I20_TOOL_DEFINITION_CHARS}"),
        );
        return Imported {
            name,
            definition: None,
            output,
            record,
        };
    }
    Imported {
        name,
        definition: Some(definition),
        output,
        record,
    }
}

/// Converts a JSON Schema object into the flat parameter model (LIM-I3,
/// LIM-I4): string, integer, boolean, enum, string array, one nested object.
fn convert_input(schema: &Value) -> Result<ParamSchema, String> {
    let map = schema
        .as_object()
        .ok_or_else(|| "schema_unsupported:input:non-object".to_owned())?;
    if let Some(t) = map.get("type")
        && t != "object"
    {
        return Err(format!("schema_unsupported:input:type {t}"));
    }
    let required: Vec<String> = map
        .get("required")
        .and_then(Value::as_array)
        .map(|r| {
            r.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    let properties = map
        .get("properties")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    if properties.len() > LIM_I3_TOOL_PARAMETERS {
        return Err(format!(
            "too_many_parameters:{}>{LIM_I3_TOOL_PARAMETERS}",
            properties.len()
        ));
    }
    // Every name is checked before anything is converted, so that no reason
    // below repeats a name that was not.
    for (i, name) in properties.keys().enumerate() {
        if !valid_parameter_name(name) {
            return Err(format!(
                "definition_invalid:parameter {} has a name that is not allowed",
                i + 1
            ));
        }
    }
    let mut params = Vec::new();
    for (i, (name, prop)) in properties.iter().enumerate() {
        let ty = convert_param(prop, name, &format!("parameter {}", i + 1), 0)?;
        let description = prop
            .get("description")
            .and_then(Value::as_str)
            .map(|d| {
                let (clean, _) = sanitize(d);
                clean.split_whitespace().collect::<Vec<_>>().join(" ")
            })
            .filter(|d| !d.is_empty())
            .unwrap_or_else(|| format!("The {name} parameter."));
        let description: String = description.chars().take(LIM_I5_CHARS).collect();
        params.push(Param {
            name: name.clone(),
            ty,
            optional: !required.contains(name),
            description,
        });
    }
    Ok(ParamSchema { params })
}

/// Parameter descriptions are bounded like messages (LIM-I5).
const LIM_I5_CHARS: usize = mimlys_core::limits::LIM_I5_MESSAGE_CHARS;

/// The reason a tool is unavailable as it is kept and shown: some of it comes
/// from the server (a name, a keyword), so it is cleaned like a description and
/// cut to the same bound as a message (LIM-I5).
fn clean_reason(reason: &str) -> String {
    let (clean, _) = sanitize(reason);
    let clean = clean.split_whitespace().collect::<Vec<_>>().join(" ");
    if clean.chars().count() > LIM_I5_CHARS {
        clean.chars().take(LIM_I5_CHARS - 1).collect::<String>() + "…"
    } else {
        clean
    }
}

/// The longest a parameter's name may be (LIM-I10): as long as a tool's name.
const PARAMETER_NAME_CHARS: usize = 64;

/// A parameter's name is plain: ASCII letters and digits, `_`, `.` and `-`,
/// from one to [`PARAMETER_NAME_CHARS`] characters. A name that is anything else
/// is never shown to the user or the model as it is.
fn valid_parameter_name(name: &str) -> bool {
    !name.is_empty()
        && name.chars().count() <= PARAMETER_NAME_CHARS
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
}

/// An enumeration value as the model is shown it: cleaned like a description
/// and cut to the same bound as one (LIM-I5).
fn clean_enum_value(value: &str) -> String {
    let (clean, _) = sanitize(value);
    clean
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(LIM_I5_CHARS)
        .collect()
}

/// `at` says where the parameter is for a reason that names no name: its
/// position in the schema, counted from 1 (`parameter 2`, `parameter 2, field 1`).
fn convert_param(prop: &Value, name: &str, at: &str, depth: usize) -> Result<ParamType, String> {
    let map = prop
        .as_object()
        .ok_or_else(|| format!("schema_unsupported:{name}:non-object"))?;
    for key in map.keys() {
        if [
            "anyOf",
            "oneOf",
            "allOf",
            "$ref",
            "not",
            "patternProperties",
        ]
        .contains(&key.as_str())
        {
            return Err(format!("schema_unsupported:{name}:{key}"));
        }
    }
    let ty_name: String = match map.get("type") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(items)) => {
            // `["string", "null"]`: the non-null type.
            let names: Vec<&str> = items.iter().filter_map(Value::as_str).collect();
            match names.iter().find(|n| **n != "null") {
                Some(n) if names.len() <= 2 => (*n).to_owned(),
                _ => return Err(format!("schema_unsupported:{name}:type {items:?}")),
            }
        }
        None if map.contains_key("enum") => "string".to_owned(),
        _ => return Err(format!("schema_unsupported:{name}:missing type")),
    };
    let int = |key: &str| map.get(key).and_then(Value::as_i64);
    let uint = |key: &str| map.get(key).and_then(Value::as_u64).map(|v| v as usize);
    Ok(match ty_name.as_str() {
        "string" => {
            if let Some(values) = map.get("enum") {
                let values = values
                    .as_array()
                    .ok_or_else(|| format!("schema_unsupported:{name}:non-array enum"))?;
                // Counted before any value is read (handoff 009i, A-32): the
                // work of cleaning them, and their share of the definition, are
                // bounded by it.
                if values.len() > LIM_I21_ENUM_VALUES {
                    return Err(format!(
                        "too_many_enum_values:{name}:{}>{LIM_I21_ENUM_VALUES}",
                        values.len()
                    ));
                }
                let values: Vec<String> = values
                    .iter()
                    .map(|v| {
                        v.as_str().map(clean_enum_value).ok_or_else(|| {
                            format!("schema_unsupported:{name}:non-string enum value")
                        })
                    })
                    .collect::<Result<_, _>>()?;
                // Values that cleaning emptied or made equal are one value, or none.
                let mut kept: Vec<String> = Vec::new();
                for value in values {
                    if !value.is_empty() && !kept.contains(&value) {
                        kept.push(value);
                    }
                }
                if kept.is_empty() {
                    return Err(format!(
                        "definition_invalid:{at} has no enumeration value that is allowed"
                    ));
                }
                ParamType::Enum { values: kept }
            } else {
                ParamType::String {
                    min_chars: uint("minLength").unwrap_or(0),
                    max_chars: uint("maxLength").unwrap_or(LIM_D4_TEXT_CHARS),
                    max_bytes: None,
                }
            }
        }
        "integer" => ParamType::Integer {
            min: int("minimum").unwrap_or(i64::MIN / 2),
            max: int("maximum").unwrap_or(i64::MAX / 2),
        },
        "boolean" => ParamType::Boolean,
        "array" => {
            let items = map
                .get("items")
                .ok_or_else(|| format!("schema_unsupported:{name}:array without items"))?;
            if items.get("type") != Some(&Value::String("string".into())) {
                return Err(format!("schema_unsupported:{name}:array of non-strings"));
            }
            ParamType::StringArray {
                max_items: uint("maxItems").unwrap_or(LIM_I7_ENTRIES),
                max_chars: items
                    .get("maxLength")
                    .and_then(Value::as_u64)
                    .map(|v| v as usize)
                    .unwrap_or(LIM_D4_TEXT_CHARS),
            }
        }
        "object" => {
            if depth >= 1 {
                return Err(format!(
                    "schema_unsupported:{name}:nesting deeper than LIM-I4"
                ));
            }
            let required: Vec<String> = map
                .get("required")
                .and_then(Value::as_array)
                .map(|r| {
                    r.iter()
                        .filter_map(|v| v.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default();
            let properties = map
                .get("properties")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            if properties.len() > LIM_I3_TOOL_PARAMETERS {
                return Err(format!("too_many_parameters:{name}"));
            }
            for (i, sub) in properties.keys().enumerate() {
                if !valid_parameter_name(sub) {
                    return Err(format!(
                        "definition_invalid:{at}, field {} has a name that is not allowed",
                        i + 1
                    ));
                }
            }
            let mut params = Vec::new();
            for (i, (sub, p)) in properties.iter().enumerate() {
                params.push(Param {
                    name: sub.clone(),
                    ty: convert_param(
                        p,
                        &format!("{name}.{sub}"),
                        &format!("{at}, field {}", i + 1),
                        depth + 1,
                    )?,
                    optional: !required.contains(sub),
                    description: p
                        .get("description")
                        .and_then(Value::as_str)
                        .map(|d| sanitize(d).0)
                        .filter(|d| !d.is_empty())
                        .unwrap_or_else(|| format!("The {sub} field."))
                        .chars()
                        .take(LIM_I5_CHARS)
                        .collect(),
                });
            }
            ParamType::Object {
                schema: ParamSchema { params },
            }
        }
        other => return Err(format!("schema_unsupported:{name}:type {other}")),
    })
}

#[cfg(test)]
mod tests;
