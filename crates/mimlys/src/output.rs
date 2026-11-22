//! Human lines or one JSON value per command (REQ-CLI-004).

use crate::app::AppError;
use serde::Serialize;
use std::io::Write;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

/// The reader of standard output has gone (a closed pipe: `mimlys events <task> | head`).
/// Set by the first write that finds it so, and by [`Output::reader_gone`] where the
/// platform can tell without a write (REQ-CLI-009).
static READER_GONE: AtomicBool = AtomicBool::new(false);

/// The first write to standard output that failed for any other reason (a full disk, an I/O
/// error), kept to be said once, on standard error, when the command ends.
static WRITE_FAILED: Mutex<Option<String>> = Mutex::new(None);

/// The one write to standard output: every line of every command goes through here
/// (REQ-CLI-009). A reader that has gone ends the output, without a message and without an
/// error: the command goes on to its end with the exit code its work earned. Any other failure
/// ends the output the same way and is kept for [`write_failure`]. Nothing is written once the
/// output has ended, so that no later line is a second failure.
fn put(bytes: &[u8], flush: bool) {
    if READER_GONE.load(Ordering::SeqCst) || write_failure().is_some() {
        return;
    }
    let mut stdout = std::io::stdout().lock();
    let written = stdout
        .write_all(bytes)
        .and_then(|()| if flush { stdout.flush() } else { Ok(()) });
    if let Err(e) = written {
        if e.kind() == std::io::ErrorKind::BrokenPipe {
            // `EPIPE` on Unix, `ERROR_NO_DATA` and `ERROR_BROKEN_PIPE` on Windows: the standard
            // library maps all of them to this kind.
            READER_GONE.store(true, Ordering::SeqCst);
        } else if let Ok(mut failed) = WRITE_FAILED.lock() {
            failed.get_or_insert_with(|| e.to_string());
        }
    }
}

/// The first failure, other than a reader that has gone, of a write to standard output.
pub fn write_failure() -> Option<String> {
    WRITE_FAILED.lock().ok().and_then(|failed| failed.clone())
}

/// Whether standard output is a pipe (or a terminal) whose reader has gone, found without
/// writing. A write that fails says it for itself; this is for a command that waits and so
/// writes nothing for a long time (`events --follow`). Only Unix can ask: the poll of the
/// descriptor reports an error when a pipe has no reader. Windows learns it at the next write.
#[cfg(unix)]
fn reader_gone_by_poll() -> bool {
    use rustix::event::Timespec;
    use rustix::event::{PollFd, PollFlags, poll};
    let stdout = std::io::stdout();
    // `ERR` and `HUP` are reported whatever is asked; `NVAL` (standard output closed) is not
    // a reader that has gone: the standard library treats such a stream as written.
    let mut fds = [PollFd::new(&stdout, PollFlags::empty())];
    let now = Timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    matches!(poll(&mut fds, Some(&now)), Ok(1))
        && fds[0].revents().intersects(PollFlags::ERR | PollFlags::HUP)
}

#[cfg(not(unix))]
fn reader_gone_by_poll() -> bool {
    false
}

/// Where output goes and in which form.
#[derive(Clone, Copy, Debug)]
pub struct Output {
    json: bool,
}

impl Output {
    /// Human text, or JSON when `json`.
    pub fn new(json: bool) -> Self {
        Self { json }
    }

    /// Whether JSON mode is on.
    pub fn json(&self) -> bool {
        self.json
    }

    /// A human line (nothing in JSON mode).
    pub fn line(&self, text: impl AsRef<str>) {
        if !self.json {
            let text = text.as_ref();
            let mut line = String::with_capacity(text.len() + 1);
            line.push_str(text);
            line.push('\n');
            put(line.as_bytes(), false);
        }
    }

    /// Whether the reader of standard output has gone (REQ-CLI-009): a command that waits for
    /// something to print asks it between its polls, and ends when it is true.
    pub fn reader_gone(&self) -> bool {
        if READER_GONE.load(Ordering::SeqCst) {
            return true;
        }
        if reader_gone_by_poll() {
            READER_GONE.store(true, Ordering::SeqCst);
            return true;
        }
        false
    }

    /// Text exactly as given, with nothing added (nothing in JSON mode): what a command that
    /// prints a stored copy writes, so that a pipe gets the bytes. A reader that has gone
    /// (a closed pipe) ends the output without an error: it has what it wanted.
    pub fn raw(&self, text: &str) {
        if self.json {
            return;
        }
        put(text.as_bytes(), true);
    }

    /// The command's JSON value (nothing in human mode).
    pub fn value<T: Serialize>(&self, value: &T) {
        if self.json {
            let mut text = serde_json::to_string(value).unwrap_or_default();
            text.push('\n');
            put(text.as_bytes(), false);
        }
    }

    /// An error: the JSON error object on standard output in JSON mode,
    /// else one line on standard error.
    pub fn error(&self, error: &AppError) {
        if self.json {
            let mut text = error.to_json().to_string();
            text.push('\n');
            put(text.as_bytes(), false);
        } else {
            eprintln!(
                "{}",
                crate::messages::error_line(error.kind(), &error.to_string())
            );
        }
    }
}
