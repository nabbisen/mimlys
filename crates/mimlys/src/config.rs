//! Configuration (external design §19.1; REQ-CFG-001 … REQ-CFG-005).
//!
//! One TOML file with `config_version = 1`. Every section refuses unknown
//! keys; secrets are references (`env:NAME`, `file:PATH`); every default is
//! a value of the limits register; every range error states the range.

use mimlys_core::limits::{
    LIM_C1_CONTEXT_CHARS, LIM_C1_CONTEXT_CHARS_RANGE, LIM_H1_HOST_DEADLINE_MS,
    LIM_H1_HOST_DEADLINE_MS_RANGE, LIM_M1_REQUEST_MS, LIM_M1_REQUEST_MS_RANGE, LIM_M2_MAX_TOKENS,
    LIM_M2_MAX_TOKENS_RANGE, LIM_O5_EXTERNAL_DEADLINE_MS, LIM_O5_EXTERNAL_DEADLINE_MS_RANGE,
    LIM_O6_SERVER_STARTUP_MS, LIM_O6_SERVER_STARTUP_MS_RANGE, LIM_O16_SERVER_SHUTDOWN_MS,
    LIM_O16_SERVER_SHUTDOWN_MS_RANGE, LIM_O16_SERVER_SHUTDOWN_WARN_BELOW_MS, LIM_S1_FUEL,
    LIM_S1_FUEL_RANGE, LIM_S2_DEADLINE_MS, LIM_S2_DEADLINE_MS_RANGE, LIM_S3_MEMORY_MIB,
    LIM_S3_MEMORY_MIB_RANGE, LIM_S4_OUTPUT_BYTES, LIM_S4_OUTPUT_BYTES_RANGE,
    LIM_W1_WORKSPACE_BYTES, LIM_W1_WORKSPACE_BYTES_RANGE, LIM_W2_WORKSPACE_FILES,
};
use mimlys_core::policy::{OnCancel, OnReject, Policy, Profile, Recovery};
use mimlys_core::spi::Locality;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The configuration schema version this binary understands.
pub const CONFIG_VERSION: u32 = 1;
/// The environment variable naming the configuration file.
pub const CONFIG_ENV: &str = "MIMLYS_CONFIG";
/// The file name under the platform configuration directory.
pub const CONFIG_FILE: &str = "config.toml";

/// A configuration error: the key path and what is wrong, with the
/// permitted values (CLR-3). Exit code 1.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("configuration{}: {reason}", key_text(.key))]
pub struct ConfigError {
    /// The key path, empty when the file as a whole is at fault.
    pub key: String,
    /// What is wrong and what is permitted.
    pub reason: String,
}

fn key_text(key: &str) -> String {
    if key.is_empty() {
        String::new()
    } else {
        format!(" key `{key}`")
    }
}

impl ConfigError {
    fn new(key: &str, reason: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            reason: reason.into(),
        }
    }
}

fn range<T: PartialOrd + std::fmt::Display>(
    key: &str,
    value: T,
    (min, max): (T, T),
) -> Result<(), ConfigError> {
    if value < min || value > max {
        return Err(ConfigError::new(
            key,
            format!("{value} is outside {min}..={max}"),
        ));
    }
    Ok(())
}

/// A secret given by reference (REQ-CFG-003). An inline value is refused
/// when the file is read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SecretRef {
    /// An environment variable.
    Env(String),
    /// A file whose content is the secret.
    File(PathBuf),
}

impl SecretRef {
    /// Parses `env:NAME` or `file:PATH`.
    pub fn parse(text: &str) -> Result<Self, String> {
        if let Some(name) = text.strip_prefix("env:") {
            if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                return Err("`env:` must be followed by a variable name".into());
            }
            return Ok(SecretRef::Env(name.to_owned()));
        }
        if let Some(path) = text.strip_prefix("file:") {
            if path.is_empty() {
                return Err("`file:` must be followed by a path".into());
            }
            return Ok(SecretRef::File(PathBuf::from(path)));
        }
        Err("secrets are given by reference as `env:NAME` or `file:PATH`, never inline".into())
    }

    /// The reference as written.
    pub fn reference(&self) -> String {
        match self {
            SecretRef::Env(name) => format!("env:{name}"),
            SecretRef::File(path) => format!("file:{}", path.display()),
        }
    }

    /// Reads the secret. Never logged, never printed.
    pub fn resolve(&self) -> Result<String, ConfigError> {
        match self {
            SecretRef::Env(name) => std::env::var(name).map_err(|_| {
                ConfigError::new(
                    "backend.api_key",
                    format!("environment variable {name} is not set"),
                )
            }),
            SecretRef::File(path) => std::fs::read_to_string(path)
                .map(|s| s.trim_end().to_owned())
                .map_err(|e| {
                    ConfigError::new(
                        "backend.api_key",
                        format!("cannot read the secret file: {}", e.kind()),
                    )
                }),
        }
    }
}

impl Serialize for SecretRef {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.reference())
    }
}

impl<'de> Deserialize<'de> for SecretRef {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        SecretRef::parse(&text).map_err(serde::de::Error::custom)
    }
}

