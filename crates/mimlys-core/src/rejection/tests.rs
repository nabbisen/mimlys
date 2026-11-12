use super::*;

/// REQ-DEC-008: exactly eight codes with snake_case names, round-tripping through serde.
#[test]
fn req_dec_008_rejection_codes_are_the_eight_of_ed_6_4() {
    let names: Vec<&str> = RejectionCode::ALL.iter().map(|c| c.as_str()).collect();
    assert_eq!(
        names,
        [
            "malformed",
            "schema",
            "unknown_tool",
            "invalid_argument",
            "denied",
            "state",
            "budget",
            "approval_rejected"
        ]
    );
    for code in RejectionCode::ALL {
        let json = serde_json::to_string(&code).unwrap();
        assert_eq!(json, format!("\"{code}\""));
        let back: RejectionCode = serde_json::from_str(&json).unwrap();
        assert_eq!(back, code);
    }
    assert!(serde_json::from_str::<RejectionCode>("\"timeout\"").is_err());
}

/// REQ-DEC-008 / LIM-I5: every hint comes from the table and fits the limit; details carry fixed keys.
#[test]
fn req_dec_008_hints_and_details() {
    assert!(hints_within_limit());
    let r = Rejection::malformed(MalformedProblem::TooLarge, Some(10));
    let json = serde_json::to_value(&r).unwrap();
    assert_eq!(json["code"], "malformed");
    assert_eq!(json["detail"]["problem"], "too_large");
    assert_eq!(json["detail"]["limit"], 10);
    assert_eq!(json["hint"], RejectionCode::Malformed.hint());
    let r = Rejection::denied(Denial::new(DeniedReason::PathOutsideWorkspace));
    let json = serde_json::to_value(&r).unwrap();
    assert_eq!(json["detail"]["reason"], "path_outside_workspace");
    assert!(json["detail"].get("grant_kind").is_none());
    assert_eq!(json["hint"], DeniedReason::PathOutsideWorkspace.hint());
    // CLR-5: the hint follows the reason, and an argument denial names the position and the bound.
    let r = Rejection::denied(Denial::argument(2, "--all"));
    let json = serde_json::to_value(&r).unwrap();
    assert_eq!(json["detail"]["reason"], "argument_not_allowed");
    assert_eq!(json["detail"]["argument"], 2);
    assert_eq!(json["detail"]["value"], "--all");
    assert_eq!(json["hint"], DeniedReason::ArgumentNotAllowed.hint());
    assert_eq!(
        serde_json::to_value(Rejection::denied(Denial::too_many(2))).unwrap()["detail"]["max"],
        2
    );
    let r = Rejection::schema("$.tool", "string", None);
    assert_eq!(
        serde_json::to_value(&r).unwrap()["detail"]["path"],
        "$.tool"
    );
}

/// The words in backticks of a hint.
fn ticked(hint: &str) -> Vec<&str> {
    hint.split('`').skip(1).step_by(2).collect()
}

/// Every key a detail may carry.
const KEYS: [&str; 13] = [
    "problem",
    "limit",
    "path",
    "expected",
    "allowed",
    "tool",
    "visible_tools",
    "min",
    "max",
    "reason",
    "grant_kind",
    "argument",
    "value",
];

/// REQ-DEC-008, CLR-5: a hint names a key of the detail only when every rejection of its code
/// (and reason) carries that key; the hints of the table are checked against the keys each
/// constructor always sets.
#[test]
fn req_dec_008_every_hint_names_only_keys_the_segment_carries() {
    let carried = |rejection: &Rejection| -> Vec<String> {
        rejection
            .detail
            .to_value()
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect()
    };
    let every = [
        Rejection::malformed(MalformedProblem::NotObject, None),
        Rejection::schema("$.x", "string", None),
        Rejection::new(
            RejectionCode::UnknownTool,
            Detail::UnknownTool {
                tool: "x".into(),
                visible_tools: vec![],
            },
        ),
        Rejection::invalid_argument("$.x", None, None, None),
        Rejection::new(
            RejectionCode::State,
            Detail::State {
                reason: StateReason::NotRunning,
            },
        ),
        Rejection::new(
            RejectionCode::Budget,
            Detail::Budget {
                budget: BudgetKind::Deadline,
            },
        ),
    ];
    for rejection in &every {
        let keys = carried(rejection);
        for word in ticked(rejection.hint) {
            if KEYS.contains(&word) {
                assert!(
                    keys.iter().any(|k| k == word),
                    "{}: the hint names `{word}`, which the detail does not carry",
                    rejection.code
                );
            }
        }
    }
    // A denial: whichever way it is built, the keys it always has.
    let denials = [
        Denial::new(DeniedReason::PathOutsideWorkspace),
        Denial::new(DeniedReason::PathExcluded),
        Denial::new(DeniedReason::GrantMissing),
        Denial::grant(DeniedReason::GrantMissing, "host_tool"),
        Denial::grant(DeniedReason::DestinationNotGranted, "network"),
        Denial::new(DeniedReason::SchemeNotHttps),
        Denial::new(DeniedReason::EffectNotAllowed),
        Denial::new(DeniedReason::PromoteNotAllowed),
        Denial::argument(0, "--all"),
        Denial::too_many(2),
    ];
    for denial in denials {
        let rejection = Rejection::denied(denial);
        let keys = carried(&rejection);
        for word in ticked(rejection.hint) {
            if KEYS.contains(&word) {
                assert!(
                    keys.iter().any(|k| k == word),
                    "{}: the hint names `{word}`, which the detail does not carry",
                    rejection.hint
                );
            }
        }
    }
    // The table: the hints of `schema`, `unknown_tool` and `invalid_argument` name the keys their
    // details always carry.
    assert_eq!(ticked(RejectionCode::Schema.hint()), ["path", "expected"]);
    assert_eq!(ticked(RejectionCode::UnknownTool.hint()), ["visible_tools"]);
    assert_eq!(ticked(RejectionCode::InvalidArgument.hint()), ["path"]);
}

