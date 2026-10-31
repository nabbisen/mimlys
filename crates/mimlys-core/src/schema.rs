//! A bounded JSON-Schema validator for the structured results of external
//! tools (RFC 009 §6; REQ-MCP-006, REQ-TOOL-005). It understands the
//! constructs that generated schemas use (`type` with one or several
//! names, `properties`, `required`, `additionalProperties`, `items`,
//! `enum`, `const`, `anyOf`, `oneOf`, `$ref` into `$defs`, numeric and
//! length bounds) and refuses everything else at parse time, so that a
//! tool whose results the runtime cannot validate is unavailable rather
//! than trusted (fail closed). `format`, `description`, `title`,
//! `default`, `examples` and `$schema` are documentation and ignored.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// The deepest nesting a schema may have.
pub const MAX_DEPTH: usize = 12;

/// Keywords that carry no validation meaning and are ignored.
const IGNORED: [&str; 10] = [
    "description",
    "title",
    "default",
    "examples",
    "$schema",
    "$id",
    "format",
    "deprecated",
    "readOnly",
    "writeOnly",
];

/// Why a schema could not be parsed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("unsupported schema at {path}: {construct}")]
pub struct Unsupported {
    /// JSON path inside the schema.
    pub path: String,
    /// The construct: a keyword, a type name or a reference.
    pub construct: String,
}

/// Why a value does not conform.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("output invalid at {path}: {reason}")]
pub struct Violation {
    /// JSON path inside the value.
    pub path: String,
    /// What was expected.
    pub reason: String,
}

/// A parsed output schema.
#[derive(Clone, Debug, PartialEq)]
pub struct OutputSchema {
    root: Node,
    defs: BTreeMap<String, Node>,
}

#[derive(Clone, Debug, PartialEq)]
enum Node {
    /// Any value.
    Any,
    /// A reference into `$defs`.
    Ref(String),
    /// One of several alternatives.
    AnyOf(Vec<Node>),
    /// Exactly one of several alternatives.
    OneOf(Vec<Node>),
    /// Every node must hold (`type` and bounds beside `anyOf`/`oneOf`).
    All(Vec<Node>),
    /// A typed value with its constraints.
    Typed(Box<Typed>),
}

#[derive(Clone, Debug, PartialEq)]
struct Typed {
    /// Accepted type names, empty for untyped constraints.
    types: Vec<String>,
    properties: BTreeMap<String, Node>,
    required: Vec<String>,
    /// `None` keeps unknown properties; `Some(node)` validates them; `Some(Any)` when `true`.
    additional: Option<Node>,
    /// Explicitly `false`.
    no_additional: bool,
    items: Option<Node>,
    enum_values: Option<Vec<Value>>,
    const_value: Option<Value>,
    minimum: Option<f64>,
    maximum: Option<f64>,
    min_length: Option<usize>,
    max_length: Option<usize>,
    min_items: Option<usize>,
    max_items: Option<usize>,
}

impl OutputSchema {
    /// Parses a schema; an unknown construct is refused with its path.
    pub fn parse(schema: &Value) -> Result<Self, Unsupported> {
        let mut defs = BTreeMap::new();
        if let Some(raw) = schema.get("$defs").or_else(|| schema.get("definitions")) {
            let map = raw.as_object().ok_or_else(|| Unsupported {
                path: "$defs".into(),
                construct: "non-object $defs".into(),
            })?;
            for (name, def) in map {
                defs.insert(name.clone(), parse_node(def, &format!("$defs/{name}"), 1)?);
            }
        }
        let root = parse_node(schema, "", 0)?;
        let parsed = Self { root, defs };
        parsed.check_refs(&parsed.root, "")?;
        for (name, node) in &parsed.defs {
            parsed.check_refs(node, &format!("$defs/{name}"))?;
        }
        Ok(parsed)
    }

