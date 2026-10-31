//! Tool definitions and the flat input-schema model (external design §11;
//! REQ-TOOL-001, REQ-CTX-005). This is deliberately not JSON Schema: an
//! object of at most LIM-I3 named parameters of primitive, enumeration,
//! array or one-level object type, nested at most LIM-I4 deep.

use crate::action::EffectView;
use crate::limits::{
    LIM_I1_TOOL_DEFINITIONS_CHARS, LIM_I2_TOOL_DESCRIPTION_CHARS, LIM_I3_TOOL_PARAMETERS,
    LIM_I4_SCHEMA_DEPTH,
};
use crate::path::{PathError, PlatformRules, WorkspacePath};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Where a tool runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceProfile {
    /// In the runtime process over capability handles.
    Builtin,
    /// In the worker's WASM sandbox.
    Sandbox,
    /// In the confined worker as a host process (M6).
    Host,
    /// In an external MCP server process (M5).
    External,
}

/// What the import of an external tool recorded (RFC 009 §2): the server,
/// the pinned hash, what the server asserted and what the import changed
/// or refused. Annotations are kept for the record and used for nothing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportRecord {
    /// The configured server name.
    pub server: String,
    /// Hash of the imported definition (LIM-U6).
    pub hash: crate::hash::Hash,
    /// The server's annotations, verbatim.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<Value>,
    /// The description was cut to LIM-I2.
    #[serde(default)]
    pub description_truncated: bool,
    /// Whether the administrator declared the tool (else the restrictive defaults apply).
    #[serde(default)]
    pub declared: bool,
    /// Why the tool is unavailable, when it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unavailable: Option<String>,
}

/// Who provides a tool.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "provider", rename_all = "snake_case")]
pub enum Provider {
    /// Built into the runtime.
    Builtin,
    /// Served by an MCP server.
    Mcp {
        /// The server's configured name.
        server: String,
    },
    /// A declared host tool.
    Host,
}

/// The type of one parameter.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ParamType {
    /// A string.
    String {
        /// Minimum length in characters.
        #[serde(default)]
        min_chars: usize,
        /// Maximum length in characters.
        max_chars: usize,
        /// Maximum size in bytes, when bounded by bytes (LIM-W3).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_bytes: Option<usize>,
    },
    /// A path inside the workspace; canonicalized at stage 6.
    WorkspacePath,
    /// An integer.
    Integer {
        /// Minimum.
        min: i64,
        /// Maximum.
        max: i64,
    },
    /// A boolean.
    Boolean,
    /// One of fixed values.
    Enum {
        /// The values.
        values: Vec<String>,
    },
    /// An array of strings.
    StringArray {
        /// Maximum items.
        max_items: usize,
        /// Maximum characters per item.
        max_chars: usize,
    },
    /// A nested object (counts one level of depth).
    Object {
        /// Its parameters.
        schema: ParamSchema,
    },
}

/// One named parameter.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Param {
    /// Name.
    pub name: String,
    /// Type.
    #[serde(flatten)]
    pub ty: ParamType,
    /// Whether it may be omitted.
    #[serde(default)]
    pub optional: bool,
    /// One sentence for the model.
    pub description: String,
}

/// A flat object schema.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ParamSchema {
    /// Parameters, in declaration order.
    pub params: Vec<Param>,
}

/// A violation of the schema's shape (stage 5) or bounds (stage 8).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SchemaViolation {
    /// JSON path of the offending value.
    pub path: String,
    /// What was expected.
    pub expected: String,
    /// Lower bound, when a bound was violated.
    pub min: Option<i64>,
    /// Upper bound, when a bound was violated.
    pub max: Option<i64>,
    /// Allowed values, when an enumeration was violated.
    pub allowed: Option<Vec<String>>,
}

impl SchemaViolation {
    fn shape(path: String, expected: impl Into<String>) -> Self {
        Self {
            path,
            expected: expected.into(),
            min: None,
            max: None,
            allowed: None,
        }
    }
}

