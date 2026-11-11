//! `mimlys check` (REQ-CFG-001): validates the configuration and prints it
//! resolved, secrets as references.

use crate::app::AppError;
use crate::config::Loaded;
use crate::messages;
use crate::output::Output;

/// Prints the resolved configuration; the load already validated it.
pub fn run(loaded: &Loaded, out: &Output) -> Result<(), AppError> {
    out.line(messages::config_source(
        loaded
            .path
            .as_deref()
            .map(|p| p.to_string_lossy().into_owned())
            .as_deref(),
    ));
    out.line(messages::data_dir(
        &loaded.config.data_dir()?.to_string_lossy(),
    ));
    out.line(messages::CHECK_OK);
    let in_temp = loaded.config.data_dir_in_temp_dir();
    if in_temp {
        out.line(messages::data_dir_in_temp_warning(
            &loaded.config.data_dir()?.to_string_lossy(),
        ));
    }
    // The same as the runtime does before it starts a worker: make the cache
    // (a no-op elsewhere than macOS with an Xcode), and say if it cannot be made.
    let xcrun_missing = crate::app::warm_xcrun_cache().map(|f| f.to_string_lossy().into_owned());
    if let Some(file) = &xcrun_missing {
        out.line(messages::xcrun_cache_missing_warning(file));
    }
    for (tool, server) in loaded.config.declared_external_tools() {
        out.line(messages::ceiling_external(
            &tool,
            &format!(
                "declared by [servers.{server}.tools.{}]",
                tool.split_once('.').map_or(tool.as_str(), |(_, t)| t)
            ),
        ));
    }
    for tool in loaded.config.undeclared_external_tools() {
        out.line(messages::ceiling_undeclared_warning(&tool));
    }
    for (server, ms) in loaded.config.short_shutdown_servers() {
        out.line(messages::server_shutdown_short_warning(&server, ms));
    }
    for name in loaded.config.host_tools.keys() {
        out.line(messages::ceiling_host_tool(name));
    }
    out.line("");
    out.line(loaded.config.to_toml().trim_end());
    let mut value = serde_json::json!({
        "ok": true,
        "config_path": loaded.path.as_ref().map(|p| p.to_string_lossy().into_owned()),
        "data_dir": loaded.config.data_dir()?.to_string_lossy(),
        "data_dir_in_temporary_directory": in_temp,
        "config": loaded.config,
        "external_tools": loaded.config.declared_external_tools().into_iter().map(|(t, s)| serde_json::json!({"tool": t, "declared_by": s})).collect::<Vec<_>>(),
        "undeclared_external_tools": loaded.config.undeclared_external_tools(),
        "short_shutdown_servers": loaded.config.short_shutdown_servers().into_iter().map(|(s, ms)| serde_json::json!({"server": s, "shutdown_ms": ms})).collect::<Vec<_>>(),
        "host_tools": loaded.config.host_tools.keys().collect::<Vec<_>>(),
    });
    if let Some(file) = xcrun_missing {
        value["xcrun_cache_missing"] = serde_json::Value::String(file);
    }
    out.value(&value);
    Ok(())
}
