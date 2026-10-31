//! Lexical path canonicalization (external design §6.3 stage 6;
//! REQ-DEC-006, REQ-DEC-007, REQ-WS-011). No filesystem is touched: a
//! path that leaves the workspace root is reported as such so that the
//! validator can deny it identically whether or not it exists.

use serde::{Deserialize, Serialize};
use std::fmt;
use unicode_normalization::UnicodeNormalization;

/// Whether path comparison folds case (determined per workspace filesystem).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaseMode {
    /// `A.rs` and `a.rs` are different files.
    Sensitive,
    /// `A.rs` and `a.rs` are the same file (NTFS, APFS defaults).
    Insensitive,
}

/// Which platform's naming rules apply.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlatformRules {
    /// POSIX: any byte but NUL and `/`.
    Posix,
    /// Windows: reserved names, forbidden characters, no trailing dot or space.
    Windows,
}

impl PlatformRules {
    /// The rules of the platform this binary runs on.
    pub fn current() -> Self {
        if cfg!(windows) {
            PlatformRules::Windows
        } else {
            PlatformRules::Posix
        }
    }
}

/// Why a path was refused.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error, Serialize, Deserialize)]
#[serde(tag = "path_error", rename_all = "snake_case")]
pub enum PathError {
    /// Empty input.
    #[error("the path is empty")]
    Empty,
    /// A control character (including NUL).
    #[error("the path contains a control character")]
    ControlCharacter,
    /// An absolute path, a drive letter or a UNC prefix.
    #[error("absolute paths are not allowed")]
    Absolute,
    /// `..` would leave the workspace root (stage 7 denies this).
    #[error("the path leaves the workspace")]
    Escapes,
    /// A name invalid on the platform.
    #[error("invalid name `{name}`: {reason}")]
    InvalidName {
        /// The component.
        name: String,
        /// Why.
        reason: &'static str,
    },
}

/// A canonical path relative to the workspace root: forward slashes, no
/// `.` or `..`, NFC, never absolute. The empty path is the root.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WorkspacePath(String);

impl WorkspacePath {
    /// The workspace root.
    pub fn root() -> Self {
        Self(String::new())
    }

    /// Canonicalizes with the current platform's rules.
    pub fn canonicalize(input: &str) -> Result<Self, PathError> {
        Self::canonicalize_with(input, PlatformRules::current())
    }

    /// Canonicalizes with the given platform's rules.
    pub fn canonicalize_with(input: &str, rules: PlatformRules) -> Result<Self, PathError> {
        if input.is_empty() {
            return Err(PathError::Empty);
        }
        if input.chars().any(|c| c.is_control()) {
            return Err(PathError::ControlCharacter);
        }
        let normalized: String = input.nfc().collect();
        if is_absolute(&normalized) {
            return Err(PathError::Absolute);
        }
        let mut components: Vec<&str> = Vec::new();
        for component in normalized.split(['/', '\\']) {
            match component {
                "" | "." => {}
                ".." => {
                    if components.pop().is_none() {
                        return Err(PathError::Escapes);
                    }
                }
                name => {
                    check_name(name, rules)?;
                    components.push(name);
                }
            }
        }
        Ok(Self(components.join("/")))
    }

    /// The canonical text; empty for the root.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether this is the root.
    pub fn is_root(&self) -> bool {
        self.0.is_empty()
    }

    /// The components.
    pub fn components(&self) -> impl Iterator<Item = &str> {
        self.0.split('/').filter(|c| !c.is_empty())
    }

    /// The parent, or `None` for the root.
    pub fn parent(&self) -> Option<WorkspacePath> {
        if self.is_root() {
            return None;
        }
        Some(match self.0.rfind('/') {
            Some(i) => WorkspacePath(self.0[..i].to_owned()),
            None => WorkspacePath::root(),
        })
    }

    /// The last component, or `None` for the root.
    pub fn file_name(&self) -> Option<&str> {
        self.components().last()
    }

    /// Whether two paths name the same entry under `mode`.
    pub fn same_as(&self, other: &WorkspacePath, mode: CaseMode) -> bool {
        match mode {
            CaseMode::Sensitive => self.0 == other.0,
            CaseMode::Insensitive => self.0.to_lowercase() == other.0.to_lowercase(),
        }
    }

    /// Whether `self` is `ancestor` or lies below it, under `mode`.
    pub fn starts_with(&self, ancestor: &WorkspacePath, mode: CaseMode) -> bool {
        if ancestor.is_root() {
            return true;
        }
        let (a, b) = match mode {
            CaseMode::Sensitive => (self.0.clone(), ancestor.0.clone()),
            CaseMode::Insensitive => (self.0.to_lowercase(), ancestor.0.to_lowercase()),
        };
        a == b || a.starts_with(&format!("{b}/"))
    }
}

impl fmt::Display for WorkspacePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_root() {
            f.write_str(".")
        } else {
            f.write_str(&self.0)
        }
    }
}

impl fmt::Debug for WorkspacePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "WorkspacePath({self})")
    }
}

fn is_absolute(text: &str) -> bool {
    if text.starts_with('/') || text.starts_with('\\') {
        return true;
    }
    let bytes = text.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

const WINDOWS_RESERVED: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

fn check_name(name: &str, rules: PlatformRules) -> Result<(), PathError> {
    if rules == PlatformRules::Windows {
        if name.contains(['<', '>', ':', '"', '|', '?', '*']) {
            return Err(PathError::InvalidName {
                name: name.to_owned(),
                reason: "contains a character Windows forbids",
            });
        }
        if name.ends_with('.') || name.ends_with(' ') {
            return Err(PathError::InvalidName {
                name: name.to_owned(),
                reason: "ends with a dot or a space",
            });
        }
        let stem = name.split('.').next().unwrap_or(name).to_ascii_uppercase();
        if WINDOWS_RESERVED.contains(&stem.as_str()) {
            return Err(PathError::InvalidName {
                name: name.to_owned(),
                reason: "is a reserved device name",
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
