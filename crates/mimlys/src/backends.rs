//! The named backends (REQ-CFG-007, REQ-MDL-010, REQ-SEC-010; RFC 015 decision 3 and Amendment 1).
//!
//! `[backends.<name>]` is a table of backends. This module is where one is found by its name
//! and made ([`Backend::make`]), probed ([`Backend::probe`]) and given its engine gate
//! ([`Backend::gate`]); where the configuration's rules about them are checked
//! ([`Config::validate_backends`]); and where the one that runs a task is chosen: the one named
//! [`DEFAULT`], and none other, never a guess ([`Config::default_backend`]). A command that needs
//! a backend by name (`model probe <name>`) uses [`Config::backend`], which names the configured
//! ones when it finds none.
//!
//! **Locality is checked against the URL** (REQ-SEC-010): `local` is allowed only for a loopback
//! host, read from the URL as the HTTP client reads it and never resolved ([`host_of`]).

use crate::app::AppError;
use crate::config::{
    BackendKind, BackendSection, Config, ConfigError, DecisionTransport, Loaded, StructuredOutput,
    range,
};
use mimlys_core::limits::{
    LIM_C1_CONTEXT_CHARS_RANGE, LIM_M1_REQUEST_MS_RANGE, LIM_M2_MAX_TOKENS_RANGE,
    LIM_O12_MAX_IN_FLIGHT_RANGE,
};
use mimlys_core::spi::{
    BackendDescriptor, BackendError, BoxFuture, Cancellation, GenerateRequest, Locality,
    ModelBackend,
};
use mimlys_model::fake::FakeBackend;
use mimlys_model::openai::{Mode, OpenAiBackend, OpenAiConfig, Probe, Resolved, TransportMode};
use std::collections::BTreeMap;
use std::sync::Arc;

/// The backend a task runs on until a level names another (RFC 015, Amendment 1): the entry named
/// `default`, which a version-1 file's `[backend]` is.
pub const DEFAULT: &str = "default";

/// The longest backend name, in characters.
pub const NAME_MAX_CHARS: usize = 32;

/// Why `name` is not a backend name, or `None` when it is: lowercase ASCII letters, digits, `_`
/// and `-`, at most [`NAME_MAX_CHARS`], starting with a letter or a digit (a name that starts with
/// `-` could not be given on a command line: it would read as a flag).
pub fn name_problem(name: &str) -> Option<String> {
    let ok = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit();
    if name.is_empty() {
        return Some("a backend name is not empty".to_owned());
    }
    if name.chars().count() > NAME_MAX_CHARS {
        return Some(format!(
            "a backend name is at most {NAME_MAX_CHARS} characters"
        ));
    }
    if !name.chars().next().is_some_and(ok) || !name.chars().all(|c| ok(c) || c == '_' || c == '-')
    {
        return Some(
            "a backend name is lowercase letters, digits, `_` and `-`, starting with a letter or a digit"
                .to_owned(),
        );
    }
    None
}

/// What a URL's host is, for the locality rule.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Host {
    /// The host as the URL parser reads it (the text the HTTP client connects to): lower case,
    /// an IPv6 address in brackets.
    pub text: String,
    /// `localhost`, an address of `127.0.0.0/8`, or `::1`: nothing else.
    pub loopback: bool,
}

/// Why a URL has no host the rule can read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UrlProblem {
    /// It does not parse (the reason is the parser's; the text is not echoed, it may carry a
    /// password).
    NotAUrl(String),
    /// The scheme is neither `http` nor `https`.
    Scheme(String),
    /// There is no host.
    NoHost,
}

impl std::fmt::Display for UrlProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UrlProblem::NotAUrl(why) => write!(f, "is not a URL ({why})"),
            UrlProblem::Scheme(scheme) => {
                write!(
                    f,
                    "has the scheme `{scheme}`; an endpoint is `http` or `https`"
                )
            }
            UrlProblem::NoHost => f.write_str("has no host"),
        }
    }
}

