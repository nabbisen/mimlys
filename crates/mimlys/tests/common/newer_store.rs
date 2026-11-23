//! The store of a newer release, for the suites of REQ-MAIN-006 (`newer_records.rs`,
//! `newer_writers.rs`): made by this release (the real `mimlys`, and the persistence crate for the
//! records of an action), then written to as a newer release would write it, with raw SQL.

use mimlys_core::action::{
    Action, ActionInput, EffectKind, EffectView, Execution, ExecutionMetrics, Proposal, Termination,
};
use mimlys_core::clock::{Clock, SystemClock};
use mimlys_core::event::{Actor, Event, EventRecord};
use mimlys_core::hash::hash_bytes;
use mimlys_core::ids::{ActionId, EventId, ExecutionId, ObservationId, StepId, SystemIds, TaskId};
use mimlys_core::observation::{Observation, ObservationStatus};
use mimlys_core::spi::{Persistence, PersistenceRead};
use mimlys_core::step::{Step, StepOutcome};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::process::Command;

pub const MARKER: &str = "(unknown to this release)";
pub const MARKER_KEY: &str = "unknown_to_this_release";

pub struct Env {
    pub dir: tempfile::TempDir,
    pub config: PathBuf,
    pub data: PathBuf,
}

impl Env {
    pub fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        let config = dir.path().join("config.toml");
        std::fs::write(
            &config,
            format!(
                "config_version = 1\n[runtime]\ndata_dir = {:?}\n",
                data.to_string_lossy()
            ),
        )
        .unwrap();
        Self { dir, config, data }
    }

    pub fn command(&self, args: &[&str]) -> Command {
        let mut cmd = super::mimlys();
        cmd.arg("--config").arg(&self.config).args(args);
        cmd.env("MIMLYS_CONFIG", "");
        cmd
    }

    /// Exit code, standard output, standard error.
    pub fn run(&self, args: &[&str]) -> (i32, String, String) {
        let out = super::run_bounded(&mut self.command(args), self.dir.path());
        (out.code(), out.stdout, out.stderr)
    }

    pub fn json(&self, args: &[&str]) -> (i32, Value) {
        let mut all = vec!["--json"];
        all.extend_from_slice(args);
        let (code, stdout, stderr) = self.run(&all);
        let value = serde_json::from_str(stdout.trim())
            .unwrap_or_else(|e| panic!("not JSON ({e}): {stdout}\n{stderr}"));
        (code, value)
    }

    /// A task held in `created` by the real binary, on a scope of its own.
    pub fn new_task(&self, name: &str) -> String {
        let scope = self.dir.path().join(format!("scope-{name}"));
        std::fs::create_dir_all(scope.join("src")).unwrap();
        std::fs::write(scope.join("src/lib.rs"), "fn a() {}\n").unwrap();
        let (code, value) = self.json(&[
            "task",
            "new",
            name,
            "--scope",
            scope.to_str().unwrap(),
            "--hold",
        ]);
        assert_eq!(code, 0, "{value}");
        value["task"]["task_id"].as_str().unwrap().to_owned()
    }

    pub fn sql(&self, statement: &str, params: &[&dyn rusqlite::ToSql]) {
        let conn = rusqlite::Connection::open(self.data.join("mimlys.db")).unwrap();
        conn.execute(statement, params).unwrap();
    }

    /// The task, as a newer release wrote it: the state column and the record patched.
    pub fn write_task(&self, id: &str, state: &str, patch: Value) {
        self.sql(
            "UPDATE tasks SET state = ?2, record = json_patch(record, ?3) WHERE task_id = ?1",
            &[&id, &state, &patch.to_string()],
        );
    }

    /// An event of the task as a newer release wrote it: the task's first event with `patch`.
    pub fn write_event(&self, id: &str, event_id: u32, name: &str, patch: Value) {
        self.sql(
            "INSERT INTO events (event_id, at, name, task_id, step_id, action_id, record)
             SELECT ?3, at, ?2, task_id, NULL, NULL,
                    json_patch(json_set(record, '$.event', ?2, '$.event_id', ?3), ?4)
             FROM events WHERE task_id = ?1 ORDER BY position LIMIT 1",
            &[
                &id,
                &name,
                &format!("00000000-00ff-7000-8000-{event_id:012x}"),
                &patch.to_string(),
            ],
        );
    }

    /// Every column of every row the task owns, as text.
    pub fn rows_of(&self, id: &str) -> String {
        let conn = rusqlite::Connection::open(self.data.join("mimlys.db")).unwrap();
        let tables: Vec<String> = conn
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        let mut out = String::new();
        for table in tables {
            let columns: Vec<String> = conn
                .prepare(&format!("SELECT name FROM pragma_table_info('{table}')"))
                .unwrap()
                .query_map([], |r| r.get(0))
                .unwrap()
                .map(Result::unwrap)
                .collect();
            if !columns.iter().any(|c| c == "task_id") {
                continue;
            }
            let quoted = columns
                .iter()
                .map(|c| format!("quote({c})"))
                .collect::<Vec<_>>()
                .join(" || '|' || ");
            let mut stmt = conn
                .prepare(&format!(
                    "SELECT {quoted} FROM {table} WHERE task_id = ?1 ORDER BY rowid"
                ))
                .unwrap();
            for row in stmt.query_map([id], |r| r.get::<_, String>(0)).unwrap() {
                out.push_str(&format!("{table}: {}\n", row.unwrap()));
            }
        }
        out
    }
}

