use super::*;
use mimlys_core::action::EffectKind;
use mimlys_core::ids::ScriptedIds;

/// External design §13.1: every message round-trips through JSON with its version and tag.
#[test]
fn ed_13_1_protocol_messages_round_trip() {
    let ids = ScriptedIds::default();
    let requests = vec![
        Request::Probe,
        Request::Execute {
            execution_id: ExecutionId::new(&ids),
            action_id: ActionId::new(&ids),
            tool: "run_script".into(),
            arguments: serde_json::json!({"language": "javascript", "source": "1"}),
            effect: EffectView {
                kind: EffectKind::WorkspaceWrite,
                reversible: true,
                idempotent: false,
            },
            deadline_ms: 30_000,
            workspace_root: "/w".into(),
            read_only: false,
            replay: Some(Replay {
                seed: vec![1, 2, 3],
                clock_ms: 1_700_000_000_000,
            }),
        },
        Request::Cancel {
            execution_id: ExecutionId::new(&ids),
        },
        Request::Shutdown,
    ];
    for body in requests {
        let frame = Frame {
            version: PROTOCOL_VERSION,
            body: body.clone(),
        };
        let json = serde_json::to_value(&frame).unwrap();
        assert_eq!(json["version"], PROTOCOL_VERSION);
        assert!(json.get("request").is_some());
        let back: Frame<Request> = serde_json::from_value(json).unwrap();
        assert_eq!(back.body, body);
    }
    let mut clauses = std::collections::BTreeMap::new();
    clauses.insert("N".to_owned(), ClauseState::ENFORCED);
    clauses.insert(
        "M".to_owned(),
        ClauseState::Unavailable {
            reason: "no_cgroup".into(),
        },
    );
    let response = Response::ProbeResult {
        clauses,
        engine_version: "50.0.3".into(),
        interpreters: Default::default(),
        interpreter_errors: Default::default(),
        cgroup: None,
    };
    let json = serde_json::to_value(&response).unwrap();
    assert_eq!(json["response"], "probe_result");
    assert_eq!(json["clauses"]["N"]["state"], "enforced");
    let back: Response = serde_json::from_value(json).unwrap();
    assert_eq!(back, response);
}

/// External design §13.1: a frame is a big-endian length and a JSON body; a frame above the
/// limit in force or of a foreign version is refused; end of stream is reported.
#[test]
fn ed_13_1_framing() {
    let limit = frame_limit(mimlys_core::limits::LIM_S4_OUTPUT_BYTES as usize);
    let mut buf = Vec::new();
    write_frame(&mut buf, &Request::Probe, limit).unwrap();
    assert_eq!(&buf[..4], &(buf.len() as u32 - 4).to_be_bytes());
    let mut cursor = std::io::Cursor::new(buf.clone());
    let back: Request = read_frame(&mut cursor, limit).unwrap();
    assert_eq!(back, Request::Probe);
    assert!(matches!(
        read_frame::<Request>(&mut cursor, limit),
        Err(FrameError::Eof)
    ));
    let mut big = (limit as u32 + 1).to_be_bytes().to_vec();
    big.extend_from_slice(b"{}");
    assert!(matches!(
        read_frame::<Request>(&mut std::io::Cursor::new(big), limit),
        Err(FrameError::TooLarge { .. })
    ));
    let foreign = framed(&serde_json::json!({"version": 3, "request": "probe"}));
    assert!(matches!(
        read_frame::<Request>(&mut std::io::Cursor::new(foreign), limit),
        Err(FrameError::Version(3))
    ));
}

/// A JSON value as the bytes of a frame.
fn framed(value: &serde_json::Value) -> Vec<u8> {
    let body = serde_json::to_vec(value).unwrap();
    let mut bytes = (body.len() as u32).to_be_bytes().to_vec();
    bytes.extend_from_slice(&body);
    bytes
}

fn record(stdout: Vec<u8>, stderr: Vec<u8>) -> Response {
    Response::ExecutionRecord {
        execution_id: ExecutionId::new(&ScriptedIds::default()),
        termination: mimlys_core::action::Termination::Exited { exit_code: 0 },
        payload: serde_json::json!({"termination": "exited", "exit_code": 0}),
        stdout,
        stderr,
        notices: vec![],
        metrics: Default::default(),
    }
}

/// Every byte value, `n` bytes long: text, controls and bytes that are no UTF-8.
fn every_byte(n: usize) -> Vec<u8> {
    (0..n).map(|i| (i % 256) as u8).collect()
}

/// External design §13.1 (handoff 004d-4): a captured stream travels as base64 text, at about a
/// byte a byte (never as an array of numbers, which is three to four characters a byte), and
/// comes back exactly, whatever the bytes are: no UTF-8 is assumed.
#[test]
fn ed_13_1_a_stream_travels_as_base64_text_at_about_a_byte_a_byte() {
    const MIB: usize = 1 << 20;
    let out = every_byte(MIB);
    let err = vec![0xff, 0xfe, 0x00, b'\r', b'\n'];
    let response = record(out.clone(), err.clone());
    let json = serde_json::to_value(&response).unwrap();
    assert!(json["stdout"].is_string(), "text, not an array of numbers");
    assert_eq!(json["stdout"].as_str().unwrap().len(), encoded_len(MIB));
    let mut buf = Vec::new();
    write_frame(&mut buf, &response, frame_limit(MIB)).unwrap();
    // 4/3 of the stream, and the record's own few hundred bytes.
    assert!(
        buf.len() < encoded_len(MIB) + 1024,
        "{} bytes for a stream of {MIB}",
        buf.len()
    );
    let back: Response = read_frame(&mut std::io::Cursor::new(buf), frame_limit(MIB)).unwrap();
    match back {
        Response::ExecutionRecord { stdout, stderr, .. } => {
            assert_eq!(stdout, out);
            assert_eq!(stderr, err);
        }
        other => panic!("{other:?}"),
    }
}

