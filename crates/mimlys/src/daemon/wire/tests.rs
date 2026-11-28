use super::*;
use serde_json::json;

/// Reads `value` as a frame of the daemon's protocol and writes `body` as one, and says whether
/// the two are the same JSON: the frame is `version` and the body's own fields.
fn same_on_the_wire<T>(body: &T, value: serde_json::Value)
where
    T: Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
{
    let mut framed = Vec::new();
    write(&mut framed, body).unwrap();
    let written: serde_json::Value = serde_json::from_slice(&framed[4..]).unwrap();
    assert_eq!(written, value, "what is written");
    let back: T = read(&mut std::io::Cursor::new(&framed)).unwrap();
    assert_eq!(&back, body, "what is read back");
    // And what an older release wrote, as it wrote it, is read.
    let mut text = Vec::new();
    let bytes = serde_json::to_vec(&value).unwrap();
    text.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    text.extend_from_slice(&bytes);
    let from_text: T = read(&mut std::io::Cursor::new(text)).unwrap();
    assert_eq!(&from_text, body);
}

/// REQ-SEC-016, external design §19.5: **the daemon's hello is fixed for every version.** A test
/// that fails when a field is renamed or removed: the frame is exactly these fields.
#[test]
fn req_sec_016_the_daemons_hello_has_a_fixed_shape() {
    same_on_the_wire(
        &DaemonHello {
            protocol: "mimlys-daemon".into(),
            hello_version: 1,
            runtime_version: "0.14.0".into(),
            config_hash: "ab".repeat(32),
            salt: "cd".repeat(32),
        },
        json!({
            "version": 1,
            "protocol": "mimlys-daemon",
            "hello_version": 1,
            "runtime_version": "0.14.0",
            "config_hash": "ab".repeat(32),
            "salt": "cd".repeat(32),
        }),
    );
    assert_eq!(PROTOCOL, "mimlys-daemon");
    assert_eq!((HELLO_VERSION, FRAME_VERSION), (1, 1));
}

/// REQ-SEC-016: **the client's hello is fixed for every version**, and carries no secret's value.
#[test]
fn req_sec_016_the_clients_hello_has_a_fixed_shape() {
    let mut secrets = BTreeMap::new();
    secrets.insert("backends.default.api_key".to_owned(), "ef".repeat(32));
    same_on_the_wire(
        &ClientHello {
            runtime_version: "0.14.0".into(),
            config_hash: "ab".repeat(32),
            secrets,
        },
        json!({
            "version": 1,
            "runtime_version": "0.14.0",
            "config_hash": "ab".repeat(32),
            "secrets": {"backends.default.api_key": "ef".repeat(32)},
        }),
    );
}

/// REQ-SEC-016: **the daemon's answer to the hello is fixed for every version**: accepted or not,
/// with the refusal's kind and words, and the refusal of writes beside an accepted connection.
#[test]
fn req_sec_016_the_answer_to_the_hello_has_a_fixed_shape() {
    same_on_the_wire(
        &HelloAnswer {
            accepted: true,
            refusal: None,
            writes_refused: None,
        },
        json!({"version": 1, "accepted": true}),
    );
    same_on_the_wire(
        &HelloAnswer {
            accepted: false,
            refusal: Some(Refusal {
                kind: RefusalKind::Version,
                message: "the daemon runs 0.14.0".into(),
                reference: None,
            }),
            writes_refused: None,
        },
        json!({
            "version": 1,
            "accepted": false,
            "refusal": {"kind": "version", "message": "the daemon runs 0.14.0"},
        }),
    );
    same_on_the_wire(
        &HelloAnswer {
            accepted: true,
            refusal: None,
            writes_refused: Some(Refusal {
                kind: RefusalKind::Secret,
                message: "m".into(),
                reference: Some("backends.default.api_key".into()),
            }),
        },
        json!({
            "version": 1,
            "accepted": true,
            "writes_refused": {
                "kind": "secret",
                "message": "m",
                "reference": "backends.default.api_key",
            },
        }),
    );
    // Every kind a refusal can have, by its word.
    for (kind, word) in [
        (RefusalKind::Version, "version"),
        (RefusalKind::Configuration, "configuration"),
        (RefusalKind::Secret, "secret"),
        (RefusalKind::OwnTree, "own_tree"),
        (RefusalKind::Stopping, "stopping"),
    ] {
        assert_eq!(serde_json::to_value(kind).unwrap(), json!(word));
    }
}

