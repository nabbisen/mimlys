//! The `Persistence` implementation (external design §16.2; REQ-PERS-003,
//! REQ-PERS-004, REQ-PERS-005): every method is one immediate transaction
//! that writes the record, enforces the write ordering against the stored
//! status, appends the event, and returns after the commit. Blobs are
//! written and fsynced before their rows.

use crate::crash;
use crate::sqlite::{Store, io, poisoned, restrict_file, storage};
use mimlys_core::action::{Action, ActionStatus, Execution};
use mimlys_core::approval::ApprovalRequest;
use mimlys_core::event::EventRecord;
use mimlys_core::grant::Grant;
use mimlys_core::ids::{ActionId, BlobId, SnapshotId, TaskId};
use mimlys_core::observation::Observation;
use mimlys_core::question::Question;
use mimlys_core::rejection::Detail;
use mimlys_core::snapshot::{Promotion, SnapshotRecord, WorkspaceState};
use mimlys_core::spi::DeletedRows;
use mimlys_core::spi::{
    Persistence, PersistenceError, PersistenceRead, PositionedEvent, TaskSummary,
};
use mimlys_core::step::Step;
use mimlys_core::task::{PlanVersion, Task};
use rusqlite::{Transaction, TransactionBehavior, params};
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::io::Write;

/// The version of every record type written by this runtime (REQ-MAIN-002).
pub const RECORD_VERSION: u32 = 1;

/// LIM-S4: bound of a stored blob, in bytes (1 MiB).
pub const LIM_S4_BLOB_BYTES: usize = 1024 * 1024;

fn json<T: Serialize>(value: &T) -> Result<String, PersistenceError> {
    serde_json::to_string(value).map_err(|_| PersistenceError::Storage {
        detail: "serialize".into(),
    })
}

fn parse<T: DeserializeOwned>(text: String) -> Result<T, PersistenceError> {
    serde_json::from_str(&text).map_err(|_| PersistenceError::Storage {
        detail: "deserialize".into(),
    })
}

impl Store {
    /// Runs `body` in one immediate transaction with the crash point around the commit.
    fn transaction<R>(
        &self,
        point: &'static str,
        body: impl FnOnce(&Transaction<'_>) -> Result<R, PersistenceError>,
    ) -> Result<R, PersistenceError> {
        let mut conn = self.conn.lock().map_err(|_| poisoned())?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage("begin"))?;
        let result = body(&tx)?;
        crash::maybe(point, "before");
        tx.commit().map_err(storage("commit"))?;
        #[cfg(test)]
        self.transactions
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        crash::maybe(point, "after");
        Ok(result)
    }
}

fn append_event(tx: &Transaction<'_>, event: &EventRecord) -> Result<(), PersistenceError> {
    tx.execute(
        "INSERT INTO events (event_id, at, name, task_id, step_id, action_id, record) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            event.event_id.to_string(),
            event.at.to_rfc3339(),
            event.event.name(),
            event.task_id.map(|t| t.to_string()),
            event.step_id.map(|s| s.to_string()),
            event.action_id.map(|a| a.to_string()),
            json(event)?,
        ],
    )
    .map_err(storage("insert_event"))?;
    Ok(())
}

fn upsert_task(tx: &Transaction<'_>, task: &Task) -> Result<(), PersistenceError> {
    tx.execute(
        "INSERT INTO tasks (task_id, state, goal, created_at, updated_at, policy_hash, decision_schema_version, runtime_version, record, record_version)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT (task_id) DO UPDATE SET state = excluded.state, updated_at = excluded.updated_at, record = excluded.record",
        params![
            task.task_id.to_string(),
            task.state.name(),
            task.goal,
            task.created_at.to_rfc3339(),
            task.updated_at.to_rfc3339(),
            task.versions.policy_hash.as_str(),
            task.versions.decision_schema_version,
            task.versions.runtime_version,
            json(task)?,
            RECORD_VERSION,
        ],
    )
    .map_err(storage("upsert_task"))?;
    Ok(())
}

fn upsert_step(tx: &Transaction<'_>, step: &Step) -> Result<(), PersistenceError> {
    tx.execute(
        "INSERT INTO steps (step_id, task_id, idx, outcome, record, record_version, rejection_detail) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT (step_id) DO UPDATE SET outcome = excluded.outcome, record = excluded.record,
             rejection_detail = COALESCE(excluded.rejection_detail, steps.rejection_detail)",
        params![
            step.step_id.to_string(),
            step.task_id.to_string(),
            step.index,
            step.outcome.and_then(|o| serde_json::to_value(o).ok().and_then(|v| v.as_str().map(str::to_owned))),
            json(step)?,
            RECORD_VERSION,
            step.rejection_detail.as_ref().map(json).transpose()?,
        ],
    )
    .map_err(storage("upsert_step"))?;
    Ok(())
}