/// The host of an endpoint's URL, and whether it is loopback (REQ-SEC-010).
///
/// The rule is on the host **as written in the URL**, after the parse that the HTTP client itself
/// makes, so that what is judged is what is dialled; it is never resolved, so a name that
/// resolves to a loopback address is not loopback. Loopback is `localhost` exactly, an address of
/// `127.0.0.0/8` and `::1`; nothing else: not `localhost.` or `x.localhost`, not `0.0.0.0` (not a
/// destination), not `::ffff:127.0.0.1` (an IPv4 address in an IPv6 form: write `127.0.0.1`), not
/// `::`. A user name and password in the URL are not the host (`http://127.0.0.1@example.org/` is
/// `example.org`); an IPv6 zone does not parse.
pub fn host_of(url: &str) -> Result<Host, UrlProblem> {
    let parsed = url::Url::parse(url).map_err(|e| UrlProblem::NotAUrl(e.to_string()))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(UrlProblem::Scheme(parsed.scheme().to_owned()));
    }
    let loopback = match parsed.host().ok_or(UrlProblem::NoHost)? {
        url::Host::Domain(name) => name == "localhost",
        url::Host::Ipv4(ip) => ip.octets()[0] == 127,
        url::Host::Ipv6(ip) => ip == std::net::Ipv6Addr::LOCALHOST,
    };
    let text = parsed.host_str().ok_or(UrlProblem::NoHost)?.to_owned();
    Ok(Host { text, loopback })
}

impl BackendSection {
    /// The range, name, kind and locality rules of one backend (REQ-CFG-001, REQ-SEC-010), keyed
    /// `backends.<name>.<key>`.
    fn validate(&self, name: &str, remote_allowed: bool) -> Result<(), ConfigError> {
        let key = |k: &str| format!("backends.{name}.{k}");
        range(&key("timeout_ms"), self.timeout_ms, LIM_M1_REQUEST_MS_RANGE)?;
        range(
            &key("max_in_flight"),
            self.max_in_flight,
            LIM_O12_MAX_IN_FLIGHT_RANGE,
        )?;
        range(&key("max_tokens"), self.max_tokens, LIM_M2_MAX_TOKENS_RANGE)?;
        range(
            &key("context_chars"),
            self.context_chars,
            LIM_C1_CONTEXT_CHARS_RANGE,
        )?;
        if !(0.0..=2.0).contains(&self.temperature) || self.temperature.is_nan() {
            return Err(ConfigError {
                key: key("temperature"),
                reason: format!("{} is outside 0.0–2.0", self.temperature),
            });
        }
        if self.locality == Locality::Remote && !remote_allowed {
            return Err(ConfigError {
                key: key("locality"),
                reason: format!(
                    "backend `{name}` is `remote`, which needs policy.remote_backend_allowed = true"
                ),
            });
        }
        let refuse = |k: &str, reason: &str| {
            Err(ConfigError {
                key: key(k),
                reason: reason.to_owned(),
            })
        };
        match self.kind {
            BackendKind::Fake => {
                if self.url.is_some() || self.model.is_some() || self.api_key.is_some() {
                    return refuse(
                        "kind",
                        "`fake` takes no url, model or api_key; use `script` for a script file",
                    );
                }
                if self.decision_transport == DecisionTransport::ToolCalls {
                    return refuse(
                        "decision_transport",
                        "the `fake` backend answers from a script and has no `tool_calls` transport",
                    );
                }
                // No URL: it sends nothing anywhere, so there is nothing to compare its
                // declaration with.
            }
            BackendKind::OpenaiCompatible => {
                let Some(url) = &self.url else {
                    return refuse("url", "required for `openai_compatible`");
                };
                if self.model.is_none() {
                    return refuse("model", "required for `openai_compatible`");
                }
                if self.script.is_some() {
                    return refuse("script", "only the `fake` backend reads a script");
                }
                let host = host_of(url).map_err(|problem| ConfigError {
                    key: key("url"),
                    reason: format!("backend `{name}`: the url {problem}"),
                })?;
                if self.locality == Locality::Local && !host.loopback {
                    return Err(ConfigError {
                        key: key("locality"),
                        reason: format!(
                            "backend `{name}` is declared `local`, but the host of its url, {}, is not loopback (`localhost`, 127.0.0.0/8 or ::1, read as written and never resolved); declare it `remote` (which needs policy.remote_backend_allowed = true and a profile with external = true), or give it a loopback url",
                            host.text
                        ),
                    });
                }
            }
        }
        Ok(())
    }
}

