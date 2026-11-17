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
        full_payload: None,
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
            full_payload: None,
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
        // The whole result the user keeps is redacted as the payload is.
        full_payload: Some(serde_json::json!({"whole": "pa$$word-123"})),
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

/// One class of invisible characters (REQ-CTX-008; handoff 009h, item 2): every
/// character of each range is removed and counted, the character just before and
/// the one just after the range are not (they are drawn, or belong to another
/// class and are named), and the text around is kept.
fn removes_the_class(ranges: &[(u32, u32)], kept_around: &[u32]) {
    for &(first, last) in ranges {
        for code in first..=last {
            let c = char::from_u32(code).unwrap();
            let (clean, removed) = sanitize(&format!("a{c}b"));
            assert_eq!((clean.as_str(), removed), ("ab", 1), "U+{code:04X}");
        }
    }
    for &code in kept_around {
        let c = char::from_u32(code).unwrap();
        let text = format!("a{c}b");
        assert_eq!(sanitize(&text), (text, 0), "U+{code:04X} is drawn");
    }
}

/// REQ-CTX-008: the soft hyphen and the combining grapheme joiner.
#[test]
fn req_ctx_008_the_soft_hyphen_and_the_grapheme_joiner_are_removed() {
    removes_the_class(
        &[(0x00AD, 0x00AD), (0x034F, 0x034F)],
        &[0x00AC, 0x00AE, 0x034E, 0x0350],
    );
}

/// REQ-CTX-008: the Hangul fillers, the halfwidth one included.
#[test]
fn req_ctx_008_the_hangul_fillers_are_removed() {
    removes_the_class(
        &[(0x115F, 0x1160), (0x3164, 0x3164), (0xFFA0, 0xFFA0)],
        &[0x115E, 0x1161, 0x3163, 0x3165, 0xFF9F, 0xFFA1],
    );
}

/// REQ-CTX-008: the Khmer inherent vowels.
#[test]
fn req_ctx_008_the_khmer_inherent_vowels_are_removed() {
    removes_the_class(&[(0x17B4, 0x17B5)], &[0x17B3, 0x17B6]);
}

/// REQ-CTX-008: the Mongolian free variation selectors, with the vowel
/// separator that was already removed.
#[test]
fn req_ctx_008_the_mongolian_selectors_are_removed() {
    removes_the_class(&[(0x180B, 0x180F)], &[0x180A, 0x1810]);
}

/// REQ-CTX-008: the variation selectors, the ones that follow the emoji of a
/// text (U+FE0F) among them, and the supplement behind the tag characters.
#[test]
fn req_ctx_008_the_variation_selectors_are_removed() {
    removes_the_class(&[(0xFE00, 0xFE0F), (0xE0100, 0xE01EF)], &[0xFDFF, 0xFE10]);
    assert_eq!(sanitize("\u{2764}\u{FE0F}"), ("\u{2764}".to_owned(), 1));
}

/// REQ-CTX-008: the specials before the object replacement character
/// (interlinear annotation among them). U+FFFC and U+FFFD are drawn and stay.
#[test]
fn req_ctx_008_the_specials_before_the_object_replacement_are_removed() {
    removes_the_class(&[(0xFFF0, 0xFFFB)], &[0xFFEF, 0xFFFC, 0xFFFD]);
}

/// REQ-CTX-008: the Duployan shorthand format controls.
#[test]
fn req_ctx_008_the_shorthand_format_controls_are_removed() {
    removes_the_class(&[(0x1BCA0, 0x1BCA3)], &[0x1BC9F, 0x1BCA4]);
}

/// REQ-CTX-008: the musical symbol format controls.
#[test]
fn req_ctx_008_the_musical_format_controls_are_removed() {
    removes_the_class(&[(0x1D173, 0x1D17A)], &[0x1D172, 0x1D17B]);
}

/// REQ-CTX-008: the tag characters and everything to the end of their block
/// (U+E0000 to U+E0FFF); the character after it is outside the block and stays.
#[test]
fn req_ctx_008_the_tag_block_is_removed_whole() {
    removes_the_class(&[(0xE0000, 0xE0FFF)], &[0xE1000]);
}

/// REQ-CTX-008: the format characters the lists did not hold: the Arabic number
/// signs and the end of ayah, the Syriac abbreviation mark, the Arabic marks
/// above, the disputed end of ayah and the Kaithi number sign.
#[test]
fn req_ctx_008_the_missing_format_characters_are_removed() {
    removes_the_class(
        &[
            (0x0600, 0x0605),
            (0x06DD, 0x06DD),
            (0x070F, 0x070F),
            (0x0890, 0x0891),
            (0x08E2, 0x08E2),
            (0x110BD, 0x110BD),
        ],
        &[
            0x0606, 0x06DC, 0x06DE, 0x070E, 0x0710, 0x0892, 0x08E1, 0x08E3, 0x110BC, 0x110BE,
        ],
    );
}

/// REQ-CTX-008: the visible symbols stay, whatever their names say: the blank
/// braille pattern and the object replacement character are drawn, as are the
/// ordinary text, the joiner-free emoji and a combining mark.
#[test]
fn req_ctx_008_visible_symbols_stay() {
    for text in [
        "\u{2800}",
        "\u{FFFC}",
        "\u{FFFD}",
        "plain text, tabs\tand\nlines\r\n",
        "\u{1F600}",
        "e\u{0301}",
        "\u{00A0}",
        "\u{3000}",
    ] {
        assert_eq!(sanitize(text), (text.to_owned(), 0), "{text:?}");
    }
}
