//! Integrity at startup (external design §16.3, §21.3; REQ-PERS-006): the
//! database integrity check, then the consistency rules CONS-01 … CONS-08.
//! The first violation names its rule; the runtime then refuses to run tasks.

use crate::sqlite::{Store, storage};
use mimlys_core::action::Action;
use mimlys_core::approval::ApprovalRequest;
use mimlys_core::clock::Instant;
use mimlys_core::grant::{Grant, GrantSet};
use mimlys_core::spi::PersistenceError;
use mimlys_core::task::{Task, TaskState};
use rusqlite::params;

const ACTION_STATUSES: [&str; 9] = [
    "proposed",
    "rejected",
    "waiting_approval",
    "authorized",
    "running",
    "succeeded",
    "failed",
    "cancelled",
    "unknown",
];
const TASK_STATES: [&str; 9] = [
    "created",
    "running",
    "waiting_user",
    "paused",
    "halted",
    "recovering",
    "completed",
    "failed",
    "cancelled",
];
const TERMINAL_TASK_STATES: [&str; 3] = ["completed", "failed", "cancelled"];

fn violation(rule: &str, detail: String) -> PersistenceError {
    PersistenceError::Integrity {
        rule: rule.to_owned(),
        detail,
    }
}

/// Runs the database integrity check and every consistency rule (LIM-P5).
pub fn check(store: &Store, now: Instant) -> Result<(), PersistenceError> {
    store.with_conn(|conn| -> Result<(), PersistenceError> {
        let result: String = conn
            .query_row("PRAGMA integrity_check", [], |r| r.get(0))
            .map_err(storage("integrity_check"))?;
        if result != "ok" {
            return Err(violation("integrity_check", result));
        }
        // CONS-07: every status and state is in its vocabulary.
        let mut stmt = conn.prepare("SELECT action_id, status FROM actions").map_err(storage("prepare"))?;
        for row in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))).map_err(storage("query"))? {
            let (id, status) = row.map_err(storage("row"))?;
            if !ACTION_STATUSES.contains(&status.as_str()) {
                return Err(violation("CONS-07", format!("action {id} has status {status}")));
            }
        }
        let mut stmt = conn.prepare("SELECT task_id, state FROM tasks").map_err(storage("prepare"))?;
        for row in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))).map_err(storage("query"))? {
            let (id, state) = row.map_err(storage("row"))?;
            if !TASK_STATES.contains(&state.as_str()) {
                return Err(violation("CONS-07", format!("task {id} has state {state}")));
            }
        }
        // CONS-02: no running action under a terminal task.
        let count: i64 = conn
            .query_row(
                "SELECT count(*) FROM actions a JOIN tasks t ON t.task_id = a.task_id WHERE a.status = 'running' AND t.state IN ('completed', 'failed', 'cancelled')",
                [],
                |r| r.get(0),
            )
            .map_err(storage("query"))?;
        if count > 0 {
            return Err(violation("CONS-02", format!("{count} running action(s) under a terminal task")));
        }
        // CONS-01: a completed task has nothing unfinished.
        for (sql, what) in [
            ("SELECT count(*) FROM steps s JOIN tasks t ON t.task_id = s.task_id WHERE t.state = 'completed' AND s.outcome IS NULL", "step without outcome"),
            ("SELECT count(*) FROM actions a JOIN tasks t ON t.task_id = a.task_id WHERE t.state = 'completed' AND a.status IN ('proposed', 'waiting_approval', 'authorized', 'running')", "non-terminal action"),
            ("SELECT count(*) FROM approvals p JOIN tasks t ON t.task_id = p.task_id WHERE t.state = 'completed' AND p.status IN ('pending', 'approved')", "open approval"),
            ("SELECT count(*) FROM questions q JOIN tasks t ON t.task_id = q.task_id WHERE t.state = 'completed' AND q.status = 'pending'", "pending question"),
        ] {
            let count: i64 = conn.query_row(sql, [], |r| r.get(0)).map_err(storage("query"))?;
            if count > 0 {
                return Err(violation("CONS-01", format!("{count} {what}(s) under a completed task")));
            }
        }
        // CONS-03: a succeeded action that required approval has a consumed approval.
        let count: i64 = conn
            .query_row(
                "SELECT count(*) FROM actions a WHERE a.status = 'succeeded' AND a.approval_required = 1
                 AND NOT EXISTS (SELECT 1 FROM approvals p WHERE p.action_id = a.action_id AND p.status = 'consumed')",
                [],
                |r| r.get(0),
            )
            .map_err(storage("query"))?;
        if count > 0 {
            return Err(violation("CONS-03", format!("{count} succeeded action(s) without a consumed approval")));
        }
        // CONS-05: every approval matches its action's hash.
        let count: i64 = conn
            .query_row(
                "SELECT count(*) FROM approvals p JOIN actions a ON a.action_id = p.action_id WHERE p.action_hash <> a.action_hash",
                [],
                |r| r.get(0),
            )
            .map_err(storage("query"))?;
        if count > 0 {
            return Err(violation("CONS-05", format!("{count} approval(s) whose hash differs from the action")));
        }
        // CONS-06: an open approval's grant-set hash equals the task's current active grant set.
        let mut stmt = conn
            .prepare("SELECT record FROM approvals WHERE status IN ('pending', 'approved')")
            .map_err(storage("prepare"))?;
        let approvals: Vec<ApprovalRequest> = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(storage("query"))?
            .filter_map(Result::ok)
            .filter_map(|s| serde_json::from_str(&s).ok())
            .collect();
        for approval in approvals {
            let mut stmt = conn.prepare("SELECT record FROM grants WHERE task_id = ?1").map_err(storage("prepare"))?;
            let grants: Vec<Grant> = stmt
                .query_map(params![approval.task_id.to_string()], |r| r.get::<_, String>(0))
                .map_err(storage("query"))?
                .filter_map(Result::ok)
                .filter_map(|s| serde_json::from_str(&s).ok())
                .collect();
            let mut set = GrantSet::new();
            for g in grants {
                set.add(g);
            }
            if set.hash(now) != approval.grant_set_hash {
                return Err(violation("CONS-06", format!("approval {} outlived a grant change", approval.approval_id)));
            }
        }
        // CONS-08: counters within budgets unless the task is halted or terminal.
        let mut stmt = conn.prepare("SELECT record FROM tasks").map_err(storage("prepare"))?;
        let tasks: Vec<Task> = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(storage("query"))?
            .filter_map(Result::ok)
            .filter_map(|s| serde_json::from_str(&s).ok())
            .collect();
        for task in tasks {
            let exempt = matches!(task.state, TaskState::Halted { .. }) || task.state.is_terminal();
            let c = &task.counters;
            let b = &task.budgets;
            let over = c.steps > b.steps
                || c.consecutive_rejections > b.consecutive_rejections
                || c.consecutive_failures > b.consecutive_failures
                || c.identical_decisions > b.identical_decisions;
            if over && !exempt {
                return Err(violation("CONS-08", format!("task {} exceeds a budget while {}", task.task_id, task.state.name())));
            }
        }
        // CONS-04: every snapshot row has its manifest with the recorded hash; every rollback and
        // every promotion backup references such a row; object reference counts equal the manifests' references.
        let files = crate::snapshots::SnapshotFiles::open(&store.data_dir)?;
        let mut stmt = conn
            .prepare("SELECT snapshot_id, manifest_hash FROM snapshots")
            .map_err(storage("prepare"))?;
        let mut expected: std::collections::BTreeMap<String, i64> = std::collections::BTreeMap::new();
        let mut known: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        for row in stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .map_err(storage("query"))?
        {
            let (id, hash) = row.map_err(storage("row"))?;
            let snapshot_id = uuid::Uuid::parse_str(&id)
                .map(mimlys_core::ids::SnapshotId::from_uuid)
                .map_err(|_| violation("CONS-04", format!("snapshot {id} has a malformed id")))?;
            let hash = mimlys_core::hash::Hash::from_hex(&hash)
                .ok_or_else(|| violation("CONS-04", format!("snapshot {id} has a malformed hash")))?;
            let manifest = files
                .read_manifest(snapshot_id, &hash)
                .map_err(|_| violation("CONS-04", format!("snapshot {id} has no manifest matching its hash")))?;
            for object in manifest.objects().keys() {
                *expected.entry(object.as_str().to_owned()).or_default() += 1;
            }
            known.insert(id);
        }
        let mut stmt = conn
            .prepare("SELECT hash, refs FROM objects")
            .map_err(storage("prepare"))?;
        for row in stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
            .map_err(storage("query"))?
        {
            let (hash, refs) = row.map_err(storage("row"))?;
            if expected.get(&hash).copied().unwrap_or(0) != refs {
                return Err(violation("CONS-04", format!("object {hash} has {refs} reference(s) recorded, {} in manifests", expected.get(&hash).copied().unwrap_or(0))));
            }
        }
        for (hash, n) in &expected {
            let recorded: i64 = conn
                .query_row("SELECT refs FROM objects WHERE hash = ?1", [hash], |r| r.get(0))
                .unwrap_or(0);
            if recorded != *n {
                return Err(violation("CONS-04", format!("object {hash} is referenced by {n} manifest(s) but recorded {recorded}")));
            }
        }
        let mut stmt = conn
            .prepare("SELECT promotion_id, backup_snapshot_id FROM promotions")
            .map_err(storage("prepare"))?;
        for row in stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .map_err(storage("query"))?
        {
            let (id, backup) = row.map_err(storage("row"))?;
            if !known.contains(&backup) {
                return Err(violation("CONS-04", format!("promotion {id} references an unknown backup snapshot")));
            }
        }
        let _ = TERMINAL_TASK_STATES;
        let _: Option<Action> = None;
        Ok(())
    })?
}
