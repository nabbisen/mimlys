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
    // REQ-CONF-011: the same refusal as a declaration's `read_paths`; an
    // entry for the container covers the whole directory.
    if let Some(reason) = crate::toolchain::read_path_refusal(&dir) {
        return Err(AppError::Io { detail: reason });
    }
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

/// `toolchain revoke --all [--yes]` (REQ-CONF-008, handoff 011e): removes every
/// permission the container holds on mimlys's own directories and the
/// declared read paths (the record's entries and the ones a bounded scan
/// finds although the record lacks them), then the container profile, then
/// the record. Elsewhere nothing was changed outside the data directory.
pub fn revoke_all(
    app: &App,
    yes: bool,
    interaction: &mut dyn Interaction,
    out: &Output,
) -> Result<(), AppError> {
    let guard = app.container.lock().expect("container");
    let Some(container) = guard.as_ref() else {
        out.line(messages::TOOLCHAIN_REVOKE_ALL_NOTHING);
        out.value(&crate::toolchain::Outcome::default().to_json());
        return Ok(());
    };
    // The bounded scan: mimlys's own directories and the declared read paths.
    let mut candidates: Vec<(String, crate::toolchain::Source)> = Vec::new();
    let mut own: Vec<std::path::PathBuf> = vec![
        app.data_dir.join("workspaces"),
        app.data_dir.join("tmp"),
        app.data_dir.join("interpreters"),
    ];
    own.extend(
        app.interpreters
            .iter()
            .filter_map(|i| i.module.parent().map(Path::to_path_buf)),
    );
    if let Some(dir) = crate::app::worker_binary().parent() {
        own.push(dir.to_path_buf());
    }
    for p in own {
        candidates.push((
            crate::toolchain::normalized(&p),
            crate::toolchain::Source::Runtime,
        ));
    }
    for d in &app.host_tools {
        for p in &d.read_paths {
            candidates.push((
                crate::toolchain::normalized(Path::new(p)),
                crate::toolchain::Source::User,
            ));
        }
    }
    let plan = crate::toolchain::plan_revoke_all(&container.record, &candidates, |p| {
        crate::toolchain::applied(container, Path::new(p)) == Some(true)
    });
    let rows: Vec<(String, &str, &str)> = plan
        .iter()
        .map(|p| {
            let (source, found) = p.words();
            (p.path.clone(), source, found)
        })
        .collect();
    for line in messages::toolchain_revoke_all_plan(&container.profile, &rows) {
        out.line(line);
    }
    let confirmed = yes
        || (interaction.interactive()
            && !out.json()
            && interaction.confirm(&messages::toolchain_revoke_all_confirm(
                plan.len(),
                &container.profile,
            )));
    if !confirmed {
        if !interaction.interactive() || out.json() {
            return Err(AppError::Io {
                detail: messages::TOOLCHAIN_REVOKE_ALL_NEEDS_YES.to_owned(),
            });
        }
        out.line(messages::TOOLCHAIN_NOT_CONFIRMED);
        out.value(&crate::toolchain::Outcome::default().to_json());
        return Ok(());
    }
    let outcome = crate::toolchain::revoke_all(container, &app.data_dir, &plan);
    for p in &outcome.removed {
        out.line(messages::toolchain_revoke_all_removed(&p.path));
    }
    for (p, reason) in &outcome.failed {
        out.line(messages::toolchain_revoke_all_failed(&p.path, reason));
    }
    out.line(messages::toolchain_revoke_all_profile(
        outcome.profile_deleted,
        outcome.record_deleted,
    ));
    if outcome.complete() {
        out.value(&outcome.to_json());
        Ok(())
    } else {
        Err(AppError::Partial {
            message: format!(
                "{} permission(s) could not be removed; run `mimlys toolchain revoke --all` again to retry exactly those",
                outcome.failed.len()
            ),
            report: outcome.to_json(),
        })
    }
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
