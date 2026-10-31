//! The validation pipeline (external design §6.3; REQ-DEC-004 … 008,
//! REQ-ARCH-005). Twelve stages in a fixed order; a failed stage stops the
//! pipeline; no filesystem is touched before execution; an out-of-scope path
//! is denied identically whether or not it exists (INV-03). Only this module
//! can construct an [`AuthorizedAction`] (INV-01).

use crate::action::EffectView;
use crate::decision::{Decision, parse_strict};
use crate::hash::Hash;
use crate::path::{PathError, PlatformRules};
use crate::rejection::{
    BudgetKind, Denial, DeniedReason, Detail, Rejection, RejectionCode, StateReason,
};
use crate::tool::{SchemaViolation, ToolDefinition};
use serde_json::{Map, Value};

/// What stage 9 derives from a tool and its arguments.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Derived {
    /// The effect in force for this action.
    pub effect: EffectView,
    /// Whether the user must approve before execution.
    pub approval_required: bool,
}

/// Stage 7: grants, exclusions, destinations and the profile's effect rules (handoff 003).
pub trait GrantCheck {
    /// Allows the action or names the denial.
    fn check(&self, tool: &ToolDefinition, args: &Map<String, Value>) -> Result<(), Denial>;
}

/// Stage 9: effect and approval derivation (handoff 003).
pub trait EffectDerivation {
    /// Derives the effect and the approval requirement.
    fn derive(&self, tool: &ToolDefinition, args: &Map<String, Value>) -> Derived;
}

/// Stage 10: budget reservation (handoff 003 over the task's budgets).
pub trait BudgetReservation {
    /// Reserves the step's execution and returns the effective deadline in
    /// milliseconds (the minimum of the tool default, the task's remaining
    /// time and the profile's cap, REQ-TOOL-006), or names the exhausted budget.
    fn reserve(
        &self,
        tool: &ToolDefinition,
        args: &Map<String, Value>,
        derived: &Derived,
    ) -> Result<i64, BudgetKind>;
}

/// Everything the validator needs about the task.
pub struct ValidationContext<'a> {
    /// Whether the task is `running`.
    pub task_running: bool,
    /// Whether an approval or question is pending.
    pub pending_user_item: bool,
    /// The tools visible to the task.
    pub tools: &'a [ToolDefinition],
    /// The platform's naming rules.
    pub rules: PlatformRules,
    /// Stage 7.
    pub grants: &'a dyn GrantCheck,
    /// Stage 9.
    pub effects: &'a dyn EffectDerivation,
    /// Stage 10.
    pub budgets: &'a dyn BudgetReservation,
}

/// A decision that is not an action.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ControlDecision {
    /// Replace the plan.
    Plan {
        /// Steps.
        steps: Vec<String>,
    },
    /// Ask the user.
    AskUser {
        /// The question.
        question: String,
    },
    /// Declare completion.
    Complete {
        /// Summary.
        summary: String,
    },
    /// Give up.
    GiveUp {
        /// Why.
        reason: String,
    },
}

/// A validated, authorized action. Constructible only by [`validate`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthorizedAction {
    tool: String,
    arguments: Map<String, Value>,
    effect: EffectView,
    approval_required: bool,
    deadline_ms: i64,
    decision_hash: Hash,
    reason: Option<String>,
    _sealed: Sealed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Sealed;

impl AuthorizedAction {
    /// The tool's name.
    pub fn tool(&self) -> &str {
        &self.tool
    }

    /// Canonical arguments (paths canonicalized at stage 6).
    pub fn arguments(&self) -> &Map<String, Value> {
        &self.arguments
    }

    /// The effect in force.
    pub fn effect(&self) -> EffectView {
        self.effect
    }

    /// Whether the user must approve before execution.
    pub fn approval_required(&self) -> bool {
        self.approval_required
    }

    /// The effective deadline in milliseconds.
    pub fn deadline_ms(&self) -> i64 {
        self.deadline_ms
    }

    /// The hash of the decision that produced it.
    pub fn decision_hash(&self) -> &Hash {
        &self.decision_hash
    }

    /// The model's stated reason; display-only, untrusted.
    pub fn reason(&self) -> Option<&str> {
        self.reason.as_deref()
    }
}

/// The result of validation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// An action may execute (after approval, when required).
    Authorized(AuthorizedAction),
    /// A control decision for the loop.
    Control {
        /// The decision.
        decision: ControlDecision,
        /// Its hash.
        decision_hash: Hash,
    },
    /// The decision was refused.
    Rejected {
        /// Why.
        rejection: Rejection,
        /// The decision's hash when it parsed.
        decision_hash: Option<Hash>,
    },
}

impl Outcome {
    /// The decision's hash when one was parsed.
    pub fn decision_hash(&self) -> Option<&Hash> {
        match self {
            Outcome::Authorized(a) => Some(&a.decision_hash),
            Outcome::Control { decision_hash, .. } => Some(decision_hash),
            Outcome::Rejected { decision_hash, .. } => decision_hash.as_ref(),
        }
    }
}

