use super::*;
use crate::policy::Policy;

fn view(kind: EffectKind, reversible: bool, idempotent: bool) -> EffectView {
    EffectView {
        kind,
        reversible,
        idempotent,
    }
}

/// REQ-TOOL-002: impossible declarations are refused at construction.
#[test]
fn req_tool_002_declaration_rules() {
    assert!(declare(EffectKind::WorkspaceWrite, true, false).is_ok());
    assert_eq!(
        declare(EffectKind::WorkspaceWrite, false, false),
        Err(EffectDeclError::WorkspaceWriteNotReversible)
    );
    assert_eq!(
        declare(EffectKind::ExternalWrite, true, false),
        Err(EffectDeclError::ExternalWriteReversible)
    );
    assert!(declare(EffectKind::ExternalWrite, false, true).is_ok());
    assert_eq!(
        declare(EffectKind::WorkspaceRead, false, true),
        Err(EffectDeclError::ReadNotReversible)
    );
    assert!(declare(EffectKind::HostWrite, false, false).is_ok());
    assert!(declare(EffectKind::HostWrite, true, false).is_ok());
    assert!(check(view(EffectKind::None, true, true)).is_ok());
}

/// REQ-APR-001 / ED §7.2: the exhaustive derivation table — approval, after-unknown, parallel.
#[test]
fn req_apr_001_derivation_table() {
    let policy = Policy::shipped();
    let workspace = policy.profile("workspace").unwrap();
    let mut listing = workspace.clone();
    listing.approval_tools = vec!["delete_path".into()];
    let cases: &[(EffectKind, bool, bool, bool, AfterUnknown, bool)] = &[
        (
            EffectKind::None,
            true,
            true,
            false,
            AfterUnknown::RedoPermitted,
            true,
        ),
        (
            EffectKind::WorkspaceRead,
            true,
            true,
            false,
            AfterUnknown::RedoPermitted,
            true,
        ),
        (
            EffectKind::WorkspaceWrite,
            true,
            false,
            false,
            AfterUnknown::Rollback,
            false,
        ),
        (
            EffectKind::HostRead,
            true,
            true,
            false,
            AfterUnknown::RedoPermitted,
            true,
        ),
        (
            EffectKind::HostWrite,
            true,
            false,
            false,
            AfterUnknown::UndoThenHalt,
            false,
        ),
        (
            EffectKind::HostWrite,
            false,
            false,
            true,
            AfterUnknown::Halt,
            false,
        ),
        (
            EffectKind::ExternalRead,
            false,
            true,
            false,
            AfterUnknown::RedoPermitted,
            true,
        ),
        (
            EffectKind::ExternalRead,
            false,
            false,
            false,
            AfterUnknown::Halt,
            true,
        ),
        (
            EffectKind::ExternalWrite,
            false,
            false,
            true,
            AfterUnknown::Halt,
            false,
        ),
    ];
    for (kind, reversible, idempotent, approval, after, parallel) in cases {
        let got = derive(
            "read_file",
            view(*kind, *reversible, *idempotent),
            workspace,
        );
        assert_eq!(got.approval_required, *approval, "{kind:?}");
        assert_eq!(got.after_unknown, *after, "{kind:?}");
        assert_eq!(got.parallel_ok, *parallel, "{kind:?}");
    }
    // A listed tool requires approval whatever its effect.
    assert!(
        derive(
            "delete_path",
            view(EffectKind::WorkspaceWrite, true, true),
            &listing
        )
        .approval_required
    );
    assert!(
        !derive(
            "write_file",
            view(EffectKind::WorkspaceWrite, true, true),
            &listing
        )
        .approval_required
    );
}
