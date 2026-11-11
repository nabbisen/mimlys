//! `mimlys status` (REQ-OBS-006).

use crate::app::{AppError, ReadApp, StoreView, make_backend, probe_backend_of, probe_confinement};
use crate::messages;
use crate::output::Output;
use serde::Serialize;

/// The status object: the runtime's status plus the process's view.
#[derive(Serialize)]
struct Status<'a> {
    #[serde(flatten)]
    runtime: &'a mimlys_core::runtime::RuntimeStatus,
    /// The backend: kind, model, locality, and the probe's findings.
    backend: BackendView,
    data_dir: String,
    /// The configured data directory is inside the temporary directory (REQ-WS-001).
    data_dir_in_temporary_directory: bool,
    /// macOS: `xcrun`'s cache file, when it is missing and could not be made.
    #[serde(skip_serializing_if = "Option::is_none")]
    xcrun_cache_missing: Option<String>,
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
}

/// One declared host tool: its command, whether it is the dependency step
/// (`network`), and the reason it cannot run on this machine, if any
/// (`acl_not_applied: <path>` until `toolchain grant`; RFC 011 §7).
#[derive(Serialize)]
struct HostToolView {
    name: String,
    command: String,
    network: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    unavailable: Option<String>,
}

/// `backend` in the status: the descriptor and the probe (RFC 008 §4).
#[derive(Serialize)]
struct BackendView {
    kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    locality: mimlys_core::spi::Locality,
    #[serde(flatten)]
    probe: Option<crate::app::BackendProbe>,
}

/// Prints the status. `status` only reads (REQ-PERS-013): it opens the store
/// as a reader without the lock, so it answers while another runtime runs a
/// task, and it recovers nothing, reaps nothing and records nothing.
pub async fn run(app: &ReadApp, out: &Output) -> Result<(), AppError> {
    let config = &app.loaded.config;
    let policy = config.resolve_policy()?;
    let (backend_adapter, openai) = make_backend(config, &app.loaded, None)?;
    let (backend_probe, notice) = probe_backend_of(&app.loaded, openai.as_ref()).await?;
    if let Some(notice) = notice {
        out.line(notice);
    }
    let confinement = probe_confinement(config, &app.data_dir, &app.interpreters, &app.host_tools);
    let activity = app.reader.activity()?;
    let mut status = mimlys_core::runtime::RuntimeStatus {
        version: env!("CARGO_PKG_VERSION").to_owned(),
        backend: backend_adapter.describe(),
        tier: confinement.tier_with(&policy.accept_best_effort).describe(),
        clauses: confinement.clauses.clone(),
        unavailable_clauses: confinement.unavailable_lines(),
        running: activity.running,
        pending: activity.pending,
    };
    let probe = Some(backend_probe);
    let store = app.store_view()?;
    let locality = serde_json::to_value(status.backend.locality)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default();
    for line in messages::status_lines(
        &status.version,
        &app.data_dir.to_string_lossy(),
        &status.backend.kind,
        status.backend.model.as_deref(),
        &locality,
        probe.as_ref(),
        &status.tier,
    ) {
        out.line(line);
    }
    for line in messages::store_lines(&store) {
        out.line(line);
    }
    let backend = BackendView {
        kind: std::mem::take(&mut status.backend.kind),
        model: status.backend.model.take(),
        locality: status.backend.locality,
        probe,
    };
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
        backend.probe.as_ref().and_then(|p| p.decision_transport),
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
    let extra = app.dependency_step_read_set_extra();
    if !extra.is_empty() {
        out.line(messages::dependency_step_reads(&extra));
    }
    out.line(messages::status_running(status.running.len()));
    for (id, kind) in &status.pending {
        out.line(messages::status_pending(&id.to_string(), *kind));
    }
    out.value(&Status {
        runtime: &status,
        backend,
        data_dir: app.data_dir.to_string_lossy().into_owned(),
        data_dir_in_temporary_directory: data_dir_in_temp,
        xcrun_cache_missing: xcrun_missing,
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
                unavailable: app.host_tools_unavailable.get(&d.name).cloned(),
            })
            .collect(),
        read_set: app.read_set(),
        dependency_step_read_set_extra: app.dependency_step_read_set_extra(),
        store,
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
}