fn upsert_action(tx: &Transaction<'_>, action: &Action) -> Result<(), PersistenceError> {
    tx.execute(
        "INSERT INTO actions (action_id, task_id, step_id, status, approval_required, action_hash, created_at, record, record_version)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT (action_id) DO UPDATE SET status = excluded.status, record = excluded.record",
        params![
            action.action_id.to_string(),
            action.task_id.to_string(),
            action.step_id.to_string(),
            action.status.name(),
            action.content().approval_required() as i64,
            action.content().action_hash().as_str(),
            action.created_at.to_rfc3339(),
            json(action)?,
            RECORD_VERSION,
        ],
    )
    .map_err(storage("upsert_action"))?;
    Ok(())
}

/// Enforces the write ordering: the stored status must be one of `allowed` (REQ-PERS-004).
fn expect_status(
    tx: &Transaction<'_>,
    action_id: ActionId,
    allowed: &[&str],
    line: &str,
) -> Result<(), PersistenceError> {
    let stored: Option<String> = tx
        .query_row(
            "SELECT status FROM actions WHERE action_id = ?1",
            [action_id.to_string()],
            |r| r.get(0),
        )
        .ok();
    match stored {
        Some(s) if allowed.contains(&s.as_str()) => Ok(()),
        Some(s) => Err(PersistenceError::Ordering {
            detail: format!("{line} requires status in {allowed:?}, found {s}"),
        }),
        None => Err(PersistenceError::Ordering {
            detail: format!("{line} requires a recorded action"),
        }),
    }
}

fn upsert_approval(
    tx: &Transaction<'_>,
    approval: &ApprovalRequest,
) -> Result<(), PersistenceError> {
    tx.execute(
        "INSERT INTO approvals (approval_id, task_id, action_id, status, action_hash, grant_set_hash, record, record_version)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT (approval_id) DO UPDATE SET status = excluded.status, record = excluded.record",
        params![
            approval.approval_id.to_string(),
            approval.task_id.to_string(),
            approval.action_id.to_string(),
            status_name(&approval.status)?,
            approval.action_hash.as_str(),
            approval.grant_set_hash.as_str(),
            json(approval)?,
            RECORD_VERSION,
        ],
    )
    .map_err(storage("upsert_approval"))?;
    Ok(())
}

fn status_name<T: Serialize>(value: &T) -> Result<String, PersistenceError> {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .ok_or(PersistenceError::Storage {
            detail: "status_name".into(),
        })
}