/// A definition was itself invalid.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum DefinitionError {
    /// The name is not lowercase snake_case (optionally `server.tool`) of at most 64 characters.
    #[error("invalid tool name `{0}`")]
    Name(String),
    /// The description exceeds LIM-I2.
    #[error("description of `{0}` exceeds LIM-I2")]
    Description(String),
    /// More than LIM-I3 parameters.
    #[error("`{0}` has more than LIM-I3 parameters")]
    TooManyParameters(String),
    /// Nested deeper than LIM-I4.
    #[error("`{0}` nests deeper than LIM-I4")]
    TooDeep(String),
    /// Two parameters share a name.
    #[error("`{0}` declares parameter `{1}` twice")]
    DuplicateParameter(String, String),
    /// A destination argument is not a parameter.
    #[error("`{0}` names destination argument `{1}` that is not a parameter")]
    UnknownDestination(String, String),
}

impl ParamSchema {
    fn depth(&self) -> usize {
        1 + self
            .params
            .iter()
            .map(|p| match &p.ty {
                ParamType::Object { schema } => schema.depth(),
                _ => 0,
            })
            .max()
            .unwrap_or(0)
    }

    fn check(&self, tool: &str) -> Result<(), DefinitionError> {
        if self.params.len() > LIM_I3_TOOL_PARAMETERS {
            return Err(DefinitionError::TooManyParameters(tool.to_owned()));
        }
        let mut seen = std::collections::HashSet::new();
        for p in &self.params {
            if !seen.insert(p.name.as_str()) {
                return Err(DefinitionError::DuplicateParameter(
                    tool.to_owned(),
                    p.name.clone(),
                ));
            }
            if let ParamType::Object { schema } = &p.ty {
                schema.check(tool)?;
            }
        }
        Ok(())
    }

    /// Stage 5: shape — required parameters present, no unknown properties, right JSON types.
    pub fn validate_shape(&self, args: &Map<String, Value>) -> Result<(), SchemaViolation> {
        self.validate_shape_at("$", args)
    }

    fn validate_shape_at(
        &self,
        prefix: &str,
        args: &Map<String, Value>,
    ) -> Result<(), SchemaViolation> {
        for key in args.keys() {
            if !self.params.iter().any(|p| &p.name == key) {
                return Err(SchemaViolation::shape(
                    format!("{prefix}.{key}"),
                    "no such parameter",
                ));
            }
        }
        for p in &self.params {
            let path = format!("{prefix}.{}", p.name);
            let Some(value) = args.get(&p.name) else {
                if p.optional {
                    continue;
                }
                return Err(SchemaViolation::shape(path, "a required parameter"));
            };
            match (&p.ty, value) {
                (ParamType::String { .. } | ParamType::WorkspacePath, Value::String(_)) => {}
                (ParamType::Integer { .. }, Value::Number(n)) if n.is_i64() => {}
                (ParamType::Boolean, Value::Bool(_)) => {}
                (ParamType::Enum { .. }, Value::String(_)) => {}
                (ParamType::StringArray { .. }, Value::Array(items)) => {
                    if let Some(i) = items.iter().position(|v| !v.is_string()) {
                        return Err(SchemaViolation::shape(format!("{path}[{i}]"), "string"));
                    }
                }
                (ParamType::Object { schema }, Value::Object(map)) => {
                    schema.validate_shape_at(&path, map)?
                }
                (ty, _) => return Err(SchemaViolation::shape(path, type_name(ty))),
            }
        }
        Ok(())
    }

    /// Stage 8: bounds — lengths, ranges, enumeration membership.
    pub fn validate_bounds(&self, args: &Map<String, Value>) -> Result<(), SchemaViolation> {
        self.validate_bounds_at("$", args)
    }

