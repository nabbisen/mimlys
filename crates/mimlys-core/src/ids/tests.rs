use super::*;

/// LIM-U5: system identifiers are version 7 and render lowercase hyphenated.
#[test]
fn lim_u5_system_ids_are_v7_lowercase_hyphenated() {
    let id = TaskId::new(&SystemIds);
    assert!(id.is_v7());
    let text = id.to_string();
    assert_eq!(text.len(), 36);
    assert_eq!(text, text.to_lowercase());
    assert_eq!(text.matches('-').count(), 4);
}

/// REQ-ARCH-006: scripted identifiers are deterministic, ordered and still version 7.
#[test]
fn req_arch_006_scripted_ids_are_deterministic_and_v7() {
    let a = ScriptedIds::starting_at(10);
    let b = ScriptedIds::starting_at(10);
    let x1 = ActionId::new(&a);
    let x2 = ActionId::new(&a);
    let y1 = ActionId::new(&b);
    let y2 = ActionId::new(&b);
    assert_eq!(x1, y1);
    assert_eq!(x2, y2);
    assert_ne!(x1, x2);
    assert!(x1 < x2, "version 7 identifiers are time-ordered");
    assert!(x1.is_v7() && x2.is_v7());
}

/// LIM-U5: identifiers serialize as their string form and parse back.
#[test]
fn lim_u5_ids_round_trip_through_serde() {
    let id = StepId::new(&ScriptedIds::default());
    let json = serde_json::to_string(&id).unwrap();
    assert_eq!(json, format!("\"{id}\""));
    let back: StepId = serde_json::from_str(&json).unwrap();
    assert_eq!(back, id);
    assert_eq!(StepId::from_uuid(id.as_uuid()), id);
}