/// The reading half of the store, written once for both handles: the
/// writer's [`Store`] and the read-only handle of `reader` (REQ-PERS-013).
/// Each needs `conn` and `data_dir` fields.
macro_rules! persistence_read {
    ($t:ty) => {
        impl $t {
            fn query<T: DeserializeOwned>(
                &self,
                sql: &str,
                params: &[&dyn rusqlite::ToSql],
            ) -> Result<Vec<T>, PersistenceError> {
                let conn = self.conn.lock().map_err(|_| poisoned())?;
                let mut stmt = conn.prepare(sql).map_err(storage("prepare"))?;
                let rows = stmt
                    .query_map(params, |r| r.get::<_, String>(0))
                    .map_err(storage("query"))?;
                let mut out = Vec::new();
                for row in rows {
                    out.push(parse(row.map_err(storage("row"))?)?);
                }
                Ok(out)
            }

            fn query_one<T: DeserializeOwned>(
                &self,
                sql: &str,
                params: &[&dyn rusqlite::ToSql],
            ) -> Result<Option<T>, PersistenceError> {
                Ok(self.query::<T>(sql, params)?.into_iter().next())
            }
        }

    impl PersistenceRead for $t {
        fn load_task(&self, task_id: TaskId) -> Result<Option<Task>, PersistenceError> {
            self.query_one(
                "SELECT record FROM tasks WHERE task_id = ?1",
                &[&task_id.to_string()],
            )
        }

        fn list_tasks(&self) -> Result<Vec<TaskSummary>, PersistenceError> {
            let conn = self.conn.lock().map_err(|_| poisoned())?;
            let mut stmt = conn
                .prepare("SELECT task_id, goal, state, created_at, updated_at FROM tasks ORDER BY created_at DESC")
                .map_err(storage("prepare"))?;
            let rows = stmt
                .query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, String>(4)?,
                    ))
                })
                .map_err(storage("query"))?;
            let mut out = Vec::new();
            for row in rows {
                let (id, goal, state, created, updated) = row.map_err(storage("row"))?;
                out.push(TaskSummary {
                    task_id: TaskId::from_uuid(id.parse().map_err(|_| PersistenceError::Storage {
                        detail: "uuid".into(),
                    })?),
                    goal,
                    state,
                    created_at: mimlys_core::clock::Instant::parse(&created).map_err(|_| {
                        PersistenceError::Storage {
                            detail: "instant".into(),
                        }
                    })?,
                    updated_at: mimlys_core::clock::Instant::parse(&updated).map_err(|_| {
                        PersistenceError::Storage {
                            detail: "instant".into(),
                        }
                    })?,
                });
            }
            Ok(out)
        }

        fn tasks_in_state(&self, state: &str) -> Result<Vec<TaskId>, PersistenceError> {
            let conn = self.conn.lock().map_err(|_| poisoned())?;
            let mut stmt = conn
                .prepare("SELECT task_id FROM tasks WHERE state = ?1 ORDER BY created_at")
                .map_err(storage("prepare"))?;
            let rows = stmt
                .query_map([state], |r| r.get::<_, String>(0))
                .map_err(storage("query"))?;
            let mut out = Vec::new();
            for row in rows {
                let id = row.map_err(storage("row"))?;
                out.push(TaskId::from_uuid(id.parse().map_err(|_| {
                    PersistenceError::Storage {
                        detail: "uuid".into(),
                    }
                })?));
            }
            Ok(out)
        }

        fn load_grants(&self, task_id: TaskId) -> Result<Vec<Grant>, PersistenceError> {
            self.query(
                "SELECT record FROM grants WHERE task_id = ?1 ORDER BY rowid",
                &[&task_id.to_string()],
            )
        }

        fn load_steps(&self, task_id: TaskId) -> Result<Vec<Step>, PersistenceError> {
            let conn = self.conn.lock().map_err(|_| poisoned())?;
            let mut stmt = conn
                .prepare("SELECT record, rejection_detail FROM steps WHERE task_id = ?1 ORDER BY idx")
                .map_err(storage("prepare"))?;
            let rows = stmt
                .query_map([task_id.to_string()], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
                })
                .map_err(storage("query"))?;
            let mut out = Vec::new();
            for row in rows {
                let (record, detail) = row.map_err(storage("row"))?;
                let mut step: Step = parse(record)?;
                // A step recorded before schema version 3 has no detail: the code's hint is its feedback.
                if let (Some(code), Some(text)) = (step.rejection, detail) {
                    step.rejection_detail = serde_json::from_str::<serde_json::Value>(&text)
                        .ok()
                        .and_then(|value| Detail::from_value(code, &value));
                }
                out.push(step);
            }
            Ok(out)
        }

        fn load_actions(&self, task_id: TaskId) -> Result<Vec<Action>, PersistenceError> {
            self.query(
                "SELECT record FROM actions WHERE task_id = ?1 ORDER BY created_at, rowid",
                &[&task_id.to_string()],
            )
        }

        fn load_running_action(&self, task_id: TaskId) -> Result<Option<Action>, PersistenceError> {
            self.query_one("SELECT record FROM actions WHERE task_id = ?1 AND status = 'running' ORDER BY rowid DESC", &[&task_id.to_string()])
        }

        fn load_observations(&self, task_id: TaskId) -> Result<Vec<Observation>, PersistenceError> {
            self.query(
                "SELECT o.record FROM observations o JOIN actions a ON a.action_id = o.action_id WHERE a.task_id = ?1 ORDER BY o.rowid",
                &[&task_id.to_string()],
            )
        }

        fn load_executions(&self, action_id: ActionId) -> Result<Vec<Execution>, PersistenceError> {
            self.query(
                "SELECT record FROM executions WHERE action_id = ?1 ORDER BY attempt",
                &[&action_id.to_string()],
            )
        }

        fn load_approvals(&self, task_id: TaskId) -> Result<Vec<ApprovalRequest>, PersistenceError> {
            self.query(
                "SELECT record FROM approvals WHERE task_id = ?1 ORDER BY rowid",
                &[&task_id.to_string()],
            )
        }

        fn load_pending_approval(
            &self,
            task_id: TaskId,
        ) -> Result<Option<ApprovalRequest>, PersistenceError> {
            self.query_one(
                "SELECT record FROM approvals WHERE task_id = ?1 AND status IN ('pending', 'approved') ORDER BY rowid DESC",
                &[&task_id.to_string()],
            )
        }

        fn load_pending_question(&self, task_id: TaskId) -> Result<Option<Question>, PersistenceError> {
            self.query_one("SELECT record FROM questions WHERE task_id = ?1 AND status = 'pending' ORDER BY rowid DESC", &[&task_id.to_string()])
        }

        fn load_questions(&self, task_id: TaskId) -> Result<Vec<Question>, PersistenceError> {
            self.query(
                "SELECT record FROM questions WHERE task_id = ?1 ORDER BY rowid",
                &[&task_id.to_string()],
            )
        }

        fn load_plan(&self, task_id: TaskId) -> Result<Option<PlanVersion>, PersistenceError> {
            self.query_one(
                "SELECT record FROM plan_versions WHERE task_id = ?1 ORDER BY version DESC",
                &[&task_id.to_string()],
            )
        }

        fn events_since(
            &self,
            position: u64,
            task_id: Option<TaskId>,
            limit: usize,
        ) -> Result<Vec<PositionedEvent>, PersistenceError> {
            let conn = self.conn.lock().map_err(|_| poisoned())?;
            let mut stmt = conn
                .prepare("SELECT position, record FROM events WHERE position > ?1 AND (?2 IS NULL OR task_id = ?2) ORDER BY position LIMIT ?3")
                .map_err(storage("prepare"))?;
            let rows = stmt
                .query_map(
                    params![
                        position as i64,
                        task_id.map(|t| t.to_string()),
                        limit as i64
                    ],
                    |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)),
                )
                .map_err(storage("query"))?;
            let mut out = Vec::new();
            for row in rows {
                let (position, record) = row.map_err(storage("row"))?;
                out.push(PositionedEvent {
                    position: position as u64,
                    record: parse(record)?,
                });
            }
            Ok(out)
        }

        fn metrics(
            &self,
            task_id: Option<TaskId>,
        ) -> Result<mimlys_core::spi::Metrics, PersistenceError> {
            crate::metrics::metrics(&self.conn, task_id)
        }

        fn load_snapshots(&self, task_id: TaskId) -> Result<Vec<SnapshotRecord>, PersistenceError> {
            self.query(
                "SELECT record FROM snapshots WHERE task_id = ?1 ORDER BY created_at, rowid",
                &[&task_id.to_string()],
            )
        }

        fn load_snapshot(
            &self,
            snapshot_id: SnapshotId,
        ) -> Result<Option<SnapshotRecord>, PersistenceError> {
            self.query_one(
                "SELECT record FROM snapshots WHERE snapshot_id = ?1",
                &[&snapshot_id.to_string()],
            )
        }

        fn load_workspace_state(
            &self,
            task_id: TaskId,
        ) -> Result<Option<WorkspaceState>, PersistenceError> {
            let conn = self.conn.lock().map_err(|_| poisoned())?;
            let state: Option<String> = conn
                .query_row(
                    "SELECT state FROM workspaces WHERE task_id = ?1",
                    params![task_id.to_string()],
                    |r| r.get(0),
                )
                .ok();
            Ok(state.and_then(|s| match s.as_str() {
                "allocated" => Some(WorkspaceState::Allocated),
                "active" => Some(WorkspaceState::Active),
                "deleted" => Some(WorkspaceState::Deleted),
                _ => None,
            }))
        }

        fn load_promotions(&self, task_id: TaskId) -> Result<Vec<Promotion>, PersistenceError> {
            self.query(
                "SELECT record FROM promotions WHERE task_id = ?1 ORDER BY applied_at, rowid",
                &[&task_id.to_string()],
            )
        }

        fn object_references(
            &self,
            hashes: &[mimlys_core::hash::Hash],
        ) -> Result<Vec<(mimlys_core::hash::Hash, u64, u64)>, PersistenceError> {
            let conn = self.conn.lock().map_err(|_| poisoned())?;
            let mut stmt = conn
                .prepare("SELECT size, refs FROM objects WHERE hash = ?1")
                .map_err(storage("prepare"))?;
            let mut out = Vec::new();
            for hash in hashes {
                if let Ok((size, refs)) = stmt.query_row(params![hash.as_str()], |r| {
                    Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
                }) {
                    out.push((hash.clone(), size as u64, refs as u64));
                }
            }
            Ok(out)
        }

        fn blob_bytes(&self, task_id: TaskId) -> Result<u64, PersistenceError> {
            let conn = self.conn.lock().map_err(|_| poisoned())?;
            let total: i64 = conn
                .query_row(
                    "SELECT coalesce(sum(size), 0) FROM blobs WHERE task_id = ?1",
                    params![task_id.to_string()],
                    |r| r.get(0),
                )
                .map_err(storage("blob_bytes"))?;
            Ok(total as u64)
        }

        fn get_blob(&self, blob_id: BlobId) -> Result<Vec<u8>, PersistenceError> {
            let exists: i64 = {
                let conn = self.conn.lock().map_err(|_| poisoned())?;
                conn.query_row(
                    "SELECT count(*) FROM blobs WHERE blob_id = ?1",
                    [blob_id.to_string()],
                    |r| r.get(0),
                )
                .map_err(storage("query"))?
            };
            if exists == 0 {
                return Err(PersistenceError::NotFound {
                    what: "blob".into(),
                });
            }
            std::fs::read(self.data_dir.join("blobs").join(blob_id.to_string()))
                .map_err(io("blob_read"))
        }
    }
    };
}