    fn validate_bounds_at(
        &self,
        prefix: &str,
        args: &Map<String, Value>,
    ) -> Result<(), SchemaViolation> {
        for p in &self.params {
            let path = format!("{prefix}.{}", p.name);
            let Some(value) = args.get(&p.name) else {
                continue;
            };
            match (&p.ty, value) {
                (
                    ParamType::String {
                        min_chars,
                        max_chars,
                        max_bytes,
                    },
                    Value::String(s),
                ) => {
                    let n = s.chars().count();
                    if n < *min_chars || n > *max_chars {
                        return Err(SchemaViolation {
                            path,
                            expected: "string length within bounds".into(),
                            min: Some(*min_chars as i64),
                            max: Some(*max_chars as i64),
                            allowed: None,
                        });
                    }
                    if let Some(limit) = max_bytes
                        && s.len() > *limit
                    {
                        return Err(SchemaViolation {
                            path,
                            expected: "string size in bytes within bounds".into(),
                            min: None,
                            max: Some(*limit as i64),
                            allowed: None,
                        });
                    }
                }
                (ParamType::Integer { min, max }, Value::Number(n)) => {
                    let v = n.as_i64().unwrap_or(i64::MAX);
                    if v < *min || v > *max {
                        return Err(SchemaViolation {
                            path,
                            expected: "integer within bounds".into(),
                            min: Some(*min),
                            max: Some(*max),
                            allowed: None,
                        });
                    }
                }
                (ParamType::Enum { values }, Value::String(s)) => {
                    if !values.contains(s) {
                        return Err(SchemaViolation {
                            path,
                            expected: "one of the allowed values".into(),
                            min: None,
                            max: None,
                            allowed: Some(values.clone()),
                        });
                    }
                }
                (
                    ParamType::StringArray {
                        max_items,
                        max_chars,
                    },
                    Value::Array(items),
                ) => {
                    if items.len() > *max_items {
                        return Err(SchemaViolation {
                            path,
                            expected: "array length within bounds".into(),
                            min: Some(0),
                            max: Some(*max_items as i64),
                            allowed: None,
                        });
                    }
                    for (i, item) in items.iter().enumerate() {
                        if item
                            .as_str()
                            .is_some_and(|s| s.chars().count() > *max_chars)
                        {
                            return Err(SchemaViolation {
                                path: format!("{path}[{i}]"),
                                expected: "string length within bounds".into(),
                                min: Some(0),
                                max: Some(*max_chars as i64),
                                allowed: None,
                            });
                        }
                    }
                }
                (ParamType::Object { schema }, Value::Object(map)) => {
                    schema.validate_bounds_at(&path, map)?
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Stage 6: canonicalizes every `WorkspacePath` parameter in place.
    /// Returns the path and the error for the first failure.
    pub fn canonicalize_paths(
        &self,
        args: &mut Map<String, Value>,
        rules: PlatformRules,
    ) -> Result<(), (String, PathError)> {
        self.canonicalize_paths_at("$", args, rules)
    }

    fn canonicalize_paths_at(
        &self,
        prefix: &str,
        args: &mut Map<String, Value>,
        rules: PlatformRules,
    ) -> Result<(), (String, PathError)> {
        for p in &self.params {
            let path = format!("{prefix}.{}", p.name);
            match (&p.ty, args.get_mut(&p.name)) {
                (ParamType::WorkspacePath, Some(Value::String(s))) => {
                    let canonical =
                        WorkspacePath::canonicalize_with(s, rules).map_err(|e| (path, e))?;
                    *s = canonical.as_str().to_owned();
                }
                (ParamType::Object { schema }, Some(Value::Object(map))) => {
                    schema.canonicalize_paths_at(&path, map, rules)?;
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// The names of the `WorkspacePath` parameters at the top level.
    pub fn path_params(&self) -> impl Iterator<Item = &str> {
        self.params
            .iter()
            .filter(|p| matches!(p.ty, ParamType::WorkspacePath))
            .map(|p| p.name.as_str())
    }
}

fn type_name(ty: &ParamType) -> &'static str {
    match ty {
        ParamType::String { .. } => "string",
        ParamType::WorkspacePath => "workspace path (string)",
        ParamType::Integer { .. } => "integer",
        ParamType::Boolean => "boolean",
        ParamType::Enum { .. } => "string from the allowed values",
        ParamType::StringArray { .. } => "array of strings",
        ParamType::Object { .. } => "object",
    }
}

/// A tool as the runtime knows it (REQ-TOOL-001).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolDefinition {
    /// Lowercase snake_case name; `server.tool` for external tools.
    pub name: String,
    /// At most LIM-I2 characters.
    pub description: String,
    /// Input schema.
    pub input: ParamSchema,
    /// Output schema, when declared.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<ParamSchema>,
    /// Effect with its flags.
    pub effect: EffectView,
    /// Default deadline in milliseconds.
    pub deadline_ms: i64,
    /// Where it runs.
    pub profile: ResourceProfile,
    /// Parameters that denote network destinations.
    #[serde(default)]
    pub destination_arguments: Vec<String>,
    /// Who provides it.
    #[serde(flatten)]
    pub provider: Provider,
}

impl ToolDefinition {
    /// Checks the definition against REQ-TOOL-001's bounds.
    pub fn check(&self) -> Result<(), DefinitionError> {
        if !valid_name(&self.name) {
            return Err(DefinitionError::Name(self.name.clone()));
        }
        if self.description.chars().count() > LIM_I2_TOOL_DESCRIPTION_CHARS {
            return Err(DefinitionError::Description(self.name.clone()));
        }
        self.input.check(&self.name)?;
        if self.input.depth() > LIM_I4_SCHEMA_DEPTH {
            return Err(DefinitionError::TooDeep(self.name.clone()));
        }
        if let Some(output) = &self.output {
            output.check(&self.name)?;
        }
        for dest in &self.destination_arguments {
            if !self.input.params.iter().any(|p| &p.name == dest) {
                return Err(DefinitionError::UnknownDestination(
                    self.name.clone(),
                    dest.clone(),
                ));
            }
        }
        Ok(())
    }
}

fn valid_name(name: &str) -> bool {
    if name.is_empty() || name.len() > 64 {
        return false;
    }
    name.split('.').count() <= 2
        && name.split('.').all(|part| {
            part.chars().next().is_some_and(|c| c.is_ascii_lowercase())
                && part
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        })
}

/// The definitions exceed LIM-I1 when rendered.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("tool definitions render to {chars} characters, above LIM-I1 ({limit})")]
pub struct DefinitionsTooLarge {
    /// Rendered size.
    pub chars: usize,
    /// The limit.
    pub limit: usize,
}

/// Renders the model-facing definitions as compact JSON within LIM-I1
/// (REQ-CTX-005). Names, descriptions and parameters only; effects and
/// deadlines are the runtime's business.
pub fn render_definitions(tools: &[ToolDefinition]) -> Result<String, DefinitionsTooLarge> {
    let rendered: Vec<Value> = tools
        .iter()
        .map(|t| {
            serde_json::json!({
                "name": t.name,
                "description": t.description,
                "parameters": render_params(&t.input),
            })
        })
        .collect();
    let text = serde_json::to_string(&rendered).expect("definitions always serialize");
    let chars = text.chars().count();
    if chars > LIM_I1_TOOL_DEFINITIONS_CHARS {
        return Err(DefinitionsTooLarge {
            chars,
            limit: LIM_I1_TOOL_DEFINITIONS_CHARS,
        });
    }
    Ok(text)
}

fn render_params(schema: &ParamSchema) -> Value {
    Value::Array(
        schema
            .params
            .iter()
            .map(|p| {
                let mut v = serde_json::json!({
                    "name": p.name,
                    "type": type_name(&p.ty),
                    "description": p.description,
                });
                if p.optional {
                    v["optional"] = Value::Bool(true);
                }
                match &p.ty {
                    ParamType::Enum { values } => v["values"] = serde_json::json!(values),
                    ParamType::Integer { min, max } => {
                        v["min"] = (*min).into();
                        v["max"] = (*max).into();
                    }
                    ParamType::Object { schema } => v["parameters"] = render_params(schema),
                    _ => {}
                }
                v
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests;
