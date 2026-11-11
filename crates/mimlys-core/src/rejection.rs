//! Rejections (external design §6.4; REQ-DEC-008): a code from a fixed
//! vocabulary, a typed `detail` with fixed keys, and a hint of at most
//! LIM-I5 characters from a fixed table that says what the model may
//! change. A rejection never carries filesystem contents or policy internals.

use crate::limits::LIM_I5_MESSAGE_CHARS;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Why the runtime refused a decision before any execution.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RejectionCode {
    /// Not exactly one JSON object, too large, or duplicate keys.
    Malformed,
    /// Missing or unknown field, wrong type, or bad enumeration value.
    Schema,
    /// The named tool is not visible to the task.
    UnknownTool,
    /// An argument is out of bounds or invalid.
    InvalidArgument,
    /// Outside the workspace or the grants, an excluded path, an ungranted
    /// destination, or an effect the profile does not allow.
    Denied,
    /// The decision is not allowed in the current state.
    State,
    /// The action would exceed a budget.
    Budget,
    /// The user rejected the action.
    ApprovalRejected,
}

impl RejectionCode {
    /// Every code, in the order of external design §6.4.
    pub const ALL: [RejectionCode; 8] = [
        RejectionCode::Malformed,
        RejectionCode::Schema,
        RejectionCode::UnknownTool,
        RejectionCode::InvalidArgument,
        RejectionCode::Denied,
        RejectionCode::State,
        RejectionCode::Budget,
        RejectionCode::ApprovalRejected,
    ];

    /// The snake_case name used on the wire.
    pub fn as_str(self) -> &'static str {
        match self {
            RejectionCode::Malformed => "malformed",
            RejectionCode::Schema => "schema",
            RejectionCode::UnknownTool => "unknown_tool",
            RejectionCode::InvalidArgument => "invalid_argument",
            RejectionCode::Denied => "denied",
            RejectionCode::State => "state",
            RejectionCode::Budget => "budget",
            RejectionCode::ApprovalRejected => "approval_rejected",
        }
    }

    /// The hint of the fixed table (external design §6.4), at most LIM-I5 characters.
    pub fn hint(self) -> &'static str {
        match self {
            RejectionCode::Malformed => {
                "Return exactly one JSON object with a `decision` field and nothing before or after it."
            }
            RejectionCode::Schema => "Fix the field at `path` so that it matches `expected`.",
            RejectionCode::UnknownTool => "Use one of the tools listed in `visible_tools`.",
            RejectionCode::InvalidArgument => {
                "Change the argument at `path` to a value within the stated bounds."
            }
            RejectionCode::Denied => DeniedReason::GrantMissing.hint(),
            RejectionCode::State => {
                "Wait for the pending item; no new action is accepted until it is resolved."
            }
            RejectionCode::Budget => {
                "The budget is exhausted; finish with `complete` or `give_up`."
            }
            RejectionCode::ApprovalRejected => {
                "The user rejected this action; choose another approach or ask the user."
            }
        }
    }
}

impl RejectionCode {
    /// The hint as the model reads it in `transport`. In the `tool_calls`
    /// transport the envelope is not described to the model: `malformed` and
    /// `unknown_tool` tell it to call exactly one of the provided functions;
    /// every other hint is the same in both.
    pub fn hint_for(self, transport: crate::native::DecisionTransport) -> &'static str {
        match (transport, self) {
            (crate::native::DecisionTransport::ToolCalls, RejectionCode::Malformed) => {
                "Call exactly one of the provided functions, with its arguments as one JSON object. A call written into your text is not read."
            }
            (crate::native::DecisionTransport::ToolCalls, RejectionCode::UnknownTool) => {
                "Call one of the provided functions."
            }
            _ => self.hint(),
        }
    }
}

impl fmt::Display for RejectionCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What was wrong with a malformed decision.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MalformedProblem {
    /// The value is not a JSON object, or is not valid JSON.
    NotObject,
    /// More than one JSON value.
    MultipleValues,
    /// Text after the object.
    TrailingText,
    /// A key appears twice in one object.
    DuplicateKey,
    /// Larger than LIM-D1.
    TooLarge,
}

