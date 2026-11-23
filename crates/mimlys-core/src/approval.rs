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

mod arguments;

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

/// The label before the model's reason on the approval screen.
const REASON_LABEL: &str = "Model's stated reason (untrusted): ";

/// The model's reason as the request keeps it: sanitized and bounded by LIM-D2.
fn bounded_reason(reason: &str) -> String {
    let (clean, _) = sanitize(reason);
    clean.chars().take(LIM_D2_REASON_CHARS).collect()
}

/// The arguments of an action cannot all be named within LIM-I6 together with
/// the fixed lines and the model's reason: the request is not put to the user
/// (REQ-APR-002).
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error(
    "the arguments cannot all be named within LIM-I6: {needed} characters are needed, {available} are left"
)]
pub struct ArgumentsDoNotFit {
    /// Characters the argument names and the markers of the cut values need.
    pub needed: usize,
    /// Characters LIM-I6 leaves for the arguments line.
    pub available: usize,
}

/// Renders an authorized action: tool, effect flags, what will happen,
/// arguments with resolved absolute paths, grants used, expiry. Every
/// external value is JSON-quoted, so a newline or a quote cannot start a
/// new line or a fake label (REQ-APR-003, THR-21).
///
/// The request, with the model's `reason` that follows it on the screen
/// ([`ApprovalRequest::display_text`]), is at most LIM-I6 characters, and no
/// argument is left out by that bound (REQ-APR-002): every argument is named, in
/// an order that puts destinations and paths first; each value has its own
/// share of what the fixed lines and the reason leave, and a value that does
/// not fit says that it is cut and how many characters are left out. The whole
/// arguments are in [`argument_views`]. When even the names and those markers
/// do not fit, the request cannot be made and the caller refuses the action.
pub fn render(
    action: &AuthorizedAction,
    tool: &ToolDefinition,
    workspace_root: &str,
    grants_used: &[String],
    expires_at: Option<Instant>,
    tier: Option<&str>,
    reason: Option<&str>,
) -> Result<Rendered, ArgumentsDoNotFit> {
    let effect = action.effect();
    let mut grants: Vec<&str> = Vec::new();
    for g in grants_used {
        if !grants.contains(&g.as_str()) {
            grants.push(g);
        }
    }
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
        value: String::new(),
    });
    lines.push(RenderedLine {
        label: "Grants used".into(),
        value: if grants.is_empty() {
            "none (implicit workspace rights)".to_owned()
        } else {
            grants.join(", ")
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
    // What is left for the arguments line: LIM-I6 less the other lines, as the
    // text lays them out, and less the reason that follows them.
    let reason_chars = reason.map_or(0, |r| {
        let quoted = serde_json::to_string(&bounded_reason(r)).expect("a string serializes");
        1 + REASON_LABEL.chars().count() + quoted.chars().count()
    });
    let fixed = rendered.text().chars().count() + reason_chars;
    let available = LIM_I6_APPROVAL_CHARS.saturating_sub(fixed);
    let views = argument_views(
        action.arguments(),
        &tool.destination_arguments,
        &tool.input.path_params().collect::<Vec<_>>(),
        Some(workspace_root),
    );
    rendered.lines[arguments_index].value = arguments::fit(&views, available)?;
    Ok(rendered)
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

/// One argument of an action, in full and sanitized (REQ-APR-002): what the
/// user can read before deciding, whatever the approval screen had room for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArgumentView {
    /// The argument's name, sanitized.
    pub name: String,
    /// The value as the screen shows it: a string JSON-quoted, any other
    /// value as its JSON text; sanitized.
    pub value: String,
    /// For a path parameter, the resolved absolute path, JSON-quoted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved: Option<String>,
}

impl ArgumentView {
    /// The argument on one line: `name = value`, with the resolved path of a
    /// path parameter after it.
    pub fn text(&self) -> String {
        let mut text = format!("{} = {}", arguments::shown_name(&self.name), self.value);
        if let Some(resolved) = &self.resolved {
            text.push_str(&format!(" (resolved: {resolved})"));
        }
        text
    }
}

/// Every argument of an action, in full and sanitized, in the order the
/// approval screen names them: the destination arguments, then the path
/// parameters (each group by name), then the others, the shortest value first
/// (by name among equals), so that what identifies the action comes before
/// what is long. `root` resolves the path parameters, when it is known.
pub fn argument_views(
    arguments: &serde_json::Map<String, Value>,
    destinations: &[String],
    path_params: &[&str],
    root: Option<&str>,
) -> Vec<ArgumentView> {
    let root = root.map(|r| r.trim_end_matches(['/', '\\']));
    let mut views: Vec<(u8, usize, ArgumentView)> = arguments
        .iter()
        .map(|(name, value)| {
            let shown = quote_value(value);
            let is_path = path_params.contains(&name.as_str());
            let resolved = root.filter(|_| is_path).map(|root| {
                let relative = value.as_str().unwrap_or("");
                quote_str(&if relative.is_empty() {
                    root.to_owned()
                } else {
                    format!("{root}/{relative}")
                })
            });
            let group = if destinations.contains(name) {
                0
            } else if is_path {
                1
            } else {
                2
            };
            let length = if group == 2 { shown.chars().count() } else { 0 };
            let view = ArgumentView {
                name: sanitize(name).0,
                value: shown,
                resolved,
            };
            (group, length, view)
        })
        .collect();
    views.sort_by(|a, b| (a.0, a.1, &a.2.name).cmp(&(b.0, b.1, &b.2.name)));
    views.into_iter().map(|(_, _, view)| view).collect()
}

/// The arguments as `[v]iew` and `task show --arguments` print them: one
/// line per argument, none cut.
pub fn arguments_text(arguments: &[ArgumentView]) -> String {
    if arguments.is_empty() {
        return "none".to_owned();
    }
    arguments
        .iter()
        .map(ArgumentView::text)
        .collect::<Vec<_>>()
        .join("\n")
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
    /// Approve the action, with an optional note for the model (from 0.13): it reaches the model
    /// as `<<user kind="approval_note">>` with the action's observation.
    Approve {
        /// Reaches the model as `user` text.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
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
    /// Every argument of the action in full and sanitized, in the order of the
    /// rendering (REQ-APR-002). A request made before this was recorded has none
    /// here; the control API fills it in from the action when it shows the request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arguments: Option<Vec<ArgumentView>>,
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
    /// The user's note on the decision: on a rejection it is the model's `rejection_note`, on an
    /// approval (from 0.13) its `approval_note`.
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
        let model_reason = model_reason.map(bounded_reason);
        Self {
            approval_id,
            task_id,
            action_id,
            action_hash,
            grant_set_hash,
            rendered,
            model_reason,
            arguments: None,
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

    /// Records every argument of the action in full (REQ-APR-002).
    #[must_use]
    pub fn with_arguments(mut self, arguments: Vec<ArgumentView>) -> Self {
        self.arguments = Some(arguments);
        self
    }

    /// The canonical text shown to the user: the rendering, then the model's
    /// reason labelled as untrusted (REQ-APR-002). At most LIM-I6 characters
    /// when the rendering was made with this request's reason ([`render`]).
    pub fn display_text(&self) -> String {
        let mut text = self.rendered.text();
        if let Some(reason) = &self.model_reason {
            text.push('\n');
            text.push_str(REASON_LABEL);
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
        // A note is the user's own words, bounded as a rejection's note is, whichever way the
        // decision goes.
        let bounded = |note: Option<String>| -> Result<Option<String>, ApprovalError> {
            if note
                .as_ref()
                .is_some_and(|n| n.chars().count() > LIM_D2_REASON_CHARS)
            {
                return Err(ApprovalError::NoteTooLong);
            }
            Ok(note.map(|n| sanitize(&n).0))
        };
        match decision {
            UserDecision::Approve { note } => {
                self.note = bounded(note)?;
                self.status = ApprovalStatus::Approved;
            }
            UserDecision::Reject { note, halt } => {
                self.note = bounded(note)?;
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
