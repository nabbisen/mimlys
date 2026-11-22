//! Metrics derived from events and records (REQ-OBS-003, REQ-OBS-008).

use crate::sqlite::{poisoned, storage};
use mimlys_core::event::{AdmissionState, Event, EventRecord};
use mimlys_core::ids::TaskId;
use mimlys_core::newer::{Read, UnknownWord};
use mimlys_core::rejection::RejectionCode;
use mimlys_core::spi::{ContextSums, Metrics, PersistenceError};
use mimlys_core::step::Step;
use mimlys_core::task::Task;
use rusqlite::params;

/// Counts over all tasks or one task.
pub fn metrics(
    conn: &std::sync::Mutex<rusqlite::Connection>,
    task_id: Option<TaskId>,
) -> Result<Metrics, PersistenceError> {
    let conn = conn.lock().map_err(|_| poisoned())?;
    let conn = &*conn;
    (|| -> Result<Metrics, PersistenceError> {
        let mut m = Metrics::default();
        // The names a newer release wrote, by the group that counts them (REQ-MAIN-006).
        let mut unknown: std::collections::BTreeSet<UnknownWord> = Default::default();
        let mut stmt = conn
            .prepare("SELECT state, record FROM tasks WHERE ?1 IS NULL OR task_id = ?1")
            .map_err(storage("prepare"))?;
        let rows = stmt
            .query_map(params![task_id.map(|t| t.to_string())], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })
            .map_err(storage("query"))?;
        for row in rows {
            let (state, record) = row.map_err(storage("row"))?;
            // A task whose state is not one of this release's is counted by the name it has.
            if let Ok(Read::Newer(newer)) = Read::<Task>::parse(&record)
                && newer.is_unknown("state")
            {
                unknown.insert(UnknownWord::new("tasks_by_state", state.clone()));
            }
            *m.tasks_by_state.entry(state).or_default() += 1;
        }
        let mut stmt = conn
            .prepare("SELECT record FROM events WHERE ?1 IS NULL OR task_id = ?1 ORDER BY position")
            .map_err(storage("prepare"))?;
        let rows = stmt
            .query_map(params![task_id.map(|t| t.to_string())], |r| {
                r.get::<_, String>(0)
            })
            .map_err(storage("query"))?;
        for row in rows {
            let text = row.map_err(storage("row"))?;
            let record = match Read::<EventRecord>::parse(&text) {
                Ok(Read::Known(record)) => record,
                // An event a newer release wrote: counted by its name, and in the groups its
                // words belong to (REQ-MAIN-006). Never skipped, never a failure.
                Ok(Read::Newer(newer)) => {
                    count_newer_event(&mut m, &mut unknown, &newer);
                    continue;
                }
                // A damaged record is skipped, as it always was.
                Err(_) => continue,
            };
            *m.events_by_name
                .entry(record.event.name().to_owned())
                .or_default() += 1;
            match &record.event {
                Event::DecisionRejected {
                    code,
                    reason,
                    grant_kind,
                } => {
                    *m.rejections_by_code
                        .entry(code.as_str().to_owned())
                        .or_default() += 1;
                    // REQ-OBS-008: denials by reason and by grant kind.
                    if *code == RejectionCode::Denied {
                        if let Some(reason) = reason {
                            *m.denials_by_reason.entry(reason.clone()).or_default() += 1;
                        }
                        if let Some(kind) = grant_kind {
                            *m.denials_by_grant_kind.entry(kind.clone()).or_default() += 1;
                        }
                    }
                }
                Event::TaskStateChanged {
                    to,
                    reason: Some(reason),
                    ..
                } if to == "halted" => {
                    let name = serde_json::to_value(reason)
                        .ok()
                        .and_then(|v| v.as_str().map(str::to_owned))
                        .unwrap_or_default();
                    *m.halts_by_reason.entry(name).or_default() += 1;
                }
                Event::ApprovalDecided { decision, .. } => {
                    let name = serde_json::to_value(decision)
                        .ok()
                        .and_then(|v| v.as_str().map(str::to_owned))
                        .unwrap_or_default();
                    *m.approvals_by_decision.entry(name).or_default() += 1;
                }
                Event::ActionFinished { status, .. } => {
                    *m.actions_by_status.entry(status.clone()).or_default() += 1
                }
                // REQ-OBS-009: the waits for a slot that ended in an admission.
                Event::TaskAdmissionChanged {
                    state: AdmissionState::Admitted,
                    waited_ms: Some(waited_ms),
                    ..
                } => {
                    m.admission_wait_ms += waited_ms;
                    m.admission_waits += 1;
                }
                _ => {}
            }
        }
        let mut stmt = conn
            .prepare("SELECT record FROM steps WHERE ?1 IS NULL OR task_id = ?1 ORDER BY idx")
            .map_err(storage("prepare"))?;
        let rows = stmt
            .query_map(params![task_id.map(|t| t.to_string())], |r| {
                r.get::<_, String>(0)
            })
            .map_err(storage("query"))?;
        let mut sums = ContextSums::default();
        for row in rows {
            let text = row.map_err(storage("row"))?;
            if let Ok(step) = serde_json::from_str::<Step>(&text) {
                sums.add(&step);
                // REQ-OBS-009: the requests that waited for the engine's gate.
                if let Some(waited) = step.measures.as_ref().and_then(|m| m.engine_wait_ms) {
                    m.engine_wait_ms += waited.max(0) as u64;
                    m.engine_waits += 1;
                }
            }
        }
        m.context = sums.finish();
        m.unknown_to_this_release = unknown.into_iter().collect();
        Ok(m)
    })()
}

/// Which map of the metrics counts a word, and the name of that group.
type Group = (
    fn(&mut Metrics) -> &mut std::collections::BTreeMap<String, u64>,
    &'static str,
);

/// Counts an event that holds a word this release does not know: by the name it was written
/// with, and in the group of each of its words that a group counts (`halts_by_reason`,
/// `rejections_by_code`, `approvals_by_decision`). The words that are not known are listed with
/// their group.
fn count_newer_event(
    m: &mut Metrics,
    unknown: &mut std::collections::BTreeSet<UnknownWord>,
    newer: &mimlys_core::newer::Newer,
) {
    let name = newer.text("event").unwrap_or("").to_owned();
    if newer.is_unknown("event") {
        unknown.insert(UnknownWord::new("events_by_name", name.clone()));
    }
    *m.events_by_name.entry(name.clone()).or_default() += 1;
    let (field, (map, group)): (&str, Group) = match name.as_str() {
        "task.state_changed" if newer.text("to") == Some("halted") => {
            ("reason", (|m| &mut m.halts_by_reason, "halts_by_reason"))
        }
        "decision.rejected" => (
            "code",
            (|m| &mut m.rejections_by_code, "rejections_by_code"),
        ),
        "approval.decided" => (
            "decision",
            (|m| &mut m.approvals_by_decision, "approvals_by_decision"),
        ),
        "action.finished" => (
            "status",
            (|m| &mut m.actions_by_status, "actions_by_status"),
        ),
        _ => return,
    };
    if let Some(word) = newer.text(field) {
        *map(m).entry(word.to_owned()).or_default() += 1;
        if newer.is_unknown(field) {
            unknown.insert(UnknownWord::new(group, word));
        }
    }
}
