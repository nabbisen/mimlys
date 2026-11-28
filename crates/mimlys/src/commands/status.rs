//! `mimlys status` (REQ-OBS-006).

use crate::app::{AppError, ReadApp, StoreView, probe_confinement};
use crate::backends::Backend;
use crate::messages;
use crate::output::Output;
use serde::Serialize;

/// The status object: the runtime's status plus the process's view.
#[derive(Serialize)]
struct Status<'a> {
    #[serde(flatten)]
    runtime: &'a mimlys_core::runtime::RuntimeStatus,
    /// The backend a task runs on, the entry named `default`: what `backends` holds for it, as
    /// it was before several backends; `null` when no backend is named `default`. **Deprecated**
    /// (RFC 015, Amendment 1): kept until the release that prepares 1.0; read `backends`.
    backend: Option<BackendView>,
    /// Which mode is in force: `embedded`, or `served` by a daemon (REQ-ARCH-009, from 0.14).
    mode: &'static str,
    /// The daemon, when served: its process, version, socket and uptime.
    #[serde(skip_serializing_if = "Option::is_none")]
    daemon: Option<serde_json::Value>,
    /// Every configured backend, `default` first, then by name: each probed at once within one
    /// LIM-M4 (REQ-MDL-010).
    backends: Vec<BackendView>,
    /// The three effort levels (REQ-TASK-013): each with its backend and its keys, and whether a
    /// task could be created at it now (`configured`, `allowed`).
    effort: Vec<EffortView>,
    data_dir: String,
    /// The configured data directory is inside the temporary directory (REQ-WS-001).
    data_dir_in_temporary_directory: bool,
    /// macOS: `xcrun`'s cache file, when it is missing and could not be made.
    #[serde(skip_serializing_if = "Option::is_none")]
    xcrun_cache_missing: Option<String>,
    /// macOS: why the sweep from outside the sandbox is unavailable (the test
    /// that finds a worker's members did not hold at the verification); shown
    /// only then. The tier is the same either way.
    #[serde(skip_serializing_if = "Option::is_none")]
    outside_sweep_unavailable: Option<String>,
    /// Windows: the one-time migration of 0.11 (the container, the per-task
    /// entries) has not finished; shown only then.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    permissions_migration_pending: bool,
    config: Option<String>,
    /// What recovery did at startup: always empty here, because `status` recovers
    /// nothing (REQ-PERS-013); the field stays so that the JSON shape does.
    recovered: Vec<serde_json::Value>,
    engine_version: &'static str,
    interpreters: Vec<serde_json::Value>,
    /// The configured MCP servers (RFC 009 §8).
    servers: Vec<mimlys_mcp::ServerStatus>,
    /// The declared host tools (RFC 011 §6, §7).
    host_tools: Vec<HostToolView>,
    /// The worker's read set (REQ-CONF-008).
    read_set: Vec<String>,
    /// What the dependency step's worker reads beyond `read_set` (empty off Linux).
    dependency_step_read_set_extra: Vec<String>,
    /// The store as a reader finds it: free, in use by another runtime (and
    /// which), needing recovery, or not created yet (REQ-PERS-013).
    store: StoreView,
    /// The user's runtimes, the slots and the engine gates (RFC 013 decision 8).
    machine: crate::machine::Machine,
}

/// One effort level in the status (REQ-TASK-013; external design §19.4, "Shown"): the keys it sets,
/// the backend it runs on, and why a task could not be created at it, when it could not.
#[derive(Serialize)]
struct EffortView {
    level: &'static str,
    /// The level has a key, shipped or configured; `standard` always has (it is today's behaviour).
    /// A level that is not configured is refused when chosen.
    configured: bool,
    /// The default profile's `effort_max` allows it.
    allowed: bool,
    /// The `effort_max` of the default profile, and what sets it, when one is set.
    #[serde(skip_serializing_if = "Option::is_none")]
    effort_max: Option<String>,
    /// The backend it runs on: its own, else `default`.
    backend: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    steps: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    deadline_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    context_chars: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    recent_steps: Option<u32>,
}

