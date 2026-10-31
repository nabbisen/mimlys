//! Approval requests (external design §10; REQ-APR-002 … 009): rendered by
//! the runtime from the canonical action, never from model text; bound to
//! the action hash and the grant-set hash; one-shot; expiring into a halt.

use crate::clock::Instant;
use crate::event::Actor;
use crate::hash::Hash;
use crate::ids::{ActionId, ApprovalId, TaskId};
use crate::limits::{LIM_D2_REASON_CHARS, LIM_I6_APPROVAL_CHARS};
use crate::observation::sanitize;
use crate::tool::ToolDefinition;
use crate::validate::AuthorizedAction;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One labelled line of a rendered request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RenderedLine {
    /// Fixed label.
    pub label: String,
    /// Value; every external string inside is JSON-quoted.
    pub value: String,
}

/// The runtime's rendering of an action for the user (REQ-APR-002).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rendered {
    /// Lines in display order.
    pub lines: Vec<RenderedLine>,
}

impl Rendered {
    /// The text of the lines, one per line, labels padded.
    pub fn text(&self) -> String {
        let width = self
            .lines
            .iter()
            .map(|l| l.label.chars().count())
            .max()
            .unwrap_or(0);
        self.lines
            .iter()
            .map(|l| format!("{:<width$}  {}", l.label, l.value, width = width))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Renders an authorized action: tool, effect flags, what will happen,
/// arguments with resolved absolute paths, grants used, expiry. Every
/// external value is JSON-quoted, so a newline or a quote cannot start a
/// new line or a fake label (REQ-APR-003, THR-21). The whole rendering is
/// bounded by LIM-I6; only argument values are cut to fit.
pub fn render(
    action: &AuthorizedAction,
    tool: &ToolDefinition,
    workspace_root: &str,
    grants_used: &[String],
    expires_at: Option<Instant>,
    tier: Option<&str>,
) -> Rendered {
    let effect = action.effect();
    let mut lines = vec![
        RenderedLine {
            label: "Tool".into(),
            value: action.tool().to_owned(),
        },
        RenderedLine {
            label: "Effect".into(),
            value: format!(
                "{} · reversible: {} · idempotent: {}",
                serde_json::to_value(effect.kind)
                    .expect("serializes")
                    .as_str()
                    .unwrap_or("?"),
                yes_no(effect.reversible),
                yes_no(effect.idempotent)
            ),
        },
        RenderedLine {
            label: "Will".into(),
            value: will_sentence(tool, action),
        },
    ];
    let arguments_index = lines.len();
    lines.push(RenderedLine {
        label: "Arguments".into(),
        value: render_arguments(action, tool, workspace_root),
    });
    lines.push(RenderedLine {
        label: "Grants used".into(),
        value: if grants_used.is_empty() {
            "none (implicit workspace rights)".to_owned()
        } else {
            grants_used.join(", ")
        },
    });
    if let Some(tier) = tier {
        // A host tool runs under this machine's confinement; the user approving
        // it sees the tier, with any accepted best-effort clause (ED §13.4).
        lines.push(RenderedLine {
            label: "Tier".into(),
            value: tier.to_owned(),
        });
    }
    lines.push(RenderedLine {
        label: "Expires".into(),
        value: expires_at.map_or_else(|| "never".to_owned(), |t| t.to_rfc3339()),
    });
    let mut rendered = Rendered { lines };
    let total = rendered.text().chars().count();
    if total > LIM_I6_APPROVAL_CHARS {
        let over = total - LIM_I6_APPROVAL_CHARS;
        let value = &rendered.lines[arguments_index].value;
        let keep = value.chars().count().saturating_sub(over + 1);
        let cut: String = value.chars().take(keep).collect();
        rendered.lines[arguments_index].value = format!("{cut}…");
    }
    rendered
}

/// Characters of a path shown inside the "Will" sentence.
const WILL_PATH_CHARS: usize = 80;

fn shorten(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_owned()
    } else {
        format!("{}…", s.chars().take(max).collect::<String>())
    }
}

fn yes_no(b: bool) -> &'static str {
    if b { "yes" } else { "no" }
}

