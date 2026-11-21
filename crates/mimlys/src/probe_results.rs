//! The fitness probe's record, as `status` reads it (REQ-MDL-010, REQ-MDL-011; RFC 015 decision 5
//! and Amendment 1): one entry per backend, the latest, in `<data_dir>/probe-results.json`.
//!
//! This module reads. The command that writes the file (`mimlys model probe`, handoff 015a item
//! 5) writes the shape below, atomically and owner-only, and a reader never writes: `status` is a
//! read command (REQ-PERS-013).
//!
//! ```json
//! {
//!   "version": 1,
//!   "backends": {
//!     "default": {
//!       "class": "acts",
//!       "steps": 4,
//!       "model_ms_per_step": 820,
//!       "transport": "tool_calls",
//!       "model": "Qwen2.5-7B-Instruct-Q4_K_M.gguf",
//!       "at": "2026-11-22T10:15:00Z"
//!     },
//!     "small": { "class": "incomplete", "detail": "asked_user", "steps": 3, "at": "2026-11-22T10:16:30Z" }
//!   }
//! }
//! ```
//!
//! `class` is one of [`CLASSES`]; `detail` is the reason of an `incomplete` (`not_finished`,
//! `asked_user`, `gave_up`); `model` is the model the backend named when it was measured, so that a
//! result is not shown as the answer for another model; `at` is the instant, in UTC.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// The file's name under the data directory.
pub const FILE: &str = "probe-results.json";

/// The version of the file's shape.
pub const VERSION: u32 = 1;

/// The classes of a probe (REQ-MDL-011).
pub const CLASSES: [&str; 5] = [
    "acts",
    "incomplete",
    "plans_only",
    "no_structured_call",
    "unreachable",
];

/// What one probe found, as the file holds it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LastProbe {
    /// One of [`CLASSES`]; a word this release does not know is shown as it is, marked.
    pub class: String,
    /// For `incomplete`: `not_finished`, `asked_user` or `gave_up`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// The steps the mini-task took.
    #[serde(default)]
    pub steps: u32,
    /// The model's time per step, in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_ms_per_step: Option<u64>,
    /// The decision transport it ran under.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport: Option<String>,
    /// The model the backend named when it was measured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// When, in UTC (`2026-11-22T10:15:00Z`).
    pub at: String,
}

impl LastProbe {
    /// The day it was measured: the date part of `at`, or `at` as it is when it has none.
    pub fn date(&self) -> &str {
        match self.at.split_once('T') {
            Some((date, _)) if date.len() == 10 => date,
            _ => &self.at,
        }
    }

    /// Whether this release knows the class.
    pub fn class_known(&self) -> bool {
        CLASSES.contains(&self.class.as_str())
    }
}

#[derive(Deserialize)]
struct File {
    version: u32,
    #[serde(default)]
    backends: BTreeMap<String, LastProbe>,
}

/// The record, or why it cannot be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Results {
    /// What the file holds, per backend (empty when there is no file: nothing was probed).
    Read(BTreeMap<String, LastProbe>),
    /// The file is there and is not readable: the reason. It is not "not probed", and it is not
    /// repaired by a reader.
    Unreadable(String),
}

impl Results {
    /// The last probe of the backend named `name`, when there is one.
    pub fn of(&self, name: &str) -> Option<&LastProbe> {
        match self {
            Results::Read(map) => map.get(name),
            Results::Unreadable(_) => None,
        }
    }
}

/// Reads `<data_dir>/probe-results.json`. A file that is not there is no probe; one that cannot
/// be read, is not JSON of this shape, or is of a newer version is [`Results::Unreadable`].
pub fn read(data_dir: &Path) -> Results {
    let path = data_dir.join(FILE);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Results::Read(BTreeMap::new());
        }
        Err(e) => return Results::Unreadable(format!("cannot read {FILE}: {}", e.kind())),
    };
    match serde_json::from_str::<File>(&text) {
        Ok(file) if file.version == VERSION => Results::Read(file.backends),
        Ok(file) => Results::Unreadable(format!(
            "{FILE} is of version {}, which this release does not read (it reads {VERSION})",
            file.version
        )),
        Err(_) => Results::Unreadable(format!("{FILE} is not a record of the fitness probe")),
    }
}

#[cfg(test)]
mod tests;
