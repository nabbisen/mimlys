use super::*;
use tokio::io::{AsyncReadExt, BufReader};

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

/// Every line of `input`, read through a buffer of `capacity` bytes (a small
/// one makes a line arrive in many pieces).
fn read_all(input: &[u8], max: usize, capacity: usize) -> Vec<StderrLine> {
    runtime().block_on(async {
        let mut reader = BufReader::with_capacity(capacity, input);
        let mut lines = LineReader::new(max);
        let mut out = Vec::new();
        while let Some(line) = lines.next(&mut reader).await.unwrap() {
            out.push(line);
        }
        out
    })
}

fn line(text: &str, cut: bool) -> StderrLine {
    StderrLine {
        bytes: text.as_bytes().to_vec(),
        cut,
    }
}

/// REQ-MCP-001 (handoff 009h, item 3): lines within the bound come out whole, however the
/// stream is divided, with a missing newline at the end and a carriage return handled.
#[test]
fn req_mcp_001_lines_within_the_bound_are_read_whole() {
    for capacity in [1, 2, 3, 5, 64] {
        let lines = read_all(b"one\ntwo words\n\nlast", 16, capacity);
        assert_eq!(
            lines,
            [
                line("one", false),
                line("two words", false),
                line("", false),
                line("last", false)
            ],
            "capacity {capacity}"
        );
    }
    // A line of exactly the bound is not cut; one byte more is.
    let lines = read_all(b"abcd\nabcde\nabcdef\nz\n", 4, 3);
    assert_eq!(
        lines,
        [
            line("abcd", false),
            line("abcd", true),
            line("abcd", true),
            line("z", false)
        ]
    );
    assert_eq!(line("ab\r", false).text(), "ab");
    assert_eq!(read_all(b"", 4, 8), []);
}

/// REQ-MCP-001 (handoff 009h, item 3): a line over the bound is returned cut at
/// once, the cut is marked in its text, the rest of it is discarded up to the newline
/// (however far, whichever way the stream is divided), and the next line is read as
/// it is.
#[test]
fn req_mcp_001_a_long_line_is_cut_marked_and_the_rest_discarded_to_its_newline() {
    let mut input = Vec::new();
    input.extend_from_slice(b"before\n");
    input.extend(std::iter::repeat_n(b'x', 100_000));
    input.extend_from_slice(b"\nafter\n");
    for capacity in [1, 7, 4096, 1 << 20] {
        let lines = read_all(&input, STDERR_LINE_MAX_BYTES, capacity);
        assert_eq!(lines.len(), 3, "capacity {capacity}");
        assert_eq!(lines[0], line("before", false));
        assert!(lines[1].cut);
        assert_eq!(lines[1].bytes.len(), STDERR_LINE_MAX_BYTES);
        assert!(lines[1].bytes.iter().all(|b| *b == b'x'));
        assert_eq!(lines[2], line("after", false));
    }
    let text = line("abc", true).text();
    assert!(text.starts_with("abc [..."), "{text}");
    assert!(text.contains(&STDERR_LINE_MAX_BYTES.to_string()), "{text}");
    // A character the cut splits is dropped, not replaced by garbage.
    let split = StderrLine {
        bytes: "ab\u{e9}".as_bytes()[..3].to_vec(),
        cut: true,
    };
    assert!(split.text().starts_with("ab ["), "{}", split.text());
}

/// REQ-MCP-001 (handoff 009h, item 3): a line with no newline that does not end is
/// read to the end of the stream with one cut line out and nothing kept (the
/// memory is the bound, not the stream's length): 64 MiB of one line, then the end.
#[test]
fn req_mcp_001_a_line_that_never_ends_is_one_cut_line_and_keeps_nothing() {
    runtime().block_on(async {
        let mut reader = BufReader::new(tokio::io::repeat(b'y').take(64 * 1024 * 1024));
        let mut lines = LineReader::new(STDERR_LINE_MAX_BYTES);
        let first = lines.next(&mut reader).await.unwrap().unwrap();
        assert!(first.cut);
        assert_eq!(first.bytes.len(), STDERR_LINE_MAX_BYTES);
        assert_eq!(lines.next(&mut reader).await.unwrap(), None);
        assert!(lines.kept.is_empty(), "nothing is kept");
        assert!(lines.kept.capacity() <= 2 * STDERR_LINE_MAX_BYTES);
    });
}

/// REQ-MCP-001 (handoff 009h, item 3): the rate bound admits `limit` lines in a
/// window, counts the rest, gives the count when the window ends, and starts again.
#[test]
fn req_mcp_001_the_rate_bound_counts_what_it_drops_and_says_so_once_per_window() {
    let start = Instant::now();
    let second = Duration::from_secs(1);
    let mut bound = RateBound::new(3, second, start);
    assert!(!bound.has_dropped());
    let admitted: Vec<bool> = (0..7).map(|_| bound.admit()).collect();
    assert_eq!(admitted, [true, true, true, false, false, false, false]);
    assert!(bound.has_dropped());
    // The window has not ended.
    assert_eq!(bound.roll(start + second / 2), None);
    assert!(!bound.admit());
    // It ends: one count, of everything dropped in it, and the lines are admitted again.
    assert_eq!(bound.roll(start + second), Some(5));
    assert!(!bound.has_dropped());
    assert_eq!(bound.window_end(), start + second + second);
    assert!(bound.admit());
    // A window with nothing dropped says nothing.
    assert_eq!(bound.roll(start + 2 * second + second), None);
    // The end of the stream gives the count of the window that was running.
    for _ in 0..5 {
        bound.admit();
    }
    assert_eq!(bound.finish(), Some(2));
    assert_eq!(bound.finish(), None);
    assert_eq!(dropped_text(1), "1 line dropped");
    assert_eq!(dropped_text(2500), "2500 lines dropped");
}