/// The whole file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Must be [`CONFIG_VERSION`].
    pub config_version: u32,
    /// `[runtime]`.
    #[serde(default)]
    pub runtime: RuntimeSection,
    /// `[backend]`.
    #[serde(default)]
    pub backend: BackendSection,
    /// `[policy]`.
    #[serde(default)]
    pub policy: PolicySection,
    /// `[sandbox]` (validated now, used from M2).
    #[serde(default)]
    pub sandbox: SandboxSection,
    /// `[workspace]`.
    #[serde(default)]
    pub workspace: WorkspaceSection,
    /// `[logging]`.
    #[serde(default)]
    pub logging: LoggingSection,
    /// `[servers.<name>]` (RFC 009 §1).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub servers: BTreeMap<String, ServerSection>,
    /// `[host_tools.<name>]` (RFC 011 §6).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub host_tools: BTreeMap<String, HostToolSection>,
    /// `[interpreters.<language>]` (RFC 005 §6).
    #[serde(default)]
    pub interpreters: BTreeMap<String, InterpreterSection>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            config_version: CONFIG_VERSION,
            runtime: RuntimeSection::default(),
            backend: BackendSection::default(),
            policy: PolicySection::default(),
            sandbox: SandboxSection::default(),
            workspace: WorkspaceSection::default(),
            logging: LoggingSection::default(),
            servers: BTreeMap::new(),
            host_tools: BTreeMap::new(),
            interpreters: BTreeMap::new(),
        }
    }
}

/// `[interpreters.<language>]`: where the module is and which hash it must
/// `[host_tools.<name>]`: one declared host tool (RFC 011 §6; REQ-CONF-006).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostToolSection {
    /// The program: absolute, or a name resolved against the `bin` directories of `read_paths`.
    pub command: String,
    /// Fixed arguments.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    /// Literal arguments the model may add.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_args: Vec<String>,
    /// A regular expression any other added argument must match.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arg_pattern: Option<String>,
    /// How many arguments the model may add.
    #[serde(default)]
    pub max_args: usize,
    /// `workspace_write`: the tool writes only inside the workspace, reversible
    /// through the snapshots; `external_read` exactly when `network = true`
    /// (the dependency step).
    pub effect: HostToolEffect,
    /// The deadline (LIM-H1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<i64>,
    /// Toolchain and cache directories the tool may read (`~` expanded).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub read_paths: Vec<String>,
    /// Fixed environment variables.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    /// Variables copied from mimlys's environment.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub env_allow: Vec<String>,
    /// `true` makes this the dependency step: network allowed; the effect must
    /// be declared `external_read` (REQ-CONF-007, REQ-CONF-006).
    #[serde(default)]
    pub network: bool,
}

/// The effect a host tool may declare. Host tools write only inside the
/// workspace in this release: the worker's write set holds nothing else,
/// so a `host_write` declaration would promise what the confinement
/// refuses; it needs declared write paths first (RFC 011, deferred).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostToolEffect {
    /// Writes only inside the workspace (reversible through the snapshots).
    WorkspaceWrite,
    /// Reads from outside the host: the dependency step, the one tool with
    /// `network = true` (and only that tool).
    ExternalRead,
}

/// Why `host_read` and `host_write` grants, and the ceiling roots that would
/// bound them, are refused in this release (external design §9.1): no tool
/// reads or writes a host path through them yet, so a grant would change
/// nothing, and a root in the ceiling would promise what nothing uses.
pub const HOST_PATH_GRANTS_RESERVED: &str = "reserved: no tool reads or writes host paths through host_read or host_write grants in this release (host tools read their declared read_paths; promotion writes the project scope); remove it";

/// `~` and `~/…` expanded to the home directory; other paths unchanged.
pub fn expand_home(path: &str) -> String {
    let home = directories::BaseDirs::new().map(|b| b.home_dir().to_path_buf());
    match (path.strip_prefix("~/"), path == "~", home) {
        (Some(rest), _, Some(h)) => h.join(rest).to_string_lossy().into_owned(),
        (None, true, Some(h)) => h.to_string_lossy().into_owned(),
        _ => path.to_owned(),
    }
}

impl Config {
    /// The host tools in force as the worker and the policy take them:
    /// `~` expanded, `env_allow` resolved from this process's environment
    /// into `env`, the deadline defaulted (RFC 011 §6).
    pub fn host_tools(&self) -> Vec<mimlys_core::host_tool::HostToolDeclaration> {
        self.host_tools
            .iter()
            .map(|(name, s)| {
                let mut env: BTreeMap<String, String> = s
                    .env
                    .iter()
                    .map(|(k, v)| (k.clone(), expand_home(v)))
                    .collect();
                for var in &s.env_allow {
                    if let Ok(v) = std::env::var(var) {
                        env.entry(var.clone()).or_insert(v);
                    }
                }
                mimlys_core::host_tool::HostToolDeclaration {
                    name: name.clone(),
                    command: expand_home(&s.command),
                    args: s.args.clone(),
                    allowed_args: s.allowed_args.clone(),
                    arg_pattern: s.arg_pattern.clone(),
                    max_args: s.max_args,
                    effect: match s.effect {
                        HostToolEffect::WorkspaceWrite => {
                            mimlys_core::action::EffectKind::WorkspaceWrite
                        }
                        HostToolEffect::ExternalRead => {
                            mimlys_core::action::EffectKind::ExternalRead
                        }
                    },
                    reversible: true,
                    timeout_ms: s.timeout_ms.unwrap_or(LIM_H1_HOST_DEADLINE_MS),
                    read_paths: s.read_paths.iter().map(|p| expand_home(p)).collect(),
                    env,
                    env_allow: s.env_allow.clone(),
                    network: s.network,
                }
            })
            .collect()
    }
}