/// A stream that is not base64 is refused as a malformed frame, not read as something else.
#[test]
fn ed_13_1_a_stream_that_is_not_base64_is_refused() {
    let mut json = serde_json::to_value(record(vec![1, 2, 3], vec![])).unwrap();
    json["stdout"] = serde_json::json!("not base64 !!");
    assert!(matches!(
        read_frame::<Response>(&mut std::io::Cursor::new(framed(&json)), 1 << 20),
        Err(FrameError::Malformed(_))
    ));
}

/// External design §13.1 (handoff 004d-4): the limit is derived from the bounds in force on both
/// sides: both streams at the larger of LIM-S4 (as configured) and LIM-H4, encoded, and the
/// record's margin. A record whose two streams are at their bound is carried, and what is above
/// the limit is refused before a byte is written.
#[test]
fn ed_13_1_the_frame_limit_follows_the_bounds_in_force() {
    use mimlys_core::limits::LIM_H4_HOST_OUTPUT_BYTES as H4;
    const MIB: usize = 1 << 20;
    // The default: the host tool's bound is the larger.
    assert_eq!(frame_limit(MIB), 2 * encoded_len(H4) + RECORD_MARGIN_BYTES);
    assert_eq!(frame_limit(64 * 1024), frame_limit(MIB), "H4 is the floor");
    // A configured bound above LIM-H4 moves it.
    assert_eq!(
        frame_limit(16 * MIB),
        2 * encoded_len(16 * MIB) + RECORD_MARGIN_BYTES
    );
    // Held within LIM-S4's own range: a wrong flag asks for no more than the largest bound.
    let largest = frame_limit(16 * MIB);
    assert_eq!(
        frame_limit(64 * MIB),
        largest,
        "the old maximum (64 MiB) is held to the new one"
    );
    assert_eq!(frame_limit(usize::MAX), largest);
    assert!(largest <= u32::MAX as usize, "the length prefix holds it");
    // The old constant (twice LIM-S4 and 64 KiB) could not carry LIM-H4 once encoded.
    assert!(frame_limit(MIB) > 2 * 4 * MIB);
    for output_bytes in [MIB, 6 * MIB, 16 * MIB] {
        let bound = output_bytes.max(H4);
        let response = record(vec![b'x'; bound], vec![b'y'; bound]);
        let limit = frame_limit(output_bytes);
        let mut buf = Vec::new();
        write_frame(&mut buf, &response, limit).expect("both streams at their bound fit");
        assert!(buf.len() <= limit + 4);
        let back: Response = read_frame(&mut std::io::Cursor::new(buf), limit).unwrap();
        assert_eq!(back, response);
        // One byte below the frame's size is refused, and nothing is written.
        let size = serde_json::to_vec(&Frame {
            version: PROTOCOL_VERSION,
            body: &response,
        })
        .unwrap()
        .len();
        let mut refused = Vec::new();
        assert!(matches!(
            write_frame(&mut refused, &response, size - 1),
            Err(FrameError::TooLarge { len, limit }) if len == size && limit == size - 1
        ));
        assert!(refused.is_empty(), "a refused frame writes no byte");
    }
}

/// External design §13.1 (handoff 004d-4): the version is read before the body's shape is
/// blamed. A version-1 record (its streams as arrays of numbers) and a version-1 request are
/// refused as `Version(1)`, by name, not as malformed; a frame of the right version whose body
/// is wrong is still malformed.
#[test]
fn ed_13_1_a_frame_of_another_version_is_refused_by_its_version() {
    let old = serde_json::json!({
        "version": 1, "response": "execution_record",
        "execution_id": ExecutionId::new(&ScriptedIds::default()),
        "termination": {"termination": "exited", "exit_code": 0},
        "payload": {}, "stdout": [104, 105], "stderr": [],
        "notices": [], "metrics": {"wall_ms": 0, "stdout_bytes": 2, "stderr_bytes": 0}
    });
    assert!(matches!(
        read_frame::<Response>(&mut std::io::Cursor::new(framed(&old)), 1 << 20),
        Err(FrameError::Version(1))
    ));
    let probe = framed(&serde_json::json!({"version": 1, "request": "probe"}));
    assert!(matches!(
        read_frame::<Request>(&mut std::io::Cursor::new(probe), 1 << 20),
        Err(FrameError::Version(1))
    ));
    let wrong = framed(&serde_json::json!({"version": 2, "request": "nothing"}));
    assert!(matches!(
        read_frame::<Request>(&mut std::io::Cursor::new(wrong), 1 << 20),
        Err(FrameError::Malformed(_))
    ));
}

/// A frame that declares more than it sends ends as a failed read, not as a frame.
#[test]
fn ed_13_1_a_frame_shorter_than_it_declares_is_an_error() {
    let mut bytes = 1000u32.to_be_bytes().to_vec();
    bytes.extend_from_slice(b"{\"version\": 2");
    assert!(matches!(
        read_frame::<Request>(&mut std::io::Cursor::new(bytes), 1 << 20),
        Err(FrameError::Io(_))
    ));
}
