//! Context segments (external design §15.2; REQ-CTX-002): every labelled
//! segment is one JSON object between marker lines. Because JSON string
//! encoding escapes line breaks, no external content can produce a line
//! equal to a marker. This module holds the segment writer, the structural
//! compression rules of §15.3, the prefix measure and the redactor; the
//! assembler itself is `runtime::context`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The provenance label of a segment.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SegmentLabel {
    /// A tool's result.
    Observation,
    /// The runtime's feedback (a rejection, a notice).
    Feedback,
    /// The user's text (goal, answer, note).
    User,
    /// The generated tool definitions.
    ToolDefinitions,
    /// The current plan.
    Plan,
    /// A forked task's history: the source's goal, plan and last steps (REQ-TASK-004).
    History,
}

impl SegmentLabel {
    /// The label's name on the marker line.
    pub fn as_str(self) -> &'static str {
        match self {
            SegmentLabel::Observation => "observation",
            SegmentLabel::Feedback => "feedback",
            SegmentLabel::User => "user",
            SegmentLabel::ToolDefinitions => "tool_definitions",
            SegmentLabel::Plan => "plan",
            SegmentLabel::History => "history",
        }
    }
}

/// Writes one delimited segment:
///
/// ```text
/// <<observation tool="run_script" action="…">>
/// { … }
/// <</observation>>
/// ```
///
/// Attribute values are JSON-quoted; the body is compact JSON on one line.
pub fn write_segment(label: SegmentLabel, attrs: &[(&str, &str)], body: &Value) -> String {
    let mut out = String::new();
    out.push_str("<<");
    out.push_str(label.as_str());
    for (key, value) in attrs {
        out.push(' ');
        out.push_str(key);
        out.push('=');
        out.push_str(&serde_json::to_string(value).expect("a string always serializes"));
    }
    out.push_str(">>\n");
    out.push_str(&serde_json::to_string(body).expect("a value always serializes"));
    out.push_str("\n<</");
    out.push_str(label.as_str());
    out.push_str(">>\n");
    out
}

/// The closing marker of a label.
pub fn closing_marker(label: SegmentLabel) -> String {
    format!("<</{}>>", label.as_str())
}

#[cfg(test)]
mod tests;

// ---------------------------------------------------------------------------
// Compression rules (external design §15.3; REQ-CTX-003, REQ-CTX-004) and
// redaction (REQ-CTX-007). Every rule is structural: no model is asked to
// summarize (D-12).
// ---------------------------------------------------------------------------

/// Collapses runs of identical consecutive lines to one line followed by a
/// marker line `[… repeated N times …]`. Runs of two keep both lines (a
/// marker would be longer than the saving).
pub fn collapse_repeated_lines(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut lines = text.split('\n').peekable();
    let mut first = true;
    while let Some(line) = lines.next() {
        let mut repeats = 1usize;
        while lines.peek() == Some(&line) {
            lines.next();
            repeats += 1;
        }
        let shown = if repeats > 2 { 1 } else { repeats };
        for _ in 0..shown {
            if !first {
                out.push('\n');
            }
            first = false;
            out.push_str(line);
        }
        if repeats > 2 {
            out.push('\n');
            out.push_str(&format!("[… repeated {repeats} times …]"));
        }
    }
    out
}

/// The fixed summary of a compressed step: built from structured fields
/// only and rendered within LIM-C4 characters.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepSummary {
    /// The step's index.
    pub step: u32,
    /// The decision kind (`act`, `plan`, `ask_user`, `complete`, `give_up`) or `rejected`.
    pub decision: String,
    /// The tool, for an action.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    /// The `path` argument, when the action had one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// The action's final status.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// The failure cause's name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cause: Option<String>,
    /// The rejection code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rejection: Option<String>,
    /// The step's outcome when it was not a plain continuation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
    /// Notice codes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notices: Vec<String>,
    /// The first non-empty line of the observation's output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_line: Option<String>,
}

impl StepSummary {
    /// The summary as compact JSON within `limit` characters; a longer
    /// rendering drops `first_line`, then is cut at a character boundary.
    pub fn render(&self, limit: usize) -> String {
        let full = serde_json::to_string(self).expect("serializes");
        if full.chars().count() <= limit {
            return full;
        }
        let shorter = StepSummary {
            first_line: None,
            ..self.clone()
        };
        let text = serde_json::to_string(&shorter).expect("serializes");
        if text.chars().count() <= limit {
            return text;
        }
        text.chars().take(limit).collect()
    }
}

