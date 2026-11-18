//! `mimlys task output <task> [<action>] [--stdout | --stderr | --result]` (handoff 004d;
//! REQ-PERS-005; external design §19.2): the user's copies of what an action produced, whole.
//!
//! The model reads an excerpt of each stream (LIM-C2) and a bounded result (LIM-I19); the
//! runtime keeps the whole of each for the user: a stream's whole text as a blob, a result's as
//! `full_payload`. This command shows them. Without a flag it says what the action has: for the
//! result and each stream, its size, whether the model read a cut of it, and the flag that shows
//! it. With a flag it prints that copy and nothing else, so that its output can be piped.
//!
//! It is a reader: it holds a [`ReadApp`], which opens the store without its lock, so it answers
//! while another runtime holds the store (REQ-PERS-013).

use crate::app::{AppError, ReadApp};
use crate::messages;
use crate::output::Output;
use mimlys_core::action::{Action, Execution};
use mimlys_core::ids::{ActionId, TaskId};
use mimlys_core::observation::{Notice, Observation, Stream};
use mimlys_core::runtime::RuntimeError;
use mimlys_core::spi::{PersistenceError, PersistenceRead};
use serde::Serialize;
use serde_json::{Value, json};

/// The copy a flag names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Which {
    /// `--stdout`.
    Stdout,
    /// `--stderr`.
    Stderr,
    /// `--result`.
    Result,
}

impl Which {
    /// The word `--json` calls the copy.
    pub fn kind(self) -> &'static str {
        match self {
            Which::Stdout => "stdout",
            Which::Stderr => "stderr",
            Which::Result => "result",
        }
    }

    /// The flag that shows the copy.
    pub fn flag(self) -> &'static str {
        match self {
            Which::Stdout => "--stdout",
            Which::Stderr => "--stderr",
            Which::Result => "--result",
        }
    }

    /// The words the human text calls the copy.
    pub fn words(self) -> &'static str {
        match self {
            Which::Stdout => "standard output",
            Which::Stderr => "standard error",
            Which::Result => "result",
        }
    }

    /// The three copies, in the order the index lists them.
    pub const ALL: [Which; 3] = [Which::Result, Which::Stdout, Which::Stderr];
}

/// Where a copy's text comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// A stream's blob: its whole text, kept because the excerpt is a cut of it.
    Blob,
    /// A stream's excerpt, which is its whole text (nothing was cut).
    Excerpt,
    /// The observation's `full_payload`: the result as it was before it was cut for the model.
    FullPayload,
    /// The observation's payload, which is the whole result (nothing was cut).
    Payload,
}

/// Why a copy cannot be shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Unavailable {
    /// An external tool has no streams.
    NoStreams,
    /// The model read a cut of the stream and no whole copy was kept: the action ran before the
    /// release that keeps it, or the copy could not be written.
    NotKept,
    /// The execution names a blob that is gone: `task delete` removed it with the task's outputs.
    Removed,
}

/// One copy, as the action has it.
#[derive(Clone, Debug, PartialEq)]
pub struct Copy {
    /// Which.
    pub which: Which,
    /// What it is, or why it cannot be shown.
    pub state: Result<Held, Unavailable>,
}

/// The copy itself, with what the index says of it.
#[derive(Clone, Debug, PartialEq)]
pub struct Held {
    /// Where it comes from.
    pub source: Source,
    /// Its size in bytes: a stream's text as UTF-8, a result as compact JSON.
    pub bytes: u64,
    /// Whether the model read a cut of it.
    pub cut: bool,
    /// The copy.
    pub content: Content,
}

/// A copy's content.
#[derive(Clone, Debug, PartialEq)]
pub enum Content {
    /// A stream's text.
    Text(String),
    /// A result.
    Json(Value),
}

/// The action's latest execution and the observation it recorded.
#[derive(Clone, Debug)]
pub struct Found {
    /// The task.
    pub task_id: TaskId,
    /// The action.
    pub action: Action,
    /// Its latest execution.
    pub execution: Execution,
    /// The observation that execution recorded.
    pub observation: Observation,
}

fn refusal(kind: &'static str, detail: impl Into<String>) -> AppError {
    AppError::Output {
        detail: detail.into(),
        kind,
    }
}

/// The action `text` names among `actions`: an id, or a unique prefix of at least four
/// characters, as for a task.
fn resolve_action<'a>(actions: &'a [Action], text: &str) -> Result<&'a Action, AppError> {
    let text = text.trim();
    if let Ok(uuid) = uuid::Uuid::parse_str(text) {
        let id = ActionId::from_uuid(uuid);
        return actions
            .iter()
            .find(|a| a.action_id == id)
            .ok_or_else(|| refusal("not_found", format!("the task has no action {text}")));
    }
    if text.len() < 4 {
        return Err(refusal(
            "invalid_request",
            "give an action id or at least four characters of it",
        ));
    }
    let matches: Vec<&Action> = actions
        .iter()
        .filter(|a| a.action_id.to_string().starts_with(text))
        .collect();
    match matches.as_slice() {
        [one] => Ok(one),
        [] => Err(refusal(
            "not_found",
            format!("no action of the task starts with {text}"),
        )),
        many => Err(refusal(
            "invalid_request",
            format!(
                "{} actions start with {text}; give more characters",
                many.len()
            ),
        )),
    }
}

