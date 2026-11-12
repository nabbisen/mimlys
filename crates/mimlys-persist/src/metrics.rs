//! Metrics derived from events and records (REQ-OBS-003, REQ-OBS-008).

use crate::sqlite::{poisoned, storage};
use mimlys_core::event::{Event, EventRecord};
use mimlys_core::ids::TaskId;
use mimlys_core::rejection::RejectionCode;
use mimlys_core::spi::{ContextSums, Metrics, PersistenceError};
use mimlys_core::step::Step;
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
        let mut stmt = conn
            .prepare(
                "SELECT state, count(*) FROM tasks WHERE ?1 IS NULL OR task_id = ?1 GROUP BY state",
            )
            .map_err(storage("prepare"))?;
        let rows = stmt
            .query_map(params![task_id.map(|t| t.to_string())], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
            })
            .map_err(storage("query"))?;
        for row in rows {
            let (state, n) = row.map_err(storage("row"))?;
            m.tasks_by_state.insert(state, n as u64);
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
            let Ok(record) = serde_json::from_str::<EventRecord>(&text) else {
                continue;
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
            }
        }
        m.context = sums.finish();
        Ok(m)
    })()
}