impl Config {
    /// The rules of the `[backends]` table (REQ-CFG-007): each entry's name and values, its
    /// locality against its URL (REQ-SEC-010), and that entries on one endpoint agree on
    /// `max_in_flight`.
    ///
    /// Entries are on **one endpoint** when the engine gate treats them as one
    /// ([`crate::registry::endpoint_of`]): the same scheme, host (in lower case) and port, whatever
    /// the path, user name or query; LIM-O12 is counted per endpoint, so two limits for one count
    /// cannot both hold. `localhost` and `127.0.0.1` are different endpoints to the gate, and so
    /// to this rule.
    pub(crate) fn validate_backends(&self) -> Result<(), ConfigError> {
        for (name, b) in &self.backends {
            if let Some(problem) = name_problem(name) {
                return Err(ConfigError {
                    key: format!("backends.{name}"),
                    reason: problem,
                });
            }
            b.validate(name, self.policy.remote_backend_allowed)?;
        }
        let mut seen: BTreeMap<String, (&str, usize)> = BTreeMap::new();
        for (name, b) in &self.backends {
            let Some((key, shown)) = b.url.as_deref().and_then(crate::registry::endpoint_of) else {
                continue;
            };
            match seen.get(&key) {
                Some((first, limit)) if *limit != b.max_in_flight => {
                    return Err(ConfigError {
                        key: format!("backends.{name}.max_in_flight"),
                        reason: format!(
                            "backends `{first}` and `{name}` are on one endpoint, {shown}, with max_in_flight {limit} and {}; the engine gate counts requests per endpoint (LIM-O12), so entries on one endpoint must agree",
                            b.max_in_flight
                        ),
                    });
                }
                Some(_) => {}
                None => {
                    seen.insert(key, (name, b.max_in_flight));
                }
            }
        }
        Ok(())
    }

    /// The names of the configured backends, in order.
    pub fn backend_names(&self) -> Vec<&str> {
        self.backends.keys().map(String::as_str).collect()
    }

    /// The backend named `name`; the error names the configured ones. Nothing falls back to
    /// another (RFC 015, Amendment 1).
    pub fn backend(&self, name: &str) -> Result<&BackendSection, ConfigError> {
        self.backends.get(name).ok_or_else(|| ConfigError {
            key: "backends".into(),
            reason: crate::messages::no_backend_named(name, &self.backend_names()),
        })
    }

    /// The backend a task runs on: the one named [`DEFAULT`], which a version-1 file's `[backend]`
    /// is. A configuration without it has none: the error says so and lists the configured ones.
    pub fn default_backend(&self) -> Result<&BackendSection, ConfigError> {
        self.backend(DEFAULT)
    }
}

/// The backend's probe result as `status` shows it (RFC 008 §4).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct BackendProbe {
    /// The server answered the model listing.
    pub reachable: bool,
    /// The configured model is listed, when the listing was readable.
    pub model_listed: Option<bool>,
    /// The structured-output mode in force; none under the `tool_calls` transport, which does not use it.
    pub structured_output: Option<Resolved>,
    /// `auto` ended below `json_schema`.
    pub fell_back: bool,
    /// A fixed-vocabulary reason when something was not as configured.
    pub detail: Option<String>,
    /// The decision transport in force (REQ-MDL-009), once settled.
    pub decision_transport: Option<mimlys_core::native::DecisionTransport>,
    /// Why `auto` chose `envelope`, or why a configured `tool_calls` is refused: the engine's reason.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transport_reason: Option<String>,
    /// Whether the engine accepts `parallel_tool_calls: false`; under `tool_calls` only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parallel_tool_calls: Option<bool>,
    /// The engine's context length in tokens, as the engine reports it; null when
    /// it reports none (REQ-MDL-003). Not applicable to the fake backend.
    pub context_tokens: Option<u64>,
    /// The engine that reported it (`llama_cpp`, `ollama`); absent when unknown.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_engine: Option<mimlys_model::openai::context::Engine>,
    /// llama.cpp: the parallel slots its `-c` is shared among; absent otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_slots: Option<u64>,
    /// The tokens a request can need: the budget at the lowest characters per
    /// token measured for the transport in force, plus the completion bound; null
    /// for the fake backend.
    pub context_needed_tokens: Option<u64>,
    /// Whether the engine holds the configured model in memory (REQ-MDL-010): Ollama's
    /// `/api/ps`; always `true` for a llama-server; null (unknown) for another engine and for the
    /// fake backend.
    pub resident: Option<bool>,
}

