//! Observations and notices (external design §8; REQ-CTX-006, REQ-CTX-008,
//! REQ-CTX-009, REQ-TOOL-008): the normalized, bounded, provenance-labelled
//! result the model sees. Every cut is reported; invisible characters are
//! removed with a notice; invalid UTF-8 is replaced with a notice.

use crate::action::{ExecutionMetrics, FailureCause};
use crate::clock::Instant;
use crate::ids::{ActionId, ObservationId};
use crate::limits::{LIM_C2_EXCERPT_HEAD_CHARS, LIM_C2_EXCERPT_TAIL_CHARS};
use crate::spi::ExecutionOutcome;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Which captured stream a notice refers to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stream {
    /// Standard output.
    Stdout,
    /// Standard error.
    Stderr,
}

/// A statement by the runtime about something it changed (external design §8).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum Notice {
    /// A stream exceeded its bound; the excerpt keeps head and tail.
    OutputTruncated {
        /// Which stream.
        stream: Stream,
        /// Its full size.
        total_bytes: u64,
    },
    /// Returned content was cut to the requested size.
    ContentTruncated {
        /// The full size in characters.
        total_chars: u64,
    },
    /// A listing was cut to LIM-I7.
    EntriesTruncated {
        /// The full count.
        total: u64,
    },
    /// Invalid UTF-8 was replaced.
    InvalidUtf8Replaced {
        /// Which stream.
        stream: Stream,
    },
    /// Control, zero-width, tag or bidirectional characters were removed.
    CharactersRemoved {
        /// How many.
        count: u64,
    },
    /// Content blocks of an external tool's result were not passed on: the
    /// payload is the first text block when the result has no structured
    /// content (handoff 009h).
    ContentOmitted {
        /// How many blocks.
        count: u64,
    },
    /// Entries were skipped by a listing or search, aggregated by reason.
    EntriesSkipped {
        /// Why.
        reason: SkipReason,
        /// How many.
        count: u64,
    },
    /// A symbolic link was not materialized.
    SymlinkOmitted {
        /// Its path.
        path: String,
    },
    /// A configured secret was redacted.
    SecretRedacted {
        /// Its configured name.
        name: String,
    },
    /// The failed execution's effects were rolled back.
    RolledBack,
    /// The execution was a runtime retry.
    Retried {
        /// The attempt number.
        attempt: u32,
    },
    /// A host tool never started; the execution is `host_fault{unavailable}`.
    HostToolNotStarted {
        /// Why: the command, its temporary directory or the spawn.
        reason: String,
    },
    /// The user's stop signal came before the action began: it was closed
    /// `cancelled` without starting, so nothing ran (REQ-CLI-005, external
    /// design §19.2). There is no execution behind this observation.
    StoppedBeforeStart,
}

/// Why a listing or search skipped entries.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkipReason {
    /// A symbolic link; never followed by a search.
    Symlink,
    /// Larger than LIM-W3.
    TooLarge,
    /// Could not be read.
    Unreadable,
}

/// The status of an observation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservationStatus {
    /// The action succeeded.
    Succeeded,
    /// The action failed.
    Failed,
    /// The action was cancelled.
    Cancelled,
    /// The outcome is unknown (recovery).
    Unknown,
}

impl ObservationStatus {
    /// The snake_case name.
    pub fn name(self) -> &'static str {
        match self {
            ObservationStatus::Succeeded => "succeeded",
            ObservationStatus::Failed => "failed",
            ObservationStatus::Cancelled => "cancelled",
            ObservationStatus::Unknown => "unknown",
        }
    }
}

/// The loop's verdict on an execution: status, cause and whether the
/// effects were rolled back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Verdict {
    /// Status.
    pub status: ObservationStatus,
    /// The failure cause, when failed.
    pub cause: Option<FailureCause>,
    /// Whether the effects were rolled back (REQ-TOOL-010).
    pub rolled_back: bool,
}

/// What the model sees of an action's result (external design §8).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Observation {
    /// Identifier.
    pub observation_id: ObservationId,
    /// The action.
    pub action_id: ActionId,
    /// The tool.
    pub tool: String,
    /// Status.
    pub status: ObservationStatus,
    /// The failure cause, when failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cause: Option<FailureCause>,
    /// Whether the effects were rolled back (REQ-TOOL-010).
    pub rolled_back: bool,
    /// The tool's structured result, validated against its output schema.
    pub payload: Value,
    /// Standard output excerpt.
    pub stdout: String,
    /// Standard error excerpt.
    pub stderr: String,
    /// Notices.
    pub notices: Vec<Notice>,
    /// Measurements.
    pub metrics: ExecutionMetrics,
    /// When created.
    pub created_at: Instant,
}

