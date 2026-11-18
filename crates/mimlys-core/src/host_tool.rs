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
    /// `true` admits arguments that name a path outside the workspace: a `..`
    /// component, or a start with `/`, `\`, `~`, a drive or `\\`. Without it
    /// no argument may, whatever `arg_pattern` admits (A-34; external design
    /// §13.3). `check` and `status` show it.
    #[serde(default)]
    pub allow_paths: bool,
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
    /// match of `arg_pattern`, at most `max_args`; and, unless the declaration
    /// says `allow_paths = true`, none of them names a path outside the
    /// workspace (see [`argument_names_a_path`]), whatever the pattern admits.
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
            if !(literal || matched) || (!self.allow_paths && argument_names_a_path(a)) {
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

/// Whether a model's argument names a path outside the workspace (A-34;
/// external design §13.3): the argument, each part of it between `=` signs,
/// and what follows a short option's letter (`-I` in `-I../x`) is taken apart,
/// and a part that holds a `..` component (between `/` or `\`), or begins with
/// `/`, `\` (so `\\` too), `~` or a drive (`X:`, a letter and a colon, not the
/// `::` of a Rust path), is a path. The same on every platform: a declaration
/// is not valid on one machine and refused on another. The command's fixed
/// `args` are the administrator's and are not looked at.
pub fn argument_names_a_path(argument: &str) -> bool {
    let mut parts = vec![argument];
    for piece in argument.split('=') {
        parts.push(piece);
        if let Some(value) = short_option_value(piece) {
            parts.push(value);
        }
    }
    parts.into_iter().any(part_names_a_path)
}

/// The text after a short option's letter: `../x` in `-I../x`. `None` for a
/// long option (`--x`), for no option, and when nothing follows the letter.
fn short_option_value(piece: &str) -> Option<&str> {
    let rest = piece.strip_prefix('-')?;
    if rest.starts_with('-') {
        return None;
    }
    let mut chars = rest.chars();
    chars.next()?;
    let value = chars.as_str();
    (!value.is_empty()).then_some(value)
}

/// One part of an argument: a `..` component, or a start that names a root.
fn part_names_a_path(part: &str) -> bool {
    part.split(['/', '\\']).any(|component| component == "..")
        || part.starts_with(['/', '\\', '~'])
        || starts_with_drive(part)
}

/// `X:` at the start: a letter and a colon that is not the first of `::`.
fn starts_with_drive(part: &str) -> bool {
    let mut chars = part.chars();
    matches!(
        (chars.next(), chars.next(), chars.next()),
        (Some(letter), Some(':'), after) if letter.is_ascii_alphabetic() && after != Some(':')
    )
}

/// Whether an `arg_pattern` is one that, until 0.10, took paths as allowed: it
/// names `/` or `\.` (an escaped dot). Such a declaration relied on the
/// override that 0.11 removed, and `check` says so.
pub fn pattern_relied_on_the_path_override(pattern: &str) -> bool {
    pattern.contains('/') || pattern.contains("\\.")
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
        if is_batch_script(&self.command) {
            return Err(err("command", batch_script_refusal(&self.command)));
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
        // A-34 (scope of 0.11): a declaration that relied on a pattern naming
        // `/` or `\.` to admit paths fails here, with the reason and the
        // remedy, instead of failing at run time with a bare refusal.
        if !self.allow_paths {
            if let Some(p) = &self.arg_pattern
                && pattern_relied_on_the_path_override(p)
            {
                return Err(err(
                    "arg_pattern",
                    format!(
                        "the pattern {p:?} names `/` or `\\.`, which until 0.10 admitted arguments that name paths; a host tool's argument now names no path outside its workspace (a `..` component, or a start with `/`, `\\`, `~` or a drive), whatever the pattern admits. If the tool needs such arguments, add allow_paths = true to [host_tools.{}]; otherwise write a pattern without `/` and `\\.`",
                        self.name
                    ),
                ));
            }
            if let Some(literal) = self.allowed_args.iter().find(|a| argument_names_a_path(a)) {
                return Err(err(
                    "allowed_args",
                    format!(
                        "{literal:?} names a path outside the workspace (a `..` component, or a start with `/`, `\\`, `~` or a drive), which a host tool's argument may not unless the declaration says so. Add allow_paths = true to [host_tools.{}], or remove the argument",
                        self.name
                    ),
                ));
            }
        }
        Ok(())
    }
}

/// Whether a command names a batch script, `.cmd` or `.bat`, in any case.
/// Windows runs such a file through `cmd.exe` (a shell), and the arguments
/// are quoted for the C runtime and not for `cmd.exe`; no shell is involved
/// in a host tool (REQ-CONF-006). The same rule on every platform, so that a
/// declaration is not valid on one machine and refused on another. Windows
/// ignores trailing dots and spaces in a file name, so `run.cmd.` is `run.cmd`.
pub fn is_batch_script(command: &str) -> bool {
    let name = command.trim().trim_end_matches(['.', ' ']);
    name.rsplit_once('.').is_some_and(|(_, extension)| {
        !extension.contains(['/', '\\'])
            && (extension.eq_ignore_ascii_case("cmd") || extension.eq_ignore_ascii_case("bat"))
    })
}

/// The reason a batch script is refused as a command: what it is, why it is
/// refused (REQ-CONF-006: no shell is involved) and the remedy (CLR-5).
/// `shown` is the command as written or as it resolves.
pub fn batch_script_refusal(shown: &str) -> String {
    format!(
        "\"{shown}\" is a batch script, which Windows runs through a shell (cmd.exe); no shell is involved in a host tool (REQ-CONF-006); declare the program the script runs"
    )
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
    /// The runtime's data directory, a path inside it, or an ancestor of it:
    /// every task's tree and the store would be readable to every task's tool
    /// (handoff 011o-4, A-06's read half).
    DataDirectory {
        /// How the entry relates to the data directory.
        relation: DataDirRelation,
        /// The data directory, as the runtime resolved it.
        data_dir: String,
    },
}

/// How a read path lies against the data directory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DataDirRelation {
    /// The data directory itself.
    Is,
    /// A path inside the data directory.
    Inside,
    /// An ancestor of the data directory.
    Contains,
}

