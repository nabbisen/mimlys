//! The `tool_calls` decision transport in the loop (RFC 012; REQ-MDL-009,
//! REQ-DEC-003, REQ-OBS-001), over the fakes.

use super::*;
use crate::event::Event;
use crate::native::DecisionTransport;

fn received(h: &Harness) -> Vec<(crate::hash::Hash, DecisionTransport, Option<u64>)> {
    h.persistence
        .events()
        .into_iter()
        .filter_map(|e| match e.event {
            Event::DecisionReceived {
                hash,
                transport,
                text_chars,
            } => Some((hash, transport, text_chars)),
            _ => None,
        })
        .collect()
}

/// REQ-OBS-001, REQ-MDL-009: `decision.received` records how the decision
/// arrived, the length of text beside the call when there was any, and the
/// canonical hash of the (mapped) envelope, which is the hash the same
/// decision has in the envelope transport.
#[test]
fn req_obs_001_decision_received_records_transport_and_text_chars() {
    // An action and a control decision: the event is written on two different paths.
    let decisions = vec![
        act("read_file", "a.txt"),
        act("read_file", "b.txt"),
        json!({"decision": "plan", "steps": ["read a.txt"]}),
    ];
    let backend = Arc::new(ScriptedBackend::with_decisions(decisions.clone()).tool_calls());
    backend.push_text_chars(Some(41));
    backend.push_text_chars(None);
    backend.push_text_chars(Some(7));
    let h = Harness::with_backend(
        backend,
        Policy::shipped(),
        vec![],
        crate::limits::LIM_C1_CONTEXT_CHARS,
    );
    let id = h.create();
    h.step(id);
    h.step(id);
    h.step(id);
    let got = received(&h);
    assert_eq!(got.len(), 3);
    for ((hash, transport, _), decision) in got.iter().zip(&decisions) {
        assert_eq!(*transport, DecisionTransport::ToolCalls);
        assert_eq!(
            *hash,
            crate::hash::hash_canonical(decision),
            "the hash is the canonical hash of the envelope"
        );
    }
    assert_eq!(got[0].2, Some(41));
    assert_eq!(got[1].2, None);
    assert_eq!(got[2].2, Some(7), "a control decision records it too");
    let requests = h.backend.requests();
    assert_eq!(requests[0].transport, DecisionTransport::ToolCalls);
    let names: Vec<&str> = requests[0]
        .functions
        .iter()
        .map(|f| f.name.as_str())
        .collect();
    assert!(names.contains(&"read_file") && names.contains(&"complete"));
    assert_eq!(
        &names[names.len() - 4..],
        ["plan", "ask_user", "complete", "give_up"]
    );
}

/// REQ-MDL-009: the envelope transport is what a backend with one transport
/// has always been: no functions, `envelope` in the event, no `text_chars`.
#[test]
fn req_mdl_009_the_envelope_transport_offers_no_functions() {
    let h = Harness::new(vec![act("read_file", "a.txt")]);
    let id = h.create();
    h.step(id);
    let request = &h.backend.requests()[0];
    assert_eq!(request.transport, DecisionTransport::Envelope);
    assert!(request.functions.is_empty());
    let got = received(&h);
    assert_eq!(got.len(), 1);
    assert_eq!((got[0].1, got[0].2), (DecisionTransport::Envelope, None));
}

/// REQ-MDL-009: a tool whose function name collides is left out of the
/// functions and of the capability summary, and named to the model with the
/// reason; the tools that do not collide are offered unchanged.
#[test]
fn req_mdl_009_a_colliding_tool_is_unavailable_with_the_reason() {
    let mut policy = Policy::shipped();
    for name in ["a.b", "a__b"] {
        policy.ceiling.tools.push(name.into());
        policy
            .profiles
            .get_mut("workspace")
            .unwrap()
            .tools
            .push(name.into());
    }
    let backend =
        Arc::new(ScriptedBackend::with_decisions(vec![act("read_file", "x")]).tool_calls());
    let h = Harness::with_backend_and_tools(
        backend,
        policy,
        vec![],
        crate::limits::LIM_C1_CONTEXT_CHARS,
        vec![
            tool("a.b", EffectKind::WorkspaceRead),
            tool("a__b", EffectKind::WorkspaceRead),
        ],
    );
    let id = h.create();
    h.step(id);
    let request = &h.backend.requests()[0];
    let names: Vec<&str> = request.functions.iter().map(|f| f.name.as_str()).collect();
    assert!(names.contains(&"read_file"));
    assert!(!names.contains(&"a__b") && !names.contains(&"a.b"));
    let context: String = request
        .messages
        .iter()
        .map(|m| m.content.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        context.contains("<<feedback kind=\"tool_unavailable\">>"),
        "{context}"
    );
    assert!(context.contains("function_name_collision"));
    assert!(context.contains("\"tool\":\"a.b\"") && context.contains("\"tool\":\"a__b\""));
    let capabilities = context
        .split("<<feedback kind=\"capabilities\">>")
        .nth(1)
        .unwrap();
    let capabilities = capabilities.split("<</feedback>>").next().unwrap();
    assert!(!capabilities.contains("a__b"), "{capabilities}");
}

/// REQ-MDL-009: a transport that cannot be settled (a configured
/// `tool_calls` the engine refuses) halts the task `backend_unavailable`; no
/// request is made, in particular none in the envelope transport.
#[test]
fn req_mdl_009_an_unsettled_transport_halts_without_a_fallback() {
    let backend = Arc::new(ScriptedBackend::with_decisions(vec![act(
        "read_file",
        "a.txt",
    )]));
    backend.fail_transport(BackendError::Unavailable {
        detail: "tool_calls_refused: HTTP 500: tools param requires --jinja flag".into(),
    });
    let h = Harness::with_backend(
        backend,
        Policy::shipped(),
        vec![],
        crate::limits::LIM_C1_CONTEXT_CHARS,
    );
    let id = h.create();
    let result = h.step(id);
    assert_eq!(result.outcome, StepOutcome::Halted);
    assert_eq!(
        result.state,
        TaskState::Halted {
            halt_reason: HaltReason::BackendUnavailable
        }
    );
    assert!(
        h.backend.requests().is_empty(),
        "no request went out, in either transport"
    );
    use crate::spi::Persistence;
    let steps = h.persistence.load_steps(id).unwrap();
    assert_eq!(steps.len(), 1);
    assert!(
        steps[0].measures.is_none(),
        "no request was assembled, so there is nothing to measure (and no zero in the averages)"
    );
}
