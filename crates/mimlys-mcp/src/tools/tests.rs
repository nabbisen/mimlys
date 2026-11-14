use super::*;
use mimlys_core::action::{EffectKind, EffectView, outcome_is_unknown};

fn provider() -> McpTools {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let id = TaskId::new(&mimlys_core::ids::SystemIds);
    // The runtime only lends its handle to `open`; the mapping does not use it.
    rt.block_on(McpTools::open(id, Arc::new(Servers::new(vec![]))))
}

fn failed(reason: &str) -> ExecutionOutcome {
    provider().map_outcome(
        "t.x",
        CallOutcome::Failed {
            reason: reason.into(),
        },
        7,
    )
}

fn external_write() -> EffectView {
    EffectView {
        kind: EffectKind::ExternalWrite,
        reversible: false,
        idempotent: false,
    }
}

/// REQ-TOOL-009, REQ-PERS-008 (handoff 010e-4, item 2): a request that was not
/// written (`send:closed`) never left. It is `host_fault{unavailable}` with the
/// cause `tool_unavailable`, what a server not reachable gives, and the outcome
/// of an action that cannot be undone is not unknown: nothing was sent.
#[test]
fn req_tool_009_a_request_not_written_is_unavailable_and_not_unknown() {
    let outcome = failed("send:closed");
    assert_eq!(
        outcome.termination,
        Termination::HostFault {
            detail: HostFaultDetail::Unavailable
        }
    );
    assert_eq!(outcome.failure, Some(FailureCause::ToolUnavailable));
    assert_eq!(outcome.payload, json!({"reason": "send:closed"}));
    assert!(!outcome_is_unknown(external_write(), &outcome.termination));
    // The same value as a server that could not be reached.
    let unreachable = unavailable("startup_timeout", ExecutionMetrics::default());
    assert_eq!(unreachable.termination, outcome.termination);
    assert_eq!(unreachable.failure, outcome.failure);
}

/// REQ-TOOL-009, REQ-PERS-008 (handoff 010e-4, item 2): a connection closed
/// after the request was written (`call:closed`) stays `host_fault{worker_died}`:
/// the call may have taken effect, so the outcome of an action that cannot be
/// undone is unknown.
#[test]
fn req_pers_008_a_connection_closed_after_the_request_is_a_lost_worker() {
    let outcome = failed("call:closed");
    assert_eq!(
        outcome.termination,
        Termination::HostFault {
            detail: HostFaultDetail::WorkerDied
        }
    );
    assert_eq!(
        outcome.failure,
        Some(FailureCause::SandboxFault {
            detail: HostFaultDetail::WorkerDied
        })
    );
    assert!(outcome_is_unknown(external_write(), &outcome.termination));
}

/// REQ-TOOL-009, REQ-PERS-008 (handoff 010e-4, item 2): a server whose restart
/// limit (LIM-X3) was reached has made no call, so it gives what a server that
/// could not be started gives: `host_fault{unavailable}`, the cause
/// `tool_unavailable`, and nothing is unknown for any effect.
#[test]
fn req_tool_009_a_server_past_its_restart_limit_is_unavailable_and_not_unknown() {
    let died = map_server_error(ServerError::Died { server: "t".into() }, 3);
    assert_eq!(
        died.termination,
        Termination::HostFault {
            detail: HostFaultDetail::Unavailable
        }
    );
    assert_eq!(died.failure, Some(FailureCause::ToolUnavailable));
    assert_eq!(died.payload, json!({"reason": "died"}));
    assert_eq!(died.metrics.wall_ms, 3);
    assert!(!outcome_is_unknown(external_write(), &died.termination));
    let unreachable = map_server_error(
        ServerError::Unavailable {
            server: "t".into(),
            reason: "startup_timeout".into(),
        },
        3,
    );
    assert_eq!(unreachable.termination, died.termination);
    assert_eq!(unreachable.failure, died.failure);
    assert_eq!(unreachable.payload, json!({"reason": "startup_timeout"}));
}