/// The three levels as the configuration leaves them (REQ-TASK-013), and the lines that say them.
fn effort_views(
    config: &crate::config::Config,
    policy: &mimlys_core::policy::Policy,
    backends: &[BackendView],
) -> (Vec<EffortView>, Vec<String>) {
    use mimlys_core::effort::{Bound, EffortLevel};
    let levels = config.effort_levels();
    let (max, bound) = policy
        .profile(&policy.default_profile)
        .map(|p| policy.effort_bound(p))
        .unwrap_or((EffortLevel::Deep, Bound::None));
    let set_by = match &bound {
        Bound::Profile(name) => Some(format!("[policy.profiles.{name}] effort_max")),
        Bound::Ceiling => Some("[policy.ceiling] effort_max".to_owned()),
        Bound::None => None,
    };
    let mut views = Vec::new();
    let mut lines = Vec::new();
    for level in EffortLevel::ALL {
        let keys = levels.keys(level);
        let backend = keys
            .backend
            .clone()
            .unwrap_or_else(|| crate::backends::DEFAULT.to_owned());
        let model = backends
            .iter()
            .find(|v| v.name == backend)
            .and_then(|v| v.model.clone());
        let view = EffortView {
            level: level.name(),
            configured: levels.is_configured(level),
            allowed: level <= max,
            effort_max: set_by.as_ref().map(|by| format!("{max} ({by})")),
            backend,
            model,
            steps: keys.steps,
            deadline_ms: keys.deadline_ms,
            context_chars: keys.context_chars,
            recent_steps: keys.recent_steps,
        };
        lines.push(messages::status_effort_line(
            level.name(),
            &messages::effort_state(&view.state_parts(), set_by.as_deref(), max),
        ));
        views.push(view);
    }
    (views, lines)
}

impl EffortView {
    /// What the line of the level is made of.
    fn state_parts(&self) -> messages::EffortState<'_> {
        messages::EffortState {
            level: self.level,
            configured: self.configured,
            allowed: self.allowed,
            backend: &self.backend,
            model: self.model.as_deref(),
            steps: self.steps,
            deadline_ms: self.deadline_ms,
            context_chars: self.context_chars,
            recent_steps: self.recent_steps,
        }
    }
}

/// One declared host tool: its command, whether it is the dependency step
/// (`network`), and the reason it cannot run on this machine, if any
/// (`acl_not_applied: <path>` until `toolchain grant`; RFC 011 §7).
#[derive(Serialize)]
struct HostToolView {
    name: String,
    command: String,
    network: bool,
    /// The declaration says `allow_paths = true` (A-34): shown only then.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    allow_paths: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    unavailable: Option<String>,
}

/// One backend in the status: its name, the descriptor, the probe's findings (RFC 008 §4,
/// REQ-MDL-010) or the reason it cannot be used, and the last fitness probe (REQ-MDL-011), `null`
/// when it was never probed.
#[derive(Clone, Serialize)]
struct BackendView {
    name: String,
    kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    locality: mimlys_core::spi::Locality,
    #[serde(flatten)]
    probe: Option<crate::app::BackendProbe>,
    /// Why the backend cannot be used as configured, when it cannot (a refused `schema` mode, a
    /// script that cannot be read): the probe's findings are then absent. Shown only then.
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    last_probe: Option<crate::probe_results::LastProbe>,
}