/// Decides whether a declared read path (REQ-CONF-011, LIM-H5) may join the
/// read set. Pure: `path` and `home` are absolute and already resolved by the
/// caller (`~` expanded, links followed), `entries` are the names directly
/// inside `path` (empty when it is a file or does not exist). Landlock has
/// no deny rule and a container ACL covers a whole directory, so the check
/// is on the declaration, before anything is granted. `data_dir` is the
/// runtime's data directory (resolved the same way), which a read path may
/// not be, lie inside, or contain: the worker's write set is narrowed to one
/// task, but its read set keeps every declared read path.
pub fn check_read_path(
    path: &std::path::Path,
    home: Option<&std::path::Path>,
    data_dir: Option<&std::path::Path>,
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
    if let Some(data_dir) = data_dir {
        let data = parts(data_dir);
        let relation = if wanted == data {
            Some(DataDirRelation::Is)
        } else if wanted.len() > data.len() && wanted[..data.len()] == data[..] {
            Some(DataDirRelation::Inside)
        } else if data.len() > wanted.len() && data[..wanted.len()] == wanted[..] {
            Some(DataDirRelation::Contains)
        } else {
            None
        };
        if let Some(relation) = relation {
            return Err(ReadPathRefusal::DataDirectory {
                relation,
                data_dir: data_dir.to_string_lossy().into_owned(),
            });
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
            ReadPathRefusal::DataDirectory { .. } => {
                "declare the toolchain's own directories, which lie outside the data directory, for example \"~/.cargo/bin\" and \"~/.rustup\"".to_owned()
            }
            _ => "declare the toolchain's own directories, for example \"~/.cargo/bin\" and \"~/.rustup\"".to_owned(),
        };
        let found = match self {
            ReadPathRefusal::Root => "is a filesystem root".to_owned(),
            ReadPathRefusal::Home => "is the home directory".to_owned(),
            ReadPathRefusal::AncestorOfHome => "contains the home directory".to_owned(),
            ReadPathRefusal::HoldsCredentials { entry } => {
                format!("directly holds \"{entry}\"")
            }
            ReadPathRefusal::DataDirectory { relation, data_dir } => match relation {
                DataDirRelation::Is => format!("is the data directory \"{data_dir}\""),
                DataDirRelation::Inside => {
                    format!("is inside the data directory \"{data_dir}\"")
                }
                DataDirRelation::Contains => {
                    format!("contains the data directory \"{data_dir}\"")
                }
            },
        };
        if let ReadPathRefusal::DataDirectory { .. } = self {
            return format!(
                "\"{shown}\" {found} (REQ-CONF-011: a read path must not reach the tasks' trees or the store); {narrower}"
            );
        }
        format!(
            "\"{shown}\" {found} (REQ-CONF-011, LIM-H5: a read path must not expose credentials); {narrower}"
        )
    }
}

#[cfg(test)]
mod tests;