impl BackendProbe {
    /// The warning line for a context below the need; `None` when it is
    /// sufficient or unknown (REQ-MDL-003).
    pub fn context_warning(&self) -> Option<String> {
        if !self.context_too_small() {
            return None;
        }
        let (tokens, needed) = (self.context_tokens?, self.context_needed_tokens?);
        Some(crate::messages::context_too_small_warning(
            self.context_engine,
            tokens,
            needed,
            self.context_slots,
        ))
    }

    /// The engine reports a context length below the need (REQ-MDL-003).
    pub fn context_too_small(&self) -> bool {
        self.context_needed_tokens.is_some_and(|need| {
            mimlys_model::openai::context::is_too_small(self.context_tokens, need)
        })
    }
}

/// One configured backend, made: its name, its section, the adapter that talks to it, and, for
/// an `openai_compatible` one, the adapter as that type (for the probe).
#[derive(Clone)]
pub struct Backend {
    /// The name the configuration gives it.
    pub name: String,
    /// Its section.
    pub section: BackendSection,
    /// The adapter the runtime calls.
    pub adapter: Arc<dyn ModelBackend>,
    /// The adapter, when the backend is `openai_compatible`.
    pub openai: Option<Arc<OpenAiBackend>>,
}

impl Backend {
    /// Makes the backend `name` of the configuration (or, in a test, around `supplied`); nothing
    /// is contacted. An unknown name is the error [`Config::backend`] gives.
    pub fn make(
        loaded: &Loaded,
        name: &str,
        supplied: Option<Arc<dyn ModelBackend>>,
    ) -> Result<Self, AppError> {
        let section = loaded.config.backend(name)?.clone();
        let mut openai: Option<Arc<OpenAiBackend>> = None;
        let adapter: Arc<dyn ModelBackend> = match supplied {
            Some(b) => b,
            None => match section.kind {
                BackendKind::Fake => match &section.script {
                    Some(path) => {
                        Arc::new(FakeBackend::from_script(path).map_err(|e| AppError::Io {
                            detail: format!(
                                "cannot read the script of backend `{name}`, {}: {}",
                                path.display(),
                                e.kind()
                            ),
                        })?)
                    }
                    None => Arc::new(FakeBackend::with_decisions(vec![])),
                },
                BackendKind::OpenaiCompatible => {
                    let secret = format!("backends.{name}.api_key");
                    let made = Arc::new(OpenAiBackend::new(OpenAiConfig {
                        url: section.url.clone().unwrap_or_default(),
                        model: section.model.clone().unwrap_or_default(),
                        api_key: loaded
                            .secrets
                            .iter()
                            .find(|(n, _)| *n == secret)
                            .map(|(_, value)| value.clone()),
                        timeout_ms: section.timeout_ms,
                        max_tokens: section.max_tokens,
                        temperature: section.temperature,
                        decision_transport: match section.decision_transport {
                            DecisionTransport::Auto => TransportMode::Auto,
                            DecisionTransport::ToolCalls => TransportMode::ToolCalls,
                            DecisionTransport::Envelope => TransportMode::Envelope,
                        },
                        structured_output: match section.structured_output {
                            StructuredOutput::Auto => Mode::Auto,
                            StructuredOutput::Schema => Mode::Schema,
                            StructuredOutput::None => Mode::None,
                        },
                        locality: section.locality,
                    }));
                    openai = Some(Arc::clone(&made));
                    made
                }
            },
        };
        Ok(Self {
            name: name.to_owned(),
            section,
            adapter,
            openai,
        })
    }