/// Prints the status. `status` only reads (REQ-PERS-013): it opens the store
/// as a reader without the lock, so it answers while another runtime runs a
/// task, and it recovers nothing, reaps nothing and records nothing.
pub async fn run(app: &ReadApp, out: &Output) -> Result<(), AppError> {
    let config = &app.loaded.config;
    let policy = config.resolve_policy()?;
    // Every configured backend, probed at once: all the probes are started before any is waited
    // for, so the whole is bounded by one LIM-M4 however many backends there are (REQ-MDL-010).
    // One that cannot be made or probed is said on its own line and the others are shown: a
    // diagnosis does not stop at the first thing that is wrong (handoff 015a-5, item 4).
    let made = Backend::make_each(&app.loaded);
    let usable: Vec<Backend> = made
        .iter()
        .filter_map(|(_, made)| made.as_ref().ok().cloned())
        .collect();
    let mut probed = crate::backends::probe_all(&usable).await.into_iter();
    let last_probes = crate::probe_results::read(&app.data_dir);
    let several = made.len() > 1;
    let mut views = Vec::new();
    let mut backend_lines = Vec::new();
    let mut warnings = Vec::new();
    for (name, made) in &made {
        let last = |model: Option<&str>| messages::last_probe_text(&last_probes, name, model);
        // What the backend is: from the adapter, or from the configuration when it could not be
        // made.
        let (backend, outcome) = match made {
            Ok(backend) => (
                Some(backend),
                probed
                    .next()
                    .expect("one probe for each backend that was made")
                    .map_err(|e| e.to_string()),
            ),
            Err(error) => (None, Err(error.to_string())),
        };
        let (kind, model, locality) = match backend {
            Some(backend) => {
                let described = backend.adapter.describe();
                (described.kind, described.model, described.locality)
            }
            None => {
                let section = app.loaded.config.backend(name)?;
                let kind = serde_json::to_value(section.kind)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_owned))
                    .unwrap_or_default();
                (kind, section.model.clone(), section.locality)
            }
        };
        let locality_text = serde_json::to_value(locality)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default();
        let last_text = last(model.as_deref());
        let (probe, error) = match outcome {
            Ok((probe, notice)) => {
                if let Some(notice) = notice {
                    out.line(if several {
                        messages::for_backend(name, &notice)
                    } else {
                        notice
                    });
                }
                backend_lines.push(messages::status_backend_line(
                    name,
                    &kind,
                    model.as_deref(),
                    &locality_text,
                    Some(&probe),
                    &last_text,
                ));
                // The run goes on: a context below the need is a warning, never an error
                // (REQ-MDL-003).
                if let Some(warning) = probe.context_warning() {
                    warnings.push(if several {
                        messages::for_backend(name, &warning)
                    } else {
                        warning
                    });
                }
                (Some(probe), None)
            }
            // A backend that cannot be used as configured (`schema` against a server that refuses
            // it, a script that cannot be read): its own line says why, and the others go on.
            Err(reason) => {
                backend_lines.push(messages::status_backend_unusable_line(
                    name,
                    &kind,
                    model.as_deref(),
                    &locality_text,
                    &reason,
                    &last_text,
                ));
                (None, Some(reason))
            }
        };
        views.push(BackendView {
            name: name.clone(),
            kind,
            model,
            locality,
            probe,
            error,
            last_probe: last_probes.of(name).cloned(),
        });
    }
    // No backend named `default`: the line says so, as every command that needs one does.
    let no_default = config.default_backend().err();
    if let Some(error) = &no_default {
        out.line(error.reason.clone());
    }
    let default_view = views
        .iter()
        .find(|v| v.name == crate::backends::DEFAULT)
        .cloned();
    let (confinement, outside_sweep_unavailable) = probe_confinement(
        config,
        &app.data_dir,
        &app.interpreters,
        &app.host_tools,
        policy.ceiling.cpu_share,
    );
    let activity = app.reader.activity()?;
    let mut status = mimlys_core::runtime::RuntimeStatus {
        version: env!("CARGO_PKG_VERSION").to_owned(),
        // Shown through `backend` and `backends`; this field of the runtime's own status is
        // emptied (the object is flattened into this one).
        backend: mimlys_core::spi::BackendDescriptor {
            kind: String::new(),
            model: None,
            locality: mimlys_core::spi::Locality::Local,
        },
        tier: confinement.tier_with(&policy.accept_best_effort).describe(),
        clauses: confinement.clauses.clone(),
        unavailable_clauses: confinement.unavailable_lines(),
        cpu: confinement.cpu.clone(),
        unknown_to_this_release: activity.unknown_words(),
        running: activity.running,
        pending: activity.pending,
        unknown_state: activity.unknown_state,
        created: activity.created,
    };
    let store = app.store_view()?;
    for line in messages::status_head_lines(&status.version, &app.data_dir.to_string_lossy()) {
        out.line(line);
    }
    // Which mode is in force, and, when served, what `daemon status` says first.
    let (daemon, daemon_trouble) = daemon_mode(&app.loaded);
    // A task that waits in a daemon's line is created and will start: `daemon status` lists it
    // there, and it is not one that was left.
    if let Some(d) = &daemon {
        status
            .created
            .retain(|id| !d.queued.iter().any(|(queued, _)| queued == id));
    }
    match &daemon {
        Some(d) => {
            out.line(messages::mode_served(d.pid, &d.runtime_version, &d.socket));
            if let Some(first) = messages::daemon_status_lines(d, true).into_iter().next() {
                out.line(first);
            }
        }
        None => out.line(messages::MODE_EMBEDDED),
    }
    if let Some(trouble) = &daemon_trouble {
        out.line(messages::daemon_unreachable(trouble));
    }
    for line in backend_lines {
        out.line(line);
    }
    let (effort, effort_lines) = effort_views(config, &policy, &views);
    for line in effort_lines {
        out.line(line);
    }
    out.line(messages::status_tier_line(&status.tier));
    for warning in warnings {
        out.line(warning);
    }
    for line in messages::store_lines(&store) {
        out.line(line);
    }
    // The user's other runtimes, the slots and the engine gates, from the registry (RFC 013
    // decision 8). A registry that cannot be used is said here, as a task would meet it.
    let machine = crate::machine::view(
        crate::registry::default_dir(),
        config.runtime.admission_slots,
    );
    for line in messages::machine_lines(&machine) {
        out.line(line);
    }
    let backend_view = default_view;
    let data_dir_in_temp = app.loaded.config.data_dir_in_temp_dir();
    if data_dir_in_temp {
        out.line(messages::data_dir_in_temp_warning(
            &app.data_dir.to_string_lossy(),
        ));
    }
    let xcrun_missing = crate::app::warm_xcrun_cache().map(|f| f.to_string_lossy().into_owned());
    if let Some(file) = &xcrun_missing {
        out.line(messages::xcrun_cache_missing_warning(file));
    }
    out.line(messages::config_source(
        app.loaded
            .path
            .as_deref()
            .map(|p| p.to_string_lossy().into_owned())
            .as_deref(),
    ));
    out.line(messages::status_engine(
        mimlys_exec::sandbox::ENGINE_VERSION,
    ));
    for clause in mimlys_core::confine::Clause::ALL {
        let state = match status.clauses.get(&clause) {
            Some(state) => state.describe(),
            None => "unknown".to_owned(),
        };
        out.line(messages::status_clause(
            clause.letter(),
            clause.name(),
            &state,
        ));
    }
    // The CPU part of clause C, beside the clauses; never part of the tier (REQ-CONF-013).
    if let Some(cpu) = &status.cpu {
        out.line(messages::status_cpu_part(&cpu.describe()));
    }
    for i in &app.interpreters {
        let installed = i.module.is_file();
        out.line(messages::interpreter_line(
            &i.language,
            installed,
            &i.sha256[..12],
            mimlys_exec::interpreters::pinned(&i.language).map(|p| p.build),
        ));
    }
    for d in &app.host_tools {
        out.line(messages::host_tool_line(
            &d.name,
            &d.command,
            d.network,
            d.allow_paths,
            app.host_tools_unavailable.get(&d.name).map(String::as_str),
        ));
    }
    // A server runs in the data directory, and a reader creates none. Before the
    // first command that writes there is none to run in, and a server started
    // anywhere else (the command's current directory) would be a side effect of a
    // read command that nobody asked for: so none is started, and the declared
    // tools are listed as declared (REQ-PERS-013, handoff 013).
    let data_dir_exists = app.data_dir.is_dir();
    let configured = config.servers(&app.data_dir, &app.loaded.secrets);
    let servers_not_started = !data_dir_exists && !configured.is_empty();
    let mut servers = if configured.is_empty() {
        Vec::new()
    } else if !data_dir_exists {
        configured.iter().map(not_started).collect()
    } else {
        let servers_handle = std::sync::Arc::new(mimlys_mcp::tools::Servers::new(
            configured
                .into_iter()
                .map(mimlys_mcp::server::Server::new)
                .collect(),
        ));
        // Started to be listed, then ended; the events of their start and stop
        // belong to a store a reader does not write, so they are not recorded.
        let listed = servers_handle.status().await;
        servers_handle.shutdown().await;
        listed
    };
    // The model sees only the tools that can be functions of the transport; the user
    // is told which are left out, and why, here and not by the model (REQ-MDL-009).
    if matches!(
        backend_view
            .as_ref()
            .and_then(|v| v.probe.as_ref())
            .and_then(|p| p.decision_transport),
        Some(mimlys_core::native::DecisionTransport::ToolCalls)
    ) {
        mark_function_name_unavailable(&mut servers);
    }
    if servers_not_started {
        out.line(messages::servers_not_started());
    }
    for s in &servers {
        out.line(messages::server_line(s));
        for (tool, reason) in &s.unavailable {
            out.line(messages::server_unavailable(tool, reason));
        }
        for tool in &s.undeclared {
            out.line(messages::server_undeclared(tool));
        }
    }
    if let Some(reason) = &outside_sweep_unavailable {
        out.line(messages::outside_sweep_unavailable(reason));
    }
    let migration_pending = app.container.as_ref().is_some_and(|c| c.migration_pending);
    if migration_pending {
        out.line(messages::MIGRATION_PENDING);
    }
    let extra = app.dependency_step_read_set_extra();
    if !extra.is_empty() {
        out.line(messages::dependency_step_reads(&extra));
    }
    out.line(messages::status_running(status.running.len()));
    for (id, wait) in &status.pending {
        out.line(messages::status_pending(&id.to_string(), wait));
    }
    for (id, word) in &status.unknown_state {
        out.line(messages::status_unknown_state(&id.to_string(), word));
    }
    for id in &status.created {
        out.line(messages::status_created(&id.to_string()));
    }
    out.value(&Status {
        runtime: &status,
        backend: backend_view,
        backends: views,
        effort,
        mode: if daemon.is_some() { "served" } else { "embedded" },
        daemon: daemon.as_ref().map(|d| {
            serde_json::json!({
                "pid": d.pid,
                "version": d.runtime_version,
                "socket": d.socket,
                "started_at": d.started_at,
                "uptime_secs": d.uptime_secs,
            })
        }),
        data_dir: app.data_dir.to_string_lossy().into_owned(),
        data_dir_in_temporary_directory: data_dir_in_temp,
        xcrun_cache_missing: xcrun_missing,
        outside_sweep_unavailable,
        permissions_migration_pending: migration_pending,
        config: app
            .loaded
            .path
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned()),
        engine_version: mimlys_exec::sandbox::ENGINE_VERSION,
        servers,
        host_tools: app
            .host_tools
            .iter()
            .map(|d| HostToolView {
                name: d.name.clone(),
                command: d.command.clone(),
                network: d.network,
                allow_paths: d.allow_paths,
                unavailable: app.host_tools_unavailable.get(&d.name).cloned(),
            })
            .collect(),
        read_set: app.read_set(),
        dependency_step_read_set_extra: app.dependency_step_read_set_extra(),
        store,
        machine,
        interpreters: app
            .interpreters
            .iter()
            .map(|i| serde_json::json!({ "language": i.language, "sha256": i.sha256, "module": i.module.to_string_lossy(), "installed": i.module.is_file() }))
            .collect(),
        recovered: Vec::new(),
    });
    Ok(())
}