persistence_read!(Store);
persistence_read!(crate::reader::ReadStore);

impl Persistence for Store {
    fn create_task(
        &self,
        task: &Task,
        grants: &[Grant],
        event: &EventRecord,
    ) -> Result<(), PersistenceError> {
        self.transaction("create_task", |tx| {
            upsert_task(tx, task)?;
            for grant in grants {
                insert_grant(tx, grant)?;
            }
            append_event(tx, event)
        })
    }

    fn record_task_transition(
        &self,
        task: &Task,
        event: &EventRecord,
    ) -> Result<(), PersistenceError> {
        self.transaction("task_transition", |tx| {
            upsert_task(tx, task)?;
            append_event(tx, event)
        })
    }

    fn record_step_started(
        &self,
        step: &Step,
        event: &EventRecord,
    ) -> Result<(), PersistenceError> {
        self.transaction("step_started", |tx| {
            upsert_step(tx, step)?;
            append_event(tx, event)
        })
    }

    fn record_proposed(
        &self,
        step: &Step,
        action: &Action,
        event: &EventRecord,
    ) -> Result<(), PersistenceError> {
        if action.status != ActionStatus::Proposed {
            return Err(PersistenceError::Ordering {
                detail: "proposed requires status proposed".into(),
            });
        }
        self.transaction("proposed", |tx| {
            upsert_step(tx, step)?;
            upsert_action(tx, action)?;
            append_event(tx, event)
        })
    }

