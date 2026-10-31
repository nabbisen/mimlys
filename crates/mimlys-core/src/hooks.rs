//! The validator hooks over grants, policy and budgets (external design
//! §6.3 stages 7, 9 and 10; handoff 003 §3.4). The validator calls them in
//! order; they never touch the filesystem.

use crate::clock::Instant;
use crate::effect;
use crate::grant::GrantSet;
use crate::host_tool::ArgViolation;
use crate::path::{CaseMode, WorkspacePath};
use crate::policy::{ExclusionMatcher, Profile};
use crate::rejection::{BudgetKind, Denial, DeniedReason};
use crate::tool::{ParamType, ToolDefinition};
use crate::validate::{BudgetReservation, Derived, EffectDerivation, GrantCheck};
use serde_json::{Map, Value};

/// What the hooks know about the task at this step.
#[derive(Clone, Copy, Debug)]
pub struct TaskView {
    /// The current instant.
    pub now: Instant,
    /// Milliseconds until the task's deadline.
    pub remaining_ms: i64,
    /// Bytes the workspace may still grow by, when the quota is tracked (REQ-WS-008).
    pub workspace_remaining_bytes: Option<u64>,
}

/// The policy hooks (REQ-CAP-001, REQ-WS-003, REQ-APR-001, REQ-TOOL-006, REQ-WS-008).
pub struct PolicyHooks<'a> {
    /// The task's grants.
    pub grants: &'a GrantSet,
    /// The task's profile.
    pub profile: &'a Profile,
    /// Compiled exclusions (materialization and secrets together).
    pub exclusions: &'a ExclusionMatcher,
    /// Case mode of the workspace.
    pub case: CaseMode,
    /// The task at this step.
    pub task: TaskView,
    /// The declared host tools (RFC 011 §6).
    pub host_tools: &'a std::collections::BTreeMap<String, crate::host_tool::HostToolDeclaration>,
}

impl GrantCheck for PolicyHooks<'_> {
    /// Stage 7: excluded workspace paths, ungranted destinations and ungranted host tools.
    /// Escapes and absolute paths were denied before this hook (REQ-DEC-007).
    fn check(&self, tool: &ToolDefinition, args: &Map<String, Value>) -> Result<(), Denial> {
        for name in tool.input.path_params() {
            if let Some(Value::String(s)) = args.get(name) {
                let path = if s.is_empty() {
                    WorkspacePath::root()
                } else {
                    WorkspacePath::canonicalize(s)
                        .map_err(|_| Denial::new(DeniedReason::PathOutsideWorkspace))?
                };
                if self.exclusions.is_excluded(&path) {
                    return Err(Denial::new(DeniedReason::PathExcluded));
                }
            }
        }
        for name in &tool.destination_arguments {
            if let Some(Value::String(url)) = args.get(name)
                && !self.grants.permits_destination(url, self.task.now)
            {
                return Err(Denial::grant(
                    DeniedReason::DestinationNotGranted,
                    "network",
                ));
            }
        }
        if tool.name == "run_host_tool"
            && let Some(Value::String(host_tool)) = args.get("tool")
        {
            if !self.grants.permits_host_tool(host_tool, self.task.now) {
                return Err(Denial::grant(DeniedReason::GrantMissing, "host_tool"));
            }
            let Some(declaration) = self.host_tools.get(host_tool) else {
                return Err(Denial::grant(DeniedReason::GrantMissing, "host_tool"));
            };
            let extra: Vec<String> = args
                .get("args")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default();
            match declaration.validate_args(&extra) {
                Ok(_) => {}
                Err(ArgViolation::TooMany { max }) => return Err(Denial::too_many(max)),
                Err(ArgViolation::NotAllowed { index, argument }) => {
                    return Err(Denial::argument(index, &argument));
                }
            }
        }
        Ok(())
    }
}

impl EffectDerivation for PolicyHooks<'_> {
    /// Stage 9: the tool's declared effect; approval per `effect::derive`.
    fn derive(&self, tool: &ToolDefinition, args: &Map<String, Value>) -> Derived {
        // A host tool's effect is its declaration's (RFC 011 §6, §8).
        let effect = match (tool.name.as_str(), args.get("tool").and_then(Value::as_str)) {
            ("run_host_tool", Some(name)) => self
                .host_tools
                .get(name)
                .map(|d| d.effect_view())
                .unwrap_or(tool.effect),
            _ => tool.effect,
        };
        let implications = effect::derive(&tool.name, effect, self.profile);
        Derived {
            effect,
            approval_required: implications.approval_required,
        }
    }
}

impl BudgetReservation for PolicyHooks<'_> {
    /// Stage 10: the deadline must not have passed; a write larger than the
    /// remaining workspace quota is refused; the effective deadline is the
    /// smaller of the tool's default and the task's remaining time.
    fn reserve(
        &self,
        tool: &ToolDefinition,
        args: &Map<String, Value>,
        _derived: &Derived,
    ) -> Result<i64, BudgetKind> {
        if self.task.remaining_ms <= 0 {
            return Err(BudgetKind::Deadline);
        }
        if let Some(remaining) = self.task.workspace_remaining_bytes {
            let incoming: u64 = tool
                .input
                .params
                .iter()
                .filter(|p| {
                    matches!(
                        p.ty,
                        ParamType::String {
                            max_bytes: Some(_),
                            ..
                        }
                    )
                })
                .filter_map(|p| args.get(&p.name).and_then(Value::as_str))
                .map(|s| s.len() as u64)
                .sum();
            if incoming > remaining {
                return Err(BudgetKind::Workspace);
            }
        }
        Ok(tool.deadline_ms.min(self.task.remaining_ms))
    }
}

#[cfg(test)]
mod tests;
