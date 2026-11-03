//! `mimlys toolchain list|grant|revoke` (REQ-CONF-008; RFC 011 §7): the
//! read set the worker runs under, declared tool by declared tool, and on
//! Windows the reversible directory permissions the AppContainer needs.
//! On Linux and macOS the kernel takes the paths from the specification, so
//! `grant` and `revoke` have nothing to apply and say so.

use crate::app::{App, AppError};
use crate::messages;
use crate::output::Output;
use crate::prompt::Interaction;
use std::path::Path;

/// Prints the read set and each declared tool's paths; on Windows, whether
/// each path carries the container's entry, read from the directory.
pub fn list(app: &App, out: &Output) -> Result<(), AppError> {
    let read_set = app.read_set();
    let container = app.container.lock().expect("container").clone();
    if let Some(c) = &container {
        out.line(messages::container_line(&c.profile, &c.sid_text));
    }
    for p in &read_set {
        out.line(messages::read_set_line(p, app.path_applied(p)));
    }
    for d in &app.host_tools {
        let unavailable = app.host_tools_unavailable.get(&d.name).map(String::as_str);
        out.line(messages::host_tool_line(
            &d.name,
            &d.command,
            d.network,
            unavailable,
        ));
        for p in &d.read_paths {
            out.line(messages::declared_path_line(p, app.path_applied(p)));
        }
    }
    out.value(&serde_json::json!({
        "read_set": read_set.iter().map(|p| serde_json::json!({
            "path": p, "applied": app.path_applied(p),
        })).collect::<Vec<_>>(),
        "host_tools": app.host_tools.iter().map(|d| serde_json::json!({
            "name": d.name, "command": d.command, "read_paths": d.read_paths, "network": d.network,
            "unavailable": app.host_tools_unavailable.get(&d.name),
        })).collect::<Vec<_>>(),
        "container": container.as_ref().map(|c| serde_json::json!({
            "profile": c.profile, "sid": c.sid_text,
            "entries": c.record.entries,
        })),
    }));
    Ok(())
}

/// Grants the container read access to a toolchain directory (Windows).
pub fn grant(
    app: &App,
    path: &str,
    yes: bool,
    interaction: &mut dyn Interaction,
    out: &Output,
) -> Result<(), AppError> {
    let mut guard = app.container.lock().expect("container");
    let Some(container) = guard.as_mut() else {
        out.line(messages::TOOLCHAIN_NOTHING_TO_APPLY);
        out.value(&serde_json::json!({"applied": false, "reason": "not_needed"}));
        return Ok(());
    };
    let dir = existing_dir(path)?;
    if !yes && !interaction.confirm(&messages::toolchain_grant_confirm(&dir, &container.profile)) {
        out.line(messages::TOOLCHAIN_NOT_CONFIRMED);
        out.value(&serde_json::json!({"applied": false, "reason": "not_confirmed"}));
        return Ok(());
    }
    crate::toolchain::grant(container, &app.data_dir, Path::new(&dir))?;
    out.line(messages::toolchain_granted(&dir));
    out.value(&serde_json::json!({"applied": true, "path": dir}));
    Ok(())
}

/// Removes the container's entries from a directory (Windows); mimlys's own
/// directories are refused, since the worker cannot run without them.
pub fn revoke(
    app: &App,
    path: &str,
    yes: bool,
    interaction: &mut dyn Interaction,
    out: &Output,
) -> Result<(), AppError> {
    let mut guard = app.container.lock().expect("container");
    let Some(container) = guard.as_mut() else {
        out.line(messages::TOOLCHAIN_NOTHING_TO_APPLY);
        out.value(&serde_json::json!({"applied": false, "reason": "not_needed"}));
        return Ok(());
    };
    let dir = existing_dir(path)?;
    if container.record.entries.iter().any(|e| {
        e.source == crate::toolchain::Source::Runtime && crate::toolchain::same_path(&e.path, &dir)
    }) {
        return Err(AppError::Io {
            detail: messages::toolchain_revoke_runtime_path(&dir),
        });
    }
    if !yes && !interaction.confirm(&messages::toolchain_revoke_confirm(&dir)) {
        out.line(messages::TOOLCHAIN_NOT_CONFIRMED);
        out.value(&serde_json::json!({"applied": false, "reason": "not_confirmed"}));
        return Ok(());
    }
    crate::toolchain::revoke(container, &app.data_dir, Path::new(&dir))?;
    out.line(messages::toolchain_revoked(&dir));
    out.value(&serde_json::json!({"applied": true, "path": dir}));
    Ok(())
}

/// The directory as the record names it: canonical, without the verbatim prefix.
fn existing_dir(path: &str) -> Result<String, AppError> {
    let expanded = crate::config::expand_home(path);
    let dir = std::fs::canonicalize(&expanded).map_err(|_| AppError::Io {
        detail: format!("{path} is not an existing directory"),
    })?;
    if !dir.is_dir() {
        return Err(AppError::Io {
            detail: format!("{path} is not a directory"),
        });
    }
    Ok(crate::toolchain::normalized(&dir))
}
