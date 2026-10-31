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
            out.line(messages::downloading(p.url));
            download(p.url)?
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

fn download(url: &str) -> Result<Vec<u8>, AppError> {
    let limit = mimlys_core::limits::LIM_S5_MODULE_BYTES;
    let response = ureq::get(url).call().map_err(|e| AppError::Io {
        detail: format!("download failed: {e}"),
    })?;
    let mut body = response.into_body();
    let mut bytes = Vec::new();
    use std::io::Read;
    body.as_reader()
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| AppError::Io {
            detail: format!("download failed: {}", e.kind()),
        })?;
    if bytes.len() as u64 > limit {
        return Err(AppError::Io {
            detail: "the artifact is larger than LIM-S5".into(),
        });
    }
    Ok(bytes)
}