/// `[servers.<name>]`: one MCP server as a child process (RFC 009 §1).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerSection {
    /// The program.
    pub command: String,
    /// Its arguments.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    /// Environment variables copied from mimlys's environment; nothing else is inherited.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub env_allow: Vec<String>,
    /// Secrets placed in the child's environment, by reference.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env_secrets: BTreeMap<String, SecretRef>,
    /// Startup deadline (LIM-O6).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub startup_ms: Option<i64>,
    /// The grace after its standard input closes, before its group is killed (LIM-O16).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shutdown_ms: Option<i64>,
    /// The server's declared maximum call time; no tool deadline falls below it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_call_ms: Option<i64>,
    /// The declared tools, by the server's tool name (REQ-MCP-004).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tools: BTreeMap<String, ToolDeclarationSection>,
}

/// `[servers.<name>.tools.<tool>]`: the administrator's effect declaration.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolDeclarationSection {
    /// `external_read` or `external_write`.
    pub effect: ExternalEffect,
    /// Whether a repeat has the same result.
    #[serde(default)]
    pub idempotent: bool,
    /// Parameters holding destination URLs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub destination_arguments: Vec<String>,
    /// Deadline (LIM-O5 range).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<i64>,
}

/// The two effects an external tool may declare.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExternalEffect {
    /// Reads outside the host.
    ExternalRead,
    /// Writes outside the host.
    ExternalWrite,
}

impl Config {
    /// The servers in force, with secrets resolved from `secrets`
    /// (`servers.<name>.env.<VAR>`), as the MCP crate takes them.
    pub fn servers(
        &self,
        data_dir: &Path,
        secrets: &[(String, String)],
    ) -> Vec<mimlys_mcp::ServerConfig> {
        self.servers
            .iter()
            .map(|(name, s)| {
                let mut config =
                    mimlys_mcp::ServerConfig::new(name, PathBuf::from(&s.command), s.args.clone());
                config.env_allow = s.env_allow.clone();
                config.env_secrets = s
                    .env_secrets
                    .keys()
                    .filter_map(|var| {
                        let key = format!("servers.{name}.env.{var}");
                        secrets
                            .iter()
                            .find(|(k, _)| *k == key)
                            .map(|(_, v)| (var.clone(), v.clone()))
                    })
                    .collect();
                config.current_dir = Some(data_dir.to_path_buf());
                config.startup_ms = s.startup_ms.unwrap_or(LIM_O6_SERVER_STARTUP_MS);
                config.shutdown_ms = s.shutdown_ms.unwrap_or(LIM_O16_SERVER_SHUTDOWN_MS);
                config.max_call_ms = s.max_call_ms;
                config.tools = s
                    .tools
                    .iter()
                    .map(|(tool, d)| {
                        (
                            tool.clone(),
                            mimlys_mcp::Declaration {
                                effect: match d.effect {
                                    ExternalEffect::ExternalRead => {
                                        mimlys_core::action::EffectKind::ExternalRead
                                    }
                                    ExternalEffect::ExternalWrite => {
                                        mimlys_core::action::EffectKind::ExternalWrite
                                    }
                                },
                                idempotent: d.idempotent,
                                destination_arguments: d.destination_arguments.clone(),
                                timeout_ms: d.timeout_ms.unwrap_or(LIM_O5_EXTERNAL_DEADLINE_MS),
                            },
                        )
                    })
                    .collect();
                config
            })
            .collect()
    }

    /// The external tools the declarations admit to the ceiling (RFC 009 §4),
    /// as `server.tool`, with the server that declares each.
    pub fn declared_external_tools(&self) -> Vec<(String, String)> {
        self.servers
            .iter()
            .flat_map(|(server, s)| {
                s.tools
                    .keys()
                    .map(move |tool| (format!("{server}.{tool}"), server.clone()))
            })
            .collect()
    }

    /// The policy in force: the overlays resolved, then every declared
    /// external tool admitted to the ceiling (RFC 009 §4).
    pub fn resolve_policy(&self) -> Result<Policy, ConfigError> {
        let admitted: Vec<String> = self
            .declared_external_tools()
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        let declared: Vec<String> = self.host_tools.keys().cloned().collect();
        let mut policy = self.policy.resolve_with(&admitted, &declared)?;
        for d in self.host_tools() {
            policy.host_tools.insert(d.name.clone(), d);
        }
        Ok(policy)
    }

    /// Servers whose configured `shutdown_ms` is below LIM-O16's warning
    /// bound (`check` warns), with the value.
    pub fn short_shutdown_servers(&self) -> Vec<(String, i64)> {
        self.servers
            .iter()
            .filter_map(|(name, s)| {
                s.shutdown_ms
                    .filter(|v| *v < LIM_O16_SERVER_SHUTDOWN_WARN_BELOW_MS)
                    .map(|v| (name.clone(), v))
            })
            .collect()
    }

    /// External tools named in `policy.ceiling.tools` without a declaration:
    /// they run under the restrictive defaults (`check` warns).
    pub fn undeclared_external_tools(&self) -> Vec<String> {
        let declared = self.declared_external_tools();
        self.policy
            .ceiling
            .tools
            .iter()
            .flatten()
            .filter(|t| t.contains('.') && !declared.iter().any(|(d, _)| d == *t))
            .cloned()
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect()
    }
}

/// `[interpreters.<language>]`: where a language's module is and what hash it must
/// have. A pinned language needs neither key (the shipped build applies);
/// another language needs both.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InterpreterSection {
    /// Module path; the data directory's `interpreters/<sha256>.wasm` when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub module: Option<PathBuf>,
    /// Expected SHA-256; the pinned hash when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    /// The script file name inside `/tmp`; `main.<language>` when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub script_name: Option<String>,
}