/// The action's latest execution with its observation. The action is the one named, else the
/// task's latest that ran. An action that never ran (it was rejected, cancelled before it began,
/// or is still waiting for its approval) has no execution and says so.
pub fn find(
    persistence: &dyn PersistenceRead,
    task_id: TaskId,
    action: Option<&str>,
) -> Result<Found, AppError> {
    if persistence.load_task(task_id)?.is_none() {
        return Err(AppError::Runtime(RuntimeError::NotFound {
            what: "task".into(),
        }));
    }
    let actions = persistence.load_actions(task_id)?;
    let (action, executions) = match action {
        Some(text) => {
            let action = resolve_action(&actions, text)?;
            let executions = persistence.load_executions(action.action_id)?;
            if executions.is_empty() {
                return Err(refusal(
                    "state",
                    messages::action_never_ran(
                        &action.action_id.to_string(),
                        action.content().tool(),
                        action.status.name(),
                    ),
                ));
            }
            (action.clone(), executions)
        }
        None => {
            let mut latest = None;
            for action in actions.iter().rev() {
                let executions = persistence.load_executions(action.action_id)?;
                if !executions.is_empty() {
                    latest = Some((action.clone(), executions));
                    break;
                }
            }
            latest.ok_or_else(|| refusal("state", messages::task_has_no_action_that_ran()))?
        }
    };
    let execution = executions
        .into_iter()
        .max_by_key(|e| e.attempt)
        .expect("at least one execution");
    // An execution and its observation are one transaction; an action that was stopped before its
    // call also has an observation, which no execution stands behind.
    let observation = persistence
        .load_observations(task_id)?
        .into_iter()
        .rfind(|o| {
            o.action_id == action.action_id
                && !o
                    .notices
                    .iter()
                    .any(|n| matches!(n, Notice::StoppedBeforeStart))
        })
        .ok_or_else(|| {
            AppError::Persistence(PersistenceError::NotFound {
                what: "observation".into(),
            })
        })?;
    Ok(Found {
        task_id,
        action,
        execution,
        observation,
    })
}

/// Whether the tool is an external one (`<server>.<tool>`): it has no streams.
fn is_external(tool: &str) -> bool {
    tool.contains('.')
}

fn text_copy(
    persistence: &dyn PersistenceRead,
    found: &Found,
    which: Which,
) -> Result<Result<Held, Unavailable>, AppError> {
    if is_external(found.action.content().tool()) {
        return Ok(Err(Unavailable::NoStreams));
    }
    let (blob, excerpt, stream) = match which {
        Which::Stdout => (
            found.execution.stdout_blob,
            &found.observation.stdout,
            Stream::Stdout,
        ),
        _ => (
            found.execution.stderr_blob,
            &found.observation.stderr,
            Stream::Stderr,
        ),
    };
    if let Some(blob) = blob {
        return Ok(match persistence.get_blob(blob) {
            Ok(bytes) => {
                let text = String::from_utf8_lossy(&bytes).into_owned();
                Ok(Held {
                    source: Source::Blob,
                    bytes: bytes.len() as u64,
                    cut: true,
                    content: Content::Text(text),
                })
            }
            Err(PersistenceError::NotFound { .. }) => Err(Unavailable::Removed),
            Err(e) => return Err(e.into()),
        });
    }
    // No blob: the excerpt is the whole text unless the observation says it was cut.
    let cut = found
        .observation
        .notices
        .iter()
        .any(|n| matches!(n, Notice::OutputTruncated { stream: s, .. } if *s == stream));
    if cut {
        return Ok(Err(Unavailable::NotKept));
    }
    Ok(Ok(Held {
        source: Source::Excerpt,
        bytes: excerpt.len() as u64,
        cut: false,
        content: Content::Text(excerpt.clone()),
    }))
}

fn result_copy(found: &Found) -> Result<Held, Unavailable> {
    let observation = &found.observation;
    let (source, value) = match &observation.full_payload {
        Some(whole) => (Source::FullPayload, whole.clone()),
        None => (Source::Payload, observation.payload.clone()),
    };
    let bytes = serde_json::to_vec(&value).map(|v| v.len()).unwrap_or(0) as u64;
    Ok(Held {
        cut: observation.full_payload.is_some(),
        source,
        bytes,
        content: Content::Json(value),
    })
}

/// The copy `which` of what `found` holds.
pub fn copy(
    persistence: &dyn PersistenceRead,
    found: &Found,
    which: Which,
) -> Result<Copy, AppError> {
    let state = match which {
        Which::Result => result_copy(found),
        Which::Stdout | Which::Stderr => text_copy(persistence, found, which)?,
    };
    Ok(Copy { which, state })
}