/// One fixed sentence per tool saying what will happen (external design §10.1).
fn will_sentence(tool: &ToolDefinition, action: &AuthorizedAction) -> String {
    // The sentence shows a shortened path; the full value is in the arguments line.
    let path = |name: &str| {
        action
            .arguments()
            .get(name)
            .and_then(Value::as_str)
            .map(|p| quote_str(&shorten(p, WILL_PATH_CHARS)))
            .unwrap_or_else(|| "\"?\"".to_owned())
    };
    match tool.name.as_str() {
        "read_file" => format!("read the file {}", path("path")),
        "write_file" => format!(
            "create or overwrite the file {} in the workspace",
            path("path")
        ),
        "replace_in_file" => format!(
            "replace text inside the file {} in the workspace",
            path("path")
        ),
        "list_dir" => format!("list the directory {} in the workspace", path("path")),
        "search_files" => "search workspace files".to_owned(),
        "delete_path" => format!("delete {} from the workspace", path("path")),
        "run_script" => "run a script inside the sandbox, with the workspace preopened".to_owned(),
        "run_wasm" => {
            "run a WASM module inside the sandbox, with the workspace preopened".to_owned()
        }
        "run_host_tool" => format!("run the host tool {} under confinement", path("tool")),
        name => match &tool.provider {
            crate::tool::Provider::Mcp { server } => format!(
                "call the external tool {} of server {}",
                quote_str(name),
                quote_str(server)
            ),
            _ => format!("run the tool {}", quote_str(name)),
        },
    }
}

fn render_arguments(
    action: &AuthorizedAction,
    tool: &ToolDefinition,
    workspace_root: &str,
) -> String {
    let path_params: Vec<&str> = tool.input.path_params().collect();
    let root = workspace_root.trim_end_matches(['/', '\\']);
    let mut parts = Vec::new();
    for (name, value) in action.arguments() {
        let shown = quote_value(value);
        if path_params.contains(&name.as_str()) {
            let relative = value.as_str().unwrap_or("");
            let resolved = if relative.is_empty() {
                root.to_owned()
            } else {
                format!("{root}/{relative}")
            };
            parts.push(format!(
                "{name} = {shown} (resolved: {})",
                quote_str(&resolved)
            ));
        } else {
            parts.push(format!("{name} = {shown}"));
        }
    }
    if parts.is_empty() {
        "none".to_owned()
    } else {
        parts.join(" · ")
    }
}

fn quote_str(s: &str) -> String {
    let (clean, _) = sanitize(s);
    serde_json::to_string(&clean).expect("a string serializes")
}

fn quote_value(v: &Value) -> String {
    match v {
        Value::String(s) => quote_str(s),
        other => {
            let (clean, _) = sanitize(&serde_json::to_string(other).expect("a value serializes"));
            clean
        }
    }
}

/// The status of an approval request (external design §10.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalStatus {
    /// Waiting for the user.
    Pending,
    /// Approved, not yet consumed.
    Approved,
    /// Consumed by the action's start.
    Consumed,
    /// Rejected by the user.
    Rejected,
    /// Expired.
    Expired,
    /// Invalidated: the action or the grants changed, or a used grant was revoked.
    Invalidated,
}

/// What the user decided (REQ-APR-007).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum UserDecision {
    /// Approve the action.
    Approve,
    /// Reject it, with an optional note for the model and a halt flag.
    Reject {
        /// Reaches the model as `user` text.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<String>,
        /// Halt the task instead of continuing.
        #[serde(default)]
        halt: bool,
    },
}

/// Why an approval was invalidated.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InvalidationReason {
    /// The action's content hash no longer matches.
    ActionChanged,
    /// The grant set's hash no longer matches.
    GrantsChanged,
    /// A grant the action used was revoked.
    GrantRevoked,
}

/// An approval operation was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ApprovalError {
    /// The request is not in the status the operation needs.
    #[error("the approval is not {needed}")]
    WrongStatus {
        /// The status the operation needs.
        needed: &'static str,
    },
    /// Consumption found a mismatch; the request is now invalidated.
    #[error("the approval was invalidated: {0:?}")]
    Invalidated(InvalidationReason),
    /// The note exceeds LIM-D2.
    #[error("the note exceeds LIM-D2")]
    NoteTooLong,
}

/// An approval request (external design §10.2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalRequest {
    /// Identifier.
    pub approval_id: ApprovalId,
    /// The task.
    pub task_id: TaskId,
    /// The action.
    pub action_id: ActionId,
    /// Hash of the action's content at request time (REQ-APR-004).
    pub action_hash: Hash,
    /// Hash of the grant set at request time (REQ-APR-004).
    pub grant_set_hash: Hash,
    /// The runtime's rendering.
    pub rendered: Rendered,
    /// The model's stated reason, sanitized and bounded; display-only.
    pub model_reason: Option<String>,
    /// When requested.
    pub requested_at: Instant,
    /// Expiry, when any (LIM-A1).
    pub expires_at: Option<Instant>,
    /// Status.
    pub status: ApprovalStatus,
    /// When decided.
    pub decided_at: Option<Instant>,
    /// Who decided.
    pub decided_by: Option<Actor>,
    /// The user's note on rejection.
    pub note: Option<String>,
    /// Whether the user asked to halt.
    pub halt: bool,
    /// Why it was invalidated, when it was.
    pub invalidation: Option<InvalidationReason>,
}