/// The characters of every message, summed (the budget's unit): its content
/// and, in the `tool_calls` transport, the name and arguments of the call it
/// carries, which are sent as well.
pub fn content_chars(messages: &[Message]) -> usize {
    messages
        .iter()
        .map(|m| {
            m.content.chars().count()
                + m.call
                    .as_ref()
                    .map_or(0, |c| c.name.chars().count() + c.arguments.chars().count())
        })
        .sum()
}

/// The request as the prefix measure sees it: the messages, with the
/// `tools` array placed after the system message, where the engines' chat
/// templates put it. Without functions, the messages as they are.
pub fn prefix_view(messages: &[Message], functions: &[crate::native::Function]) -> Vec<Message> {
    let mut view = messages.to_vec();
    if !functions.is_empty() {
        let tools = Message::text(
            Role::System,
            crate::native::tools_json(functions).to_string(),
        );
        view.insert(usize::from(!view.is_empty()), tools);
    }
    view
}

/// Serializes messages identically for prefix comparison: `role\n content\n` each.
fn serialize_for_prefix(messages: &[Message]) -> String {
    let mut out = String::new();
    for m in messages {
        out.push_str(match m.role {
            Role::System => "system",
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::Tool => "tool",
        });
        out.push('\n');
        out.push_str(&m.content);
        out.push('\n');
        if let Some(call) = &m.call {
            out.push_str(&format!("{}\n{}\n{}\n", call.id, call.name, call.arguments));
        }
        if let Some(id) = &m.call_id {
            out.push_str(id);
            out.push('\n');
        }
    }
    out
}

/// The length in characters of the longest common prefix of two requests,
/// serialized identically (REQ-CTX-010): what a prefix cache may reuse.
pub fn common_prefix_chars(previous: &[Message], current: &[Message]) -> usize {
    let a = serialize_for_prefix(previous);
    let b = serialize_for_prefix(current);
    a.chars().zip(b.chars()).take_while(|(x, y)| x == y).count()
}

/// Replaces every configured secret value by `[redacted:<name>]`
/// (REQ-CTX-007, REQ-PRIV-002). Values shorter than `MIN_SECRET_CHARS` are
/// not scanned; the configuration refuses them.
#[derive(Clone, Debug, Default)]
pub struct Redactor {
    /// (name, value, JSON-escaped value) longest value first.
    entries: Vec<(String, String, String)>,
}

impl Redactor {
    /// Builds a redactor from `(name, value)` pairs.
    pub fn new(secrets: &[(String, String)]) -> Self {
        let mut entries: Vec<(String, String, String)> = secrets
            .iter()
            .filter(|(_, value)| value.chars().count() >= crate::limits::MIN_SECRET_CHARS)
            .map(|(name, value)| {
                let escaped = serde_json::to_string(value).expect("a string serializes");
                let escaped = escaped[1..escaped.len() - 1].to_owned();
                (name.clone(), value.clone(), escaped)
            })
            .collect();
        entries.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then(a.0.cmp(&b.0)));
        Self { entries }
    }

    /// Whether there is anything to scan for.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Redacts `text`; returns the names of the secrets found, each once.
    pub fn redact(&self, text: &str) -> (String, Vec<String>) {
        let mut out = text.to_owned();
        let mut names = Vec::new();
        for (name, value, escaped) in &self.entries {
            let marker = format!("[redacted:{name}]");
            let mut hit = false;
            if out.contains(value.as_str()) {
                out = out.replace(value.as_str(), &marker);
                hit = true;
            }
            if escaped != value && out.contains(escaped.as_str()) {
                out = out.replace(escaped.as_str(), &marker);
                hit = true;
            }
            if hit {
                names.push(name.clone());
            }
        }
        (out, names)
    }

    /// Redacts every string inside a JSON value, in place; returns the names found.
    pub fn redact_value(&self, value: &mut Value) -> Vec<String> {
        let mut names = Vec::new();
        self.walk(value, &mut names);
        names.sort();
        names.dedup();
        names
    }

    fn walk(&self, value: &mut Value, names: &mut Vec<String>) {
        match value {
            Value::String(s) => {
                let (clean, found) = self.redact(s);
                if !found.is_empty() {
                    *s = clean;
                    names.extend(found);
                }
            }
            Value::Array(items) => {
                for item in items {
                    self.walk(item, names);
                }
            }
            Value::Object(map) => {
                for item in map.values_mut() {
                    self.walk(item, names);
                }
            }
            _ => {}
        }
    }
}

use crate::spi::{Message, Role};