/// A carriage return that ends no line is shown as `\r`, so that no text can hide what precedes
/// it on its line (on a terminal a carriage return returns to the start of the line, and what
/// follows overwrites what was there). A carriage return that is part of a line ending
/// (`\r\n`) stays, since it hides nothing. Every other control character was removed by the
/// cleaning before the text was stored.
pub fn show_carriage_returns(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\r' && chars.peek() != Some(&'\n') {
            out.push_str("\\r");
        } else {
            out.push(c);
        }
    }
    out
}

/// The text a flag prints: exact when `terminal` is false; on a terminal the carriage returns
/// that end no line are made visible, and the last line is ended, so that the prompt that follows
/// is not written after it.
pub fn text_to_print(text: &str, terminal: bool) -> String {
    if !terminal {
        return text.to_owned();
    }
    let mut shown = show_carriage_returns(text);
    if !shown.is_empty() && !shown.ends_with('\n') {
        shown.push('\n');
    }
    shown
}

/// The index of an action's copies as JSON (`task output --json`, no flag).
#[derive(Clone, Debug, Serialize)]
pub struct IndexJson {
    task_id: String,
    action_id: String,
    execution_id: String,
    tool: String,
    attempt: u32,
    copies: Vec<CopyJson>,
}

/// One entry of the index.
#[derive(Clone, Debug, Serialize)]
struct CopyJson {
    kind: &'static str,
    bytes: Option<u64>,
    cut: bool,
    source: Option<Source>,
    flag: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    unavailable: Option<Unavailable>,
}

/// The index, from the copies.
pub fn index_json(found: &Found, copies: &[Copy]) -> IndexJson {
    IndexJson {
        task_id: found.task_id.to_string(),
        action_id: found.action.action_id.to_string(),
        execution_id: found.execution.execution_id.to_string(),
        tool: found.action.content().tool().to_owned(),
        attempt: found.execution.attempt,
        copies: copies
            .iter()
            .map(|c| match &c.state {
                Ok(held) => CopyJson {
                    kind: c.which.kind(),
                    bytes: Some(held.bytes),
                    cut: held.cut,
                    source: Some(held.source),
                    flag: c.which.flag(),
                    unavailable: None,
                },
                Err(why) => CopyJson {
                    kind: c.which.kind(),
                    bytes: None,
                    // A stream that was cut and has no whole copy is said cut; an external
                    // tool's stream has nothing to cut.
                    cut: !matches!(why, Unavailable::NoStreams),
                    source: None,
                    flag: c.which.flag(),
                    unavailable: Some(*why),
                },
            })
            .collect(),
    }
}

/// The copy a flag names, as JSON (`task output --json` with a flag).
pub fn copy_json(found: &Found, copy: &Copy, held: &Held) -> Value {
    let mut value = json!({
        "task_id": found.task_id.to_string(),
        "action_id": found.action.action_id.to_string(),
        "execution_id": found.execution.execution_id.to_string(),
        "tool": found.action.content().tool(),
        "attempt": found.execution.attempt,
        "kind": copy.which.kind(),
        "bytes": held.bytes,
        "cut": held.cut,
        "source": held.source,
    });
    match &held.content {
        Content::Text(text) => value["text"] = json!(text),
        Content::Json(result) => value["result"] = result.clone(),
    }
    value
}

/// What a flag cannot show, as an error.
fn unavailable_error(found: &Found, which: Which, why: Unavailable) -> AppError {
    let tool = found.action.content().tool();
    match why {
        Unavailable::NoStreams => refusal("state", messages::external_tool_has_no_streams(tool)),
        Unavailable::NotKept => refusal("not_found", messages::no_whole_copy_kept(which.words())),
        Unavailable::Removed => refusal("not_found", messages::whole_copy_removed(which.words())),
    }
}

/// `task output`. `terminal` says whether standard output is a terminal, which decides how a
/// carriage return is shown (a pipe or a file gets the text exactly).
pub fn run(
    app: &ReadApp,
    task: &str,
    action: Option<&str>,
    which: Option<Which>,
    terminal: bool,
    out: &Output,
) -> Result<(), AppError> {
    let task_id = app.resolve_task(task)?;
    let persistence = app.reader.persistence();
    let found = find(persistence, task_id, action)?;
    match which {
        None => {
            let copies = Which::ALL
                .iter()
                .map(|w| copy(persistence, &found, *w))
                .collect::<Result<Vec<_>, _>>()?;
            for line in messages::output_index(
                &found.task_id.to_string(),
                &found.action.action_id.to_string(),
                found.action.content().tool(),
                found.execution.attempt,
                found.observation.status.name(),
                &copies,
            ) {
                out.line(line);
            }
            out.value(&index_json(&found, &copies));
        }
        Some(which) => {
            let one = copy(persistence, &found, which)?;
            let held = match &one.state {
                Ok(held) => held,
                Err(why) => return Err(unavailable_error(&found, which, *why)),
            };
            if out.json() {
                out.value(&copy_json(&found, &one, held));
            } else {
                match &held.content {
                    Content::Text(text) => out.raw(&text_to_print(text, terminal)),
                    Content::Json(result) => {
                        let mut text = serde_json::to_string_pretty(result).unwrap_or_default();
                        text.push('\n');
                        out.raw(&text);
                    }
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
