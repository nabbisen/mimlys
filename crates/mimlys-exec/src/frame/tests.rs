use super::*;

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Body {
    word: String,
}

fn body() -> Body {
    Body {
        word: "hello".into(),
    }
}

/// External design §13.1, §19.5: the framing is one piece of code for two protocols; a frame
/// carries the version the writer names, with the body's fields beside it, and is read only as
/// that version.
#[test]
fn ed_19_5_a_frame_carries_the_version_of_its_protocol() {
    let mut buf = Vec::new();
    write_frame_versioned(&mut buf, 7, &body(), 1024).unwrap();
    // A big-endian length, then the JSON with the version and the body's fields side by side.
    let len = u32::from_be_bytes(buf[..4].try_into().unwrap()) as usize;
    assert_eq!(len, buf.len() - 4);
    let json: serde_json::Value = serde_json::from_slice(&buf[4..]).unwrap();
    assert_eq!(json, serde_json::json!({"version": 7, "word": "hello"}));
    let back: Body = read_frame_versioned(&mut std::io::Cursor::new(&buf), 7, 1024).unwrap();
    assert_eq!(back, body());
    // Another version is that version, whatever the body holds.
    assert!(matches!(
        read_frame_versioned::<Body>(&mut std::io::Cursor::new(&buf), 8, 1024),
        Err(FrameError::Version(7))
    ));
    assert!(matches!(
        read_frame_versioned::<serde_json::Value>(&mut std::io::Cursor::new(&buf), 8, 1024),
        Err(FrameError::Version(7))
    ));
}

/// The limit is the caller's: a body above it is not written, a frame that declares more is not
/// read, and a closed stream is the end of the stream.
#[test]
fn ed_19_5_the_limit_is_the_callers_and_the_end_of_the_stream_is_reported() {
    let mut buf = Vec::new();
    assert!(matches!(
        write_frame_versioned(&mut buf, 1, &body(), 10),
        Err(FrameError::TooLarge { limit: 10, .. })
    ));
    assert!(buf.is_empty(), "nothing of a refused body is written");
    write_frame_versioned(&mut buf, 1, &body(), 1024).unwrap();
    assert!(matches!(
        read_frame_versioned::<Body>(&mut std::io::Cursor::new(&buf), 1, 10),
        Err(FrameError::TooLarge { limit: 10, .. })
    ));
    assert!(matches!(
        read_frame_versioned::<Body>(&mut std::io::Cursor::new(Vec::new()), 1, 1024),
        Err(FrameError::Eof)
    ));
    // A frame that declares more than it holds is an error of the stream, not a hang.
    let cut = &buf[..buf.len() - 2];
    assert!(matches!(
        read_frame_versioned::<Body>(&mut std::io::Cursor::new(cut), 1, 1024),
        Err(FrameError::Io(_))
    ));
}