    fn record_validation(
        &self,
        step: &Step,
        action: Option<&Action>,
        event: &EventRecord,
    ) -> Result<(), PersistenceError> {
        self.transaction("validation", |tx| {
            upsert_step(tx, step)?;
            if let Some(action) = action {
                expect_status(tx, action.action_id, &["proposed"], "validation")?;
                upsert_action(tx, action)?;
            }
            append_event(tx, event)
        })
    }

    fn record_approval_request(
        &self,
        task: &Task,
        action: &Action,
        approval: &ApprovalRequest,
        event: &EventRecord,
    ) -> Result<(), PersistenceError> {
        self.transaction("approval_request", |tx| {
            expect_status(
                tx,
                action.action_id,
                &["proposed", "waiting_approval"],
                "approval_request",
            )?;
            upsert_action(tx, action)?;
            upsert_approval(tx, approval)?;
            upsert_task(tx, task)?;
            append_event(tx, event)
        })
    }

    fn record_approval_decision(
        &self,
        task: &Task,
        approval: &ApprovalRequest,
        event: &EventRecord,
    ) -> Result<(), PersistenceError> {
        self.transaction("approval_decision", |tx| {
            upsert_approval(tx, approval)?;
            upsert_task(tx, task)?;
            append_event(tx, event)
        })
    }

    fn record_authorized(
        &self,
        action: &Action,
        consumed: Option<&ApprovalRequest>,
        event: &EventRecord,
    ) -> Result<(), PersistenceError> {
        self.transaction("authorized", |tx| {
            expect_status(
                tx,
                action.action_id,
                &["proposed", "waiting_approval", "failed"],
                "authorized",
            )?;
            upsert_action(tx, action)?;
            if let Some(approval) = consumed {
                upsert_approval(tx, approval)?;
            }
            append_event(tx, event)
        })
    }

    fn record_running(&self, action: &Action, event: &EventRecord) -> Result<(), PersistenceError> {
        self.transaction("running", |tx| {
            expect_status(tx, action.action_id, &["authorized"], "running")?;
            upsert_action(tx, action)?;
            append_event(tx, event)
        })
    }

