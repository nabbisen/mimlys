//! The download of an interpreter artifact (`interpreters install`;
//! REQ-SBX-001, external design §12 and §18). One download is bounded by
//! LIM-O9; a transient failure (no connection, a timeout, HTTP 429 or 5xx) is
//! retried LIM-O10 times after the waits LIM-O10 states; any other failure is
//! not retried. A download that still fails is a typed [`DownloadError`]
//! (`unavailable`, `component: "download"`), never `internal`.
//!
//! The decision (what is retried, how often, after what wait) is a pure
//! function ([`next`]); [`fetch`] drives it over an injected `get`, `sleep`
//! and log, so that it is tested without a network or real waits; [`http_get`]
//! is the real request.

use mimlys_core::limits::{
    LIM_O9_DOWNLOAD_MS, LIM_O10_DOWNLOAD_RETRIES, LIM_O10_DOWNLOAD_WAITS_MS, LIM_S5_MODULE_BYTES,
};
use std::time::Duration;

#[cfg(test)]
mod tests;

/// How long one attempt may take and how the retries are paced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Policy {
    /// The deadline of one attempt (LIM-O9).
    pub deadline: Duration,
    /// The wait before each retry; its length is the number of retries (LIM-O10).
    pub waits: Vec<Duration>,
}

impl Policy {
    /// LIM-O9 and LIM-O10, as shipped.
    pub fn shipped() -> Self {
        debug_assert_eq!(LIM_O10_DOWNLOAD_WAITS_MS.len(), LIM_O10_DOWNLOAD_RETRIES);
        Self {
            deadline: Duration::from_millis(LIM_O9_DOWNLOAD_MS),
            waits: LIM_O10_DOWNLOAD_WAITS_MS
                .iter()
                .map(|ms| Duration::from_millis(*ms))
                .collect(),
        }
    }
}

/// Why one attempt failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    /// The server answered with this status (not 2xx).
    Status(u16),
    /// No connection, a timeout or a broken transfer.
    Transport(String),
    /// Anything that no retry can change (a bad address, a malformed answer,
    /// an artifact larger than LIM-S5).
    Fatal(String),
}

impl Failure {
    /// Is it a transient failure: no connection, a timeout, HTTP 429 or 5xx?
    pub fn is_transient(&self) -> bool {
        match self {
            Failure::Status(code) => *code == 429 || (500..=599).contains(code),
            Failure::Transport(_) => true,
            Failure::Fatal(_) => false,
        }
    }

    fn status(&self) -> Option<u16> {
        match self {
            Failure::Status(code) => Some(*code),
            _ => None,
        }
    }

    fn describe(&self) -> String {
        match self {
            Failure::Status(code) => format!("HTTP status {code}"),
            Failure::Transport(what) | Failure::Fatal(what) => what.clone(),
        }
    }
}

/// What to do after a failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Next {
    /// Wait this long, then try again.
    Retry(Duration),
    /// Give up.
    Stop,
}

/// The retry decision, pure: a transient failure is retried while the
/// retries of the policy are not used up (`retries_done` of them are).
pub fn next(failure: &Failure, retries_done: usize, policy: &Policy) -> Next {
    match policy.waits.get(retries_done) {
        Some(wait) if failure.is_transient() => Next::Retry(*wait),
        _ => Next::Stop,
    }
}

/// A download that failed for good: typed, so that the command's error is
/// `unavailable` / `download` without anyone reading its text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DownloadError {
    /// The URL asked for.
    pub url: String,
    /// The language whose artifact it was (for the remedy).
    pub language: String,
    /// The last HTTP status, when the last failure was a status.
    pub last_status: Option<u16>,
    /// The last failure, in words.
    pub last: String,
    /// How many retries were made.
    pub retries: usize,
}

impl std::fmt::Display for DownloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "the download of {} failed after {} retries (last: {}); retry later, or obtain the artifact another way and run: mimlys interpreters install {} --from FILE (the same SHA-256 is verified)",
            self.url, self.retries, self.last, self.language
        )
    }
}

impl std::error::Error for DownloadError {}

