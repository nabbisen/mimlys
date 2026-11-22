//! Words a newer release wrote (REQ-MAIN-006; external design §22, "A reader meets a newer
//! release's word"; handoff 004e).
//!
//! **One rule for every kind of value.** A record of the store is read through [`Read::parse`]:
//!
//! - a record whose words are all known is [`Read::Known`], the typed value it always was: the
//!   twelve stages, the runtime and the writers see nothing else, and no "unknown" variant exists
//!   in any of their types;
//! - a record that fails to read *only because* a fixed vocabulary (an event name, a task state, a
//!   wait kind, a reason, a status, an effect, …) holds a word this release does not know is
//!   [`Read::Newer`]: the record as written (`raw`), and each unknown word with the field that
//!   holds it. It is never turned into a known value;
//! - any other failure (a missing field, a wrong type) is a damaged record and stays
//!   `storage failure: deserialize`.
//!
//! Which word is unknown is found by the reading itself, not by a list of vocabularies kept beside
//! the types: serde names the variant it did not know, and each place in the record where that
//! text stands is tried by replacing it with a text no vocabulary has; the place whose
//! replacement is then the unknown variant is the vocabulary's. A vocabulary that is added or
//! extended is therefore covered with no change here.
//!
//! The readers show the word with [`MARKER_TEXT`]; their JSON keeps the record as written and
//! adds [`MARKER_FIELD`] to the object that holds the word, listing `{ "field", "word" }`.
//! A record that holds no unknown word never carries the key.

use crate::spi::PersistenceError;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// What a reader prints beside a word it does not know, in parentheses.
pub const MARKER_TEXT: &str = "unknown to this release";

/// The JSON key that lists the unknown words of the object that holds it.
pub const MARKER_FIELD: &str = "unknown_to_this_release";

/// A word a fixed vocabulary does not have in this release, with where it stands.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct UnknownWord {
    /// The field of the record that holds it, from the record's root, with `.` between the
    /// names (`wait_kind`, `pending.pending`, `effect.kind`). For a map or a list, the field
    /// that holds the map or the list (`events_by_name`). `event` for the name of an event.
    pub field: String,
    /// The word as written.
    pub word: String,
}

impl UnknownWord {
    /// A word at a field.
    pub fn new(field: impl Into<String>, word: impl Into<String>) -> Self {
        Self {
            field: field.into(),
            word: word.into(),
        }
    }

    /// `field `word` (unknown to this release)`: the text a reader prints.
    pub fn text(&self) -> String {
        format!("{} `{}` ({MARKER_TEXT})", self.field, self.word)
    }
}

/// The JSON a command prints for a list of words: the key and its value, to merge into the
/// object that holds them. Empty when there is none.
pub fn marker_json(words: &[UnknownWord]) -> Option<Value> {
    if words.is_empty() {
        return None;
    }
    serde_json::to_value(words).ok()
}

/// Adds [`MARKER_FIELD`] to `value`, when it is an object and `words` is not empty.
pub fn mark(value: &mut Value, words: &[UnknownWord]) {
    if let (Some(marker), Some(object)) = (marker_json(words), value.as_object_mut()) {
        object.insert(MARKER_FIELD.to_owned(), marker);
    }
}

/// A record that holds a word this release does not know: held as it was written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Newer {
    /// The unknown words, in the order they were found; never empty.
    pub words: Vec<UnknownWord>,
    /// The record as written.
    pub raw: Value,
}

impl Newer {
    /// The record as written, with [`MARKER_FIELD`] added: what `--json` prints.
    pub fn to_json(&self) -> Value {
        let mut value = self.raw.clone();
        mark(&mut value, &self.words);
        value
    }

    /// A text field of the record as written.
    pub fn text(&self, field: &str) -> Option<&str> {
        self.raw.get(field).and_then(Value::as_str)
    }

    /// Whether `field` is one of the fields that hold an unknown word.
    pub fn is_unknown(&self, field: &str) -> bool {
        self.words.iter().any(|w| w.field == field)
    }

    /// Whether the top-level field `top` holds an unknown word, itself or in what it contains
    /// (`pending` for `pending.pending`).
    pub fn is_unknown_in(&self, top: &str) -> bool {
        self.words.iter().any(|w| {
            w.field == top
                || w.field
                    .strip_prefix(top)
                    .is_some_and(|rest| rest.starts_with('.') || rest.starts_with('['))
        })
    }