/// REQ-DEC-008, REQ-DEC-009: the body the model reads is `{code, hint, detail}`; a `denied` hint
/// follows its reason, the transport changes `malformed` and `unknown_tool` only, and a rejection
/// stored without a detail has `{code, hint}`.
#[test]
fn req_dec_009_the_feedback_body_is_the_rejections_own() {
    use crate::native::DecisionTransport::{Envelope, ToolCalls};
    let denied = Rejection::denied(Denial::new(DeniedReason::SchemeNotHttps));
    let body = feedback_body(denied.code, Some(&denied.detail), Envelope);
    assert_eq!(
        body,
        json!({
            "code": "denied",
            "hint": DeniedReason::SchemeNotHttps.hint(),
            "detail": {"reason": "scheme_not_https"}
        })
    );
    assert_eq!(body, denied.feedback(Envelope));
    assert_eq!(
        denied.feedback(ToolCalls),
        body,
        "a denial reads the same in both"
    );
    let unknown = Rejection::new(
        RejectionCode::UnknownTool,
        Detail::UnknownTool {
            tool: "x".into(),
            visible_tools: vec!["read_file".into()],
        },
    );
    assert_eq!(
        unknown.feedback(Envelope)["hint"],
        RejectionCode::UnknownTool.hint()
    );
    assert_eq!(
        unknown.feedback(ToolCalls)["hint"],
        RejectionCode::UnknownTool.hint_for(ToolCalls)
    );
    assert_eq!(
        feedback_body(RejectionCode::Denied, None, Envelope),
        json!({"code": "denied", "hint": RejectionCode::Denied.hint()}),
        "an older step has the code and its hint"
    );
}

/// REQ-DEC-008: a stored detail reads back as written for every code, only the fixed keys are
/// read, and every string and list is bounded again (LIM-I9).
#[test]
fn req_dec_008_a_stored_detail_reads_back_bounded() {
    let samples = [
        Rejection::malformed(MalformedProblem::TooLarge, Some(10)),
        Rejection::schema("$.a", "string", Some(vec!["x".into(), "y".into()])),
        Rejection::new(
            RejectionCode::UnknownTool,
            Detail::UnknownTool {
                tool: "nope".into(),
                visible_tools: vec!["read_file".into(), "list_dir".into()],
            },
        ),
        Rejection::invalid_argument("$.n", Some(1), Some(9), None),
        Rejection::denied(Denial::grant(
            DeniedReason::DestinationNotGranted,
            "network",
        )),
        Rejection::denied(Denial::argument(1, "--x")),
        Rejection::denied(Denial::too_many(3)),
        Rejection::new(
            RejectionCode::State,
            Detail::State {
                reason: StateReason::PendingUserItem,
            },
        ),
        Rejection::new(
            RejectionCode::Budget,
            Detail::Budget {
                budget: BudgetKind::Workspace,
            },
        ),
    ];
    for r in samples {
        let stored = r.detail.to_value();
        assert_eq!(
            Detail::from_value(r.code, &stored).as_ref(),
            Some(&r.detail)
        );
    }
    // Extra keys are not read; a value of the wrong shape is not a detail.
    let stored = json!({"reason": "path_outside_workspace", "path": "/etc/shadow", "x": 1});
    assert_eq!(
        Detail::from_value(RejectionCode::Denied, &stored)
            .unwrap()
            .to_value(),
        json!({"reason": "path_outside_workspace"})
    );
    assert_eq!(
        Detail::from_value(RejectionCode::Denied, &json!({"reason": 7})),
        None
    );
    assert_eq!(
        Detail::from_value(RejectionCode::Schema, &json!("text")),
        None
    );
    // A budget the code no longer builds is not read.
    assert_eq!(
        Detail::from_value(RejectionCode::Budget, &json!({"budget": "steps"})),
        None
    );
    // Bounds.
    let long = "é".repeat(5_000);
    let stored = json!({"path": long, "expected": long, "allowed": vec![long.clone(); 100]});
    let Some(Detail::Schema {
        path,
        expected,
        allowed,
    }) = Detail::from_value(RejectionCode::Schema, &stored)
    else {
        panic!("a schema detail");
    };
    assert_eq!(path.chars().count(), LIM_I9_DETAIL_CHARS);
    assert_eq!(expected.chars().count(), LIM_I9_DETAIL_CHARS);
    let allowed = allowed.unwrap();
    assert_eq!(allowed.len(), LIM_I9_DETAIL_ITEMS);
    assert!(
        allowed
            .iter()
            .all(|a| a.chars().count() == LIM_I9_DETAIL_CHARS)
    );
}