    fn record_execution(
        &self,
        execution: &Execution,
        observation: &Observation,
        event: &EventRecord,
    ) -> Result<(), PersistenceError> {
        self.transaction("execution", |tx| {
            expect_status(tx, execution.action_id, &["running"], "execution")?;
            tx.execute(
                "INSERT INTO executions (execution_id, action_id, attempt, termination, record, record_version) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    execution.execution_id.to_string(),
                    execution.action_id.to_string(),
                    execution.attempt,
                    serde_json::to_value(&execution.termination).ok().and_then(|v| v["termination"].as_str().map(str::to_owned)),
                    json(execution)?,
                    RECORD_VERSION,
                ],
            )
            .map_err(storage("insert_execution"))?;
            tx.execute(
                "INSERT INTO observations (observation_id, action_id, created_at, record, record_version) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    observation.observation_id.to_string(),
                    observation.action_id.to_string(),
                    observation.created_at.to_rfc3339(),
                    json(observation)?,
                    RECORD_VERSION,
                ],
            )
            .map_err(storage("insert_observation"))?;
            append_event(tx, event)
        })
    }

    fn record_not_started(
        &self,
        action: &Action,
        observation: &Observation,
        event: &EventRecord,
    ) -> Result<(), PersistenceError> {
        if action.status != ActionStatus::Cancelled {
            return Err(PersistenceError::Ordering {
                detail: "not_started requires the status cancelled".into(),
            });
        }
        self.transaction("not_started", |tx| {
            expect_status(tx, action.action_id, &["authorized"], "not_started")?;
            upsert_action(tx, action)?;
            tx.execute(
                "INSERT INTO observations (observation_id, action_id, created_at, record, record_version) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    observation.observation_id.to_string(),
                    observation.action_id.to_string(),
                    observation.created_at.to_rfc3339(),
                    json(observation)?,
                    RECORD_VERSION,
                ],
            )
            .map_err(storage("insert_observation"))?;
            append_event(tx, event)
        })
    }

    fn record_action_terminal(
        &self,
        action: &Action,
        event: &EventRecord,
    ) -> Result<(), PersistenceError> {
        if !action.status.is_terminal() {
            return Err(PersistenceError::Ordering {
                detail: "action_terminal requires a terminal status".into(),
            });
        }
        self.transaction("action_terminal", |tx| {
            let allowed: &[&str] = match action.status {
                ActionStatus::Rejected { .. } => &["proposed", "waiting_approval"],
                ActionStatus::Cancelled => {
                    &["proposed", "waiting_approval", "authorized", "running"]
                }
                _ => &["running"],
            };
            expect_status(tx, action.action_id, allowed, "action_terminal")?;
            upsert_action(tx, action)?;
            append_event(tx, event)
        })
    }

    fn record_step_outcome(
        &self,
        step: &Step,
        task: &Task,
        event: &EventRecord,
    ) -> Result<(), PersistenceError> {
        self.transaction("step_outcome", |tx| {
            upsert_step(tx, step)?;
            upsert_task(tx, task)?;
            append_event(tx, event)
        })
    }

    fn record_question(
        &self,
        task: &Task,
        question: &Question,
        event: &EventRecord,
    ) -> Result<(), PersistenceError> {
        self.transaction("task_transition", |tx| {
            upsert_question(tx, question)?;
            upsert_task(tx, task)?;
            append_event(tx, event)
        })
    }

    fn record_question_update(
        &self,
        task: &Task,
        question: &Question,
        event: &EventRecord,
    ) -> Result<(), PersistenceError> {
        self.transaction("task_transition", |tx| {
            upsert_question(tx, question)?;
            upsert_task(tx, task)?;
            append_event(tx, event)
        })
    }

    fn record_grant(&self, grant: &Grant, event: &EventRecord) -> Result<(), PersistenceError> {
        self.transaction("task_transition", |tx| {
            insert_grant(tx, grant)?;
            append_event(tx, event)
        })
    }

    fn record_grant_status(
        &self,
        grant: &Grant,
        event: &EventRecord,
    ) -> Result<(), PersistenceError> {
        self.transaction("task_transition", |tx| {
            tx.execute(
                "UPDATE grants SET status = ?2, record = ?3 WHERE grant_id = ?1",
                params![
                    grant.grant_id.to_string(),
                    status_name(&grant.status)?,
                    json(grant)?
                ],
            )
            .map_err(storage("update_grant"))?;
            append_event(tx, event)
        })
    }

    fn record_plan(&self, plan: &PlanVersion, event: &EventRecord) -> Result<(), PersistenceError> {
        self.transaction("task_transition", |tx| {
            tx.execute(
                "INSERT INTO plan_versions (task_id, version, record) VALUES (?1, ?2, ?3)",
                params![plan.task_id.to_string(), plan.version, json(plan)?],
            )
            .map_err(storage("insert_plan"))?;
            append_event(tx, event)
        })
    }

    fn put_blob(&self, task_id: TaskId, bytes: &[u8]) -> Result<BlobId, PersistenceError> {
        if bytes.len() > LIM_S4_BLOB_BYTES {
            return Err(PersistenceError::Storage {
                detail: "blob_too_large".into(),
            });
        }
        let blob_id = BlobId::from_uuid(uuid_v7_for_blob());
        let path = self.data_dir.join("blobs").join(blob_id.to_string());
        {
            let mut file = std::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&path)
                .map_err(io("blob_create"))?;
            file.write_all(bytes).map_err(io("blob_write"))?;
            file.sync_all().map_err(io("blob_sync"))?;
        }
        restrict_file(&path)?;
        sync_dir(&self.data_dir.join("blobs"))?;
        crash::maybe("blob", "file");
        let created_at = mimlys_core::clock::Clock::now(&mimlys_core::clock::SystemClock);
        self.transaction("blob", |tx| {
            tx.execute(
                "INSERT INTO blobs (blob_id, task_id, size, created_at) VALUES (?1, ?2, ?3, ?4)",
                params![
                    blob_id.to_string(),
                    task_id.to_string(),
                    bytes.len() as i64,
                    created_at.to_rfc3339()
                ],
            )
            .map_err(storage("insert_blob"))?;
            Ok(())
        })?;
        Ok(blob_id)
    }

    fn record_snapshot(
        &self,
        snapshot: &SnapshotRecord,
        objects: &[(mimlys_core::hash::Hash, u64)],
        event: &EventRecord,
    ) -> Result<(), PersistenceError> {
        self.transaction("snapshot", |tx| {
            tx.execute(
                "INSERT INTO snapshots (snapshot_id, task_id, reason, manifest_hash, bytes, files, created_at, action_id, record, record_version) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    snapshot.snapshot_id.to_string(),
                    snapshot.task_id.to_string(),
                    snapshot.reason.name(),
                    snapshot.manifest_hash.as_str(),
                    snapshot.bytes as i64,
                    snapshot.files as i64,
                    snapshot.created_at.to_rfc3339(),
                    snapshot.action_id.map(|a| a.to_string()),
                    json(snapshot)?,
                    RECORD_VERSION,
                ],
            )
            .map_err(storage("insert_snapshot"))?;
            for (hash, size) in objects {
                tx.execute(
                    "INSERT INTO objects (hash, size, refs) VALUES (?1, ?2, 1) ON CONFLICT(hash) DO UPDATE SET refs = refs + 1",
                    params![hash.as_str(), *size as i64],
                )
                .map_err(storage("upsert_object"))?;
            }
            append_event(tx, event)
        })
    }

    fn record_workspace_state(
        &self,
        task_id: TaskId,
        state: WorkspaceState,
        event: Option<&EventRecord>,
    ) -> Result<(), PersistenceError> {
        self.transaction("workspace_state", |tx| {
            let at = event
                .map(|e| e.at.to_rfc3339())
                .unwrap_or_else(|| mimlys_core::clock::Instant::from_unix_millis(0).map(|i| i.to_rfc3339()).unwrap_or_default());
            tx.execute(
                "INSERT INTO workspaces (task_id, state, changed_at) VALUES (?1, ?2, ?3) ON CONFLICT(task_id) DO UPDATE SET state = excluded.state, changed_at = excluded.changed_at",
                params![task_id.to_string(), state.name(), at],
            )
            .map_err(storage("upsert_workspace"))?;
            match event {
                Some(e) => append_event(tx, e),
                None => Ok(()),
            }
        })
    }

    fn record_promotion(
        &self,
        promotion: &Promotion,
        event: &EventRecord,
    ) -> Result<(), PersistenceError> {
        self.transaction("promotion", |tx| {
            tx.execute(
                "INSERT INTO promotions (promotion_id, task_id, backup_snapshot_id, applied_at, record, record_version) VALUES (?1, ?2, ?3, ?4, ?5, ?6) ON CONFLICT(promotion_id) DO UPDATE SET record = excluded.record",
                params![
                    promotion.promotion_id.to_string(),
                    promotion.task_id.to_string(),
                    promotion.backup_snapshot.to_string(),
                    promotion.applied_at.to_rfc3339(),
                    json(promotion)?,
                    RECORD_VERSION,
                ],
            )
            .map_err(storage("upsert_promotion"))?;
            append_event(tx, event)
        })
    }

    fn delete_task_data(
        &self,
        task_id: TaskId,
        audit: bool,
        event: &EventRecord,
    ) -> Result<DeletedRows, PersistenceError> {
        let files = crate::snapshots::SnapshotFiles::open(&self.data_dir)?;
        let snapshots = self.load_snapshots(task_id)?;
        // The objects each manifest references, counted once per manifest (REQ-PERS-009).
        let mut releases: std::collections::BTreeMap<mimlys_core::hash::Hash, u64> =
            std::collections::BTreeMap::new();
        for s in &snapshots {
            if let Ok(manifest) = files.read_manifest(s.snapshot_id, &s.manifest_hash) {
                for hash in manifest.objects().keys() {
                    *releases.entry(hash.clone()).or_default() += 1;
                }
            }
        }
        let id = task_id.to_string();
        self.transaction("delete", |tx| {
            let mut out = DeletedRows::default();
            for s in &snapshots {
                out.snapshots.push(s.snapshot_id);
            }
            for (hash, n) in &releases {
                tx.execute(
                    "UPDATE objects SET refs = max(refs - ?2, 0) WHERE hash = ?1",
                    params![hash.as_str(), *n as i64],
                )
                .map_err(storage("release_object"))?;
                let refs: i64 = tx
                    .query_row("SELECT refs FROM objects WHERE hash = ?1", params![hash.as_str()], |r| r.get(0))
                    .map_err(storage("object_refs"))?;
                if refs == 0 {
                    tx.execute("DELETE FROM objects WHERE hash = ?1", params![hash.as_str()])
                        .map_err(storage("delete_object"))?;
                    out.objects_at_zero.push(hash.clone());
                }
            }
            tx.execute("DELETE FROM promotions WHERE task_id = ?1", params![id])
                .map_err(storage("delete_promotions"))?;
            tx.execute("DELETE FROM snapshots WHERE task_id = ?1", params![id])
                .map_err(storage("delete_snapshots"))?;
            let mut stmt = tx
                .prepare("SELECT blob_id FROM blobs WHERE task_id = ?1")
                .map_err(storage("prepare"))?;
            for row in stmt.query_map(params![id], |r| r.get::<_, String>(0)).map_err(storage("query"))? {
                if let Ok(uuid) = uuid::Uuid::parse_str(&row.map_err(storage("row"))?) {
                    out.blobs.push(BlobId::from_uuid(uuid));
                }
            }
            drop(stmt);
            tx.execute("DELETE FROM blobs WHERE task_id = ?1", params![id])
                .map_err(storage("delete_blobs"))?;
            if audit {
                for table in ["observations", "executions"] {
                    tx.execute(
                        &format!("DELETE FROM {table} WHERE action_id IN (SELECT action_id FROM actions WHERE task_id = ?1)"),
                        params![id],
                    )
                    .map_err(storage("delete_rows"))?;
                }
                for table in ["approvals", "questions", "actions", "steps", "grants", "plan_versions", "events", "workspaces"] {
                    tx.execute(&format!("DELETE FROM {table} WHERE task_id = ?1"), params![id])
                        .map_err(storage("delete_rows"))?;
                }
                tx.execute("DELETE FROM tasks WHERE task_id = ?1", params![id])
                    .map_err(storage("delete_task"))?;
            } else {
                tx.execute(
                    "INSERT INTO workspaces (task_id, state, changed_at) VALUES (?1, 'deleted', ?2) ON CONFLICT(task_id) DO UPDATE SET state = 'deleted', changed_at = excluded.changed_at",
                    params![id, event.at.to_rfc3339()],
                )
                .map_err(storage("upsert_workspace"))?;
                append_event(tx, event)?;
            }
            Ok(out)
        })
    }

    fn append_event(&self, event: &EventRecord) -> Result<(), PersistenceError> {
        self.transaction("event", |tx| append_event(tx, event))
    }
}