    /// The first unknown word, which the refusals name.
    pub fn first(&self) -> &UnknownWord {
        &self.words[0]
    }
}

/// A record as a reader meets it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Read<T> {
    /// Every word is known: the typed value.
    Known(T),
    /// A word is not known to this release.
    Newer(Newer),
}

impl<T: Serialize> Serialize for Read<T> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Read::Known(value) => value.serialize(serializer),
            Read::Newer(newer) => newer.to_json().serialize(serializer),
        }
    }
}

/// The text no vocabulary has, put where a word was to find out whether the place is a
/// vocabulary's.
const SENTINEL: &str = "\u{1}word-of-no-vocabulary\u{1}";

/// How many unknown words of one record are named: more than a record has.
const MAX_WORDS: usize = 8;

impl<T: DeserializeOwned> Read<T> {
    /// Reads a record's text. A damaged record (not an unknown word) is the same failure it
    /// always was.
    pub fn parse(text: &str) -> Result<Self, PersistenceError> {
        match serde_json::from_str::<T>(text) {
            Ok(value) => Ok(Read::Known(value)),
            Err(error) => match find_words::<T>(text, &error.to_string()) {
                Some(newer) => Ok(Read::Newer(newer)),
                None => Err(PersistenceError::Storage {
                    detail: "deserialize".into(),
                }),
            },
        }
    }
}

impl<T: Stored> Read<T> {
    /// The typed value, or the refusal that names the word: what a writer and the runtime read.
    pub fn strict(self) -> Result<T, PersistenceError> {
        match self {
            Read::Known(value) => Ok(value),
            Read::Newer(newer) => Err(newer_error(T::KIND.name(), &newer)),
        }
    }
}

/// A type the store keeps as a record of a kind.
pub trait Stored: DeserializeOwned {
    /// The kind of record it is.
    const KIND: RecordKind;
}

/// The refusal for a record that holds a word this release does not know.
pub fn newer_error(record: &str, newer: &Newer) -> PersistenceError {
    let first = newer.first();
    PersistenceError::NewerRecord {
        record: record.to_owned(),
        field: first.field.clone(),
        word: first.word.clone(),
    }
}

/// Reads a record's text for a writer or the runtime: the typed value, or the refusal.
pub fn parse_strict<T: Stored>(text: &str) -> Result<T, PersistenceError> {
    Read::<T>::parse(text)?.strict()
}

/// The word an "unknown variant" message names, and the first variant it says is expected.
fn unknown_variant(message: &str) -> Option<(&str, Option<&str>)> {
    const START: &str = "unknown variant `";
    const MIDDLE: &str = "`, expected ";
    let start = message.find(START)? + START.len();
    let rest = &message[start..];
    let end = rest.find(MIDDLE)?;
    let word = &rest[..end];
    let expected = &rest[end + MIDDLE.len()..];
    let first = expected
        .strip_prefix("one of ")
        .unwrap_or(expected)
        .strip_prefix('`')
        .and_then(|e| e.split('`').next());
    Some((word, first))
}

/// One step of a path into a JSON value.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Seg {
    Key(String),
    Index(usize),
}

fn string_paths(value: &Value, word: &str, at: &mut Vec<Seg>, out: &mut Vec<Vec<Seg>>) {
    match value {
        Value::String(s) if s == word => out.push(at.clone()),
        Value::Object(map) => {
            for (key, inner) in map {
                at.push(Seg::Key(key.clone()));
                string_paths(inner, word, at, out);
                at.pop();
            }
        }
        Value::Array(items) => {
            for (i, inner) in items.iter().enumerate() {
                at.push(Seg::Index(i));
                string_paths(inner, word, at, out);
                at.pop();
            }
        }
        _ => {}
    }
}

fn set(value: &mut Value, path: &[Seg], text: &str) {
    let mut at = value;
    for step in path {
        at = match (step, at) {
            (Seg::Key(key), Value::Object(map)) => match map.get_mut(key) {
                Some(inner) => inner,
                None => return,
            },
            (Seg::Index(i), Value::Array(items)) => match items.get_mut(*i) {
                Some(inner) => inner,
                None => return,
            },
            _ => return,
        };
    }
    *at = Value::String(text.to_owned());
}

