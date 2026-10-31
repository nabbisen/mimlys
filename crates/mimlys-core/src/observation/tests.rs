use super::*;
use crate::action::Termination;
use crate::clock::{Clock, FakeClock};
use crate::ids::ScriptedIds;

/// REQ-CTX-008 / THR-21: invisible and control characters are removed and counted; tabs and newlines stay.
#[test]
fn req_ctx_008_sanitize_removes_invisible_characters() {
    let hostile = "ok\u{200B}\u{202E}\u{E0041}\u{FEFF}\x1b[31mred\x1b[0m\tline\nnext";
    let (clean, removed) = sanitize(hostile);
    assert_eq!(clean, "ok[31mred[0m\tline\nnext");
    assert_eq!(removed, 6);
    assert_eq!(sanitize("plain"), ("plain".to_owned(), 0));
}

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