    fn check_refs(&self, node: &Node, path: &str) -> Result<(), Unsupported> {
        match node {
            Node::Ref(name) if !self.defs.contains_key(name) => Err(Unsupported {
                path: path.to_owned(),
                construct: format!("$ref #/$defs/{name}"),
            }),
            Node::Ref(_) | Node::Any => Ok(()),
            Node::AnyOf(items) | Node::OneOf(items) | Node::All(items) => {
                for (i, item) in items.iter().enumerate() {
                    self.check_refs(item, &format!("{path}/{i}"))?;
                }
                Ok(())
            }
            Node::Typed(t) => {
                for (k, v) in &t.properties {
                    self.check_refs(v, &format!("{path}/properties/{k}"))?;
                }
                if let Some(a) = &t.additional {
                    self.check_refs(a, &format!("{path}/additionalProperties"))?;
                }
                if let Some(i) = &t.items {
                    self.check_refs(i, &format!("{path}/items"))?;
                }
                Ok(())
            }
        }
    }

    /// Validates a value; the first violation is returned with its path.
    pub fn validate(&self, value: &Value) -> Result<(), Violation> {
        self.check(&self.root, value, "$", 0)
    }

    fn check(&self, node: &Node, value: &Value, path: &str, depth: usize) -> Result<(), Violation> {
        if depth > MAX_DEPTH * 2 {
            return Err(Violation {
                path: path.to_owned(),
                reason: "value nests too deeply".into(),
            });
        }
        match node {
            Node::Any => Ok(()),
            Node::Ref(name) => {
                let target = self.defs.get(name).expect("checked at parse");
                self.check(target, value, path, depth + 1)
            }
            Node::AnyOf(items) => {
                if items
                    .iter()
                    .any(|n| self.check(n, value, path, depth + 1).is_ok())
                {
                    Ok(())
                } else {
                    Err(Violation {
                        path: path.to_owned(),
                        reason: "matches none of the alternatives".into(),
                    })
                }
            }
            Node::All(items) => {
                for n in items {
                    self.check(n, value, path, depth + 1)?;
                }
                Ok(())
            }
            Node::OneOf(items) => {
                let matches = items
                    .iter()
                    .filter(|n| self.check(n, value, path, depth + 1).is_ok())
                    .count();
                if matches == 1 {
                    Ok(())
                } else {
                    Err(Violation {
                        path: path.to_owned(),
                        reason: format!("matches {matches} alternatives, expected exactly one"),
                    })
                }
            }
            Node::Typed(t) => self.check_typed(t, value, path, depth),
        }
    }

