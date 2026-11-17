use super::*;

fn size(value: &Value) -> usize {
    serde_json::to_string(value).unwrap().chars().count()
}

/// REQ-MCP-006: the counter measures what the serialization holds, in
/// characters and in bytes, escapes and multi-byte characters included.
#[test]
fn req_mcp_006_the_size_counts_the_serialization() {
    let value = json!({"a": "é\n\"€", "b": [1, 2.5, null, true], "c€": {}});
    let text = serde_json::to_string(&value).unwrap();
    assert_eq!(serialized_size(&value, Unit::Chars), text.chars().count());
    assert_eq!(serialized_size(&value, Unit::Bytes), text.len());
    assert_ne!(text.chars().count(), text.len());
}

/// REQ-MCP-006: a result within the bound is not touched, with or without keys
/// to bound.
#[test]
fn req_mcp_006_a_result_within_the_bound_is_left_as_it_is() {
    let value = json!({"a": "x".repeat(100), "b": vec!["y"; 10]});
    assert!(bound(&value, 8000, Unit::Chars, true).is_none());
    assert!(bound(&value, size(&value), Unit::Chars, true).is_none());
    assert!(bound(&value, size(&value) - 1, Unit::Chars, true).is_some());
}

/// REQ-MCP-006, REQ-CTX-009: whatever the shape, the result fits and is valid
/// JSON; the longest strings are cut first, to one length, and a string no
/// longer than that stays whole. The total is the length of what was cut.
#[test]
fn req_mcp_006_the_longest_strings_are_cut_first_to_one_length() {
    let value = json!({
        "short": "keep me whole",
        "mid": "m".repeat(300),
        "long": ["l".repeat(20_000), "L".repeat(30_000)],
    });
    let cut = bound(&value, 2000, Unit::Chars, true).expect("over the bound");
    assert!(size(&cut.value) <= 2000, "{}", size(&cut.value));
    assert_eq!(cut.value["short"], "keep me whole");
    let long = cut.value["long"].as_array().unwrap();
    assert!(
        long.iter()
            .all(|s| s.as_str().unwrap().contains("characters omitted"))
    );
    // The same cap: the two long strings come out the same length.
    assert_eq!(
        long[0].as_str().unwrap().chars().count(),
        long[1].as_str().unwrap().chars().count()
    );
    // The middle one is cut only if the cap is under it.
    let mid = cut.value["mid"].as_str().unwrap();
    assert!(mid.chars().count() <= 300);
    let cut_mid = u64::from(mid.contains("characters omitted")) * 300;
    assert_eq!(cut.total_chars, 50_000 + cut_mid);
    // The bound is used: the cap is the largest that fits, within a character
    // or so of each of the strings cut.
    assert!(size(&cut.value) > 2000 - 10, "{}", size(&cut.value));
}

/// REQ-MCP-006: keys over LIM-I5 are cut, and two that come to one text keep
/// both values.
#[test]
fn req_mcp_006_keys_over_lim_i5_are_cut_and_none_is_lost() {
    let long = "k".repeat(300);
    let value = json!({
        format!("{long}a"): 1,
        format!("{long}b"): 2,
        "n": {format!("{long}c"): 3},
    });
    let cut = bound(&value, 8000, Unit::Chars, true).expect("keys are cut");
    let object = cut.value.as_object().unwrap();
    assert_eq!(object.len(), 3, "{object:?}");
    assert!(object.keys().all(|k| k.chars().count() <= 210));
    assert_eq!(cut.total_chars, 903);
    // Without `keys` they are left.
    assert!(bound(&value, 8000, Unit::Chars, false).is_none());
}

/// REQ-MCP-006: a result that is structure past the bound (more keys, numbers
/// and brackets than the bound holds, with no string to cut) is replaced by an
/// object that says its size: still valid JSON, still within the bound.
#[test]
fn req_mcp_006_structure_past_the_bound_is_replaced_by_its_size() {
    let value: Value = (0..5000)
        .map(|i| (format!("k{i}"), json!(i)))
        .collect::<Map<_, _>>()
        .into();
    let whole = size(&value) as u64;
    let cut = bound(&value, 8000, Unit::Chars, true).expect("over the bound");
    assert!(size(&cut.value) <= 8000);
    assert_eq!(cut.total_chars, whole);
    assert_eq!(cut.value["total_chars"], whole);
    let numbers = json!(vec![123_456_789u64; 3000]);
    let cut = bound(&numbers, 8000, Unit::Chars, true).expect("over the bound");
    assert!(cut.value.is_object() && size(&cut.value) <= 8000);
}

/// REQ-PERS-005: the stored copy is bounded in bytes, so a string of two-byte
/// characters is cut at the bytes, and the result still fits.
#[test]
fn req_pers_005_the_whole_result_is_bounded_in_bytes() {
    let value = json!({"a": "é".repeat(600_000), "b": "ok"});
    let limit = 1024 * 1024;
    assert!(serialized_size(&value, Unit::Bytes) > limit);
    let cut = bound(&value, limit, Unit::Bytes, false).expect("over the bound");
    assert!(serialized_size(&cut.value, Unit::Bytes) <= limit);
    assert_eq!(cut.value["b"], "ok");
}

/// REQ-CTX-008, REQ-CTX-007: cleaning cuts nothing: it removes the invisible
/// characters of strings and keys, counts them, and keeps the rest whole.
#[test]
fn req_ctx_008_cleaning_removes_and_counts_and_cuts_nothing() {
    let mut value = json!({
        "a\u{202E}b": "x\u{200B}y",
        "ab": "z".repeat(100_000),
        "n": ["\u{E0041}q"],
    });
    let mut cleaned = Cleaned::default();
    clean(&mut value, &Redactor::default(), &mut cleaned);
    assert_eq!(cleaned.removed, 3);
    let object = value.as_object().unwrap();
    assert_eq!(object.len(), 3);
    assert_eq!(value["ab"], "z".repeat(100_000));
    assert_eq!(value["ab#2"], "xy");
    assert_eq!(value["n"][0], "q");
}

/// REQ-MCP-006 (architect's review of 009i): strings of every length near a
/// probed cap are bounded. A cut that drops only a few more characters than its
/// marker holds is longer serialized than the text it replaced; the size of a
/// probe must still be computed, not underflow.
#[test]
fn req_mcp_006_a_cut_longer_serialized_than_its_text_is_counted() {
    let value = Value::Array((1..=600).map(|n| Value::String("a".repeat(n))).collect());
    for unit in [Unit::Chars, Unit::Bytes] {
        for limit in [50_000, 100_000, 150_000] {
            let cut = bound(&value, limit, unit, true).expect("over the bound");
            assert!(
                serialized_size(&cut.value, unit) <= limit,
                "{unit:?} {limit}"
            );
            assert!(cut.value.is_array(), "the strings are cut, not replaced");
        }
    }
}
