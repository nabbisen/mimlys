//! `mimlys interpreters list|install` (RFC 005 §6; REQ-SBX-001, REQ-SBX-007).

use crate::app::{App, AppError};
use crate::messages;
use crate::output::Output;
use mimlys_exec::interpreters::{install, pinned, sha256_hex};
use serde::Serialize;
use std::path::Path;

#[derive(Serialize)]
struct Row {
    language: String,
    sha256: String,
    module: String,
    installed: bool,
    build: Option<&'static str>,
}

fn rows(app: &App) -> Vec<Row> {
    app.interpreters
        .iter()
        .map(|i| Row {
            language: i.language.clone(),
            sha256: i.sha256.clone(),
            module: i.module.to_string_lossy().into_owned(),
            installed: i.module.is_file()
                && std::fs::read(&i.module)
                    .map(|b| sha256_hex(&b) == i.sha256)
                    .unwrap_or(false),
            build: pinned(&i.language).map(|p| p.build),
        })
        .collect()
}

/// `interpreters list`.
pub fn list(app: &App, out: &Output) -> Result<(), AppError> {
    let rows = rows(app);
    for r in &rows {
        out.line(messages::interpreter_line(
            &r.language,
            r.installed,
            &r.sha256[..12],
            r.build,
        ));
    }
    out.value(&rows);
    Ok(())
}

/// `interpreters install <language> [--from FILE]`: verifies the hash and
/// stores the module under the data directory. Without `--from`, the
/// pinned artifact is downloaded by this process (never by the worker).
pub fn install_cmd(
    app: &App,
    language: &str,
    from: Option<&Path>,
    out: &Output,
) -> Result<(), AppError> {
    let configured = app
        .interpreters
        .iter()
        .find(|i| i.language == language)
        .ok_or_else(|| AppError::Io {
            detail: messages::unknown_language(
                language,
                &app.interpreters
                    .iter()
                    .map(|i| i.language.as_str())
                    .collect::<Vec<_>>(),
            ),
        })?;
    let bytes = match from {
        Some(path) => std::fs::read(path).map_err(|e| AppError::Io {
            detail: format!("cannot read {}: {}", path.display(), e.kind()),
        })?,
        None => {
            let p = pinned(language).ok_or_else(|| AppError::Io {
                detail: messages::no_artifact(language),
            })?;
            // Debug builds only (the download tests): another address for the
            // pinned artifact. A release binary has no such variable.
            #[cfg(debug_assertions)]
            let url =
                std::env::var("MIMLYS_DEBUG_INTERPRETER_URL").unwrap_or_else(|_| p.url.into());
            #[cfg(not(debug_assertions))]
            let url = p.url.to_owned();
            out.line(messages::downloading(&url));
            download(language, &url)?
        }
    };
    let path =
        install(&app.data_dir, language, &configured.sha256, &bytes).map_err(|e| AppError::Io {
            detail: e.to_string(),
        })?;
    out.line(messages::interpreter_installed(
        language,
        &path.to_string_lossy(),
    ));
    out.value(&serde_json::json!({ "language": language, "module": path.to_string_lossy(), "sha256": configured.sha256 }));
    Ok(())
}

/// The pinned artifact, under the download policy (LIM-O9, LIM-O10):
/// transient failures are retried with one `info` line each; a failure that
/// remains is the typed `unavailable` / `download` error (external design §12).
fn download(language: &str, url: &str) -> Result<Vec<u8>, AppError> {
    crate::download::fetch(
        url,
        language,
        &crate::download::Policy::shipped(),
        crate::download::http_get,
        std::thread::sleep,
        |line| tracing::info!("{line}"),
    )
    .map_err(AppError::Download)
}
