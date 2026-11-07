use super::*;
use crate::clock::{Clock, FakeClock};
use crate::hash::hash_bytes;
use crate::ids::ScriptedIds;
use crate::limits::LIM_T3_EXECUTIONS_PER_ACTION;
use serde_json::json;

fn effect(kind: EffectKind, reversible: bool, idempotent: bool) -> EffectView {
    EffectView {
        kind,
        reversible,
        idempotent,
    }
}

fn proposal(ids: &ScriptedIds) -> Proposal {
    Proposal {
        action_id: ActionId::new(ids),
        task_id: TaskId::new(ids),
        step_id: StepId::new(ids),
        tool: "read_file".into(),
        arguments: json!({"path": "src/lib.rs"}),
        grant_set_hash: hash_bytes(b"grants"),
        effect: effect(EffectKind::WorkspaceRead, true, true),
        approval_required: false,
        deadline_ms: 30_000,
    }
}

fn in_status(status: ActionStatus) -> Action {
    let mut a = Action::propose(
        proposal(&ScriptedIds::default()),
        FakeClock::default().now(),
    );
    a.status = status;
    a
}

fn all_statuses() -> Vec<ActionStatus> {
    vec![
        ActionStatus::Proposed,
        ActionStatus::Rejected {
            code: RejectionCode::Denied,
        },
        ActionStatus::WaitingApproval,
        ActionStatus::Authorized,
        ActionStatus::Running,
        ActionStatus::Succeeded,
        ActionStatus::Failed {
            cause: FailureCause::Timeout,
        },
        ActionStatus::Cancelled,
        ActionStatus::Unknown,
    ]
}

fn all_inputs() -> Vec<ActionInput> {
    vec![
        ActionInput::Reject(RejectionCode::Schema),
        ActionInput::RequireApproval,
        ActionInput::Authorize,
        ActionInput::Start,
        ActionInput::Succeed,
        ActionInput::Fail(FailureCause::Timeout),
        ActionInput::Cancel,
        ActionInput::MarkUnknown,
    ]
}

