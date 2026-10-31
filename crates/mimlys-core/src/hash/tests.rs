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
