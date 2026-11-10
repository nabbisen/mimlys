use super::*;

/// REQ-SBX-007 / REQ-ARCH-007: a module is loaded only when its hash is the
/// configured one; installation verifies before writing.
#[test]
fn req_sbx_007_hash_pinning() {
    let dir = tempfile::tempdir().unwrap();
    let bytes = b"\0asm\x01\0\0\0";
    let hash = sha256_hex(bytes);
    let wrong = "0".repeat(64);
    let err = install(dir.path(), "javascript", &wrong, bytes).unwrap_err();
    assert!(matches!(err, InterpreterError::HashMismatch { .. }));
    assert!(
        std::fs::read_dir(dir.path().join("interpreters"))
            .map(|d| d.count() == 0)
            .unwrap_or(true),
        "nothing written on a mismatch"
    );
    let path = install(dir.path(), "javascript", &hash.to_ascii_uppercase(), bytes).unwrap();
    assert_eq!(path, default_module_path(dir.path(), &hash));
    let configured = Configured {
        language: "javascript".into(),
        module: path.clone(),
        sha256: hash.clone(),
        script_name: "main.js".into(),
    };
    assert_eq!(load_verified(&configured).unwrap(), bytes);
    std::fs::write(&path, b"\0asm\x01\0\0\0tampered").unwrap();
    assert!(matches!(
        load_verified(&configured).unwrap_err(),
        InterpreterError::HashMismatch { .. }
    ));
    let missing = Configured {
        module: dir.path().join("nope.wasm"),
        ..configured
    };
    let err = load_verified(&missing).unwrap_err();
    assert!(
        err.to_string()
            .contains("mimlys interpreters install javascript"),
        "{err}"
    );
}

#[test]
fn pinned_table_is_well_formed() {
    for p in PINNED {
        assert_eq!(p.sha256.len(), 64);
        assert!(
            p.sha256
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
        assert!(p.url.starts_with("https://"));
        assert!(pinned(p.language).is_some());
    }
    assert!(pinned("python").is_none());
}

/// REQ-SBX-001, REQ-SEC-012 (handoff 008c): the digest that pins an interpreter is
/// SHA-256 as 0.9.0 computed it, on a short and on a multi-block input.
#[test]
fn req_sbx_001_golden_sha256_hex_of_0_9_0() {
    let big: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
    assert_eq!(
        sha256_hex(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(
        sha256_hex(&big),
        "e24bc62381f1224fbbb74688663f8f9743b9680b193edd666835e97b06e730eb"
    );
}