/// The field a path names: the keys joined by `.`, an index as `[i]`.
fn field_of(path: &[Seg]) -> String {
    let mut out = String::new();
    for step in path {
        match step {
            Seg::Key(key) => {
                if !out.is_empty() {
                    out.push('.');
                }
                out.push_str(key);
            }
            Seg::Index(i) => out.push_str(&format!("[{i}]")),
        }
    }
    out
}

fn find_words<T: DeserializeOwned>(text: &str, first_error: &str) -> Option<Newer> {
    let raw: Value = serde_json::from_str(text).ok()?;
    let mut work = raw.clone();
    let mut words: Vec<UnknownWord> = Vec::new();
    let mut message = first_error.to_owned();
    for _ in 0..MAX_WORDS {
        let Some((word, expected)) = unknown_variant(&message) else {
            break;
        };
        let (word, expected) = (word.to_owned(), expected.map(str::to_owned));
        let mut paths = Vec::new();
        string_paths(&work, &word, &mut Vec::new(), &mut paths);
        // The place that is the vocabulary's: with the sentinel put there, the record's reading
        // fails on the sentinel. A text that stands elsewhere only by chance (a free string)
        // leaves the reading failing on the word.
        let found = paths.into_iter().find(|path| {
            let mut probe = work.clone();
            set(&mut probe, path, SENTINEL);
            matches!(
                serde_json::from_value::<T>(probe),
                Err(e) if unknown_variant(&e.to_string()).is_some_and(|(w, _)| w == SENTINEL)
            )
        });
        let Some(path) = found else {
            break;
        };
        words.push(UnknownWord::new(field_of(&path), word));
        // To find a second unknown word, this place takes a word its vocabulary has.
        let Some(known) = expected else {
            break;
        };
        set(&mut work, &path, &known);
        match serde_json::from_value::<T>(work.clone()) {
            Ok(_) => break,
            Err(e) => message = e.to_string(),
        }
    }
    (!words.is_empty()).then_some(Newer { words, raw })
}

/// The kinds of record a task owns, one table of the store each. [`RecordKind::check`] matches
/// every kind with no wildcard: a record type that is added does not compile until it says which
/// type reads it, so the check of a task's records cannot miss one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordKind {
    /// The task itself.
    Task,
    /// A step.
    Step,
    /// An action.
    Action,
    /// One execution of an action.
    Execution,
    /// An observation.
    Observation,
    /// An approval request.
    Approval,
    /// A question to the user.
    Question,
    /// A grant.
    Grant,
    /// A plan version.
    Plan,
    /// An event.
    Event,
    /// A snapshot.
    Snapshot,
    /// A promotion.
    Promotion,
}

impl RecordKind {
    /// Every kind.
    pub const ALL: [RecordKind; 12] = [
        RecordKind::Task,
        RecordKind::Step,
        RecordKind::Action,
        RecordKind::Execution,
        RecordKind::Observation,
        RecordKind::Approval,
        RecordKind::Question,
        RecordKind::Grant,
        RecordKind::Plan,
        RecordKind::Event,
        RecordKind::Snapshot,
        RecordKind::Promotion,
    ];

