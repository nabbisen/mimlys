//! A server's standard error, bounded (external design §11.1: "What a server
//! writes on standard error is bounded"). A server is a program the
//! administrator declared, not a trusted one: what it writes on its standard
//! error must not grow the runtime's memory or its log without limit, whether
//! it writes one line that never ends or lines without pause.
//!
//! - **A line is cut.** At most [`STDERR_LINE_MAX_BYTES`] bytes of a line are
//!   kept; the line is logged as soon as the bound is reached, with the cut
//!   marked, and the rest of it is read and discarded up to its newline. Memory
//!   per line is the bound, whatever the line's length.
//! - **Lines are counted.** At most [`STDERR_LINES_PER_WINDOW`] lines are logged
//!   in each [`STDERR_WINDOW`]; the others are read and counted, and when the
//!   window ends one line says how many were dropped. Without a pause in the
//!   server's output the log still grows, but at a bounded rate.
//!
//! Lines stay at the level `info`, where an operator reads a server's errors.
//!
//! The three values are LIM-I17, LIM-I18 and LIM-O18 (handoff 009h).

use mimlys_core::log_span::ReaderSpan;
use std::time::Duration;
use tokio::io::{AsyncBufRead, AsyncBufReadExt};
use tokio::time::Instant;

/// The most bytes of one line of a server's standard error that are kept
/// (LIM-I17).
pub const STDERR_LINE_MAX_BYTES: usize = 1024;

/// The most lines of a server's standard error that are logged in one window
/// (LIM-I18).
pub const STDERR_LINES_PER_WINDOW: u32 = 50;

/// The length of a window of the rate bound (LIM-O18).
pub const STDERR_WINDOW: Duration = Duration::from_secs(1);

/// One line read from a server's standard error.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct StderrLine {
    /// The bytes kept, without the newline: at most the bound.
    pub(crate) bytes: Vec<u8>,
    /// Whether the line was longer than the bound (the rest is discarded).
    pub(crate) cut: bool,
}

impl StderrLine {
    /// The text to log: lossy UTF-8 (a character the cut split is dropped), a
    /// trailing carriage return removed, and the cut marked.
    pub(crate) fn text(&self) -> String {
        let mut bytes = self.bytes.as_slice();
        if self.cut {
            // The bound may fall inside a character: keep the whole ones.
            if let Err(e) = std::str::from_utf8(bytes)
                && e.error_len().is_none()
            {
                bytes = &bytes[..e.valid_up_to()];
            }
        } else if let Some(stripped) = bytes.strip_suffix(b"\r") {
            bytes = stripped;
        }
        let mut text = String::from_utf8_lossy(bytes).into_owned();
        if self.cut {
            text.push_str(&format!(" [... line cut at {STDERR_LINE_MAX_BYTES} bytes]"));
        }
        text
    }
}

/// Reads lines of at most `max` bytes. A longer line is returned at once, cut,
/// and the rest of it is discarded by the following reads, up to its newline.
///
/// The state is in the reader, not in a future: a read that is dropped before it
/// ends (a timer fired meanwhile) loses nothing.
pub(crate) struct LineReader {
    max: usize,
    kept: Vec<u8>,
    discarding: bool,
}

impl LineReader {
    pub(crate) fn new(max: usize) -> Self {
        Self {
            max,
            kept: Vec::new(),
            discarding: false,
        }
    }

    /// The next line, or `None` at the end of the stream. A last line with no
    /// newline is returned at the end.
    pub(crate) async fn next<R: AsyncBufRead + Unpin>(
        &mut self,
        reader: &mut R,
    ) -> std::io::Result<Option<StderrLine>> {
        loop {
            let buf = reader.fill_buf().await?;
            if buf.is_empty() {
                // The end of the stream: a line that had no newline.
                if self.discarding || self.kept.is_empty() {
                    self.discarding = false;
                    return Ok(None);
                }
                return Ok(Some(StderrLine {
                    bytes: std::mem::take(&mut self.kept),
                    cut: false,
                }));
            }
            let newline = buf.iter().position(|b| *b == b'\n');
            if self.discarding {
                match newline {
                    Some(i) => {
                        self.discarding = false;
                        reader.consume(i + 1);
                    }
                    None => {
                        let all = buf.len();
                        reader.consume(all);
                    }
                }
                continue;
            }
            let content = newline.unwrap_or(buf.len());
            if self.kept.len() + content > self.max {
                // Over the bound: this much is kept, the line is returned cut,
                // and the rest of it up to the newline is discarded.
                let room = self.max - self.kept.len();
                self.kept.extend_from_slice(&buf[..room]);
                match newline {
                    Some(i) => reader.consume(i + 1),
                    None => {
                        self.discarding = true;
                        reader.consume(room);
                    }
                }
                return Ok(Some(StderrLine {
                    bytes: std::mem::take(&mut self.kept),
                    cut: true,
                }));
            }
            self.kept.extend_from_slice(&buf[..content]);
            match newline {
                Some(i) => {
                    reader.consume(i + 1);
                    return Ok(Some(StderrLine {
                        bytes: std::mem::take(&mut self.kept),
                        cut: false,
                    }));
                }
                None => {
                    let all = buf.len();
                    reader.consume(all);
                }
            }
        }
    }
}