/// A server that was not started because the data directory does not exist yet:
/// its declared tools, as declared, and no state of its own.
fn not_started(config: &mimlys_mcp::server::ServerConfig) -> mimlys_mcp::ServerStatus {
    mimlys_mcp::ServerStatus {
        name: config.name.clone(),
        state: "not_started".to_owned(),
        detail: Some("no data directory yet".to_owned()),
        revision: None,
        server_name: None,
        server_version: None,
        instructions: None,
        tools: Vec::new(),
        unavailable: Vec::new(),
        undeclared: Vec::new(),
        declared: config
            .tools
            .keys()
            .map(|t| format!("{}.{t}", config.name))
            .collect(),
        list_changed: false,
    }
}

/// In the `tool_calls` transport, moves an MCP tool whose function name is
/// invalid, or equals another tool's, from a server's `tools` to its
/// `unavailable` with the `function_name_*` reason, by the rule that builds the
/// functions (`mimlys_core::native::name_reasons`). Host tools are not functions
/// of their own (the single `run_host_tool` names them in an argument), so a
/// function name never makes one unavailable.
fn mark_function_name_unavailable(servers: &mut [mimlys_mcp::ServerStatus]) {
    let blocked: std::collections::BTreeMap<String, &'static str> = {
        let names: Vec<&str> = servers
            .iter()
            .flat_map(|s| s.tools.iter().map(String::as_str))
            .collect();
        names
            .iter()
            .zip(mimlys_core::native::name_reasons(&names))
            .filter_map(|(name, reason)| reason.map(|r| ((*name).to_owned(), r)))
            .collect()
    };
    if blocked.is_empty() {
        return;
    }
    for server in servers {
        let (left_out, kept): (Vec<String>, Vec<String>) = std::mem::take(&mut server.tools)
            .into_iter()
            .partition(|tool| blocked.contains_key(tool));
        server.tools = kept;
        for tool in left_out {
            let reason = blocked[&tool].to_owned();
            server.unavailable.push((tool, reason));
        }
    }
}

