//! Host tools as administrator declarations (RFC 011 §6; REQ-CONF-006,
//! REQ-CONF-008): a fixed command and arguments, an allow-list and a
//! pattern for what the model may add, the effect, the deadline, the
//! toolchain read paths and the environment. `run_host_tool{tool, args}`
//! is validated here; no shell is ever involved.

use crate::action::{EffectKind, EffectView};
use crate::limits::LIM_H1_HOST_DEADLINE_MS;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// One declared host tool.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostToolDeclaration {
    /// The name the model uses (`cargo_test`).
    pub name: String,
    /// The program: an absolute path, or a file name resolved against the
    /// `bin` directories of `read_paths` only (never the inherited `PATH`).
    pub command: String,
    /// Fixed arguments, always passed first.
    #[serde(default)]
    pub args: Vec<String>,
    /// Literal arguments the model may add.
    #[serde(default)]
    pub allowed_args: Vec<String>,
    /// A regular expression any other argument must match; absent means none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arg_pattern: Option<String>,
    /// How many arguments the model may add.
    #[serde(default)]
    pub max_args: usize,
    /// `workspace_write` (writes only inside the workspace), `host_write`, or
    /// `external_read` exactly when `network` is `true` (the dependency step).
    pub effect: EffectKind,
    /// Whether a `host_write` is reversible (the tool backs up first).
    #[serde(default)]
    pub reversible: bool,
    /// The deadline (LIM-H1).
    #[serde(default = "default_timeout")]
    pub timeout_ms: i64,
    /// Toolchain and cache directories the tool may read.
    #[serde(default)]
    pub read_paths: Vec<String>,
    /// Fixed environment variables.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Variables copied from the runtime's environment.
    #[serde(default)]
    pub env_allow: Vec<String>,
    /// `true` makes this the dependency step: clause N lifted; the effect must
    /// then be declared `external_read` (REQ-CONF-007, REQ-CONF-006).
    #[serde(default)]
    pub network: bool,
}

fn default_timeout() -> i64 {
    LIM_H1_HOST_DEADLINE_MS
}

/// The one tool through which every declared host tool runs (external design §11).
pub const RUN_HOST_TOOL: &str = "run_host_tool";

/// Why `run_host_tool` is never granted or listed by name (external design §9.1, §13.5).
pub const RUN_HOST_TOOL_NOT_A_GRANT: &str = "`run_host_tool` is not granted by name: a `host_tool:<name>` grant makes it visible for that tool (`[policy.profiles.<p>] host_tools` grants at creation; `approval_tools` may still name it)";

/// Why an argument list is refused.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
pub enum ArgViolation {
    /// More arguments than `max_args`.
    #[error("at most {max} arguments may be added")]
    TooMany {
        /// The bound.
        max: usize,
    },
    /// An argument is neither a literal of `allowed_args` nor a match of `arg_pattern`.
    #[error("argument {index} is not allowed: {argument:?}")]
    NotAllowed {
        /// Its position.
        index: usize,
        /// The argument, bounded.
        argument: String,
    },
}

impl HostToolDeclaration {
    /// The declared effect (RFC 011 §6, §8; REQ-CONF-006): `check` has
    /// already refused every declaration whose effect and `network` disagree,
    /// so this derives nothing from `network` and the file says what the
    /// runtime does (CLR-1). `workspace_write` is reversible through the
    /// snapshots; `external_read` never is.
    pub fn effect_view(&self) -> EffectView {
        EffectView {
            kind: self.effect,
            reversible: match self.effect {
                EffectKind::WorkspaceWrite => true,
                EffectKind::HostWrite => self.reversible,
                _ => false,
            },
            idempotent: false,
        }
    }

