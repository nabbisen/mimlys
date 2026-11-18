//! The last bound of every observation (handoff 013c, item 1; LIM-I19;
//! REQ-CTX-006): whatever provider made the payload, the observation holds it as
//! the model reads it, within LIM-I19, with one `content_truncated`; a provider
//! that bounded its own result first is not cut again; the whole is kept for the
//! user.

use super::*;
use crate::bound::{Unit, serialized_size};
use crate::limits::LIM_I19_RESULT_TOTAL_CHARS;
use crate::observation::Notice;

const LIM: usize = LIM_I19_RESULT_TOTAL_CHARS;

/// A step whose tool answers with `outcome`, and the observation it left.
fn observed(outcome: ExecutionOutcome) -> crate::observation::Observation {
    let h = Harness::new(vec![act("read_file", "src/lib.rs")]);
    h.tools.script("read_file", outcome);
    let id = h.create();
    h.step(id);
    let mut observations = h.persistence.load_observations(id).unwrap();
    assert_eq!(observations.len(), 1);
    observations.remove(0)
}

fn truncated_notices(notices: &[Notice]) -> Vec<u64> {
    notices
        .iter()
        .filter_map(|n| match n {
            Notice::ContentTruncated { total_chars } => Some(*total_chars),
            _ => None,
        })
        .collect()
}

/// REQ-CTX-006, LIM-I19: a provider that forgot its own bound (a fake that
/// answers with 40 000 characters in a payload) has its payload bounded by the
/// runtime: within LIM-I19, valid, the short string whole, one `content_truncated`
/// with the length that was cut, and the whole result kept for the user.
#[test]
fn req_ctx_006_a_payload_over_lim_i19_is_bounded_by_the_runtime() {
    let payload = json!({
        "text": "a".repeat(40_000),
        "note": "keep me",
        "list": vec!["b".repeat(3_000); 5],
    });
    let o = observed(ExecutionOutcome::succeeded(payload.clone(), 3));
    assert!(serialized_size(&o.payload, Unit::Chars) <= LIM);
    assert_eq!(o.payload["note"], "keep me");
    assert!(o.payload["text"].as_str().unwrap().chars().count() < 40_000);
    // Said once, with the full length of what was cut.
    assert_eq!(truncated_notices(&o.notices), [40_000 + 5 * 3_000]);
    // The user's copy is the whole payload.
    assert_eq!(o.full_payload, Some(payload));
}

/// REQ-CTX-006: a payload within LIM-I19 is not touched: no notice, no second copy.
#[test]
fn req_ctx_006_a_payload_within_lim_i19_is_not_touched() {
    let payload = json!({"text": "a".repeat(LIM - 100)});
    let o = observed(ExecutionOutcome::succeeded(payload.clone(), 3));
    assert_eq!(o.payload, payload);
    assert!(truncated_notices(&o.notices).is_empty());
    assert_eq!(o.full_payload, None);
}

/// REQ-CTX-006: a failure's payload is bounded as well, whatever the provider.
#[test]
fn req_ctx_006_a_failures_payload_is_bounded_too() {
    let o = observed(ExecutionOutcome::tool_error(
        "boom",
        json!({"detail": "x".repeat(50_000)}),
        3,
    ));
    assert!(serialized_size(&o.payload, Unit::Chars) <= LIM);
    assert_eq!(truncated_notices(&o.notices).len(), 1);
}

/// REQ-CTX-006: a result a tool bounded and said (its own `content_truncated`) is
/// not said twice, and a copy the provider kept for the user is kept as it was.
#[test]
fn req_ctx_006_what_the_tool_said_is_not_said_twice() {
    let mut outcome = ExecutionOutcome::succeeded(json!({"text": "c".repeat(30_000)}), 3);
    outcome.notices.push(Notice::ContentTruncated {
        total_chars: 123_456,
    });
    outcome.full_payload = Some(json!({"text": "the provider's own whole copy"}));
    let o = observed(outcome);
    assert!(serialized_size(&o.payload, Unit::Chars) <= LIM);
    assert_eq!(truncated_notices(&o.notices), [123_456]);
    assert_eq!(
        o.full_payload,
        Some(json!({"text": "the provider's own whole copy"}))
    );
}

/// REQ-CTX-007, REQ-CTX-006: a secret that lies across the point where the
/// runtime cuts a payload leaves nothing of itself, not even a part: the payload
/// is redacted whole before it is cut. The cut point is read off the cut of a
/// probe of the same size (the head depends on the digits of the size alone).
#[test]
fn req_ctx_007_a_secret_across_the_runtimes_cut_leaves_nothing() {
    let value = "sk-live-0123456789abcdef";
    let probe = json!({"text": "a".repeat(76_000)});
    let cut = crate::bound::bound(&probe, LIM, Unit::Chars, true).expect("over the bound");
    let head = cut.value["text"]
        .as_str()
        .and_then(|t| t.find('\n'))
        .expect("a marker");
    // The secret starts a few characters before the head ends.
    let mut text = "a".repeat(head - 6);
    text.push_str(value);
    text.push_str(&"b".repeat(76_000 - text.len()));
    assert_eq!(text.chars().count(), 76_000);
    let h = Harness::with_context(
        vec![act("read_file", "src/lib.rs")],
        Policy::shipped(),
        vec![("TOKEN".into(), value.into())],
        crate::limits::LIM_C1_CONTEXT_CHARS,
    );
    h.tools.script(
        "read_file",
        ExecutionOutcome::succeeded(json!({"text": text}), 3),
    );
    let id = h.create();
    h.step(id);
    let o = h.persistence.load_observations(id).unwrap().remove(0);
    let shown = serde_json::to_string(&o.payload).unwrap();
    assert!(serialized_size(&o.payload, Unit::Chars) <= LIM);
    assert!(!shown.contains(value));
    for part in [&value[..6], &value[value.len() - 6..]] {
        assert!(!shown.contains(part), "{part} is left of the secret");
    }
    let whole = serde_json::to_string(&o.full_payload).unwrap();
    assert!(!whole.contains(value), "the user's copy is redacted too");
    assert_eq!(truncated_notices(&o.notices).len(), 1);
}
