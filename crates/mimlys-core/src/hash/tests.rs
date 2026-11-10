use super::*;
use serde_json::json;

/// LIM-U6: key order and whitespace do not change the canonical form or the hash.
#[test]
fn lim_u6_key_order_and_whitespace_do_not_matter() {
    let a: Value =
        serde_json::from_str(r#"{ "b": [1, 2, {"z": null, "y": true}], "a": "x" }"#).unwrap();
    let b: Value = serde_json::from_str(r#"{"a":"x","b":[1,2,{"y":true,"z":null}]}"#).unwrap();
    assert_eq!(
        canonical_json(&a),
        r#"{"a":"x","b":[1,2,{"y":true,"z":null}]}"#
    );
    assert_eq!(hash_canonical(&a), hash_canonical(&b));
}

/// LIM-U6: Unicode normalization does not change the hash; a changed value does.
#[test]
fn lim_u6_nfc_normalization_and_value_sensitivity() {
    let nfd = json!({"k\u{0065}\u{0301}": "caf\u{0065}\u{0301}"});
    let nfc = json!({"k\u{00e9}": "caf\u{00e9}"});
    assert_eq!(hash_canonical(&nfd), hash_canonical(&nfc));
    assert_eq!(canonical_json(&nfd), "{\"k\u{00e9}\":\"caf\u{00e9}\"}");
    let other = json!({"k\u{00e9}": "cafe"});
    assert_ne!(hash_canonical(&nfc), hash_canonical(&other));
}

/// LIM-U6: the digest is SHA-256 in lowercase hexadecimal, 64 characters, with a known vector.
#[test]
fn lim_u6_digest_is_sha256_hex() {
    let h = hash_bytes(b"abc");
    assert_eq!(
        h.as_str(),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(Hash::from_hex(h.as_str()), Some(h.clone()));
    assert_eq!(Hash::from_hex("ABC"), None);
    assert_eq!(serde_json::to_string(&h).unwrap(), format!("\"{h}\""));
}

/// LIM-U6: strings are escaped as JSON; a key order by code point is byte order.
#[test]
fn lim_u6_canonical_escaping_and_code_point_order() {
    let v = json!({"ß": 1, "a": "quote\"and\nnewline", "Z": 2});
    assert_eq!(
        canonical_json(&v),
        r#"{"Z":2,"a":"quote\"and\nnewline","ß":1}"#
    );
}

/// LIM-U6, REQ-SEC-012 (handoff 008c): every digest the product computes is
/// byte-identical to the one 0.9.0 computed, whatever `sha2` it is built on.
/// The values were produced by the code of the tag `0.9.0` (`sha2` 0.10).
#[test]
fn lim_u6_golden_hashes_of_0_9_0() {
    let big: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
    assert_eq!(
        hash_bytes(b"").as_str(),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    assert_eq!(
        hash_bytes(&big).as_str(),
        "e24bc62381f1224fbbb74688663f8f9743b9680b193edd666835e97b06e730eb"
    );
    assert_eq!(
        hash_bytes("mimlys \u{00e9}\u{4e16}\u{754c}".as_bytes()).as_str(),
        "2d609ac48351f67c8d5c148bbf1b436ac9a728aeadb9130793017a706d404051"
    );
    let value = json!({"z": [1, 2.5, null, true], "a": {"k\u{00e9}": "caf\u{00e9}"}, "n": -7});
    assert_eq!(
        canonical_json(&value),
        "{\"a\":{\"k\u{00e9}\":\"caf\u{00e9}\"},\"n\":-7,\"z\":[1,2.5,null,true]}"
    );
    assert_eq!(
        hash_canonical(&value).as_str(),
        "3796e4ae6b8fff6038fa1ad274ba856fc6719252fd5a5705e9d621309208fc62"
    );
    // A decision's canonical hash (REQ-DEC-012).
    let decision = crate::decision::parse_strict(
        br#"{"decision":"act","tool":"read_file","arguments":{"path":"a.txt"}}"#,
    )
    .unwrap();
    assert_eq!(
        decision.hash().as_str(),
        "cfada2ba3eb4f03404577aa4f31ad728ccca4ef61b95fb10fc5b82b1e5304191"
    );
    // The shipped policy's hash (REQ-CAP-010).
    assert_eq!(
        crate::policy::Policy::shipped().hash().as_str(),
        "3ba07e708bbb090b75238ec3e2b67da882d7afda22e7adff2e83237eabab3ab0"
    );
}