/// REQ-SEC-016, REQ-CLI-008: **`stop` and its answer are fixed for every version**, so that a
/// client of any release can stop a daemon of any release.
#[test]
fn req_sec_016_stop_and_its_answer_have_a_fixed_shape() {
    same_on_the_wire(&Request::Stop, json!({"version": 1, "request": "stop"}));
    same_on_the_wire(
        &Reply::Stopped {
            pid: 4242,
            paused: vec!["task-a".into()],
            queued: vec![],
        },
        json!({
            "version": 1,
            "response": "stopped",
            "pid": 4242,
            "paused": ["task-a"],
            "queued": [],
        }),
    );
}

/// A frame of another `version` than the one that never changes is refused as that, whatever the
/// body holds: a daemon of a future framing is named, not misread.
#[test]
fn req_sec_016_a_frame_of_another_version_is_named() {
    let bytes = serde_json::to_vec(&json!({"version": 2, "request": "stop"})).unwrap();
    let mut framed = (bytes.len() as u32).to_be_bytes().to_vec();
    framed.extend_from_slice(&bytes);
    assert!(matches!(
        read::<Request>(&mut std::io::Cursor::new(framed)),
        Err(FrameError::Version(2))
    ));
}

/// The daemon's frames have a limit of their own, not the worker's.
#[test]
fn ed_19_5_the_daemons_frame_limit_is_its_own() {
    assert_eq!(FRAME_LIMIT, 16 * 1024 * 1024);
    let long = "x".repeat(FRAME_LIMIT);
    let mut out = Vec::new();
    assert!(matches!(
        write(&mut out, &json!({"word": long})),
        Err(FrameError::TooLarge { .. })
    ));
}

/// RFC 4231 test case 2: the keyed hash is HMAC-SHA-256.
#[test]
fn req_sec_016_the_keyed_hash_is_hmac_sha_256() {
    assert_eq!(
        hmac_sha256_hex(b"Jefe", b"what do ya want for nothing?"),
        "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
    );
    // A key longer than a block is hashed first (RFC 4231 test case 6).
    assert_eq!(
        hmac_sha256_hex(
            &[0xaa; 131],
            b"Test Using Larger Than Block-Size Key - Hash Key First"
        ),
        "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
    );
}

/// REQ-SEC-016: a secret is compared by a keyed hash: the same value under the same salt and
/// reference gives the same hash, another value, salt or reference another, and the value is not
/// in it.
#[test]
fn req_sec_016_a_secret_is_compared_by_a_keyed_hash_and_never_sent() {
    let a = keyed_hash("salt-1", "backends.default.api_key", "sk-value-one");
    assert_eq!(
        a,
        keyed_hash("salt-1", "backends.default.api_key", "sk-value-one")
    );
    assert_ne!(
        a,
        keyed_hash("salt-1", "backends.default.api_key", "sk-value-two")
    );
    assert_ne!(
        a,
        keyed_hash("salt-2", "backends.default.api_key", "sk-value-one")
    );
    assert_ne!(
        a,
        keyed_hash("salt-1", "backends.other.api_key", "sk-value-one")
    );
    assert!(!a.contains("sk-value-one"));
    assert_eq!(a.len(), 64);
}

/// The configuration's hash is of the file's bytes; a configuration with no file, and a file that
/// cannot be read, are told from every file and from each other.
#[test]
fn ed_19_5_the_configuration_hash_is_of_the_files_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let (a, b) = (dir.path().join("a.toml"), dir.path().join("b.toml"));
    std::fs::write(&a, "config_version = 1\n").unwrap();
    std::fs::write(&b, "config_version = 1\n# a comment\n").unwrap();
    let hash = |p: Option<&std::path::Path>| config_hash(p);
    assert_eq!(hash(Some(&a)), hash(Some(&a)));
    assert_ne!(hash(Some(&a)), hash(Some(&b)));
    assert_ne!(hash(None), hash(Some(&a)));
    assert_ne!(hash(Some(&dir.path().join("missing.toml"))), hash(None));
}