/// REQ-DEC-012: the action hash is the canonical hash of tool and arguments, independent of argument text form.
#[test]
fn req_dec_012_action_hash_is_canonical() {
    let ids = ScriptedIds::default();
    let now = FakeClock::default().now();
    let a = Action::propose(proposal(&ids), now);
    let mut p2 = proposal(&ids);
    p2.arguments = serde_json::from_str(r#"{ "path" : "src/lib.rs" }"#).unwrap();
    let b = Action::propose(p2, now);
    assert_eq!(a.content().action_hash(), b.content().action_hash());
    let mut p3 = proposal(&ids);
    p3.arguments = json!({"path": "src/main.rs"});
    assert_ne!(
        a.content().action_hash(),
        Action::propose(p3, now).content().action_hash()
    );
    assert_eq!(a.content().tool(), "read_file");
    assert_eq!(a.content().deadline_ms(), 30_000);
    assert!(!a.content().approval_required());
}

/// External design §7.1: every legal action transition, including cancellation before execution.
#[test]
fn ed_7_1_legal_action_transitions() {
    let now = FakeClock::default().now();
    let r = |s: ActionStatus, i: ActionInput| in_status(s).transition(i, now).unwrap().status;
    assert_eq!(
        r(
            ActionStatus::Proposed,
            ActionInput::Reject(RejectionCode::Denied)
        ),
        ActionStatus::Rejected {
            code: RejectionCode::Denied
        }
    );
    assert_eq!(
        r(ActionStatus::Proposed, ActionInput::RequireApproval),
        ActionStatus::WaitingApproval
    );
    assert_eq!(
        r(ActionStatus::Proposed, ActionInput::Authorize),
        ActionStatus::Authorized
    );
    assert_eq!(
        r(ActionStatus::Proposed, ActionInput::Cancel),
        ActionStatus::Cancelled
    );
    assert_eq!(
        r(
            ActionStatus::WaitingApproval,
            ActionInput::Reject(RejectionCode::ApprovalRejected)
        ),
        ActionStatus::Rejected {
            code: RejectionCode::ApprovalRejected
        }
    );
    assert_eq!(
        r(ActionStatus::WaitingApproval, ActionInput::Authorize),
        ActionStatus::Authorized
    );
    assert_eq!(
        r(ActionStatus::WaitingApproval, ActionInput::Cancel),
        ActionStatus::Cancelled
    );
    assert_eq!(
        r(ActionStatus::Authorized, ActionInput::Start),
        ActionStatus::Running
    );
    assert_eq!(
        r(ActionStatus::Authorized, ActionInput::Cancel),
        ActionStatus::Cancelled
    );
    assert_eq!(
        r(ActionStatus::Running, ActionInput::Succeed),
        ActionStatus::Succeeded
    );
    assert_eq!(
        r(
            ActionStatus::Running,
            ActionInput::Fail(FailureCause::Timeout)
        ),
        ActionStatus::Failed {
            cause: FailureCause::Timeout
        }
    );
    assert_eq!(
        r(ActionStatus::Running, ActionInput::Cancel),
        ActionStatus::Cancelled
    );
    assert_eq!(
        r(ActionStatus::Running, ActionInput::MarkUnknown),
        ActionStatus::Unknown
    );
}

/// External design §7.1: every other pair is refused; terminal statuses refuse everything.
#[test]
fn ed_7_1_illegal_action_transitions_are_refused() {
    let now = FakeClock::default().now();
    let legal: &[(&str, &str)] = &[
        ("proposed", "reject"),
        ("proposed", "require_approval"),
        ("proposed", "authorize"),
        ("proposed", "cancel"),
        ("waiting_approval", "reject"),
        ("waiting_approval", "authorize"),
        ("waiting_approval", "cancel"),
        ("authorized", "start"),
        ("authorized", "cancel"),
        ("running", "succeed"),
        ("running", "fail"),
        ("running", "cancel"),
        ("running", "mark_unknown"),
    ];
    for status in all_statuses() {
        for input in all_inputs() {
            let pair = (status.name(), input.name());
            let result = in_status(status.clone()).transition(input, now);
            if legal.contains(&pair) {
                assert!(result.is_ok(), "expected legal {pair:?}");
            } else {
                assert!(result.is_err(), "expected refusal {pair:?}");
                if status.is_terminal() {
                    assert!(result.is_err());
                }
            }
        }
    }
}

/// INV-07 / REQ-PERS-008: no input moves an `unknown` action anywhere, and `start` counts executions.
#[test]
fn req_pers_008_unknown_is_terminal_and_start_counts_executions() {
    let now = FakeClock::default().now();
    let unknown = in_status(ActionStatus::Unknown);
    for input in all_inputs() {
        assert!(unknown.transition(input, now).is_err());
    }
    let started = in_status(ActionStatus::Authorized)
        .transition(ActionInput::Start, now)
        .unwrap();
    assert_eq!(started.executions, 1);
    assert!(started.finished_at.is_none());
    let done = started.transition(ActionInput::Succeed, now).unwrap();
    assert_eq!(done.finished_at, Some(now));
}

/// REQ-TOOL-009: a failed action can be re-armed for a retry only below LIM-T3 executions.
#[test]
fn req_tool_009_rearm_respects_the_execution_budget() {
    let budgets = Budgets::default();
    let mut failed = in_status(ActionStatus::Failed {
        cause: FailureCause::Timeout,
    });
    failed.executions = 1;
    let rearmed = failed.rearm_for_retry(&budgets).unwrap();
    assert_eq!(rearmed.status, ActionStatus::Authorized);
    failed.executions = LIM_T3_EXECUTIONS_PER_ACTION;
    assert!(failed.rearm_for_retry(&budgets).is_err());
    assert!(
        in_status(ActionStatus::Succeeded)
            .rearm_for_retry(&budgets)
            .is_err()
    );
}

/// REQ-SBX-003: `exit_code` exists only for `exited`; nine termination values serialize with the tag.
#[test]
fn req_sbx_003_termination_vocabulary_and_exit_code() {
    let all = [
        Termination::Exited { exit_code: 3 },
        Termination::Trapped {
            trap: "unreachable".into(),
        },
        Termination::FuelExhausted,
        Termination::Deadline,
        Termination::MemoryLimit,
        Termination::OutputLimit,
        Termination::ProcessLimit,
        Termination::Cancelled,
        Termination::HostFault {
            detail: HostFaultDetail::WorkerDied,
        },
    ];
    let names: Vec<String> = all
        .iter()
        .map(|t| {
            serde_json::to_value(t).unwrap()["termination"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    assert_eq!(
        names,
        [
            "exited",
            "trapped",
            "fuel_exhausted",
            "deadline",
            "memory_limit",
            "output_limit",
            "process_limit",
            "cancelled",
            "host_fault"
        ]
    );
    assert_eq!(all[0].exit_code(), Some(3));
    for t in &all[1..] {
        assert_eq!(t.exit_code(), None);
        assert!(serde_json::to_value(t).unwrap().get("exit_code").is_none());
    }
}

/// External design §7.5: termination maps to the failure cause; fuel exhaustion and deadline are never conflated.
#[test]
fn ed_7_5_failure_cause_from_termination() {
    use FailureCause::*;
    assert_eq!(
        FailureCause::from_termination(&Termination::Exited { exit_code: 0 }),
        None
    );
    assert_eq!(
        FailureCause::from_termination(&Termination::Exited { exit_code: 2 }),
        Some(ToolError {
            detail: "exit_code:2".into()
        })
    );
    assert_eq!(
        FailureCause::from_termination(&Termination::FuelExhausted),
        Some(ResourceExhausted {
            limit: ResourceLimit::Fuel
        })
    );
    assert_eq!(
        FailureCause::from_termination(&Termination::Deadline),
        Some(Timeout)
    );
    // REQ-SBX-003: a tree ended for its process count is a resource limit,
    // never a tool error and never a clean exit.
    assert_eq!(
        FailureCause::from_termination(&Termination::ProcessLimit),
        Some(ResourceExhausted {
            limit: ResourceLimit::Processes
        })
    );
    assert_eq!(
        serde_json::to_value(FailureCause::ResourceExhausted {
            limit: ResourceLimit::Processes
        })
        .unwrap(),
        serde_json::json!({"cause": "resource_exhausted", "limit": "processes"})
    );
    assert_ne!(
        FailureCause::from_termination(&Termination::FuelExhausted),
        FailureCause::from_termination(&Termination::Deadline)
    );
    assert_eq!(
        FailureCause::from_termination(&Termination::Cancelled),
        None
    );
    assert_eq!(
        FailureCause::from_termination(&Termination::HostFault {
            detail: HostFaultDetail::InvalidModule
        }),
        Some(SandboxFault {
            detail: HostFaultDetail::InvalidModule
        })
    );
}

/// REQ-TOOL-009 / REQ-TOOL-010: the disposition table of external design §7.5.
#[test]
fn req_tool_009_disposition_table() {
    let b = Budgets::default();
    let read = effect(EffectKind::WorkspaceRead, true, true);
    let ws_write = effect(EffectKind::WorkspaceWrite, true, false);
    let ext_write = effect(EffectKind::ExternalWrite, false, false);
    let ext_read_idem = effect(EffectKind::ExternalRead, false, true);

    // Timeout: retry for idempotent reads with executions left; feedback otherwise.
    assert_eq!(
        disposition(&FailureCause::Timeout, read, 1, &b),
        Disposition {
            kind: DispositionKind::Retry,
            rollback_first: false
        }
    );
    assert_eq!(
        disposition(
            &FailureCause::Timeout,
            read,
            LIM_T3_EXECUTIONS_PER_ACTION,
            &b
        )
        .kind,
        DispositionKind::Feedback
    );
    // A non-idempotent workspace write never retries, but is rolled back first (REQ-TOOL-010).
    assert_eq!(
        disposition(&FailureCause::Timeout, ws_write, 1, &b),
        Disposition {
            kind: DispositionKind::Feedback,
            rollback_first: true
        }
    );
    // An idempotent reversible write may retry, after rollback.
    let ws_write_idem = effect(EffectKind::WorkspaceWrite, true, true);
    assert_eq!(
        disposition(&FailureCause::Timeout, ws_write_idem, 1, &b),
        Disposition {
            kind: DispositionKind::Retry,
            rollback_first: true
        }
    );
    // External write: never retried, never rolled back.
    assert_eq!(
        disposition(&FailureCause::Timeout, ext_write, 1, &b),
        Disposition {
            kind: DispositionKind::Feedback,
            rollback_first: false
        }
    );
    // Tool unavailable: retry, then halt.
    assert_eq!(
        disposition(&FailureCause::ToolUnavailable, ext_read_idem, 1, &b).kind,
        DispositionKind::Retry
    );
    assert_eq!(
        disposition(&FailureCause::ToolUnavailable, ext_read_idem, 2, &b).kind,
        DispositionKind::Halt {
            reason: HaltReason::ToolUnavailable
        }
    );
    // Workspace quota halts; other exhaustion is feedback.
    assert_eq!(
        disposition(
            &FailureCause::ResourceExhausted {
                limit: ResourceLimit::Workspace
            },
            ws_write,
            1,
            &b
        )
        .kind,
        DispositionKind::Halt {
            reason: HaltReason::WorkspaceQuota
        }
    );
    assert_eq!(
        disposition(
            &FailureCause::ResourceExhausted {
                limit: ResourceLimit::Fuel
            },
            read,
            1,
            &b
        )
        .kind,
        DispositionKind::Feedback
    );
    // Sandbox faults: worker died retries once when idempotent; unavailable halts; invalid module is feedback.
    let died = FailureCause::SandboxFault {
        detail: HostFaultDetail::WorkerDied,
    };
    assert_eq!(disposition(&died, read, 1, &b).kind, DispositionKind::Retry);
    assert_eq!(
        disposition(&died, read, 2, &b).kind,
        DispositionKind::Feedback
    );
    assert_eq!(
        disposition(
            &FailureCause::SandboxFault {
                detail: HostFaultDetail::Unavailable
            },
            read,
            1,
            &b
        )
        .kind,
        DispositionKind::Halt {
            reason: HaltReason::ToolUnavailable
        }
    );
    assert_eq!(
        disposition(
            &FailureCause::SandboxFault {
                detail: HostFaultDetail::InvalidModule
            },
            read,
            1,
            &b
        )
        .kind,
        DispositionKind::Feedback
    );
    // Tool error and invalid output are feedback.
    assert_eq!(
        disposition(
            &FailureCause::ToolError {
                detail: "exit_code:1".into()
            },
            read,
            1,
            &b
        )
        .kind,
        DispositionKind::Feedback
    );
    assert_eq!(
        disposition(&FailureCause::OutputInvalid, read, 1, &b).kind,
        DispositionKind::Feedback
    );
}

/// External design §7.2: the recovery rule after an unknown outcome, per effect.
#[test]
fn ed_7_2_after_unknown_per_effect() {
    assert_eq!(
        after_unknown(effect(EffectKind::None, true, true)),
        AfterUnknown::RedoPermitted
    );
    assert_eq!(
        after_unknown(effect(EffectKind::WorkspaceRead, true, true)),
        AfterUnknown::RedoPermitted
    );
    assert_eq!(
        after_unknown(effect(EffectKind::HostRead, true, true)),
        AfterUnknown::RedoPermitted
    );
    assert_eq!(
        after_unknown(effect(EffectKind::WorkspaceWrite, true, false)),
        AfterUnknown::Rollback
    );
    assert_eq!(
        after_unknown(effect(EffectKind::HostWrite, true, false)),
        AfterUnknown::UndoThenHalt
    );
    assert_eq!(
        after_unknown(effect(EffectKind::HostWrite, false, false)),
        AfterUnknown::Halt
    );
    assert_eq!(
        after_unknown(effect(EffectKind::ExternalRead, false, true)),
        AfterUnknown::RedoPermitted
    );
    assert_eq!(
        after_unknown(effect(EffectKind::ExternalRead, false, false)),
        AfterUnknown::Halt
    );
    assert_eq!(
        after_unknown(effect(EffectKind::ExternalWrite, false, true)),
        AfterUnknown::Halt
    );
}

/// External design §7.3 / §7.4: action and execution records serialize flat and round-trip; content has no public setters.
#[test]
fn ed_7_3_records_serialize_flat_and_round_trip() {
    let ids = ScriptedIds::default();
    let clock = FakeClock::default();
    let action = Action::propose(proposal(&ids), clock.now());
    let json = serde_json::to_value(&action).unwrap();
    assert_eq!(json["status"], "proposed");
    assert_eq!(json["tool"], "read_file");
    assert_eq!(json["effect"]["kind"], "workspace_read");
    let back: Action = serde_json::from_value(json).unwrap();
    assert_eq!(back, action);

    let execution = Execution {
        execution_id: ExecutionId::new(&ids),
        action_id: action.action_id,
        attempt: 1,
        started_at: clock.now(),
        ended_at: clock.now().plus_millis(12).unwrap(),
        termination: Termination::Exited { exit_code: 0 },
        snapshot_before: None,
        snapshot_after: None,
        stdout_blob: Some(BlobId::new(&ids)),
        stderr_blob: None,
        metrics: ExecutionMetrics {
            wall_ms: 12,
            stdout_bytes: 40,
            ..ExecutionMetrics::default()
        },
    };
    let json = serde_json::to_value(&execution).unwrap();
    assert_eq!(json["termination"], "exited");
    assert_eq!(json["exit_code"], 0);
    let back: Execution = serde_json::from_value(json).unwrap();
    assert_eq!(back, execution);
}