/// The records of one action that ran, written through the persistence crate as a run writes them.
pub fn run_an_action(data: &std::path::Path, task: &str, index: u32) -> ActionId {
    let store = mimlys_persist::open(data).unwrap();
    let task = store
        .load_task(TaskId::from_uuid(task.parse().unwrap()))
        .unwrap()
        .unwrap();
    let ids = SystemIds;
    let now = SystemClock.now();
    let event = |event: Event| EventRecord {
        event_id: EventId::new(&ids),
        at: now,
        actor: Actor::Runtime,
        task_id: Some(task.task_id),
        step_id: None,
        action_id: None,
        policy_hash: task.versions.policy_hash.clone(),
        event,
    };
    let mut step = Step::start(StepId::new(&ids), task.task_id, index, now);
    store
        .record_step_started(&step, &event(Event::StepStarted))
        .unwrap();
    let action = Action::propose(
        Proposal {
            action_id: ActionId::new(&ids),
            task_id: task.task_id,
            step_id: step.step_id,
            tool: "write_file".into(),
            arguments: json!({"path": "a.txt", "content": "x"}),
            grant_set_hash: hash_bytes(b"grants"),
            effect: EffectView {
                kind: EffectKind::WorkspaceWrite,
                reversible: true,
                idempotent: false,
            },
            approval_required: false,
            deadline_ms: 30_000,
        },
        now,
    );
    step.action_id = Some(action.action_id);
    store
        .record_proposed(&step, &action, &event(Event::ActionProposed))
        .unwrap();
    store
        .record_validation(&step, Some(&action), &event(Event::StepStarted))
        .unwrap();
    let action = action.transition(ActionInput::Authorize, now).unwrap();
    store
        .record_authorized(&action, None, &event(Event::ActionAuthorized))
        .unwrap();
    let action = action.transition(ActionInput::Start, now).unwrap();
    store
        .record_running(&action, &event(Event::ActionStarted))
        .unwrap();
    let execution = Execution {
        execution_id: ExecutionId::new(&ids),
        action_id: action.action_id,
        attempt: 1,
        started_at: now,
        ended_at: now,
        termination: Termination::Exited { exit_code: 0 },
        snapshot_before: None,
        snapshot_after: None,
        stdout_blob: None,
        stderr_blob: None,
        metrics: ExecutionMetrics::default(),
    };
    let observation = Observation {
        observation_id: ObservationId::new(&ids),
        action_id: action.action_id,
        tool: "write_file".into(),
        status: ObservationStatus::Succeeded,
        cause: None,
        rolled_back: false,
        payload: json!({"bytes_written": 1}),
        full_payload: None,
        stdout: String::new(),
        stderr: String::new(),
        notices: vec![],
        metrics: ExecutionMetrics::default(),
        created_at: now,
    };
    store
        .record_execution(
            &execution,
            &observation,
            &event(Event::ExecutionFinished {
                attempt: 1,
                termination: "exited".into(),
            }),
        )
        .unwrap();
    let action = action.transition(ActionInput::Succeed, now).unwrap();
    store
        .record_action_terminal(
            &action,
            &event(Event::ActionFinished {
                status: "succeeded".into(),
                cause: None,
            }),
        )
        .unwrap();
    step.finish(StepOutcome::Continued, now);
    action.action_id
}

/// The tasks of a store that holds words of a newer release, by what they hold.
pub struct Newer {
    pub env: Env,
    /// Waits for a kind of item this release has not got (a later release's `review`).
    pub wait: String,
    /// In a state this release has not got.
    pub state: String,
    /// Halted for a reason this release has not got.
    pub halt: String,
    /// Failed for a reason this release has not got.
    pub failed: String,
    /// Holds an event this release has not got.
    pub event: String,
    /// Holds an action whose status this release has not got, beside one it has.
    pub action: String,
    /// The action of `action` that this release can read, and the one it cannot.
    pub readable_action: ActionId,
    pub unreadable_action: ActionId,
    /// Holds nothing of the kind.
    pub plain: String,
}

impl Newer {
    pub fn new() -> Self {
        let env = Env::new();
        let plain = env.new_task("plain");
        let wait = env.new_task("wait");
        let state = env.new_task("state");
        let halt = env.new_task("halt");
        let failed = env.new_task("failed");
        let event = env.new_task("event");
        let action = env.new_task("action");
        let readable_action = run_an_action(&env.data, &action, 0);
        let unreadable_action = run_an_action(&env.data, &action, 1);
        env.write_task(
            &wait,
            "waiting_user",
            json!({"state": "waiting_user", "wait_kind": "review"}),
        );
        env.write_task(&state, "quarantined", json!({"state": "quarantined"}));
        env.write_task(
            &halt,
            "halted",
            json!({"state": "halted", "halt_reason": "needs_review"}),
        );
        env.write_event(
            &halt,
            1,
            "task.state_changed",
            json!({"from": "running", "to": "halted", "reason": "needs_review"}),
        );
        env.write_task(
            &failed,
            "failed",
            json!({"state": "failed", "reason": {"failure_reason": "daemon_lost"}}),
        );
        env.write_event(&event, 2, "session.attached", json!({"client": "tui"}));
        env.sql(
            "UPDATE actions SET status = 'held_for_review', record = json_set(record, '$.status', 'held_for_review') WHERE action_id = ?1",
            &[&unreadable_action.to_string()],
        );
        Self {
            env,
            wait,
            state,
            halt,
            failed,
            event,
            action,
            readable_action,
            unreadable_action,
            plain,
        }
    }

    pub fn newer_tasks(&self) -> [(&str, &str, &str); 6] {
        [
            (self.wait.as_str(), "wait_kind", "review"),
            (self.state.as_str(), "state", "quarantined"),
            (self.halt.as_str(), "halt_reason", "needs_review"),
            (self.failed.as_str(), "reason.failure_reason", "daemon_lost"),
            (self.event.as_str(), "event", "session.attached"),
            (self.action.as_str(), "status", "held_for_review"),
        ]
    }
}