/// Why a path or operation was denied.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeniedReason {
    /// The path leaves the workspace root.
    PathOutsideWorkspace,
    /// The path matches an exclusion pattern.
    PathExcluded,
    /// No grant covers the operation.
    GrantMissing,
    /// A destination argument is not covered by a network grant.
    DestinationNotGranted,
    /// A destination argument is an `http` URL; only `https` is admitted (REQ-MCP-005).
    SchemeNotHttps,
    /// The profile does not allow the effect.
    EffectNotAllowed,
    /// Promotion is not allowed here.
    PromoteNotAllowed,
    /// A host tool argument is neither an allowed literal nor a pattern match (RFC 011 §6).
    ArgumentNotAllowed,
}

impl DeniedReason {
    /// Every reason, in the order of external design §6.4.
    pub const ALL: [DeniedReason; 8] = [
        DeniedReason::PathOutsideWorkspace,
        DeniedReason::PathExcluded,
        DeniedReason::GrantMissing,
        DeniedReason::DestinationNotGranted,
        DeniedReason::SchemeNotHttps,
        DeniedReason::EffectNotAllowed,
        DeniedReason::PromoteNotAllowed,
        DeniedReason::ArgumentNotAllowed,
    ];

    /// The hint of the fixed table for this reason (CLR-5): what the model may change.
    pub fn hint(self) -> &'static str {
        match self {
            DeniedReason::PathOutsideWorkspace => {
                "Use a path relative to the workspace root; nothing outside the workspace is readable or writable."
            }
            DeniedReason::PathExcluded => {
                "The path is excluded by policy (secrets, build outputs, version control); work with other files."
            }
            DeniedReason::GrantMissing => {
                "No grant covers this; use the tools and grants you have, or ask the user for the grant named in `grant_kind` with `ask_user`."
            }
            DeniedReason::DestinationNotGranted => {
                "The destination is not covered by a `network` grant; use a granted domain, or ask the user to grant this one."
            }
            DeniedReason::SchemeNotHttps => {
                "Only `https` addresses are admitted; propose the `https` form of this URL (the same host and path), or ask the user."
            }
            DeniedReason::EffectNotAllowed => {
                "The profile does not allow this effect; choose another approach, or ask the user."
            }
            DeniedReason::PromoteNotAllowed => {
                "Promotion is the user's operation; finish the task and leave the promotion to the user."
            }
            DeniedReason::ArgumentNotAllowed => {
                "Pass only the declared literals or values matching the declared pattern, at most `max` arguments; no paths, no other flags."
            }
        }
    }
}

/// Why the state refuses a decision.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StateReason {
    /// An approval or question is pending.
    PendingUserItem,
    /// The completion check is running.
    CompletionCheckRunning,
    /// The task is not running.
    NotRunning,
}

/// Which budget an action would exceed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetKind {
    /// LIM-T1.
    Steps,
    /// The deadline.
    Deadline,
    /// LIM-T3.
    Executions,
    /// LIM-W1, LIM-W2, LIM-W3.
    Workspace,
}

/// The typed detail of a rejection, with the fixed keys of external design §6.4.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum Detail {
    /// For `malformed`.
    Malformed {
        /// What was wrong.
        problem: MalformedProblem,
        /// The limit involved, when `too_large`.
        #[serde(skip_serializing_if = "Option::is_none")]
        limit: Option<usize>,
    },
    /// For `schema`.
    Schema {
        /// JSON path of the offending field.
        path: String,
        /// What was expected.
        expected: String,
        /// Allowed values, when an enumeration.
        #[serde(skip_serializing_if = "Option::is_none")]
        allowed: Option<Vec<String>>,
    },
    /// For `unknown_tool`.
    UnknownTool {
        /// The name used.
        tool: String,
        /// The tools the task may use.
        visible_tools: Vec<String>,
    },
    /// For `invalid_argument`.
    InvalidArgument {
        /// JSON path of the argument.
        path: String,
        /// Lower bound, when any.
        #[serde(skip_serializing_if = "Option::is_none")]
        min: Option<i64>,
        /// Upper bound, when any.
        #[serde(skip_serializing_if = "Option::is_none")]
        max: Option<i64>,
        /// Allowed values, when any.
        #[serde(skip_serializing_if = "Option::is_none")]
        allowed: Option<Vec<String>>,
    },
    /// For `denied`.
    Denied {
        /// Why.
        reason: DeniedReason,
        /// The grant kind that would allow it, when any.
        #[serde(skip_serializing_if = "Option::is_none")]
        grant_kind: Option<String>,
        /// `argument_not_allowed`: the position of the refused argument.
        #[serde(skip_serializing_if = "Option::is_none")]
        argument: Option<usize>,
        /// `argument_not_allowed`: the refused argument, bounded.
        #[serde(skip_serializing_if = "Option::is_none")]
        value: Option<String>,
        /// `argument_not_allowed`: how many arguments may be added.
        #[serde(skip_serializing_if = "Option::is_none")]
        max: Option<usize>,
    },
    /// For `state`.
    State {
        /// Why.
        reason: StateReason,
    },
    /// For `budget`.
    Budget {
        /// Which.
        budget: BudgetKind,
    },
    /// For `approval_rejected`.
    ApprovalRejected {
        /// The user's note, when any.
        #[serde(skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
}