    /// The word the refusals and the readers use for the kind.
    pub fn name(self) -> &'static str {
        match self {
            RecordKind::Task => "task",
            RecordKind::Step => "step",
            RecordKind::Action => "action",
            RecordKind::Execution => "execution",
            RecordKind::Observation => "observation",
            RecordKind::Approval => "approval",
            RecordKind::Question => "question",
            RecordKind::Grant => "grant",
            RecordKind::Plan => "plan",
            RecordKind::Event => "event",
            RecordKind::Snapshot => "snapshot",
            RecordKind::Promotion => "promotion",
        }
    }

    /// Reads a record of this kind: `Some` when it holds a word this release does not know,
    /// `None` when it reads whole, the damage when it is damaged.
    pub fn check(self, text: &str) -> Result<Option<Newer>, PersistenceError> {
        fn newer_of<T: DeserializeOwned>(text: &str) -> Result<Option<Newer>, PersistenceError> {
            Ok(match Read::<T>::parse(text)? {
                Read::Known(_) => None,
                Read::Newer(newer) => Some(newer),
            })
        }
        match self {
            RecordKind::Task => newer_of::<crate::task::Task>(text),
            RecordKind::Step => newer_of::<crate::step::Step>(text),
            RecordKind::Action => newer_of::<crate::action::Action>(text),
            RecordKind::Execution => newer_of::<crate::action::Execution>(text),
            RecordKind::Observation => newer_of::<crate::observation::Observation>(text),
            RecordKind::Approval => newer_of::<crate::approval::ApprovalRequest>(text),
            RecordKind::Question => newer_of::<crate::question::Question>(text),
            RecordKind::Grant => newer_of::<crate::grant::Grant>(text),
            RecordKind::Plan => newer_of::<crate::task::PlanVersion>(text),
            RecordKind::Event => newer_of::<crate::event::EventRecord>(text),
            RecordKind::Snapshot => newer_of::<crate::snapshot::SnapshotRecord>(text),
            RecordKind::Promotion => newer_of::<crate::snapshot::Promotion>(text),
        }
    }
}

macro_rules! stored {
    ($($t:ty => $k:ident),* $(,)?) => {
        $(impl Stored for $t { const KIND: RecordKind = RecordKind::$k; })*
    };
}

stored! {
    crate::task::Task => Task,
    crate::step::Step => Step,
    crate::action::Action => Action,
    crate::action::Execution => Execution,
    crate::observation::Observation => Observation,
    crate::approval::ApprovalRequest => Approval,
    crate::question::Question => Question,
    crate::grant::Grant => Grant,
    crate::task::PlanVersion => Plan,
    crate::event::EventRecord => Event,
    crate::snapshot::SnapshotRecord => Snapshot,
    crate::snapshot::Promotion => Promotion,
}

/// A record as stored: its text, unread. `key` is the record's identifier, or an event's
/// position.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawRecord {
    /// The kind of record.
    pub kind: RecordKind,
    /// The identifier (the position, for an event).
    pub key: String,
    /// The JSON text of the record.
    pub text: String,
}

/// A record of a task that holds a word this release does not know.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewerRecord {
    /// The kind of record.
    pub kind: RecordKind,
    /// Its identifier (an event's position).
    pub key: String,
    /// What it holds.
    pub newer: Newer,
}

impl NewerRecord {
    /// The refusal that names the first word.
    pub fn error(&self) -> PersistenceError {
        newer_error(self.kind.name(), &self.newer)
    }
}

/// Every record of the task that holds a word this release does not know, whatever the table:
/// the check a command that would change or run the task makes first (item 2 of handoff 004e).
/// A damaged record is not "newer": it is the failure it always was, when it is read.
pub fn scan(
    persistence: &dyn crate::spi::PersistenceRead,
    task_id: crate::ids::TaskId,
) -> Result<Vec<NewerRecord>, PersistenceError> {
    let mut out = Vec::new();
    for kind in RecordKind::ALL {
        for record in persistence.raw_records(kind, task_id)? {
            // A damaged record is no word of a newer release; it fails when it is read.
            if let Ok(Some(newer)) = kind.check(&record.text) {
                out.push(NewerRecord {
                    kind,
                    key: record.key,
                    newer,
                });
            }
        }
    }
    Ok(out)
}

/// The refusal a writer makes: the first record of the task that holds a word this release does
/// not know, or nothing.
pub fn refuse_if_newer(
    persistence: &dyn crate::spi::PersistenceRead,
    task_id: crate::ids::TaskId,
) -> Result<(), PersistenceError> {
    match scan(persistence, task_id)?.first() {
        Some(record) => Err(record.error()),
        None => Ok(()),
    }
}

/// The records of one kind of a task, each as a reader meets it.
pub fn read_records<T: DeserializeOwned>(
    persistence: &dyn crate::spi::PersistenceRead,
    kind: RecordKind,
    task_id: crate::ids::TaskId,
) -> Result<Vec<(String, Read<T>)>, PersistenceError> {
    persistence
        .raw_records(kind, task_id)?
        .into_iter()
        .map(|record| Read::<T>::parse(&record.text).map(|read| (record.key, read)))
        .collect()
}

#[cfg(test)]
mod tests;