impl ApprovalRequest {
    /// Creates a pending request bound to the action and the grants in force.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        approval_id: ApprovalId,
        task_id: TaskId,
        action_id: ActionId,
        action_hash: Hash,
        grant_set_hash: Hash,
        rendered: Rendered,
        model_reason: Option<&str>,
        now: Instant,
        expires_at: Option<Instant>,
    ) -> Self {
        let model_reason = model_reason.map(|r| {
            let (clean, _) = sanitize(r);
            clean.chars().take(LIM_D2_REASON_CHARS).collect()
        });
        Self {
            approval_id,
            task_id,
            action_id,
            action_hash,
            grant_set_hash,
            rendered,
            model_reason,
            requested_at: now,
            expires_at,
            status: ApprovalStatus::Pending,
            decided_at: None,
            decided_by: None,
            note: None,
            halt: false,
            invalidation: None,
        }
    }

    /// The canonical text shown to the user: the rendering, then the model's
    /// reason labelled as untrusted (REQ-APR-002).
    pub fn display_text(&self) -> String {
        let mut text = self.rendered.text();
        if let Some(reason) = &self.model_reason {
            text.push_str("\nModel's stated reason (untrusted): ");
            text.push_str(&serde_json::to_string(reason).expect("a string serializes"));
        }
        text
    }

    /// Records the user's decision (REQ-APR-007). Only a pending request can be decided.
    pub fn decide(
        &mut self,
        decision: UserDecision,
        by: Actor,
        now: Instant,
    ) -> Result<(), ApprovalError> {
        if self.status != ApprovalStatus::Pending {
            return Err(ApprovalError::WrongStatus { needed: "pending" });
        }
        match decision {
            UserDecision::Approve => self.status = ApprovalStatus::Approved,
            UserDecision::Reject { note, halt } => {
                if note
                    .as_ref()
                    .is_some_and(|n| n.chars().count() > LIM_D2_REASON_CHARS)
                {
                    return Err(ApprovalError::NoteTooLong);
                }
                self.note = note.map(|n| sanitize(&n).0);
                self.halt = halt;
                self.status = ApprovalStatus::Rejected;
            }
        }
        self.decided_at = Some(now);
        self.decided_by = Some(by);
        Ok(())
    }

    /// Consumes the approval at the action's start (REQ-APR-004, REQ-APR-005):
    /// both hashes must still match, and an approval is consumed once.
    pub fn consume(
        &mut self,
        action_hash: &Hash,
        grant_set_hash: &Hash,
    ) -> Result<(), ApprovalError> {
        if self.status != ApprovalStatus::Approved {
            return Err(ApprovalError::WrongStatus { needed: "approved" });
        }
        if &self.action_hash != action_hash {
            self.invalidate(InvalidationReason::ActionChanged);
            return Err(ApprovalError::Invalidated(
                InvalidationReason::ActionChanged,
            ));
        }
        if &self.grant_set_hash != grant_set_hash {
            self.invalidate(InvalidationReason::GrantsChanged);
            return Err(ApprovalError::Invalidated(
                InvalidationReason::GrantsChanged,
            ));
        }
        self.status = ApprovalStatus::Consumed;
        Ok(())
    }

    /// Expires a pending request whose `expires_at` has passed (REQ-APR-006).
    pub fn expire(&mut self, now: Instant) -> bool {
        if self.status == ApprovalStatus::Pending && self.expires_at.is_some_and(|t| now >= t) {
            self.status = ApprovalStatus::Expired;
            self.decided_at = Some(now);
            true
        } else {
            false
        }
    }

    /// Invalidates a pending or approved request.
    pub fn invalidate(&mut self, reason: InvalidationReason) {
        if matches!(
            self.status,
            ApprovalStatus::Pending | ApprovalStatus::Approved
        ) {
            self.status = ApprovalStatus::Invalidated;
            self.invalidation = Some(reason);
        }
    }
}

#[cfg(test)]
mod tests;
