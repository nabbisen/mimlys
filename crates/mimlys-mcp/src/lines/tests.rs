use super::*;
use tokio::io::AsyncReadExt;

const LIMIT: usize = 1000;

fn reader<R: AsyncRead + Unpin>(inner: R) -> (BoundedLines<R>, Arc<LineState>) {
    BoundedLines::new(inner, "t", LIMIT)
}

/// Reads everything `reader` gives until it ends, in reads of `step` bytes: what was passed
/// on, and how it ended.
async fn drain<R: AsyncRead + Unpin>(
    mut reader: BoundedLines<R>,
    step: usize,
    most: usize,
) -> (Vec<u8>, std::io::Result<()>) {
    let mut out = Vec::new();
    let mut buf = vec![0u8; step];
    loop {
        match reader.read(&mut buf).await {
            Ok(0) => return (out, Ok(())),
            Ok(n) => out.extend_from_slice(&buf[..n]),
            Err(e) => return (out, Err(e)),
        }
        assert!(out.len() <= most, "the reader passed on {}", out.len());
    }
}

/// REQ-MCP-001, LIM-I22: a message of exactly the bound (its newline not counted) is passed
/// on; one byte more ends the stream with an error, and the bytes of that message are not
/// passed on from the byte past the bound.
#[tokio::test]
async fn req_mcp_001_a_message_of_the_bound_passes_and_one_byte_more_does_not() {
    let mut data = vec![b'a'; LIMIT];
    data.push(b'\n');
    // The reads end at, before and after the bound.
    for step in [1, 64, LIMIT - 1, LIMIT, LIMIT + 1, 4096] {
        let (r, state) = reader(std::io::Cursor::new(data.clone()));
        let (out, end) = drain(r, step, usize::MAX).await;
        assert!(end.is_ok(), "step {step}");
        assert_eq!(out, data, "step {step}");
        assert!(!state.exceeded(), "step {step}");
        assert_eq!(state.high_water(), LIMIT, "step {step}");
    }

    let mut data = vec![b'a'; LIMIT + 1];
    data.push(b'\n');
    let (r, state) = reader(std::io::Cursor::new(data));
    let (out, end) = drain(r, 64, LIMIT).await;
    let error = end.unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    assert!(error.to_string().contains("LIM-I22"), "{error}");
    assert!(state.exceeded());
    assert!(out.len() <= LIMIT, "{}", out.len());
    assert!(state.high_water() <= LIMIT);
}

/// REQ-MCP-001, LIM-I22: a stream that never ends (no newline, for ever) is cut at the bound:
/// the reader passes on at most the bound and then ends with the error, whatever the size of
/// the reads (the bound is across them), and never holds more.
#[tokio::test]
async fn req_mcp_001_a_line_that_never_ends_is_cut_at_the_bound() {
    for step in [1, 7, 64, 4096, 1 << 20] {
        let (r, state) = reader(tokio::io::repeat(b'x'));
        let (out, end) = drain(r, step, LIMIT).await;
        assert_eq!(
            end.unwrap_err().kind(),
            std::io::ErrorKind::InvalidData,
            "step {step}"
        );
        assert!(state.exceeded(), "step {step}");
        // The read that would have gone past the bound is not passed on: what was passed on
        // is within one read of the bound, and never above it.
        assert!(out.len() <= LIMIT, "step {step}: {}", out.len());
        assert!(out.len() + step > LIMIT, "step {step}: {}", out.len());
        assert_eq!(state.high_water(), out.len(), "step {step}");
    }
}

/// REQ-MCP-001: the count is of one message: messages that are each within the bound pass
/// whatever their sum, the newline resets it, and the longest one is what the reader says it
/// held.
#[tokio::test]
async fn req_mcp_001_the_count_is_of_one_message_and_a_newline_resets_it() {
    let mut data = Vec::new();
    for len in [10usize, LIMIT - 1, 3, LIMIT, 500, 0, 20] {
        data.extend(std::iter::repeat_n(b'm', len));
        data.push(b'\n');
    }
    assert!(data.len() > 2 * LIMIT);
    for step in [1, 5, 333, 8192] {
        let (r, state) = reader(std::io::Cursor::new(data.clone()));
        let (out, end) = drain(r, step, usize::MAX).await;
        assert!(end.is_ok(), "step {step}");
        assert_eq!(out, data, "step {step}");
        assert!(!state.exceeded());
        assert_eq!(state.high_water(), LIMIT, "step {step}");
    }
}

/// REQ-MCP-001: what came complete before the message that went past the bound, in the same
/// read, is passed on (an answer that arrived with the start of the flood is an answer), and
/// nothing of the message that went past it.
#[tokio::test]
async fn req_mcp_001_a_complete_message_before_the_flood_is_passed_on() {
    let first = b"{\"id\":1}\n".to_vec();
    let mut data = first.clone();
    data.extend(std::iter::repeat_n(b'f', LIMIT * 3));
    // One read takes it all.
    let (r, state) = reader(std::io::Cursor::new(data.clone()));
    let (out, end) = drain(r, 1 << 16, LIMIT).await;
    assert!(end.is_err());
    assert_eq!(out, first, "the answer, and none of the flood");
    assert!(state.exceeded());
    // The flood after a newline within the same read, with a complete message after it.
    let mut data = first.clone();
    data.extend(std::iter::repeat_n(b'f', LIMIT + 1));
    data.extend_from_slice(b"\n{\"id\":2}\n");
    let (r, _) = reader(std::io::Cursor::new(data));
    let (out, end) = drain(r, 1 << 16, LIMIT).await;
    assert!(end.is_err());
    assert_eq!(
        out, first,
        "nothing after the message that went past the bound"
    );
}

/// REQ-MCP-001: once the stream ended with the error it stays ended: a read after it is the
/// same error, never end of file (which would say the server closed its output).
#[tokio::test]
async fn req_mcp_001_the_stream_stays_ended_with_the_error() {
    let (mut r, _) = reader(tokio::io::repeat(b'x'));
    let mut buf = vec![0u8; 4096];
    let mut error = None;
    for _ in 0..10 {
        if let Err(e) = r.read(&mut buf).await {
            error = Some(e);
            break;
        }
    }
    assert!(error.is_some());
    for _ in 0..3 {
        let again = r.read(&mut buf).await.unwrap_err();
        assert_eq!(again.kind(), std::io::ErrorKind::InvalidData);
    }
}
