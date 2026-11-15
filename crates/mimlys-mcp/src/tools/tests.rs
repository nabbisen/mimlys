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
    maps(
        &provider(),
        "t.x",
        CallOutcome::Failed {
            reason: reason.into(),
        },
    )
}

/// `map_outcome` with a minute left of the call's deadline.
fn maps(provider: &McpTools, name: &str, outcome: CallOutcome) -> ExecutionOutcome {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(provider.map_outcome(name, outcome, 7, Duration::from_secs(60)))
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

/// REQ-TOOL-009, REQ-PERS-008 (handoff 010e-5, item 3): every failure at the
/// send (`send:transport`, `send:other`, as `send:closed`) precedes any write
/// (the SDK returns from the send once the request is queued), so none is a
/// tool error, and none is unknown: each is what a server not reachable gives.
#[test]
fn req_tool_009_every_send_failure_is_unavailable_and_none_a_tool_error() {
    for reason in ["send:closed", "send:transport", "send:other"] {
        let outcome = failed(reason);
        assert_eq!(
            outcome.termination,
            Termination::HostFault {
                detail: HostFaultDetail::Unavailable
            },
            "{reason}"
        );
        assert_eq!(
            outcome.failure,
            Some(FailureCause::ToolUnavailable),
            "{reason}"
        );
        assert_eq!(outcome.payload, json!({ "reason": reason }), "{reason}");
        assert!(
            !outcome_is_unknown(external_write(), &outcome.termination),
            "{reason}"
        );
    }
}

/// REQ-PERS-008 (handoff 010e-5, item 3): a write of the request that failed
/// (`call:transport`: the SDK gives the failed write as the request's response)
/// may follow a complete line, so it is a connection lost after the call:
/// `host_fault{worker_died}`, unknown for an action that cannot be undone, never
/// a tool error.
#[test]
fn req_pers_008_a_failed_write_of_the_request_is_a_lost_worker() {
    let outcome = failed("call:transport");
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
    let died = map_server_error(ServerError::RestartLimitReached { server: "t".into() }, 3);
    assert_eq!(
        died.termination,
        Termination::HostFault {
            detail: HostFaultDetail::Unavailable
        }
    );
    assert_eq!(died.failure, Some(FailureCause::ToolUnavailable));
    assert_eq!(died.payload, json!({"reason": "restart_limit_reached"}));
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

/// A provider whose tool `s.t` has the output schema `schema`.
fn provider_with_output(schema: Value) -> McpTools {
    use crate::server::Listed;
    let mut provider = provider();
    let listed = Listed {
        name: "t".into(),
        description: Some("d".into()),
        input_schema: json!({"type": "object"}),
        output_schema: Some(schema),
        annotations: None,
    };
    let imported = crate::import::import("s", &listed, None, None);
    assert_eq!(imported.record.unavailable, None);
    provider.imported.insert(imported.name.clone(), imported);
    provider
}

fn result_of(value: Value) -> CallOutcome {
    CallOutcome::Result {
        content: vec![],
        structured: Some(value),
        is_error: false,
    }
}

/// REQ-MCP-006, REQ-TOOL-005 (handoff 009g, item 3): a schema that is within the
/// bounds at import but costs more than the budget on one result ends that
/// call `output_invalid`, with the reason that says the schema was too costly,
/// and the same schema passes a result of the size it was written for.
#[test]
fn req_mcp_006_a_result_too_costly_for_its_schema_is_output_invalid() {
    let mut alternatives: Vec<Value> = (0..7)
        .map(|i| json!({"type": "string", "const": format!("other{i}")}))
        .collect();
    alternatives.push(json!({"type": "string", "const": "x"}));
    let schema = json!({
        "type": "object", "required": ["items"],
        "properties": {"items": {"type": "array", "items": {"anyOf": alternatives}}}
    });
    let provider = provider_with_output(schema);
    let small = maps(&provider, "s.t", result_of(json!({"items": ["x", "x"]})));
    assert_eq!(small.failure, None, "{:?}", small.payload);
    let big = maps(
        &provider,
        "s.t",
        result_of(json!({"items": vec!["x"; mimlys_core::schema::MAX_VISITS]})),
    );
    assert_eq!(big.failure, Some(FailureCause::OutputInvalid));
    assert_eq!(
        big.payload["reason"],
        mimlys_core::schema::TOO_COSTLY,
        "{:?}",
        big.payload
    );
}

/// REQ-MCP-007, REQ-MCP-006 (handoff 009g, item 3): the validation is part of the
/// call, so a call whose deadline has passed when its result arrives is a call
/// that timed out, and its result is not passed.
#[test]
fn req_mcp_007_a_result_that_arrives_with_no_time_left_is_a_timeout() {
    let provider = provider_with_output(json!({"type": "object"}));
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let outcome =
        rt.block_on(provider.map_outcome("s.t", result_of(json!({"a": 1})), 7, Duration::ZERO));
    assert_eq!(outcome.failure, Some(FailureCause::Timeout));
    assert_eq!(outcome.termination, Termination::Deadline);
}