/// Downloads `url` under `policy`: `get` makes one attempt, `sleep` waits,
/// `log` receives one line per retry.
pub fn fetch(
    url: &str,
    language: &str,
    policy: &Policy,
    mut get: impl FnMut(&str, Duration) -> Result<Vec<u8>, Failure>,
    mut sleep: impl FnMut(Duration),
    mut log: impl FnMut(&str),
) -> Result<Vec<u8>, DownloadError> {
    let mut retries = 0;
    loop {
        let failure = match get(url, policy.deadline) {
            Ok(bytes) => return Ok(bytes),
            Err(failure) => failure,
        };
        match next(&failure, retries, policy) {
            Next::Retry(wait) => {
                log(&format!(
                    "the download of {url} failed ({}); retry {} of {} after {} ms",
                    failure.describe(),
                    retries + 1,
                    policy.waits.len(),
                    wait.as_millis()
                ));
                sleep(wait);
                retries += 1;
            }
            Next::Stop => {
                return Err(DownloadError {
                    url: url.to_owned(),
                    language: language.to_owned(),
                    last_status: failure.status(),
                    last: failure.describe(),
                    retries,
                });
            }
        }
    }
}

/// One real attempt: a GET within `deadline`, the body bounded by LIM-S5.
///
/// The request is `reqwest`'s, the HTTP client of the model adapter too (handoff
/// 008c). Inside the CLI's multi-thread runtime it runs on that runtime
/// (`block_in_place`; no second runtime is made); called from anywhere else
/// (a test, a current-thread runtime) it runs on a short-lived thread with a
/// current-thread runtime of its own. Behaviour on the wire is that of `ureq`
/// before it: the system proxy variables are honoured, the bundled WebPKI root
/// certificates are the trust store (rustls, no platform verifier), redirects are
/// followed up to ten times, and `deadline` bounds the whole attempt, body included.
pub fn http_get(url: &str, deadline: Duration) -> Result<Vec<u8>, Failure> {
    use tokio::runtime::{Builder, Handle, RuntimeFlavor};
    if let Ok(handle) = Handle::try_current()
        && handle.runtime_flavor() == RuntimeFlavor::MultiThread
    {
        return tokio::task::block_in_place(|| handle.block_on(get(url, deadline)));
    }
    std::thread::scope(|scope| {
        scope
            .spawn(|| {
                let runtime = Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|e| Failure::Fatal(format!("no runtime for the download: {e}")))?;
                runtime.block_on(get(url, deadline))
            })
            .join()
            .unwrap_or_else(|_| Err(Failure::Fatal("the download thread panicked".into())))
    })
}

async fn get(url: &str, deadline: Duration) -> Result<Vec<u8>, Failure> {
    let client = reqwest::Client::builder()
        .timeout(deadline)
        .user_agent(concat!("mimlys/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| Failure::Fatal(e.without_url().to_string()))?;
    let mut response = client.get(url).send().await.map_err(classify)?;
    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        return Err(Failure::Status(status));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(classify_body)? {
        if bytes.len() as u64 + chunk.len() as u64 > LIM_S5_MODULE_BYTES {
            return Err(Failure::Fatal("the artifact is larger than LIM-S5".into()));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

/// A failure before or while the answer arrives. A timeout, a refused or lost
/// connection and an unresolved host are transient; a bad address, too many
/// redirects and the like are not.
fn classify(error: reqwest::Error) -> Failure {
    if error.is_timeout() {
        return Failure::Transport("timed out".into());
    }
    if error.is_connect() {
        let mut source = std::error::Error::source(&error);
        while let Some(cause) = source {
            if cause.to_string().contains("dns error") {
                return Failure::Transport("the host was not found".into());
            }
            source = cause.source();
        }
        return Failure::Transport("no connection or a broken one".into());
    }
    if error.is_body() || error.is_decode() {
        return Failure::Transport("the transfer broke".into());
    }
    Failure::Fatal(error.without_url().to_string())
}

/// A failure while the body is read.
fn classify_body(error: reqwest::Error) -> Failure {
    if error.is_timeout() {
        Failure::Transport("timed out".into())
    } else {
        Failure::Transport("the transfer broke".into())
    }
}
