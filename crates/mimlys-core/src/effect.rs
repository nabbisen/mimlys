//! Effect declarations and the one derivation function (external design
//! §7.2; REQ-TOOL-002, REQ-APR-001). A tool declares its effect once;
//! approval, recovery and parallelism derive from it here and nowhere else.

use crate::action::{AfterUnknown, EffectKind, EffectView, after_unknown};
use crate::policy::Profile;
use serde::{Deserialize, Serialize};

/// A declaration that cannot be expressed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectDeclError {
    /// `workspace_write` is always reversible through snapshots.
    #[error("workspace_write is always reversible")]
    WorkspaceWriteNotReversible,
    /// `external_write` can never be undone by the runtime.
    #[error("external_write is never reversible")]
    ExternalWriteReversible,
    /// Reads and `none` carry no reversibility; they must be declared reversible.
    #[error("an effect without writes is reversible by definition")]
    ReadNotReversible,
}

/// Builds a declaration, refusing combinations that cannot exist.
pub fn declare(
    kind: EffectKind,
    reversible: bool,
    idempotent: bool,
) -> Result<EffectView, EffectDeclError> {
    match (kind, reversible) {
        (EffectKind::WorkspaceWrite, false) => Err(EffectDeclError::WorkspaceWriteNotReversible),
        (EffectKind::ExternalWrite, true) => Err(EffectDeclError::ExternalWriteReversible),
        (
            EffectKind::None
            | EffectKind::WorkspaceRead
            | EffectKind::HostRead
            | EffectKind::ExternalRead,
            false,
        ) => Err(EffectDeclError::ReadNotReversible),
        _ => Ok(EffectView {
            kind,
            reversible,
            idempotent,
        }),
    }
}

/// Checks an existing view against the declaration rules.
pub fn check(effect: EffectView) -> Result<(), EffectDeclError> {
    declare(effect.kind, effect.reversible, effect.idempotent).map(|_| ())
}

/// What the effect of an action implies (external design §7.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Implications {
    /// The user must approve before execution (REQ-APR-001).
    pub approval_required: bool,
    /// The recovery rule after an unknown outcome.
    pub after_unknown: AfterUnknown,
    /// Whether the action could run beside others (reserved for DR-001).
    pub parallel_ok: bool,
}

/// Derives the implications of running `tool` with `effect` under `profile`.
/// Approval is required for `external_write`, for a non-reversible
/// `host_write`, and for every tool the profile lists; never otherwise.
pub fn derive(tool: &str, effect: EffectView, profile: &Profile) -> Implications {
    let by_effect = match effect.kind {
        EffectKind::ExternalWrite => true,
        EffectKind::HostWrite => !effect.reversible,
        _ => false,
    };
    Implications {
        approval_required: by_effect || profile.requires_approval(tool),
        after_unknown: after_unknown(effect),
        parallel_ok: !effect.kind.is_write(),
    }
}

#[cfg(test)]
mod tests;