/// `[runtime]`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeSection {
    /// The data directory; the platform default when absent (external design §14.1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_dir: Option<PathBuf>,
}

/// Which backend.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendKind {
    /// The scripted backend (tests, replay, the M1 demonstration).
    #[default]
    Fake,
    /// An OpenAI-compatible HTTP endpoint (M4).
    OpenaiCompatible,
}

/// Structured-output mode (M4).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StructuredOutput {
    /// Probe the endpoint.
    #[default]
    Auto,
    /// Send the JSON schema.
    Schema,
    /// Plain text with the strict parser.
    None,
}

/// How the model's decision is asked for and read (REQ-MDL-009; RFC 012).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionTransport {
    /// `tool_calls` when the engine accepts `tools`, else `envelope`; `status` says which and why.
    #[default]
    Auto,
    /// The model's native function calls; an engine that refuses `tools` is `backend_unavailable`.
    ToolCalls,
    /// The decision envelope written as text; no request carries `tools`.
    Envelope,
}

/// `[backend]`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackendSection {
    /// Which backend.
    #[serde(default)]
    pub kind: BackendKind,
    /// Endpoint of an `openai_compatible` backend.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Model name of an `openai_compatible` backend.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Where the backend runs; `remote` needs `policy.remote_backend_allowed`.
    #[serde(default = "default_locality")]
    pub locality: Locality,
    /// The decision transport (0.9).
    #[serde(default)]
    pub decision_transport: DecisionTransport,
    /// Structured-output mode; the `envelope` transport only.
    #[serde(default)]
    pub structured_output: StructuredOutput,
    /// Deadline of one model request (LIM-M1).
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: i64,
    /// Maximum completion tokens (LIM-M2).
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
    /// Context budget per request in characters (LIM-C1); `metrics` shows
    /// characters per prompt token so it can be set from a model's window.
    #[serde(default = "default_context_chars")]
    pub context_chars: usize,
    /// Sampling temperature of an `openai_compatible` backend; 0 for the
    /// most repeatable decisions the engine allows.
    #[serde(default)]
    pub temperature: f64,
    /// Credential, by reference only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<SecretRef>,
    /// Script file of the `fake` backend; an empty script when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub script: Option<PathBuf>,
}

fn default_locality() -> Locality {
    Locality::Local
}
fn default_timeout_ms() -> i64 {
    LIM_M1_REQUEST_MS
}
fn default_max_tokens() -> u32 {
    LIM_M2_MAX_TOKENS
}
fn default_context_chars() -> usize {
    LIM_C1_CONTEXT_CHARS
}

impl Default for BackendSection {
    fn default() -> Self {
        Self {
            kind: BackendKind::Fake,
            url: None,
            model: None,
            locality: Locality::Local,
            decision_transport: DecisionTransport::Auto,
            structured_output: StructuredOutput::Auto,
            timeout_ms: LIM_M1_REQUEST_MS,
            max_tokens: LIM_M2_MAX_TOKENS,
            context_chars: LIM_C1_CONTEXT_CHARS,
            temperature: 0.0,
            api_key: None,
            script: None,
        }
    }
}

/// `[policy]`: overlays on the shipped policy of external design §9.2.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicySection {
    /// Profile used when a task names none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_profile: Option<String>,
    /// Whether a `remote` backend may be used.
    #[serde(default)]
    pub remote_backend_allowed: bool,
    /// Mandatory clauses whose best-effort enforcement is accepted, by letter (`["K"]`) (RFC 011 §5).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub accept_best_effort: Vec<String>,
    /// `[policy.ceiling]`.
    #[serde(default)]
    pub ceiling: CeilingSection,
    /// `[policy.profiles.<name>]`.
    #[serde(default)]
    pub profiles: BTreeMap<String, ProfileSection>,
    /// `[policy.exclusions]`.
    #[serde(default)]
    pub exclusions: ExclusionsSection,
    /// `[policy.recovery]`.
    #[serde(default)]
    pub recovery: Recovery,
}

/// `[policy.ceiling]`: each key replaces the shipped value when present.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CeilingSection {
    /// Roots a `host_read` grant may name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_read_roots: Option<Vec<String>>,
    /// Roots a `host_write` grant may name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_write_roots: Option<Vec<String>>,
    /// Domains a `network` grant may name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network_domains: Option<Vec<String>>,
    /// Tools a `tool` grant may name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<String>>,
    /// Host tools a `host_tool` grant may name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_tools: Option<Vec<String>>,
    /// Highest step budget any profile may set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_steps: Option<u32>,
}

/// `[policy.profiles.<name>]`: a shipped profile is overlaid; a new one
/// needs at least `tools`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileSection {
    /// Tools granted at creation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<String>>,
    /// Declared host tools granted at creation (`host_tool` grants; RFC 011 §6).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_tools: Option<Vec<String>>,
    /// LIM-T1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steps: Option<u32>,
    /// LIM-T2.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consecutive_rejections: Option<u32>,
    /// LIM-T6.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consecutive_failures: Option<u32>,
    /// LIM-T4.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identical_decisions: Option<u32>,
    /// LIM-T3.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executions_per_action: Option<u32>,
    /// Tools that always need approval in this profile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_tools: Option<Vec<String>>,
    /// On rejection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_reject: Option<OnReject>,
    /// On cancellation of a running write action.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_cancel: Option<OnCancel>,
    /// Whether promotion is available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub promote: Option<bool>,
    /// Whether the task may communicate outside the host (a `remote` backend).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external: Option<bool>,
}

