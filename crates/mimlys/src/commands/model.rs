//! `mimlys model probe [<backend>]` (REQ-MDL-011, THR-40; RFC 015 decision 5 and Amendment 1):
//! the fitness probe of a model, per backend. What it makes, how it judges and what it deletes
//! are in [`crate::probe`]; this is the command around it: which backends, what is said, what is
//! recorded.

use crate::app::{App, AppError};
use crate::backends::Backend;
use crate::messages;
use crate::output::Output;
use crate::probe::drive::Stop;
use crate::probe::run::{Ended, probe_backend};
use crate::probe::{self, LeftOut};
use crate::probe_results::{self, LastProbe};
use mimlys_core::clock::{Clock, SystemClock};
use std::path::Path;
use std::sync::Arc;

/// The refusal of a probe while another process holds the store, naming the holder (LIM-O3). The
/// holder is read without the lock, as `status` reads it.
pub fn store_in_use(data_dir: &Path) -> AppError {
    let holder = mimlys_persist::open_read_only(data_dir)
        .ok()
        .and_then(|store| match store.probe_lock() {
            mimlys_persist::LockState::Held(holder) => holder,
            _ => None,
        });
    AppError::StoreInUse {
        detail: messages::probe_store_in_use(holder.as_ref().map(|h| (h.pid, h.since.as_str()))),
        pid: holder.map(|h| h.pid),
    }
}

/// What a probe command found, for `--json`.
#[derive(Default, serde::Serialize)]
struct Report {
    probed: Vec<serde_json::Value>,
    left_out: Vec<LeftOut>,
    not_measured: Vec<serde_json::Value>,
    stopped: Option<serde_json::Value>,
}

/// `model probe [<backend>]`: the backends of the plan, one after the other.
pub async fn probe(app: &App, named: Option<&str>, out: &Output) -> Result<(), AppError> {
    let plan = probe::plan(app.config(), named)?;
    // A record this release cannot read is not replaced (the probe would run, and its result
    // could not be written): said before anything is made or sent.
    probe_results::read_for_update(&app.data_dir).map_err(|e| AppError::Io {
        detail: e.to_string(),
    })?;
    let mut report = Report {
        left_out: plan.left_out.clone(),
        ..Report::default()
    };
    if let Some(remote) = &plan.remote_named {
        out.line(messages::probe_remote_notice(
            &remote.backend,
            remote.host.as_deref(),
        ));
    }
    for line in messages::probe_left_out(&plan.left_out) {
        out.line(line);
    }
    if plan.probe.is_empty() {
        out.line(messages::probe_no_local_backend());
        out.value(&report);
        return Ok(());
    }
    // Every backend is made before the first is probed: a name that cannot be made stops the
    // command before anything is made or sent.
    let mut backends = Vec::new();
    for name in &plan.probe {
        let backend = Backend::make(&app.loaded, name, None)?;
        backend.ready()?;
        backends.push(backend);
    }
    let stop = Stop::new();
    stop.listen();
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let mut left_paths: Vec<String> = Vec::new();
    for (position, backend) in backends.iter().enumerate() {
        let described = backend.adapter.describe();
        out.line(messages::probe_start(
            &backend.name,
            &described.kind,
            described.model.as_deref(),
            if backend.section.locality == mimlys_core::spi::Locality::Remote {
                "remote"
            } else {
                "local"
            },
        ));
        let remote_named = plan
            .remote_named
            .as_ref()
            .is_some_and(|r| r.backend == backend.name);
        let ended = probe_backend(app, backend, remote_named, &stop, Arc::clone(&clock)).await?;
        match ended {
            Ended::Measured { last, left } => {
                left_paths.extend(left);
                if stop.requested() {
                    // A stop that came after the measuring, while the probe was being deleted:
                    // the probe ends as stopped, and records nothing.
                    stopped(&backend.name, &backends[position + 1..], out, &mut report);
                    break;
                }
                probe_results::record(&app.data_dir, &backend.name, last.clone()).map_err(|e| {
                    AppError::Io {
                        detail: e.to_string(),
                    }
                })?;
                out.line(messages::probe_result(&backend.name, &last));
                let meaning = messages::probe_class_meaning(&last.class, last.detail.as_deref());
                if !meaning.is_empty() {
                    out.line(format!("  {meaning}"));
                }
                report.probed.push(entry(&backend.name, &last));
            }
            Ended::NotMeasured { reason, left } => {
                left_paths.extend(left);
                out.line(reason.clone());
                report
                    .not_measured
                    .push(serde_json::json!({ "backend": backend.name, "reason": reason }));
            }
            Ended::Stopped { left } => {
                left_paths.extend(left);
                stopped(&backend.name, &backends[position + 1..], out, &mut report);
                break;
            }
        }
    }
    if !left_paths.is_empty() {
        eprintln!("{}", messages::probe_left_paths(&left_paths));
    }
    out.value(&report);
    if report.not_measured.is_empty() {
        return Ok(());
    }
    // Some backend was not measured: the command did not do what it was asked, and says so by
    // its exit code; what was measured stays recorded.
    let names: Vec<String> = report
        .not_measured
        .iter()
        .filter_map(|v| v["backend"].as_str().map(str::to_owned))
        .collect();
    Err(AppError::Partial {
        message: format!("not measured: {}", names.join(", ")),
        report: serde_json::to_value(&report).unwrap_or_default(),
    })
}

/// The stop: said on the terminal, and in the report.
fn stopped(name: &str, rest: &[Backend], out: &Output, report: &mut Report) {
    let not_probed: Vec<String> = rest.iter().map(|b| b.name.clone()).collect();
    eprintln!("{}", messages::probe_stopped(name, &not_probed));
    let _ = out;
    report.stopped = Some(serde_json::json!({ "backend": name, "not_probed": not_probed }));
}

/// A backend's entry in `--json`: the record's own fields, with the name.
fn entry(name: &str, last: &LastProbe) -> serde_json::Value {
    let mut value = serde_json::to_value(last).unwrap_or_default();
    value["backend"] = serde_json::Value::String(name.to_owned());
    value
}
