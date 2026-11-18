//! A server's message is bounded before it is parsed (handoff 013c, item 3;
//! LIM-I22, REQ-MCP-001).
//!
//! The reader between a server's standard output and the transport counts the
//! bytes since the last newline, which is the message being read (one JSON-RPC
//! message is one line). A read that would take the message past the bound is not
//! passed on, and the stream ends with an error: the transport never holds more of
//! a message than the bound, and never the whole of one past it (a server that
//! writes one line that never ends costs a bounded amount of memory, not memory
//! up to the call's deadline), and the connection is closed the way it is when a
//! server ends. The server is then stopped as a server that ended, by the path the
//! connection already has for one (`Connection::call`, `Connection::is_closed` and
//! `Server::ensure_started`: its process, which is still writing, is killed with
//! its group, and the next call starts it again, counted by LIM-X3), and the log
//! says why at `warn`.
//!
//! The bytes before the offending message in the same read are passed on: a
//! complete answer that arrived with the start of the flood is still an answer.
//!
//! The SDK's own transport also has a bound (16 MiB, which drops the line and
//! goes on); this one is lower and is the one that decides.

use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, ReadBuf};

/// What the reader knows, for the connection and for the tests.
#[derive(Debug, Default)]
pub struct LineState {
    exceeded: AtomicBool,
    high_water: AtomicUsize,
}

impl LineState {
    /// Whether a message went past the bound and the stream was ended.
    pub fn exceeded(&self) -> bool {
        self.exceeded.load(Ordering::SeqCst)
    }

    /// The most bytes of one message the reader passed on before its newline: what the
    /// transport can have held of a message at once, as far as this reader let it. Never
    /// above the bound.
    pub fn high_water(&self) -> usize {
        self.high_water.load(Ordering::SeqCst)
    }
}

/// An `AsyncRead` that passes on the bytes of `inner` while the message being read is within
/// `limit` bytes (its newline not counted).
pub struct BoundedLines<R> {
    inner: R,
    server: String,
    limit: usize,
    /// Bytes of the message being read, passed on so far.
    since_newline: usize,
    ended: bool,
    state: Arc<LineState>,
}

impl<R> BoundedLines<R> {
    /// Wraps `inner`, the standard output of the server called `server`.
    pub fn new(inner: R, server: &str, limit: usize) -> (Self, Arc<LineState>) {
        let state = Arc::new(LineState::default());
        (
            Self {
                inner,
                server: server.to_owned(),
                limit,
                since_newline: 0,
                ended: false,
                state: Arc::clone(&state),
            },
            state,
        )
    }

    fn too_large(&self) -> std::io::Error {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "a message from the server is larger than {} bytes (LIM-I22)",
                self.limit
            ),
        )
    }
}

/// How much of `chunk` may be passed on, given `since` bytes of a message already passed on:
/// the count, the bytes of the message at the end of what is passed on, the largest message
/// seen, and whether a message went past `limit` (what follows the last newline before it is
/// not passed on).
fn scan(since: usize, chunk: &[u8], limit: usize) -> Scan {
    let mut since = since;
    let mut keep = 0;
    let mut high = since;
    let mut rest = chunk;
    loop {
        match rest.iter().position(|b| *b == b'\n') {
            Some(i) => {
                if since + i > limit {
                    return Scan {
                        keep,
                        since,
                        high,
                        over: true,
                    };
                }
                high = high.max(since + i);
                since = 0;
                keep += i + 1;
                rest = &rest[i + 1..];
            }
            None => {
                if since + rest.len() > limit {
                    return Scan {
                        keep,
                        since: 0,
                        high,
                        over: true,
                    };
                }
                since += rest.len();
                keep += rest.len();
                high = high.max(since);
                return Scan {
                    keep,
                    since,
                    high,
                    over: false,
                };
            }
        }
    }
}

struct Scan {
    keep: usize,
    since: usize,
    high: usize,
    over: bool,
}

impl<R: AsyncRead + Unpin> AsyncRead for BoundedLines<R> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        if self.ended {
            return Poll::Ready(Err(self.too_large()));
        }
        let before = buf.filled().len();
        match Pin::new(&mut self.inner).poll_read(cx, buf) {
            Poll::Ready(Ok(())) => {}
            other => return other,
        }
        let scanned = scan(self.since_newline, &buf.filled()[before..], self.limit);
        self.state
            .high_water
            .fetch_max(scanned.high, Ordering::SeqCst);
        if !scanned.over {
            self.since_newline = scanned.since;
            return Poll::Ready(Ok(()));
        }
        // A message went past the bound: nothing of it is passed on from here, and the stream
        // ends with an error at the next read (at this one when nothing precedes it).
        self.ended = true;
        self.state.exceeded.store(true, Ordering::SeqCst);
        tracing::warn!(
            target: "mimlys_mcp",
            server = %self.server,
            bound = self.limit,
            "a message from the server is larger than the bound of {} bytes (LIM-I22): the server is stopped",
            self.limit
        );
        buf.set_filled(before + scanned.keep);
        if scanned.keep == 0 {
            return Poll::Ready(Err(self.too_large()));
        }
        Poll::Ready(Ok(()))
    }
}

#[cfg(test)]
mod tests;