/// Runs the twelve stages of external design §6.3 over the model's bytes.
pub fn validate(ctx: &ValidationContext<'_>, raw: &[u8]) -> Outcome {
    // Stage 1: state.
    if !ctx.task_running {
        return rejected(
            Rejection::new(
                RejectionCode::State,
                Detail::State {
                    reason: StateReason::NotRunning,
                },
            ),
            None,
        );
    }
    if ctx.pending_user_item {
        return rejected(
            Rejection::new(
                RejectionCode::State,
                Detail::State {
                    reason: StateReason::PendingUserItem,
                },
            ),
            None,
        );
    }
    // Stages 2–3: size, parse, decision schema.
    let decision = match parse_strict(raw) {
        Ok(d) => d,
        Err(r) => return rejected(r, None),
    };
    let decision_hash = decision.hash();
    let (tool_name, mut arguments, reason) = match decision {
        Decision::Act {
            tool,
            arguments,
            reason,
        } => (tool, arguments, reason),
        Decision::Plan { steps } => return control(ControlDecision::Plan { steps }, decision_hash),
        Decision::AskUser { question } => {
            return control(ControlDecision::AskUser { question }, decision_hash);
        }
        Decision::Complete { summary } => {
            return control(ControlDecision::Complete { summary }, decision_hash);
        }
        Decision::GiveUp { reason } => {
            return control(ControlDecision::GiveUp { reason }, decision_hash);
        }
    };
    let hash = Some(decision_hash.clone());
    // Stage 4: tool visibility.
    let Some(tool) = ctx.tools.iter().find(|t| t.name == tool_name) else {
        return rejected(
            Rejection::new(
                RejectionCode::UnknownTool,
                Detail::UnknownTool {
                    tool: tool_name,
                    visible_tools: ctx.tools.iter().map(|t| t.name.clone()).collect(),
                },
            ),
            hash,
        );
    };
    // Stage 5: argument shape.
    if let Err(v) = tool.input.validate_shape(&arguments) {
        return rejected(Rejection::schema(v.path, v.expected, v.allowed), hash);
    }
    // Stage 6: canonicalization (invalid names) — escapes fall through to stage 7.
    let mut escape: Option<DeniedReason> = None;
    if let Err((path, err)) = tool.input.canonicalize_paths(&mut arguments, ctx.rules) {
        match err {
            PathError::Escapes | PathError::Absolute => {
                escape = Some(DeniedReason::PathOutsideWorkspace)
            }
            other => {
                return rejected(
                    Rejection::invalid_argument(path, None, None, Some(vec![other.to_string()])),
                    hash,
                );
            }
        }
    }
    // Stage 7: authorization, before any filesystem access.
    if let Some(reason) = escape {
        return rejected(Rejection::denied(Denial::new(reason)), hash);
    }
    if let Err(denial) = ctx.grants.check(tool, &arguments) {
        return rejected(Rejection::denied(denial), hash);
    }
    // Stage 8: semantic bounds.
    if let Err(v) = tool.input.validate_bounds(&arguments) {
        return rejected(bounds_rejection(v), hash);
    }
    // Stage 9: effect and approval.
    let derived = ctx.effects.derive(tool, &arguments);
    // Stage 10: budget reservation.
    let deadline_ms = match ctx.budgets.reserve(tool, &arguments, &derived) {
        Ok(ms) => ms,
        Err(budget) => {
            return rejected(
                Rejection::new(RejectionCode::Budget, Detail::Budget { budget }),
                hash,
            );
        }
    };
    // Stages 11–12 belong to the loop (approval) and the executor (execution).
    Outcome::Authorized(AuthorizedAction {
        tool: tool.name.clone(),
        arguments,
        effect: derived.effect,
        approval_required: derived.approval_required,
        deadline_ms,
        decision_hash,
        reason,
        _sealed: Sealed,
    })
}

fn rejected(rejection: Rejection, decision_hash: Option<Hash>) -> Outcome {
    Outcome::Rejected {
        rejection,
        decision_hash,
    }
}

fn control(decision: ControlDecision, decision_hash: Hash) -> Outcome {
    Outcome::Control {
        decision,
        decision_hash,
    }
}

fn bounds_rejection(v: SchemaViolation) -> Rejection {
    Rejection::invalid_argument(v.path, v.min, v.max, v.allowed)
}

/// Permissive hooks for tests: every action allowed, effect from the
/// definition, approval only for the definition's own rule, deadline from
/// the definition.
#[cfg(any(test, feature = "test-support"))]
pub mod permissive {
    use super::*;
    use crate::action::EffectKind;

    /// Allows everything.
    #[derive(Debug, Default)]
    pub struct PermissiveHooks;

    impl GrantCheck for PermissiveHooks {
        fn check(&self, _tool: &ToolDefinition, _args: &Map<String, Value>) -> Result<(), Denial> {
            Ok(())
        }
    }

    impl EffectDerivation for PermissiveHooks {
        fn derive(&self, tool: &ToolDefinition, _args: &Map<String, Value>) -> Derived {
            let e = tool.effect;
            Derived {
                effect: e,
                approval_required: matches!(e.kind, EffectKind::ExternalWrite)
                    || (e.kind == EffectKind::HostWrite && !e.reversible),
            }
        }
    }

    impl BudgetReservation for PermissiveHooks {
        fn reserve(
            &self,
            tool: &ToolDefinition,
            _args: &Map<String, Value>,
            _derived: &Derived,
        ) -> Result<i64, BudgetKind> {
            Ok(tool.deadline_ms)
        }
    }
}

#[cfg(test)]
mod tests;