/// The daemon of this store, and why it could not be reached when it could not.
fn daemon_mode(
    loaded: &crate::config::Loaded,
) -> (Option<crate::daemon::wire::DaemonStatus>, Option<String>) {
    crate::daemon::command::mode(loaded)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server(name: &str, tools: &[&str]) -> mimlys_mcp::ServerStatus {
        mimlys_mcp::ServerStatus {
            name: name.into(),
            state: "running".into(),
            detail: None,
            revision: None,
            server_name: None,
            server_version: None,
            instructions: None,
            tools: tools.iter().map(|t| (*t).to_owned()).collect(),
            unavailable: vec![("old.tool".into(), "definition_invalid".into())],
            undeclared: vec![],
            declared: vec![],
            list_changed: false,
        }
    }

    /// REQ-MDL-009: both tools of a collision are named (`a.b__c` and `a__b.c`
    /// are both `a__b__c` as functions), each with the reason; a tool that is
    /// fine stays available; the reasons the import gave are kept.
    #[test]
    fn req_mdl_009_a_collision_is_named_on_both_servers() {
        let mut servers = vec![
            server("a", &["a.b__c", "a.fine"]),
            server("a__b", &["a__b.c"]),
        ];
        mark_function_name_unavailable(&mut servers);
        assert_eq!(servers[0].tools, ["a.fine"]);
        assert_eq!(
            servers[0].unavailable,
            [
                ("old.tool".to_owned(), "definition_invalid".to_owned()),
                ("a.b__c".to_owned(), "function_name_collision".to_owned())
            ]
        );
        assert!(servers[1].tools.is_empty());
        assert_eq!(
            servers[1].unavailable[1],
            ("a__b.c".to_owned(), "function_name_collision".to_owned())
        );
        // Nothing to name: nothing changes.
        let mut fine = vec![server("a", &["a.b", "a.c"])];
        mark_function_name_unavailable(&mut fine);
        assert_eq!(fine[0].tools, ["a.b", "a.c"]);
        assert_eq!(fine[0].unavailable.len(), 1);
    }

    /// REQ-CTX-008 (handoff 009g, item 6): the lines `status` writes for a server
    /// show what the server said cleaned, whatever the status holds: the name,
    /// the version, the revision, a tool, a parameter named in a reason.
    #[test]
    fn req_ctx_008_the_lines_of_a_server_show_its_text_cleaned() {
        let dirty = "x\u{1b}[2J\u{1b}]0;title\u{7}y\u{202e}\nz";
        let mut status = server("a", &["a.fine"]);
        status.detail = Some(dirty.into());
        status.server_name = Some(dirty.into());
        status.server_version = Some(dirty.into());
        status.revision = Some(dirty.into());
        status.tools = vec![dirty.into()];
        status.unavailable = vec![(dirty.into(), dirty.into())];
        let lines = [
            messages::server_line(&status),
            messages::server_unavailable(&status.unavailable[0].0, &status.unavailable[0].1),
        ];
        for line in lines {
            assert!(!line.chars().any(char::is_control), "{line:?}");
            assert!(!line.contains('\u{202e}'), "{line:?}");
            assert!(line.contains("x[2J]0;titley z"), "{line:?}");
        }
    }
}