/// What a stage-7 denial says (external design §6.4): the reason, the grant
/// kind that would allow it, and for `argument_not_allowed` the argument's
/// position, its value and the bound.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Denial {
    /// Why.
    pub reason: DeniedReason,
    /// The grant kind that would allow it, when any.
    pub grant_kind: Option<String>,
    /// The position of a refused host-tool argument.
    pub argument: Option<usize>,
    /// The refused argument, bounded.
    pub value: Option<String>,
    /// How many arguments may be added.
    pub max: Option<usize>,
}

impl Denial {
    /// A denial with a reason and nothing else.
    pub fn new(reason: DeniedReason) -> Self {
        Self {
            reason,
            grant_kind: None,
            argument: None,
            value: None,
            max: None,
        }
    }

    /// A denial a grant of `grant_kind` would lift.
    pub fn grant(reason: DeniedReason, grant_kind: &str) -> Self {
        Self {
            grant_kind: Some(grant_kind.to_owned()),
            ..Self::new(reason)
        }
    }

    /// `argument_not_allowed`: the argument at `index` is neither a declared literal nor a pattern match.
    pub fn argument(index: usize, value: &str) -> Self {
        Self {
            argument: Some(index),
            value: Some(value.chars().take(LIM_I5_MESSAGE_CHARS).collect()),
            ..Self::new(DeniedReason::ArgumentNotAllowed)
        }
    }

    /// `argument_not_allowed`: more arguments than the declaration allows.
    pub fn too_many(max: usize) -> Self {
        Self {
            max: Some(max),
            ..Self::new(DeniedReason::ArgumentNotAllowed)
        }
    }
}

/// A refused decision (REQ-DEC-008).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Rejection {
    /// The code.
    pub code: RejectionCode,
    /// The typed detail.
    pub detail: Detail,
    /// What the model may change.
    pub hint: &'static str,
}

impl Rejection {
    /// Builds a rejection; the hint comes from the fixed table.
    pub fn new(code: RejectionCode, detail: Detail) -> Self {
        let hint = match &detail {
            Detail::Denied { reason, .. } => reason.hint(),
            _ => code.hint(),
        };
        Self { code, detail, hint }
    }

    /// A `malformed` rejection.
    pub fn malformed(problem: MalformedProblem, limit: Option<usize>) -> Self {
        Self::new(
            RejectionCode::Malformed,
            Detail::Malformed { problem, limit },
        )
    }

    /// A `schema` rejection.
    pub fn schema(
        path: impl Into<String>,
        expected: impl Into<String>,
        allowed: Option<Vec<String>>,
    ) -> Self {
        Self::new(
            RejectionCode::Schema,
            Detail::Schema {
                path: path.into(),
                expected: expected.into(),
                allowed,
            },
        )
    }

    /// An `invalid_argument` rejection.
    pub fn invalid_argument(
        path: impl Into<String>,
        min: Option<i64>,
        max: Option<i64>,
        allowed: Option<Vec<String>>,
    ) -> Self {
        Self::new(
            RejectionCode::InvalidArgument,
            Detail::InvalidArgument {
                path: path.into(),
                min,
                max,
                allowed,
            },
        )
    }

    /// A `denied` rejection; the hint follows the reason.
    pub fn denied(denial: Denial) -> Self {
        Self::new(
            RejectionCode::Denied,
            Detail::Denied {
                reason: denial.reason,
                grant_kind: denial.grant_kind,
                argument: denial.argument,
                value: denial.value,
                max: denial.max,
            },
        )
    }
}

/// Every hint is within LIM-I5; checked by the test suite.
pub fn hints_within_limit() -> bool {
    RejectionCode::ALL
        .iter()
        .all(|c| c.hint().chars().count() <= LIM_I5_MESSAGE_CHARS)
        && DeniedReason::ALL
            .iter()
            .all(|r| r.hint().chars().count() <= LIM_I5_MESSAGE_CHARS)
}

#[cfg(test)]
mod tests;
