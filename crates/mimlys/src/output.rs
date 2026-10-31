//! Human lines or one JSON value per command (REQ-CLI-004).

use crate::app::AppError;
use serde::Serialize;

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
            println!("{}", text.as_ref());
        }
    }

    /// The command's JSON value (nothing in human mode).
    pub fn value<T: Serialize>(&self, value: &T) {
        if self.json {
            println!("{}", serde_json::to_string(value).unwrap_or_default());
        }
    }

    /// An error: the JSON error object on standard output in JSON mode,
    /// else one line on standard error.
    pub fn error(&self, error: &AppError) {
        if self.json {
            println!("{}", error.to_json());
        } else {
            eprintln!(
                "{}",
                crate::messages::error_line(error.kind(), &error.to_string())
            );
        }
    }
}
