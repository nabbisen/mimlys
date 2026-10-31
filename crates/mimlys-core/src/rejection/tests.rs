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