    /// The backend a task runs on ([`DEFAULT`]), or, when the configuration has none, the backend
    /// that sends nothing and says why ([`Absent`]): a command that runs no step needs none, and
    /// a command that runs steps is refused before it by [`Config::default_backend`].
    pub fn make_default(
        loaded: &Loaded,
        supplied: Option<Arc<dyn ModelBackend>>,
    ) -> Result<Self, AppError> {
        if loaded.config.backends.contains_key(DEFAULT) {
            return Self::make(loaded, DEFAULT, supplied);
        }
        Ok(Self {
            name: DEFAULT.to_owned(),
            section: BackendSection::default(),
            adapter: supplied.unwrap_or_else(|| Arc::new(Absent)),
            openai: None,
        })
    }

    /// Every configured backend, each made on its own: the one named [`DEFAULT`] first when there
    /// is one, the others by name. Nothing is contacted. One that cannot be made (a `fake`
    /// backend's script that cannot be read) is its own error in its own place, and the others
    /// are made all the same: a diagnosis must not hide what it could show (`status`).
    pub fn make_each(loaded: &Loaded) -> Vec<(String, Result<Self, AppError>)> {
        let mut names = loaded.config.backend_names();
        if let Some(at) = names.iter().position(|n| *n == DEFAULT) {
            let default = names.remove(at);
            names.insert(0, default);
        }
        names
            .into_iter()
            .map(|name| (name.to_owned(), Self::make(loaded, name, None)))
            .collect()
    }

    /// The endpoint the engine gate is keyed by (RFC 013 decision 7): `None` for a backend with no
    /// URL, which has nothing to share a gate over.
    pub fn endpoint(&self) -> Option<(String, String)> {
        match (self.section.kind, self.section.url.as_deref()) {
            (BackendKind::OpenaiCompatible, Some(url)) => crate::registry::endpoint_of(url),
            _ => None,
        }
    }

    /// The engine gate of this backend's endpoint, in the registry of the user's runtimes in
    /// `registry_dir` (RFC 013 decision 7): at most `max_in_flight` model requests of the user at
    /// once. `None` for a backend with no endpoint. Two backends on one endpoint share one gate
    /// (and agree on `max_in_flight`: the configuration is refused otherwise).
    pub fn gate(
        &self,
        registry_dir: Result<std::path::PathBuf, String>,
        owner: crate::registry::Owner,
    ) -> Option<Arc<dyn mimlys_core::spi::Admission>> {
        match (self.section.kind, self.section.url.as_deref()) {
            (BackendKind::OpenaiCompatible, Some(url)) => {
                crate::registry::gate_in(registry_dir, url, self.section.max_in_flight, owner)
            }
            _ => None,
        }
    }

    /// Whether this backend can produce decisions at all: the fake backend without a script
    /// cannot (REQ-CFG-002 lets such a process run `status`, `check` and the commands that need
    /// no decision).
    pub fn ready(&self) -> Result<(), AppError> {
        if self.openai.is_none()
            && self.section.kind == BackendKind::Fake
            && self.section.script.is_none()
        {
            return Err(AppError::Io {
                detail: crate::messages::fake_backend_no_script(&self.name),
            });
        }
        Ok(())
    }