/// `[policy.exclusions]`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExclusionsSection {
    /// LIM-W4.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub materialize: Option<Vec<String>>,
    /// LIM-W5.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secrets: Option<Vec<String>>,
}

impl PolicySection {
    /// The policy in force: the shipped policy with this section's overlays,
    /// validated (REQ-CFG-004).
    pub fn resolve(&self) -> Result<Policy, ConfigError> {
        self.resolve_with(&[], &[])
    }

    /// As [`Self::resolve`], with `admitted` external tools in the ceiling
    /// before the profiles are checked against it (RFC 009 §4).
    pub fn resolve_with(
        &self,
        admitted: &[String],
        declared_host_tools: &[String],
    ) -> Result<Policy, ConfigError> {
        let mut policy = Policy::shipped();
        policy.config_version = CONFIG_VERSION;
        if let Some(p) = &self.default_profile {
            policy.default_profile = p.clone();
        }
        policy.remote_backend_allowed = self.remote_backend_allowed;
        for letter in &self.accept_best_effort {
            let clause = mimlys_core::confine::Clause::ALL
                .iter()
                .find(|c| c.letter().to_string().eq_ignore_ascii_case(letter))
                .copied()
                .ok_or_else(|| {
                    ConfigError::new(
                        "policy.accept_best_effort",
                        format!("`{letter}` is not a clause letter (F, N, E, K, P, M, C)"),
                    )
                })?;
            if !clause.mandatory() {
                return Err(ConfigError::new(
                    "policy.accept_best_effort",
                    format!(
                        "`{letter}` may be best-effort already; only mandatory clauses are accepted here"
                    ),
                ));
            }
            if !policy.accept_best_effort.contains(&clause) {
                policy.accept_best_effort.push(clause);
            }
        }
        let c = &self.ceiling;
        if let Some(v) = &c.host_read_roots {
            policy.ceiling.host_read_roots = v.clone();
        }
        if let Some(v) = &c.host_write_roots {
            policy.ceiling.host_write_roots = v.clone();
        }
        if let Some(v) = &c.network_domains {
            policy.ceiling.network_domains = v.clone();
        }
        if let Some(v) = &c.tools {
            policy.ceiling.tools = v.clone();
        }
        for name in admitted {
            if !policy.ceiling.tools.contains(name) {
                policy.ceiling.tools.push(name.clone());
            }
        }
        if let Some(v) = &c.host_tools {
            policy.ceiling.host_tools = v.clone();
        }
        // RFC 011 §6: a host tool declaration admits its name to the ceiling,
        // before the profiles' `host_tools` are checked against it.
        for name in declared_host_tools {
            if !policy.ceiling.host_tools.contains(name) {
                policy.ceiling.host_tools.push(name.clone());
            }
        }
        if let Some(v) = c.max_steps {
            policy.ceiling.max_steps = v;
        }
        if let Some(v) = &self.exclusions.materialize {
            policy.exclusions.materialize = v.clone();
        }
        if let Some(v) = &self.exclusions.secrets {
            policy.exclusions.secrets = v.clone();
        }
        policy.recovery = self.recovery;
        for (name, section) in &self.profiles {
            let mut profile = match policy.profiles.get(name) {
                Some(existing) => existing.clone(),
                None => Profile {
                    name: name.clone(),
                    tools: section.tools.clone().ok_or_else(|| {
                        ConfigError::new(
                            &format!("policy.profiles.{name}.tools"),
                            "a new profile must list its tools",
                        )
                    })?,
                    host_tools: vec![],
                    budgets: Default::default(),
                    approval_tools: vec![],
                    promote: false,
                    external: false,
                    on_reject: OnReject::Continue,
                    on_cancel: OnCancel::Rollback,
                },
            };
            if let Some(v) = &section.tools {
                profile.tools = v.clone();
            }
            if let Some(v) = &section.host_tools {
                profile.host_tools = v.clone();
            }
            if let Some(v) = section.steps {
                profile.budgets.steps = v;
            }
            if let Some(v) = section.consecutive_rejections {
                profile.budgets.consecutive_rejections = v;
            }
            if let Some(v) = section.consecutive_failures {
                profile.budgets.consecutive_failures = v;
            }
            if let Some(v) = section.identical_decisions {
                profile.budgets.identical_decisions = v;
            }
            if let Some(v) = section.executions_per_action {
                profile.budgets.executions_per_action = v;
            }
            if let Some(v) = &section.approval_tools {
                profile.approval_tools = v.clone();
            }
            if let Some(v) = section.on_reject {
                profile.on_reject = v;
            }
            if let Some(v) = section.on_cancel {
                profile.on_cancel = v;
            }
            if let Some(v) = section.promote {
                profile.promote = v;
            }
            if let Some(v) = section.external {
                profile.external = v;
            }
            policy.profiles.insert(name.clone(), profile);
        }
        policy
            .validate()
            .map_err(|e| ConfigError::new(&format!("policy.{}", e.key), e.reason))?;
        Ok(policy)
    }
}

/// `[sandbox]` (M2 uses it; M1 validates it).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxSection {
    /// LIM-S1.
    #[serde(default = "default_fuel")]
    pub fuel: u64,
    /// LIM-S2.
    #[serde(default = "default_sandbox_deadline_ms")]
    pub deadline_ms: i64,
    /// LIM-S3.
    #[serde(default = "default_memory_mib")]
    pub memory_mib: u32,
    /// LIM-S4.
    #[serde(default = "default_output_bytes")]
    pub output_bytes: u64,
}

