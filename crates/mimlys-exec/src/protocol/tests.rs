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
        assert_eq!(json["version"], 1);
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
        engine_version: "50.0.1".into(),
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

/// External design §13.1: a frame is a big-endian length and a JSON body; an
/// oversized or foreign-version frame is refused; end of stream is reported.
#[test]
fn ed_13_1_framing() {
    let mut buf = Vec::new();
    write_frame(&mut buf, &Request::Probe).unwrap();
    assert_eq!(&buf[..4], &(buf.len() as u32 - 4).to_be_bytes());
    let mut cursor = std::io::Cursor::new(buf.clone());
    let back: Request = read_frame(&mut cursor).unwrap();
    assert_eq!(back, Request::Probe);
    assert!(matches!(
        read_frame::<Request>(&mut cursor),
        Err(FrameError::Eof)
    ));
    let mut big = (MAX_FRAME_BYTES as u32 + 1).to_be_bytes().to_vec();
    big.extend_from_slice(b"{}");
    assert!(matches!(
        read_frame::<Request>(&mut std::io::Cursor::new(big)),
        Err(FrameError::TooLarge { .. })
    ));
    let foreign =
        serde_json::to_vec(&serde_json::json!({"version": 2, "request": "probe"})).unwrap();
    let mut framed = (foreign.len() as u32).to_be_bytes().to_vec();
    framed.extend_from_slice(&foreign);
    assert!(matches!(
        read_frame::<Request>(&mut std::io::Cursor::new(framed)),
        Err(FrameError::Version(2))
    ));
}
