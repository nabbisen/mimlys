use super::*;
use crate::action::Termination;
use crate::clock::{Clock, FakeClock};
use crate::ids::ScriptedIds;

/// LIM-C2: excerpts cut at character boundaries, keep head and tail, and report the total.
#[test]
fn lim_c2_excerpt_keeps_head_and_tail() {
    let text: String = (0..100)
        .map(|i| char::from_u32(0x3042 + i).unwrap())
        .collect();
    let (short, total) = excerpt(&text, 60, 40);
    assert_eq!(short, text);
    assert_eq!(total, None);
    let (cut, total) = excerpt(&text, 10, 5);
    assert_eq!(total, Some(100));
    assert!(cut.starts_with(&text.chars().take(10).collect::<String>()));
    assert!(cut.ends_with(&text.chars().skip(95).collect::<String>()));
    assert!(cut.contains("[… 85 characters omitted …]"));
}

/// REQ-CTX-006: invalid UTF-8 is replaced and reported.
#[test]
fn req_ctx_006_decode_lossy_reports_replacement() {
    assert_eq!(decode_lossy(b"abc"), ("abc".to_owned(), false));
    let (text, replaced) = decode_lossy(b"a\xffb");
    assert!(replaced);
    assert_eq!(text, "a\u{FFFD}b");
}

/// External design §8: an observation built from an outcome carries status, cause, excerpts and every notice.
#[test]
fn ed_8_observation_from_outcome() {
    let ids = ScriptedIds::default();
    let big = "x".repeat(LIM_C2_EXCERPT_HEAD_CHARS + LIM_C2_EXCERPT_TAIL_CHARS + 1);
    let outcome = ExecutionOutcome {
        termination: Termination::Exited { exit_code: 1 },
        failure: Some(FailureCause::ToolError {
            detail: "no_match".into(),
        }),
        payload: serde_json::json!({"error": "no_match"}),
        stdout: big.clone().into_bytes(),
        stderr: b"warn\x1b[0m \xff".to_vec(),
        notices: vec![Notice::EntriesTruncated { total: 900 }],
        metrics: ExecutionMetrics::default(),
    };
    let o = Observation::from_outcome(
        ObservationId::new(&ids),
        ActionId::new(&ids),
        "search_files",
        &outcome,
        Verdict {
            status: ObservationStatus::Failed,
            cause: outcome.failure.clone(),
            rolled_back: true,
        },
        FakeClock::default().now(),
        &crate::context::Redactor::default(),
    );
    assert_eq!(o.status, ObservationStatus::Failed);
    assert!(o.stdout.contains("characters omitted"));
    assert_eq!(o.stderr, "warn[0m \u{FFFD}");
    assert!(o.notices.contains(&Notice::EntriesTruncated { total: 900 }));
    assert!(o.notices.contains(&Notice::OutputTruncated {
        stream: Stream::Stdout,
        total_bytes: big.len() as u64
    }));
    assert!(o.notices.contains(&Notice::InvalidUtf8Replaced {
        stream: Stream::Stderr
    }));
    assert!(o.notices.contains(&Notice::CharactersRemoved { count: 1 }));
    assert!(o.notices.contains(&Notice::RolledBack));
    let json = serde_json::to_value(&o).unwrap();
    assert_eq!(json["cause"]["cause"], "tool_error");
    assert_eq!(json["notices"][0]["code"], "entries_truncated");
    let back: Observation = serde_json::from_value(json).unwrap();
    assert_eq!(back, o);
}

/// REQ-CTX-007 (handoff 009g, item 5): a stream is redacted whole before it is
/// cut, so that no part of a secret that lies across the cut is left in the
/// excerpt, at the end of the head or at the start of the tail; the notice says
/// which secret was found, once.
#[test]
fn req_ctx_007_a_secret_across_the_cut_leaves_no_part_of_itself() {
    let secret = "tok-9f8e7d6c5b4a";
    let redactor = crate::context::Redactor::new(&[("servers.x.env.TOKEN".into(), secret.into())]);
    let ids = ScriptedIds::default();
    let observe = |text: String| {
        let outcome = ExecutionOutcome {
            termination: Termination::Exited { exit_code: 0 },
            failure: None,
            payload: Value::Null,
            stdout: text.into_bytes(),
            stderr: Vec::new(),
            notices: vec![],
            metrics: ExecutionMetrics::default(),
        };
        Observation::from_outcome(
            ObservationId::new(&ids),
            ActionId::new(&ids),
            "run_script",
            &outcome,
            Verdict {
                status: ObservationStatus::Succeeded,
                cause: None,
                rolled_back: false,
            },
            FakeClock::default().now(),
            &redactor,
        )
    };
    let filler = |n: usize| "a".repeat(n);
    let total = LIM_C2_EXCERPT_HEAD_CHARS + LIM_C2_EXCERPT_TAIL_CHARS + 5000;
    // Across the end of the head: four characters of the secret on its side.
    let across_head = observe(format!(
        "{}{secret}{}",
        filler(LIM_C2_EXCERPT_HEAD_CHARS - 4),
        filler(total)
    ));
    // Across the start of the tail: the last four characters on its side.
    let tail_start = LIM_C2_EXCERPT_TAIL_CHARS;
    let across_tail = observe(format!(
        "{}{secret}{}",
        filler(total),
        filler(tail_start - 4)
    ));
    for o in [across_head, across_tail] {
        assert!(o.stdout.contains("characters omitted"), "it was cut");
        for prefix in [&secret[..4], &secret[secret.len() - 4..]] {
            assert!(!o.stdout.contains(prefix), "{prefix} is in the excerpt");
        }
        assert_eq!(
            o.notices
                .iter()
                .filter(|n| matches!(n, Notice::SecretRedacted { .. }))
                .count(),
            1,
            "{:?}",
            o.notices
        );
    }
}

/// REQ-CTX-007 (handoff 009g, item 5): the redaction of an observation reaches
/// the failure cause, and a secret named by the stream stage is not named twice.
#[test]
fn req_ctx_007_the_cause_of_an_observation_is_redacted() {
    let redactor =
        crate::context::Redactor::new(&[("backend.api_key".into(), "pa$$word-123".into())]);
    let ids = ScriptedIds::default();
    let outcome = ExecutionOutcome {
        termination: Termination::Exited { exit_code: 1 },
        failure: Some(FailureCause::ToolError {
            detail: "denied: pa$$word-123".into(),
        }),
        payload: serde_json::json!({"pa$$word-123": "pa$$word-123"}),
        stdout: b"pa$$word-123".to_vec(),
        stderr: Vec::new(),
        notices: vec![],
        metrics: ExecutionMetrics::default(),
    };
    let mut o = Observation::from_outcome(
        ObservationId::new(&ids),
        ActionId::new(&ids),
        "t",
        &outcome,
        Verdict {
            status: ObservationStatus::Failed,
            cause: outcome.failure.clone(),
            rolled_back: false,
        },
        FakeClock::default().now(),
        &redactor,
    );
    o.redact(&redactor);
    let text = serde_json::to_string(&o).unwrap();
    assert!(!text.contains("pa$$word-123"), "{text}");
    assert_eq!(
        o.notices
            .iter()
            .filter(|n| matches!(n, Notice::SecretRedacted { .. }))
            .count(),
        1,
        "{:?}",
        o.notices
    );
}