fn default_fuel() -> u64 {
    LIM_S1_FUEL
}
fn default_sandbox_deadline_ms() -> i64 {
    LIM_S2_DEADLINE_MS
}
fn default_memory_mib() -> u32 {
    LIM_S3_MEMORY_MIB
}
fn default_output_bytes() -> u64 {
    LIM_S4_OUTPUT_BYTES
}

impl Default for SandboxSection {
    fn default() -> Self {
        Self {
            fuel: LIM_S1_FUEL,
            deadline_ms: LIM_S2_DEADLINE_MS,
            memory_mib: LIM_S3_MEMORY_MIB,
            output_bytes: LIM_S4_OUTPUT_BYTES,
        }
    }
}

/// `[workspace]`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceSection {
    /// LIM-W1.
    #[serde(default = "default_max_bytes")]
    pub max_bytes: u64,
    /// LIM-W2.
    #[serde(default = "default_max_files")]
    pub max_files: u64,
}

fn default_max_bytes() -> u64 {
    LIM_W1_WORKSPACE_BYTES
}
fn default_max_files() -> u64 {
    LIM_W2_WORKSPACE_FILES
}

impl Default for WorkspaceSection {
    fn default() -> Self {
        Self {
            max_bytes: LIM_W1_WORKSPACE_BYTES,
            max_files: LIM_W2_WORKSPACE_FILES,
        }
    }
}

/// `[logging]`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoggingSection {
    /// `error`, `warn`, `info`, `debug` or `trace`.
    #[serde(default = "default_level")]
    pub level: String,
    /// Log prompts and model output (REQ-OBS-004): warns at start.
    #[serde(default)]
    pub debug_prompts: bool,
}

fn default_level() -> String {
    "info".into()
}

impl Default for LoggingSection {
    fn default() -> Self {
        Self {
            level: default_level(),
            debug_prompts: false,
        }
    }
}

/// Where the configuration came from.
#[derive(Clone, Debug, PartialEq)]
pub struct Loaded {
    /// The configuration.
    pub config: Config,
    /// The file, or `None` for the built-in defaults.
    pub path: Option<PathBuf>,
    /// Every configured secret, resolved once, as `(name, value)`
    /// (REQ-CFG-003): the backend's key under `backend.api_key`. They feed
    /// the redactor (REQ-CTX-007) and never appear in a log.
    pub secrets: Vec<(String, String)>,
}

impl Config {
    /// The interpreters in force: every pinned language, with the file's
    /// overrides, plus the languages the file adds (REQ-SBX-001).
    pub fn interpreters(
        &self,
        data_dir: &Path,
    ) -> Result<Vec<mimlys_exec::interpreters::Configured>, ConfigError> {
        use mimlys_exec::interpreters::{Configured, PINNED, default_module_path};
        let mut out = Vec::new();
        for p in PINNED {
            let section = self
                .interpreters
                .get(p.language)
                .cloned()
                .unwrap_or_default();
            let sha256 = section
                .sha256
                .clone()
                .unwrap_or_else(|| p.sha256.to_owned());
            out.push(Configured {
                language: p.language.to_owned(),
                module: section
                    .module
                    .clone()
                    .unwrap_or_else(|| default_module_path(data_dir, &sha256)),
                sha256,
                script_name: section
                    .script_name
                    .clone()
                    .unwrap_or_else(|| p.script_name.to_owned()),
            });
        }
        for (language, section) in &self.interpreters {
            if PINNED.iter().any(|p| p.language == language) {
                continue;
            }
            let key = format!("interpreters.{language}");
            let module = section.module.clone().ok_or_else(|| {
                ConfigError::new(
                    &format!("{key}.module"),
                    "required for a language mimlys does not ship",
                )
            })?;
            let sha256 = section.sha256.clone().ok_or_else(|| {
                ConfigError::new(
                    &format!("{key}.sha256"),
                    "required for a language mimlys does not ship",
                )
            })?;
            if sha256.len() != 64 || !sha256.chars().all(|c| c.is_ascii_hexdigit()) {
                return Err(ConfigError::new(
                    &format!("{key}.sha256"),
                    "must be 64 hexadecimal characters",
                ));
            }
            out.push(Configured {
                language: language.clone(),
                module,
                sha256: sha256.to_ascii_lowercase(),
                script_name: section
                    .script_name
                    .clone()
                    .unwrap_or_else(|| format!("main.{language}")),
            });
        }
        Ok(out)
    }

    /// The file to read: `--config`, then `MIMLYS_CONFIG`, then the
    /// platform's configuration directory when the file exists there.
    pub fn locate(explicit: Option<&Path>) -> Option<PathBuf> {
        if let Some(p) = explicit {
            return Some(p.to_path_buf());
        }
        if let Some(p) = std::env::var_os(CONFIG_ENV).filter(|v| !v.is_empty()) {
            return Some(PathBuf::from(p));
        }
        directories::BaseDirs::new()
            .map(|b| b.config_dir().join("mimlys").join(CONFIG_FILE))
            .filter(|p| p.is_file())
    }