/// The rate bound: at most `limit` lines in each window. The caller asks to
/// admit each line and, once a window has ended, takes the count of the lines
/// it dropped.
pub(crate) struct RateBound {
    limit: u32,
    window: Duration,
    started: Instant,
    admitted: u32,
    dropped: u64,
}

impl RateBound {
    pub(crate) fn new(limit: u32, window: Duration, now: Instant) -> Self {
        Self {
            limit,
            window,
            started: now,
            admitted: 0,
            dropped: 0,
        }
    }

    /// When the window that is running ends.
    pub(crate) fn window_end(&self) -> Instant {
        self.started + self.window
    }

    /// Whether lines were dropped in the window that is running.
    pub(crate) fn has_dropped(&self) -> bool {
        self.dropped > 0
    }

    /// Ends the window when its time has passed, and gives the count of the
    /// lines it dropped (when any). The next window starts at `now`.
    pub(crate) fn roll(&mut self, now: Instant) -> Option<u64> {
        if now < self.window_end() {
            return None;
        }
        self.started = now;
        self.admitted = 0;
        match std::mem::take(&mut self.dropped) {
            0 => None,
            n => Some(n),
        }
    }

    /// Admits one line of the window that is running, or counts it as dropped.
    pub(crate) fn admit(&mut self) -> bool {
        if self.admitted < self.limit {
            self.admitted += 1;
            true
        } else {
            self.dropped += 1;
            false
        }
    }

    /// The lines dropped so far in this window, taken at the end of the stream.
    pub(crate) fn finish(&mut self) -> Option<u64> {
        match std::mem::take(&mut self.dropped) {
            0 => None,
            n => Some(n),
        }
    }
}

/// The line that says how many lines were dropped.
pub(crate) fn dropped_text(count: u64) -> String {
    if count == 1 {
        "1 line dropped".to_owned()
    } else {
        format!("{count} lines dropped")
    }
}

/// Reads a server's standard error to its end and logs it, bounded, at `info`
/// in the span of the call in flight when each line is read.
pub(crate) async fn pump<R: AsyncBufRead + Unpin>(
    mut stderr: R,
    server: String,
    reader_span: ReaderSpan,
) {
    let mut lines = LineReader::new(STDERR_LINE_MAX_BYTES);
    let mut bound = RateBound::new(STDERR_LINES_PER_WINDOW, STDERR_WINDOW, Instant::now());
    let log = |text: &str| {
        reader_span
            .current()
            .in_scope(|| tracing::info!(target: "mimlys_mcp", server = %server, "{text}"));
    };
    loop {
        // While lines are dropped, the window's end is waited for as well, so
        // that the count is written when the window ends even if the server
        // then says nothing more.
        let read = if bound.has_dropped() {
            tokio::select! {
                read = lines.next(&mut stderr) => Some(read),
                () = tokio::time::sleep_until(bound.window_end()) => None,
            }
        } else {
            Some(lines.next(&mut stderr).await)
        };
        if let Some(count) = bound.roll(Instant::now()) {
            log(&dropped_text(count));
        }
        let Some(read) = read else { continue };
        match read {
            Ok(Some(line)) => {
                if bound.admit() {
                    log(&line.text());
                }
            }
            Ok(None) | Err(_) => break,
        }
    }
    if let Some(count) = bound.finish() {
        log(&dropped_text(count));
    }
}

#[cfg(test)]
mod tests;
