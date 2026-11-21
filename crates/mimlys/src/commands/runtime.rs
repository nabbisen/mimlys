//! `mimlys runtime reap` (RFC 013 decision 5; external design §19.2, §19.3; THR-38): removes by
//! hand the registry's stale entries (a runtime that ended without returning its slot) and the
//! ones it cannot use, and lists what it removed. Every start that takes a slot does the same;
//! this is the way to see it, or to do it when no runtime is about to start. It opens no store.

use crate::app::AppError;
use crate::messages;
use crate::output::Output;
use crate::registry::{Bounds, Registry};
use serde_json::json;
use std::sync::Arc;

/// `runtime reap`.
pub fn reap(out: &Output) -> Result<(), AppError> {
    let dir = crate::registry::default_dir().map_err(|reason| AppError::Io {
        detail: format!("the registry of the user's runtimes cannot be found: {reason}"),
    })?;
    let registry = Registry::new(
        dir.clone(),
        Bounds::default(),
        Arc::new(mimlys_core::clock::SystemClock),
    );
    let reaped = registry.reap().map_err(|reason| AppError::Io {
        detail: format!("the registry of the user's runtimes cannot be used: {reason}"),
    })?;
    if reaped.is_empty() {
        out.line(messages::NOTHING_TO_REAP);
    }
    for entry in &reaped {
        out.line(messages::reaped_line(entry));
    }
    out.value(&json!({
        "registry": dir.to_string_lossy(),
        "reaped": reaped.iter().map(|r| json!({
            "entry": r.stem,
            "kind": r.entry.as_ref().map(|e| e.kind.name()),
            "pid": r.entry.as_ref().map(|e| e.pid),
            "data_dir": r.entry.as_ref().map(|e| e.data_dir.clone()),
            "since": r.entry.as_ref().map(|e| e.since),
            "reason": r.reason,
        })).collect::<Vec<_>>(),
    }));
    Ok(())
}