    /// Probes the backend (LIM-M4; RFC 008 §4): the model listing and the structured-output mode.
    /// A `schema` backend whose server refuses the schema is an error naming the fix. Returns the
    /// probe and the notice to print when `auto` fell back to plain text. Shared by the commands
    /// that write and by `status`, which only reads (REQ-PERS-013).
    pub async fn probe(&self) -> Result<(BackendProbe, Option<String>), AppError> {
        let b = &self.section;
        let probe = match &self.openai {
            Some(adapter) => {
                let Probe {
                    reachable,
                    model_listed,
                    structured_output,
                    fell_back,
                    detail,
                    decision_transport,
                    transport_reason,
                    parallel_tool_calls,
                    context_tokens,
                    context_engine,
                    context_slots,
                    resident,
                } = adapter.probe().await;
                BackendProbe {
                    reachable,
                    model_listed,
                    structured_output,
                    fell_back,
                    detail,
                    decision_transport,
                    transport_reason,
                    parallel_tool_calls,
                    context_tokens,
                    context_engine,
                    context_slots,
                    context_needed_tokens: Some(mimlys_model::openai::context::need_tokens(
                        b.context_chars,
                        b.max_tokens,
                        decision_transport,
                    )),
                    resident,
                }
            }
            None => {
                let no_script = b.kind == BackendKind::Fake && b.script.is_none();
                BackendProbe {
                    reachable: !no_script,
                    model_listed: None,
                    structured_output: Some(Resolved::None),
                    fell_back: false,
                    detail: no_script.then(|| "no_script".to_owned()),
                    decision_transport: Some(mimlys_core::native::DecisionTransport::Envelope),
                    transport_reason: None,
                    parallel_tool_calls: None,
                    context_tokens: None,
                    context_engine: None,
                    context_slots: None,
                    context_needed_tokens: None,
                    resident: None,
                }
            }
        };
        if b.structured_output == StructuredOutput::Schema
            && probe.reachable
            && probe.detail.as_deref() == Some("format_refused")
        {
            return Err(AppError::Io {
                detail: format!(
                    "backend `{}` unusable: the server refuses json_schema; set structured_output = \"none\" or \"auto\" in [backends.{}]",
                    self.name, self.name
                ),
            });
        }
        let notice = if probe.detail.as_deref() == Some("tool_calls_refused") {
            Some(crate::messages::tool_calls_refused(
                &self.name,
                probe
                    .transport_reason
                    .as_deref()
                    .unwrap_or("no reason given"),
            ))
        } else {
            (probe.fell_back && probe.structured_output == Some(Resolved::None))
                .then(|| crate::messages::structured_output_none().to_owned())
        };
        Ok((probe, notice))
    }
}

/// The configured backends, found by name (REQ-CFG-007, REQ-MDL-010; RFC 015, Amendment 1): the
/// directory the runtime resolves a task's backend in, at every start.
///
/// A backend is **made when it is first asked for**, and never before: making one builds an
/// adapter and contacts nothing, and a command that runs no task asks for none, so it opens a
/// connection to none. A backend that has been made is kept, so that the runtime, `status` and
/// the probe of a start share one adapter (and its HTTP client) for a name. What is configured is
/// what the process read when it started: a later edit of the file is read by the next process,
/// and a task's backend is resolved again by its name there.
pub struct ConfiguredBackends {
    loaded: Loaded,
    registry_dir: Result<std::path::PathBuf, String>,
    owner: crate::registry::Owner,
    made: std::sync::Mutex<BTreeMap<String, Arc<Backend>>>,
}

impl ConfiguredBackends {
    /// The directory of `loaded`'s backends. `default`, when `made` holds it, is that one (the
    /// backend the process made at its start, or a test's own); nothing else is made yet.
    pub fn new(
        loaded: Loaded,
        registry_dir: Result<std::path::PathBuf, String>,
        owner: crate::registry::Owner,
        made: Option<Arc<Backend>>,
    ) -> Self {
        let held = made
            .into_iter()
            .map(|backend| (backend.name.clone(), backend))
            .collect();
        Self {
            loaded,
            registry_dir,
            owner,
            made: std::sync::Mutex::new(held),
        }
    }

