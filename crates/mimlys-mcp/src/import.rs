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
};
use mimlys_core::observation::sanitize;
use mimlys_core::schema::OutputSchema;
use mimlys_core::tool::{
    ImportRecord, Param, ParamSchema, ParamType, Provider, ResourceProfile, ToolDefinition,
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
pub fn import(server: &str, listed: &Listed, declaration: Option<&Declaration>) -> Imported {
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
        record.unavailable = Some(reason);
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
        format!("The external tool {name}.")
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
        if !input.params.iter().any(|p| &p.name == arg) {
            unavailable(&mut record, format!("destination_unknown:{arg}"));
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
        deadline_ms: declaration.timeout_ms,
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
    let mut params = Vec::new();
    for (name, prop) in &properties {
        let ty = convert_param(prop, name, 0)?;
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

fn convert_param(prop: &Value, name: &str, depth: usize) -> Result<ParamType, String> {
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
                let values: Vec<String> = values
                    .as_array()
                    .ok_or_else(|| format!("schema_unsupported:{name}:non-array enum"))?
                    .iter()
                    .map(|v| {
                        v.as_str().map(str::to_owned).ok_or_else(|| {
                            format!("schema_unsupported:{name}:non-string enum value")
                        })
                    })
                    .collect::<Result<_, _>>()?;
                ParamType::Enum { values }
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
            let mut params = Vec::new();
            for (sub, p) in &properties {
                params.push(Param {
                    name: sub.clone(),
                    ty: convert_param(p, &format!("{name}.{sub}"), depth + 1)?,
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