impl Observation {
    /// Normalizes an execution outcome: lossy decoding, sanitization,
    /// redaction and excerpting of both streams with the matching notices. The
    /// whole stream is redacted before it is cut, so that no part of a secret
    /// that lies across the cut is left in the excerpt (REQ-CTX-007).
    pub fn from_outcome(
        observation_id: ObservationId,
        action_id: ActionId,
        tool: &str,
        outcome: &ExecutionOutcome,
        verdict: Verdict,
        now: Instant,
        redactor: &crate::context::Redactor,
    ) -> Self {
        let Verdict {
            status,
            cause,
            rolled_back,
        } = verdict;
        let mut notices = outcome.notices.clone();
        let stdout = normalize_stream(&outcome.stdout, Stream::Stdout, redactor, &mut notices);
        let stderr = normalize_stream(&outcome.stderr, Stream::Stderr, redactor, &mut notices);
        if rolled_back {
            notices.push(Notice::RolledBack);
        }
        Self {
            observation_id,
            action_id,
            tool: tool.to_owned(),
            status,
            cause,
            rolled_back,
            payload: outcome.payload.clone(),
            stdout,
            stderr,
            notices,
            metrics: outcome.metrics.clone(),
            created_at: now,
        }
    }
}

impl Observation {
    /// The observation of an action closed `cancelled` before it began (the
    /// user's stop signal came first): no execution, no output, nothing
    /// rolled back, and the notice that says why.
    pub fn stopped_before_start(
        observation_id: ObservationId,
        action_id: ActionId,
        tool: &str,
        now: Instant,
    ) -> Self {
        Self {
            observation_id,
            action_id,
            tool: tool.to_owned(),
            status: ObservationStatus::Cancelled,
            cause: None,
            rolled_back: false,
            payload: Value::Null,
            stdout: String::new(),
            stderr: String::new(),
            notices: vec![Notice::StoppedBeforeStart],
            metrics: ExecutionMetrics::default(),
            created_at: now,
        }
    }

    /// Replaces configured secret values in the payload, both streams and
    /// the notices' texts (REQ-CTX-007), adding one `secret_redacted` notice
    /// per secret found.
    pub fn redact(&mut self, redactor: &crate::context::Redactor) {
        if redactor.is_empty() {
            return;
        }
        let mut names = redactor.redact_value(&mut self.payload);
        if let Some(cause) = &mut self.cause {
            names.extend(redactor.redact_cause(cause));
        }
        for text in [&mut self.stdout, &mut self.stderr] {
            let (clean, found) = redactor.redact(text);
            if !found.is_empty() {
                *text = clean;
                names.extend(found);
            }
        }
        for notice in &mut self.notices {
            if let Notice::SymlinkOmitted { path } = notice {
                let (clean, found) = redactor.redact(path);
                if !found.is_empty() {
                    *path = clean;
                    names.extend(found);
                }
            }
        }
        add_secret_notices(&mut self.notices, names);
    }
}

/// One `secret_redacted` notice for each name, unless the notices have it.
pub fn add_secret_notices(notices: &mut Vec<Notice>, mut names: Vec<String>) {
    names.sort();
    names.dedup();
    for name in names {
        let notice = Notice::SecretRedacted { name };
        if !notices.contains(&notice) {
            notices.push(notice);
        }
    }
}

fn normalize_stream(
    bytes: &[u8],
    stream: Stream,
    redactor: &crate::context::Redactor,
    notices: &mut Vec<Notice>,
) -> String {
    let (text, replaced) = decode_lossy(bytes);
    if replaced {
        notices.push(Notice::InvalidUtf8Replaced { stream });
    }
    let (clean, removed) = sanitize(&text);
    if removed > 0 {
        notices.push(Notice::CharactersRemoved {
            count: removed as u64,
        });
    }
    // The whole text, before the cut.
    let (clean, found) = redactor.redact(&clean);
    add_secret_notices(notices, found);
    let (excerpt, total) = excerpt(&clean, LIM_C2_EXCERPT_HEAD_CHARS, LIM_C2_EXCERPT_TAIL_CHARS);
    if total.is_some() {
        notices.push(Notice::OutputTruncated {
            stream,
            total_bytes: bytes.len() as u64,
        });
    }
    excerpt
}