    fn check_typed(
        &self,
        t: &Typed,
        value: &Value,
        path: &str,
        depth: usize,
    ) -> Result<(), Violation> {
        let violation = |reason: String| Violation {
            path: path.to_owned(),
            reason,
        };
        if !t.types.is_empty() && !t.types.iter().any(|ty| type_matches(ty, value)) {
            return Err(violation(format!(
                "expected {}, found {}",
                t.types.join(" or "),
                type_name(value)
            )));
        }
        if let Some(c) = &t.const_value
            && value != c
        {
            return Err(violation(format!("expected the constant {c}")));
        }
        if let Some(values) = &t.enum_values
            && !values.contains(value)
        {
            return Err(violation("not one of the enumerated values".into()));
        }
        if let Some(n) = value.as_f64() {
            if let Some(min) = t.minimum
                && n < min
            {
                return Err(violation(format!("below the minimum {min}")));
            }
            if let Some(max) = t.maximum
                && n > max
            {
                return Err(violation(format!("above the maximum {max}")));
            }
        }
        if let Some(s) = value.as_str() {
            let len = s.chars().count();
            if let Some(min) = t.min_length
                && len < min
            {
                return Err(violation(format!("shorter than {min} characters")));
            }
            if let Some(max) = t.max_length
                && len > max
            {
                return Err(violation(format!("longer than {max} characters")));
            }
        }
        if let Some(items) = value.as_array() {
            if let Some(min) = t.min_items
                && items.len() < min
            {
                return Err(violation(format!("fewer than {min} items")));
            }
            if let Some(max) = t.max_items
                && items.len() > max
            {
                return Err(violation(format!("more than {max} items")));
            }
            if let Some(schema) = &t.items {
                for (i, item) in items.iter().enumerate() {
                    self.check(schema, item, &format!("{path}[{i}]"), depth + 1)?;
                }
            }
        }
        if let Some(map) = value.as_object() {
            for name in &t.required {
                if !map.contains_key(name) {
                    return Err(violation(format!("missing the required property `{name}`")));
                }
            }
            for (k, v) in map {
                let child = format!("{path}.{k}");
                match t.properties.get(k) {
                    Some(schema) => self.check(schema, v, &child, depth + 1)?,
                    None if t.no_additional => {
                        return Err(Violation {
                            path: child,
                            reason: "unexpected property".into(),
                        });
                    }
                    None => {
                        if let Some(schema) = &t.additional {
                            self.check(schema, v, &child, depth + 1)?;
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

fn type_matches(ty: &str, value: &Value) -> bool {
    match ty {
        "null" => value.is_null(),
        "boolean" => value.is_boolean(),
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "number" => value.is_number(),
        "integer" => {
            value.is_i64() || value.is_u64() || value.as_f64().is_some_and(|f| f.fract() == 0.0)
        }
        _ => false,
    }
}

fn type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn parse_node(schema: &Value, path: &str, depth: usize) -> Result<Node, Unsupported> {
    let unsupported = |construct: String| Unsupported {
        path: path.to_owned(),
        construct,
    };
    if depth > MAX_DEPTH {
        return Err(unsupported("nesting deeper than the bound".into()));
    }
    match schema {
        Value::Bool(true) => return Ok(Node::Any),
        Value::Bool(false) => return Err(unsupported("`false` schema".into())),
        Value::Object(_) => {}
        _ => return Err(unsupported("schema is not an object".into())),
    }
    let map = schema.as_object().expect("object");
    if map.is_empty() {
        return Ok(Node::Any);
    }
    if let Some(reference) = map.get("$ref") {
        let text = reference
            .as_str()
            .ok_or_else(|| unsupported("non-string $ref".into()))?;
        let name = text
            .strip_prefix("#/$defs/")
            .or_else(|| text.strip_prefix("#/definitions/"))
            .ok_or_else(|| unsupported(format!("$ref {text}")))?;
        for key in map.keys() {
            if key != "$ref" && !IGNORED.contains(&key.as_str()) {
                return Err(unsupported(format!("`{key}` beside $ref")));
            }
        }
        return Ok(Node::Ref(name.to_owned()));
    }
    let alternatives = |key: &str| -> Result<Option<Vec<Node>>, Unsupported> {
        match map.get(key) {
            None => Ok(None),
            Some(Value::Array(items)) => items
                .iter()
                .enumerate()
                .map(|(i, item)| parse_node(item, &format!("{path}/{key}/{i}"), depth + 1))
                .collect::<Result<Vec<_>, _>>()
                .map(Some),
            Some(_) => Err(unsupported(format!("non-array {key}"))),
        }
    };
    let mut typed = Typed {
        types: vec![],
        properties: BTreeMap::new(),
        required: vec![],
        additional: None,
        no_additional: false,
        items: None,
        enum_values: None,
        const_value: None,
        minimum: None,
        maximum: None,
        min_length: None,
        max_length: None,
        min_items: None,
        max_items: None,
    };
    let mut any_of = None;
    let mut one_of = None;
    for (key, value) in map {
        match key.as_str() {
            k if IGNORED.contains(&k) => {}
            "$defs" | "definitions" if path.is_empty() => {}
            "type" => {
                let names: Vec<String> = match value {
                    Value::String(s) => vec![s.clone()],
                    Value::Array(items) => items
                        .iter()
                        .map(|v| {
                            v.as_str()
                                .map(str::to_owned)
                                .ok_or_else(|| unsupported("non-string type name".into()))
                        })
                        .collect::<Result<_, _>>()?,
                    _ => return Err(unsupported("type is neither a string nor an array".into())),
                };
                for n in &names {
                    if ![
                        "null", "boolean", "object", "array", "string", "number", "integer",
                    ]
                    .contains(&n.as_str())
                    {
                        return Err(unsupported(format!("type {n}")));
                    }
                }
                typed.types = names;
            }
            "properties" => {
                let props = value
                    .as_object()
                    .ok_or_else(|| unsupported("non-object properties".into()))?;
                for (name, sub) in props {
                    typed.properties.insert(
                        name.clone(),
                        parse_node(sub, &format!("{path}/properties/{name}"), depth + 1)?,
                    );
                }
            }
            "required" => {
                typed.required = value
                    .as_array()
                    .ok_or_else(|| unsupported("non-array required".into()))?
                    .iter()
                    .map(|v| {
                        v.as_str()
                            .map(str::to_owned)
                            .ok_or_else(|| unsupported("non-string required entry".into()))
                    })
                    .collect::<Result<_, _>>()?;
            }
            "additionalProperties" => match value {
                Value::Bool(false) => typed.no_additional = true,
                Value::Bool(true) => typed.additional = Some(Node::Any),
                other => {
                    typed.additional = Some(parse_node(
                        other,
                        &format!("{path}/additionalProperties"),
                        depth + 1,
                    )?)
                }
            },
            "items" => {
                typed.items = Some(parse_node(value, &format!("{path}/items"), depth + 1)?);
            }
            "enum" => {
                typed.enum_values = Some(
                    value
                        .as_array()
                        .ok_or_else(|| unsupported("non-array enum".into()))?
                        .clone(),
                );
            }
            "const" => typed.const_value = Some(value.clone()),
            "minimum" => typed.minimum = Some(number(value, key, path)?),
            "maximum" => typed.maximum = Some(number(value, key, path)?),
            "minLength" => typed.min_length = Some(number(value, key, path)? as usize),
            "maxLength" => typed.max_length = Some(number(value, key, path)? as usize),
            "minItems" => typed.min_items = Some(number(value, key, path)? as usize),
            "maxItems" => typed.max_items = Some(number(value, key, path)? as usize),
            "anyOf" => any_of = alternatives("anyOf")?,
            "oneOf" => one_of = alternatives("oneOf")?,
            other => return Err(unsupported(format!("keyword {other}"))),
        }
    }
    let has_constraints = !typed.types.is_empty()
        || !typed.properties.is_empty()
        || !typed.required.is_empty()
        || typed.additional.is_some()
        || typed.no_additional
        || typed.items.is_some()
        || typed.enum_values.is_some()
        || typed.const_value.is_some()
        || typed.minimum.is_some()
        || typed.maximum.is_some()
        || typed.min_length.is_some()
        || typed.max_length.is_some()
        || typed.min_items.is_some()
        || typed.max_items.is_some();
    match (any_of, one_of) {
        (Some(_), Some(_)) => Err(unsupported("anyOf and oneOf together".into())),
        (Some(items), None) if has_constraints => Ok(Node::All(vec![
            Node::Typed(Box::new(typed)),
            Node::AnyOf(items),
        ])),
        (None, Some(items)) if has_constraints => Ok(Node::All(vec![
            Node::Typed(Box::new(typed)),
            Node::OneOf(items),
        ])),
        (Some(items), None) => Ok(Node::AnyOf(items)),
        (None, Some(items)) => Ok(Node::OneOf(items)),
        (None, None) if has_constraints => Ok(Node::Typed(Box::new(typed))),
        (None, None) => Ok(Node::Any),
    }
}

fn number(value: &Value, key: &str, path: &str) -> Result<f64, Unsupported> {
    value.as_f64().ok_or_else(|| Unsupported {
        path: path.to_owned(),
        construct: format!("non-numeric {key}"),
    })
}

#[cfg(test)]
mod tests;