    /// Checks the model's arguments: each a literal of `allowed_args` or a
    /// match of `arg_pattern`, never `..` or an absolute path unless the
    /// pattern allows it, at most `max_args`.
    pub fn validate_args(&self, args: &[String]) -> Result<Vec<String>, ArgViolation> {
        if args.len() > self.max_args {
            return Err(ArgViolation::TooMany { max: self.max_args });
        }
        let pattern = self
            .arg_pattern
            .as_deref()
            .and_then(|p| regex::Regex::new(&format!("^(?:{p})$")).ok());
        for (index, a) in args.iter().enumerate() {
            let literal = self.allowed_args.iter().any(|l| l == a);
            let matched = pattern.as_ref().is_some_and(|p| p.is_match(a));
            let suspicious = a.contains("..") || a.starts_with('/') || a.starts_with('\\');
            if !(literal || (matched && (!suspicious || pattern_allows_paths(&self.arg_pattern)))) {
                return Err(ArgViolation::NotAllowed {
                    index,
                    argument: a.chars().take(80).collect(),
                });
            }
        }
        Ok(args.to_vec())
    }

    /// The command with its fixed arguments and the validated ones.
    pub fn command_line(&self, validated: &[String]) -> Vec<String> {
        let mut line = Vec::with_capacity(1 + self.args.len() + validated.len());
        line.push(self.command.clone());
        line.extend(self.args.iter().cloned());
        line.extend(validated.iter().cloned());
        line
    }
}

/// A pattern that names `/` or `.` explicitly is taken to allow paths.
fn pattern_allows_paths(pattern: &Option<String>) -> bool {
    pattern
        .as_deref()
        .is_some_and(|p| p.contains('/') || p.contains("\\."))
}

/// The effect as the configuration spells it (snake_case, CLR-6).
fn effect_name(effect: EffectKind) -> &'static str {
    match effect {
        EffectKind::None => "none",
        EffectKind::WorkspaceRead => "workspace_read",
        EffectKind::WorkspaceWrite => "workspace_write",
        EffectKind::HostRead => "host_read",
        EffectKind::HostWrite => "host_write",
        EffectKind::ExternalRead => "external_read",
        EffectKind::ExternalWrite => "external_write",
    }
}

/// A validation problem of a declaration (REQ-CFG-001 names the key).
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{key}: {reason}")]
pub struct DeclarationError {
    /// The key path under `host_tools.<name>`.
    pub key: String,
    /// What is wrong.
    pub reason: String,
}

impl HostToolDeclaration {
    /// Checks the declaration's own rules.
    pub fn check(&self) -> Result<(), DeclarationError> {
        let err = |key: &str, reason: String| DeclarationError {
            key: format!("host_tools.{}.{key}", self.name),
            reason,
        };
        if self.name.is_empty()
            || !self
                .name
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_lowercase())
            || !self
                .name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        {
            return Err(err(
                "name",
                "lowercase letters, digits and underscores, starting with a letter".into(),
            ));
        }
        if self.command.trim().is_empty() {
            return Err(err("command", "required".into()));
        }
        if !matches!(
            self.effect,
            EffectKind::WorkspaceWrite | EffectKind::HostWrite | EffectKind::ExternalRead
        ) {
            return Err(err(
                "effect",
                "`workspace_write`, `host_write` or `external_read`".into(),
            ));
        }
        // CLR-1, CLR-2: the file says what the runtime does. A tool with
        // network access is the dependency step and reads from outside; a
        // tool without it cannot declare an external read.
        let pair = |effect: &str| format!("effect = \"{effect}\" with network = {}", self.network);
        if self.network && self.effect != EffectKind::ExternalRead {
            return Err(err(
                "effect",
                format!(
                    "{}: a tool with network access is the dependency step; write effect = \"external_read\"",
                    pair(effect_name(self.effect))
                ),
            ));
        }
        if !self.network && self.effect == EffectKind::ExternalRead {
            return Err(err(
                "effect",
                format!(
                    "{}: an external read needs network access; write effect = \"workspace_write\", or set network = true for the dependency step",
                    pair(effect_name(self.effect))
                ),
            ));
        }
        if let Some(p) = &self.arg_pattern
            && let Err(e) = regex::Regex::new(&format!("^(?:{p})$"))
        {
            return Err(err("arg_pattern", format!("not a valid pattern: {e}")));
        }
        if self.max_args == 0 && (!self.allowed_args.is_empty() || self.arg_pattern.is_some()) {
            return Err(err(
                "max_args",
                "must be above 0 when arguments are allowed".into(),
            ));
        }
        Ok(())
    }
}