    /// Reads and validates the file at `path`, or the defaults when there
    /// is none (REQ-CFG-002).
    pub fn load(path: Option<&Path>) -> Result<Loaded, ConfigError> {
        let config = match path {
            Some(p) => {
                let text = std::fs::read_to_string(p).map_err(|e| {
                    ConfigError::new("", format!("cannot read {}: {}", p.display(), e.kind()))
                })?;
                Self::parse(&text)?
            }
            None => Config::default(),
        };
        config.validate()?;
        let secrets = config.resolve_secrets()?;
        Ok(Loaded {
            config,
            path: path.map(Path::to_path_buf),
            secrets,
        })
    }

    /// Resolves every secret reference; a value too short to redact safely
    /// is refused (REQ-CTX-007, `MIN_SECRET_CHARS`).
    pub fn resolve_secrets(&self) -> Result<Vec<(String, String)>, ConfigError> {
        let mut out = Vec::new();
        if let Some(key) = &self.backend.api_key {
            let value = key.resolve()?;
            if value.chars().count() < mimlys_core::limits::MIN_SECRET_CHARS {
                return Err(ConfigError::new(
                    "backend.api_key",
                    format!(
                        "the resolved value is shorter than {} characters and could not be redacted safely",
                        mimlys_core::limits::MIN_SECRET_CHARS
                    ),
                ));
            }
            out.push(("backend.api_key".to_owned(), value));
        }
        for (name, s) in &self.servers {
            for (var, reference) in &s.env_secrets {
                let key = format!("servers.{name}.env.{var}");
                let value = reference
                    .resolve()
                    .map_err(|e| ConfigError::new(&key, e.reason))?;
                if value.chars().count() < mimlys_core::limits::MIN_SECRET_CHARS {
                    return Err(ConfigError::new(
                        &key,
                        format!(
                            "the resolved value is shorter than {} characters and could not be redacted safely",
                            mimlys_core::limits::MIN_SECRET_CHARS
                        ),
                    ));
                }
                out.push((key, value));
            }
        }
        Ok(out)
    }

    /// Parses TOML; an unknown key or an inline secret is an error naming the key.
    pub fn parse(text: &str) -> Result<Self, ConfigError> {
        toml::from_str::<Config>(text).map_err(|e| {
            let message = e.message().to_owned();
            if message.contains("retention_days") {
                ConfigError::new(
                    "workspace.retention_days",
                    "removed: nothing expires; delete a task with `mimlys task delete`",
                )
            } else {
                ConfigError::new("", message)
            }
        })
    }