fn insert_grant(tx: &Transaction<'_>, grant: &Grant) -> Result<(), PersistenceError> {
    tx.execute(
        "INSERT INTO grants (grant_id, task_id, status, record, record_version) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![grant.grant_id.to_string(), grant.task_id.to_string(), status_name(&grant.status)?, json(grant)?, RECORD_VERSION],
    )
    .map_err(storage("insert_grant"))?;
    Ok(())
}

fn upsert_question(tx: &Transaction<'_>, question: &Question) -> Result<(), PersistenceError> {
    tx.execute(
        "INSERT INTO questions (question_id, task_id, status, record, record_version) VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT (question_id) DO UPDATE SET status = excluded.status, record = excluded.record",
        params![question.question_id.to_string(), question.task_id.to_string(), status_name(&question.status)?, json(question)?, RECORD_VERSION],
    )
    .map_err(storage("upsert_question"))?;
    Ok(())
}

fn sync_dir(path: &std::path::Path) -> Result<(), PersistenceError> {
    #[cfg(unix)]
    {
        std::fs::File::open(path)
            .and_then(|d| d.sync_all())
            .map_err(io("dir_sync"))?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

/// Blob identifiers are the store's own: they never leave the host and are
/// not part of the task's deterministic record stream.
fn uuid_v7_for_blob() -> uuid::Uuid {
    uuid::Uuid::now_v7()
}

impl Store {
    /// Blob files without a row (a crash between file and row); the garbage
    /// collector of M3 removes them (REQ-PERS-009).
    pub fn orphan_blobs(&self) -> Result<Vec<String>, PersistenceError> {
        let known: std::collections::HashSet<String> = {
            let conn = self.conn.lock().map_err(|_| poisoned())?;
            let mut stmt = conn
                .prepare("SELECT blob_id FROM blobs")
                .map_err(storage("prepare"))?;
            let rows = stmt
                .query_map([], |r| r.get::<_, String>(0))
                .map_err(storage("query"))?;
            rows.filter_map(Result::ok).collect()
        };
        let mut orphans = Vec::new();
        for entry in std::fs::read_dir(self.data_dir.join("blobs")).map_err(io("blobs_dir"))? {
            let name = entry
                .map_err(io("blobs_dir"))?
                .file_name()
                .to_string_lossy()
                .into_owned();
            if !known.contains(&name) {
                orphans.push(name);
            }
        }
        orphans.sort();
        Ok(orphans)
    }
}

#[cfg(test)]
pub(crate) mod tests;

#[cfg(test)]
mod rejection_detail_tests;

#[cfg(test)]
mod halts_tests;