/// Decodes bytes as UTF-8, replacing invalid sequences; reports whether any were replaced.
pub fn decode_lossy(bytes: &[u8]) -> (String, bool) {
    match std::str::from_utf8(bytes) {
        Ok(s) => (s.to_owned(), false),
        Err(_) => (String::from_utf8_lossy(bytes).into_owned(), true),
    }
}

/// Removes control characters (except tab, line feed and carriage return),
/// zero-width and other invisible format characters, the other default-ignorable
/// characters (soft hyphen, variation selectors, fillers, invisible musical and
/// interlinear controls), Unicode tag characters and bidirectional controls
/// (REQ-CTX-008; THR-21). Visible symbols stay, whatever their name: U+2800
/// (a blank braille pattern) and U+FFFC (the object replacement character) are
/// drawn. Returns the clean text and the number of characters removed.
pub fn sanitize(text: &str) -> (String, usize) {
    let mut out = String::with_capacity(text.len());
    let mut removed = 0;
    for c in text.chars() {
        if is_invisible(c) {
            removed += 1;
        } else {
            out.push(c);
        }
    }
    (out, removed)
}

/// Whether `c` is removed by [`sanitize`]. The ranges are a table of our own, by
/// class, so that what is removed does not move with a Unicode version.
fn is_invisible(c: char) -> bool {
    match c {
        '\t' | '\n' | '\r' => false,
        c if c.is_control() => true,
        // Zero-width characters, directional marks, line and paragraph
        // separators, embeddings and isolates, invisible operators.
        '\u{200B}'..='\u{200F}'
        | '\u{2028}'..='\u{202E}'
        | '\u{2060}'..='\u{2064}'
        | '\u{2066}'..='\u{206F}' => true,
        '\u{FEFF}' | '\u{061C}' | '\u{180E}' => true,
        // Tag characters, and the variation selectors supplement after them.
        '\u{E0000}'..='\u{E0FFF}' => true,
        // The other default-ignorable characters: the soft hyphen, the
        // combining grapheme joiner, the Hangul fillers, the Khmer inherent
        // vowels, the Mongolian free variation selectors, the variation
        // selectors, the halfwidth Hangul filler, the specials before the
        // object replacement character, the Duployan shorthand format controls
        // and the musical symbol format controls.
        '\u{00AD}'
        | '\u{034F}'
        | '\u{115F}'..='\u{1160}'
        | '\u{17B4}'..='\u{17B5}'
        | '\u{180B}'..='\u{180F}'
        | '\u{3164}'
        | '\u{FE00}'..='\u{FE0F}'
        | '\u{FFA0}'
        | '\u{FFF0}'..='\u{FFFB}'
        | '\u{1BCA0}'..='\u{1BCA3}'
        | '\u{1D173}'..='\u{1D17A}' => true,
        // Format characters (category Cf) that the lists above did not hold, a
        // prefix to a number or a mark that runs into the text around it: the
        // Arabic number signs and end of ayah, the Syriac abbreviation mark,
        // the Arabic pound and piastre marks above, the Arabic disputed end of
        // ayah, and the Kaithi number sign.
        '\u{0600}'..='\u{0605}'
        | '\u{06DD}'
        | '\u{070F}'
        | '\u{0890}'..='\u{0891}'
        | '\u{08E2}'
        | '\u{110BD}' => true,
        _ => false,
    }
}

/// Keeps the first `head` and last `tail` characters of a long text with a
/// marker in between (REQ-CTX-006, LIM-C2). Returns the excerpt and, when
/// cut, the total character count.
pub fn excerpt(text: &str, head: usize, tail: usize) -> (String, Option<usize>) {
    let total = text.chars().count();
    if total <= head + tail {
        return (text.to_owned(), None);
    }
    let head_text: String = text.chars().take(head).collect();
    let tail_text: String = text.chars().skip(total - tail).collect();
    let omitted = total - head - tail;
    (
        format!("{head_text}\n[… {omitted} characters omitted …]\n{tail_text}"),
        Some(total),
    )
}

#[cfg(test)]
mod tests;