    /// Checks versions, ranges and cross-field rules (REQ-CFG-001, CLR-3).
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.config_version != CONFIG_VERSION {
            return Err(ConfigError::new(
                "config_version",
                format!(
                    "{} is not supported; this version reads {CONFIG_VERSION}",
                    self.config_version
                ),
            ));
        }
        for (key, roots) in [
            ("host_read_roots", &self.policy.ceiling.host_read_roots),
            ("host_write_roots", &self.policy.ceiling.host_write_roots),
        ] {
            if roots.as_ref().is_some_and(|r| !r.is_empty()) {
                return Err(ConfigError::new(
                    &format!("policy.ceiling.{key}"),
                    HOST_PATH_GRANTS_RESERVED,
                ));
            }
        }
        let b = &self.backend;
        range("backend.timeout_ms", b.timeout_ms, LIM_M1_REQUEST_MS_RANGE)?;
        range("backend.max_tokens", b.max_tokens, LIM_M2_MAX_TOKENS_RANGE)?;
        range(
            "backend.context_chars",
            b.context_chars,
            LIM_C1_CONTEXT_CHARS_RANGE,
        )?;
        if !(0.0..=2.0).contains(&b.temperature) || b.temperature.is_nan() {
            return Err(ConfigError::new(
                "backend.temperature",
                format!("{} is outside 0.0–2.0", b.temperature),
            ));
        }
        if b.locality == Locality::Remote && !self.policy.remote_backend_allowed {
            return Err(ConfigError::new(
                "backend.locality",
                "`remote` needs policy.remote_backend_allowed = true",
            ));
        }
        match b.kind {
            BackendKind::Fake => {
                if b.url.is_some() || b.model.is_some() || b.api_key.is_some() {
                    return Err(ConfigError::new(
                        "backend.kind",
                        "`fake` takes no url, model or api_key; use `script` for a script file",
                    ));
                }
                if b.decision_transport == DecisionTransport::ToolCalls {
                    return Err(ConfigError::new(
                        "backend.decision_transport",
                        "the `fake` backend answers from a script and has no `tool_calls` transport",
                    ));
                }
            }
            BackendKind::OpenaiCompatible => {
                if b.url.is_none() {
                    return Err(ConfigError::new(
                        "backend.url",
                        "required for `openai_compatible`",
                    ));
                }
                if b.model.is_none() {
                    return Err(ConfigError::new(
                        "backend.model",
                        "required for `openai_compatible`",
                    ));
                }
                if b.script.is_some() {
                    return Err(ConfigError::new(
                        "backend.script",
                        "only the `fake` backend reads a script",
                    ));
                }
            }
        }
        for (name, s) in &self.servers {
            if name.is_empty()
                || !name.chars().next().is_some_and(|c| c.is_ascii_lowercase())
                || !name
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
            {
                return Err(ConfigError::new(
                    &format!("servers.{name}"),
                    "a server name is lowercase letters, digits and underscores, starting with a letter",
                ));
            }
            if s.command.trim().is_empty() {
                return Err(ConfigError::new(
                    &format!("servers.{name}.command"),
                    "required",
                ));
            }
            if let Some(v) = s.startup_ms {
                range(
                    &format!("servers.{name}.startup_ms"),
                    v,
                    LIM_O6_SERVER_STARTUP_MS_RANGE,
                )?;
            }
            if let Some(v) = s.shutdown_ms {
                range(
                    &format!("servers.{name}.shutdown_ms"),
                    v,
                    LIM_O16_SERVER_SHUTDOWN_MS_RANGE,
                )?;
            }
            if let Some(v) = s.max_call_ms {
                range(
                    &format!("servers.{name}.max_call_ms"),
                    v,
                    LIM_O5_EXTERNAL_DEADLINE_MS_RANGE,
                )?;
            }
            for (tool, d) in &s.tools {
                if let Some(v) = d.timeout_ms {
                    range(
                        &format!("servers.{name}.tools.{tool}.timeout_ms"),
                        v,
                        LIM_O5_EXTERNAL_DEADLINE_MS_RANGE,
                    )?;
                }
            }
        }
        for (name, s) in &self.host_tools {
            if let Some(v) = s.timeout_ms {
                range(
                    &format!("host_tools.{name}.timeout_ms"),
                    v,
                    LIM_H1_HOST_DEADLINE_MS_RANGE,
                )?;
            }
        }
        for d in self.host_tools() {
            d.check().map_err(|e| ConfigError::new(&e.key, e.reason))?;
        }
        // REQ-CONF-011: a read path that would expose credentials is refused
        // at load, as written, with the narrower path to declare.
        for (name, s) in &self.host_tools {
            for written in &s.read_paths {
                if let Some(reason) = crate::toolchain::read_path_refusal(written) {
                    return Err(ConfigError::new(
                        &format!("host_tools.{name}.read_paths"),
                        reason,
                    ));
                }
            }
        }
        // REQ-CONF-011, external design §13.5: a command whose canonical
        // target leaves the read paths is refused at load, on every platform,
        // naming the resolved path to declare.
        if let Ok(data_dir) = self.data_dir() {
            let fixed = crate::app::fixed_read_paths(&data_dir);
            for d in self.host_tools() {
                if let Some(reason) = crate::toolchain::command_refusal(&d, &fixed) {
                    return Err(ConfigError::new(
                        &format!("host_tools.{}.command", d.name),
                        reason,
                    ));
                }
            }
        }
        self.resolve_policy()?;
        let s = &self.sandbox;
        range("sandbox.fuel", s.fuel, LIM_S1_FUEL_RANGE)?;
        range(
            "sandbox.deadline_ms",
            s.deadline_ms,
            LIM_S2_DEADLINE_MS_RANGE,
        )?;
        range("sandbox.memory_mib", s.memory_mib, LIM_S3_MEMORY_MIB_RANGE)?;
        range(
            "sandbox.output_bytes",
            s.output_bytes,
            LIM_S4_OUTPUT_BYTES_RANGE,
        )?;
        let w = &self.workspace;
        range(
            "workspace.max_bytes",
            w.max_bytes,
            LIM_W1_WORKSPACE_BYTES_RANGE,
        )?;
        if w.max_files == 0 {
            return Err(ConfigError::new(
                "workspace.max_files",
                "must be at least 1",
            ));
        }
        self.interpreters(Path::new("."))?;
        const LEVELS: [&str; 5] = ["error", "warn", "info", "debug", "trace"];
        if !LEVELS.contains(&self.logging.level.as_str()) {
            return Err(ConfigError::new(
                "logging.level",
                format!("`{}` is not one of {LEVELS:?}", self.logging.level),
            ));
        }
        Ok(())
    }

    /// The data directory: `runtime.data_dir` or the platform's user data
    /// directory plus `mimlys` (external design §14.1).
    pub fn data_dir(&self) -> Result<PathBuf, ConfigError> {
        if let Some(p) = &self.runtime.data_dir {
            return Ok(p.clone());
        }
        directories::BaseDirs::new()
            .map(|b| b.data_local_dir().join("mimlys"))
            .ok_or_else(|| {
                ConfigError::new(
                    "runtime.data_dir",
                    "no home directory; set runtime.data_dir",
                )
            })
    }

    /// Whether a configured `runtime.data_dir` lies inside the temporary
    /// directory (REQ-WS-001): a cleaner of temporary files can then delete
    /// tasks. Both paths are compared canonical, the data directory by its
    /// longest existing ancestor when it does not exist yet. The default
    /// location is never configured and never in question.
    pub fn data_dir_in_temp_dir(&self) -> bool {
        self.runtime.data_dir.as_deref().is_some_and(|dir| {
            canonical_lenient(dir).starts_with(canonical_lenient(&std::env::temp_dir()))
        })
    }

    /// The resolved configuration as TOML, secrets as references only.
    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).unwrap_or_default()
    }
}

/// The path made absolute and canonical; a path that does not exist yet is
/// canonical up to its longest existing ancestor, followed by the rest with
/// its `..` resolved by name (nothing that does not exist can be a link).
fn canonical_lenient(path: &Path) -> PathBuf {
    use std::path::Component;
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let components: Vec<Component<'_>> = absolute.components().collect();
    for existing in (1..=components.len()).rev() {
        let prefix: PathBuf = components[..existing].iter().collect();
        if let Ok(mut canonical) = prefix.canonicalize() {
            for component in &components[existing..] {
                match component {
                    Component::ParentDir => {
                        canonical.pop();
                    }
                    Component::CurDir => {}
                    other => canonical.push(other),
                }
            }
            return canonical;
        }
    }
    absolute
}

#[cfg(test)]
mod tests;