/// Why a declared read path is refused (REQ-CONF-011).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReadPathRefusal {
    /// A filesystem root: it would make every file of the machine readable.
    Root,
    /// The user's home directory itself.
    Home,
    /// An ancestor of the user's home directory.
    AncestorOfHome,
    /// A directory that directly holds a name of LIM-H5 or LIM-W5.
    HoldsCredentials {
        /// The entry found.
        entry: String,
    },
}

/// Decides whether a declared read path (REQ-CONF-011, LIM-H5) may join the
/// read set. Pure: `path` and `home` are absolute and already resolved by the
/// caller (`~` expanded, links followed), `entries` are the names directly
/// inside `path` (empty when it is a file or does not exist). Landlock has
/// no deny rule and a container ACL covers a whole directory, so the check
/// is on the declaration, before anything is granted.
pub fn check_read_path(
    path: &std::path::Path,
    home: Option<&std::path::Path>,
    entries: &[String],
    case_insensitive: bool,
) -> Result<(), ReadPathRefusal> {
    let parts = |p: &std::path::Path| -> Vec<String> {
        p.components()
            .filter(|c| !matches!(c, std::path::Component::CurDir))
            .map(|c| {
                let s = c.as_os_str().to_string_lossy().into_owned();
                if case_insensitive {
                    s.to_lowercase()
                } else {
                    s
                }
            })
            .collect()
    };
    let wanted = parts(path);
    // A root has a prefix and a root directory component at most (`/`, `C:\`).
    if path.components().all(|c| {
        matches!(
            c,
            std::path::Component::RootDir | std::path::Component::Prefix(_)
        )
    }) {
        return Err(ReadPathRefusal::Root);
    }
    if let Some(home) = home {
        let home = parts(home);
        if wanted == home {
            return Err(ReadPathRefusal::Home);
        }
        if home.len() > wanted.len() && home[..wanted.len()] == wanted[..] {
            return Err(ReadPathRefusal::AncestorOfHome);
        }
    }
    let mut builder = globset::GlobSetBuilder::new();
    for pattern in crate::limits::LIM_W5_SECRET_PATTERNS
        .iter()
        .chain(crate::limits::LIM_H5_CREDENTIAL_NAMES.iter())
    {
        // `name/` names a directory; an entry list has names only, so both
        // kinds match (a file called `.ssh` is as unwelcome as the directory).
        let name = pattern.strip_suffix('/').unwrap_or(pattern);
        if let Ok(glob) = globset::GlobBuilder::new(name)
            .literal_separator(true)
            .case_insensitive(case_insensitive)
            .build()
        {
            builder.add(glob);
        }
    }
    if let Ok(set) = builder.build()
        && let Some(entry) = entries.iter().find(|e| set.is_match(e.as_str()))
    {
        return Err(ReadPathRefusal::HoldsCredentials {
            entry: entry.clone(),
        });
    }
    Ok(())
}

impl ReadPathRefusal {
    /// The reason, naming what was found and the narrower path to declare
    /// (CLR-5). `shown` is the path as the administrator wrote it.
    pub fn reason(&self, shown: &str) -> String {
        let narrower = match self {
            ReadPathRefusal::HoldsCredentials { .. } => format!(
                "declare the subdirectory the tool needs, for example \"{}/bin\"",
                shown.trim_end_matches(['/', '\\'])
            ),
            _ => "declare the toolchain's own directories, for example \"~/.cargo/bin\" and \"~/.rustup\"".to_owned(),
        };
        let found = match self {
            ReadPathRefusal::Root => "is a filesystem root".to_owned(),
            ReadPathRefusal::Home => "is the home directory".to_owned(),
            ReadPathRefusal::AncestorOfHome => "contains the home directory".to_owned(),
            ReadPathRefusal::HoldsCredentials { entry } => {
                format!("directly holds \"{entry}\"")
            }
        };
        format!(
            "\"{shown}\" {found} (REQ-CONF-011, LIM-H5: a read path must not expose credentials); {narrower}"
        )
    }
}

#[cfg(test)]
mod tests;