    /// The backend named `name`, made now if it has not been. An unknown name is the error
    /// [`Config::backend`] gives, naming the configured ones.
    pub fn get(&self, name: &str) -> Result<Arc<Backend>, AppError> {
        let mut made = self.made.lock().expect("backends lock");
        if let Some(backend) = made.get(name) {
            return Ok(Arc::clone(backend));
        }
        let backend = Arc::new(Backend::make(&self.loaded, name, None)?);
        made.insert(name.to_owned(), Arc::clone(&backend));
        Ok(backend)
    }

    /// The names of the backends made so far: what a command opened, and no other.
    pub fn made_names(&self) -> Vec<String> {
        self.made
            .lock()
            .expect("backends lock")
            .keys()
            .cloned()
            .collect()
    }
}

impl mimlys_core::spi::BackendDirectory for ConfiguredBackends {
    fn resolve(
        &self,
        name: &str,
    ) -> Result<mimlys_core::spi::ResolvedBackend, mimlys_core::spi::UnknownBackend> {
        let configured = || {
            self.loaded
                .config
                .backend_names()
                .into_iter()
                .map(str::to_owned)
                .collect::<Vec<_>>()
        };
        if !self.loaded.config.backends.contains_key(name) {
            return Err(mimlys_core::spi::UnknownBackend {
                name: name.to_owned(),
                configured: configured(),
                unusable: None,
            });
        }
        let backend = self
            .get(name)
            .map_err(|error| mimlys_core::spi::UnknownBackend {
                name: name.to_owned(),
                configured: configured(),
                unusable: Some(error.to_string()),
            })?;
        Ok(mimlys_core::spi::ResolvedBackend {
            backend: Arc::clone(&backend.adapter),
            gate: backend.gate(self.registry_dir.clone(), self.owner.clone()),
            context_chars: backend.section.context_chars,
        })
    }

    fn names(&self) -> Vec<String> {
        self.loaded
            .config
            .backend_names()
            .into_iter()
            .map(str::to_owned)
            .collect()
    }
}

/// What probing one backend gave: the probe, and the notice to print for it, or why it could not
/// be probed.
pub type Probed = Result<(BackendProbe, Option<String>), AppError>;

/// Probes every backend **at once** (REQ-MDL-010, LIM-M4): each probe is bounded by LIM-M4 by
/// itself ([`Backend::probe`]) and all of them are started before any is waited for, so the whole
/// is bounded by one LIM-M4, whatever the number of backends. The results are in the order of
/// `backends`. A probe that panics is an error naming nothing of its own: the caller knows
/// which backend it was by the position.
pub async fn probe_all(backends: &[Backend]) -> Vec<Probed> {
    let mut set = tokio::task::JoinSet::new();
    for (position, backend) in backends.iter().enumerate() {
        let backend = backend.clone();
        set.spawn(async move { (position, backend.probe().await) });
    }
    let mut results: Vec<Option<Probed>> = backends.iter().map(|_| None).collect();
    while let Some(joined) = set.join_next().await {
        if let Ok((position, probed)) = joined {
            results[position] = Some(probed);
        }
    }
    results
        .into_iter()
        .map(|probed| {
            probed.unwrap_or_else(|| {
                Err(AppError::Io {
                    detail: "the probe of a backend ended without an answer".to_owned(),
                })
            })
        })
        .collect()
}

/// The backend of a configuration that has none named [`DEFAULT`]: it never sends. A command that
/// runs steps is refused before a step ([`Config::default_backend`]); one that does not (a
/// `rollback`, a `task cancel`) needs no model, and this keeps it from failing for want of one.
/// If a step ever reaches it, the request fails `backend_unavailable` and nothing is sent.
struct Absent;

impl ModelBackend for Absent {
    fn describe(&self) -> BackendDescriptor {
        BackendDescriptor {
            kind: "none".to_owned(),
            model: None,
            locality: Locality::Local,
        }
    }

    fn generate<'a>(
        &'a self,
        _request: GenerateRequest,
        _cancel: Cancellation,
    ) -> BoxFuture<'a, Result<Vec<u8>, BackendError>> {
        Box::pin(async {
            Err(BackendError::Unavailable {
                detail: "no_default_backend".to_owned(),
            })
        })
    }
}

#[cfg(test)]
mod tests;
