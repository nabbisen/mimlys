use crate::integrity;
use crate::recovery::{RecoveryContext, recover_all};

fn fake_workspaces() -> mimlys_core::spi::fake::FakeWorkspaces {
    mimlys_core::spi::fake::FakeWorkspaces::new(std::sync::Arc::new(
        mimlys_core::spi::fake::FakeToolProvider::new(vec![]),
    ))
}
use crate::sqlite::{LIM_O3_LOCK_WAIT_MS, open};
use crate::store::LIM_S4_BLOB_BYTES;
use mimlys_core::action::{
    Action, ActionInput, EffectKind, EffectView, Execution, ExecutionMetrics, Proposal, Termination,
};
use mimlys_core::approval::{ApprovalRequest, Rendered, UserDecision};
use mimlys_core::clock::{Clock, FakeClock, Instant};
use mimlys_core::event::{Actor, ApprovalDecision, Event, EventRecord, RecoveryDisposition};
use mimlys_core::grant::{Grant, GrantKind};
use mimlys_core::hash::hash_bytes;
use mimlys_core::ids::{
    ActionId, ApprovalId, EventId, ExecutionId, GrantId, ObservationId, ScriptedIds, StepId, TaskId,
};
use mimlys_core::limits::LIM_T5_DEADLINE_MS;
use mimlys_core::observation::{Observation, ObservationStatus};
use mimlys_core::policy::Recovery;
use mimlys_core::spi::{Persistence, PersistenceError, PersistenceRead};
use mimlys_core::step::{Budgets, Step, StepOutcome};
use mimlys_core::task::{
    HaltReason, PendingItem, Task, TaskInput, TaskState, TaskVersions, WaitKind,
};
use serde_json::json;

pub(crate) struct Fx {
    pub(crate) ids: ScriptedIds,
    clock: FakeClock,
}

impl Fx {
    pub(crate) fn new() -> Self {
        Self {
            ids: ScriptedIds::default(),
            clock: FakeClock::default(),
        }
    }

    pub(crate) fn now(&self) -> Instant {
        self.clock.now()
    }

    pub(crate) fn event(&self, task: &Task, event: Event) -> EventRecord {
        EventRecord {
            event_id: EventId::new(&self.ids),
            at: self.now(),
            actor: Actor::Runtime,
            task_id: Some(task.task_id),
            step_id: None,
            action_id: None,
            policy_hash: hash_bytes(b"policy"),
            event,
        }
    }

    pub(crate) fn task(&self) -> Task {
        Task::create(
            mimlys_core::task::TaskSpec {
                task_id: TaskId::new(&self.ids),
                goal: "fix the failing test".into(),
                deadline_ms: LIM_T5_DEADLINE_MS,
                budgets: Budgets::default(),
                profile: "workspace".into(),
                versions: TaskVersions {
                    policy_hash: hash_bytes(b"policy"),
                    decision_schema_version: 1,
                    event_schema_version: mimlys_core::event::EVENT_SCHEMA_VERSION,
                    action_schema_version: mimlys_core::action::ACTION_SCHEMA_VERSION,
                    tool_definition_hashes: Default::default(),
                    runtime_version: "0.0.0-test".into(),
                    backend: None,
                    tool_imports: Default::default(),
                },
                scope: mimlys_core::task::ScopeSpec {
                    root: "/project".into(),
                    include: vec![],
                },
            },
            self.now(),
        )
        .unwrap()
    }

    fn grant(&self, task: &Task, tool: &str) -> Grant {
        Grant::issue(
            GrantId::new(&self.ids),
            task.task_id,
            GrantKind::Tool { name: tool.into() },
            Actor::Runtime,
            self.now(),
            None,
        )
    }

    fn action(
        &self,
        task: &Task,
        step: &Step,
        kind: EffectKind,
        approval_required: bool,
    ) -> Action {
        Action::propose(
            Proposal {
                action_id: ActionId::new(&self.ids),
                task_id: task.task_id,
                step_id: step.step_id,
                tool: "write_file".into(),
                arguments: json!({"path": "a.txt", "content": "x"}),
                grant_set_hash: hash_bytes(b"grants"),
                effect: EffectView {
                    kind,
                    reversible: kind != EffectKind::ExternalWrite,
                    idempotent: false,
                },
                approval_required,
                deadline_ms: 30_000,
            },
            self.now(),
        )
    }
}

/// The eight lines of ED §16.2 for one action, stopping after `upto` lines (0 = create only).
/// Returns the task and action as they stand.
pub(crate) fn sequence(
    store: &dyn Persistence,
    fx: &Fx,
    upto: usize,
    with_approval: bool,
) -> (Task, Option<Action>) {
    let task = fx.task();
    let grants = vec![fx.grant(&task, "write_file")];
    store
        .create_task(&task, &grants, &fx.event(&task, Event::TaskCreated))
        .unwrap();
    let task = task
        .transition(
            TaskInput::Resume {
                extend: None,
                ceiling: mimlys_core::task::ExtensionCeiling {
                    max_steps: 200,
                    max_deadline: task.deadline,
                },
            },
            fx.now(),
        )
        .unwrap();
    store
        .record_task_transition(
            &task,
            &fx.event(
                &task,
                Event::TaskStateChanged {
                    from: "created".into(),
                    to: "running".into(),
                    reason: None,
                },
            ),
        )
        .unwrap();
    if upto == 0 {
        return (task, None);
    }
    let mut step = Step::start(StepId::new(&fx.ids), task.task_id, 0, fx.now());
    store
        .record_step_started(&step, &fx.event(&task, Event::StepStarted))
        .unwrap();
    let action = fx.action(&task, &step, EffectKind::WorkspaceWrite, with_approval);
    step.action_id = Some(action.action_id);
    store
        .record_proposed(&step, &action, &fx.event(&task, Event::ActionProposed))
        .unwrap(); // 1
    if upto == 1 {
        return (task, Some(action));
    }
    let (task, action) = if with_approval {
        let action = action
            .transition(ActionInput::RequireApproval, fx.now())
            .unwrap();
        store
            .record_validation(
                &step,
                Some(&action),
                &fx.event(
                    &task,
                    Event::DecisionReceived {
                        hash: hash_bytes(b"d"),
                        transport: Default::default(),
                        text_chars: None,
                    },
                ),
            )
            .unwrap(); // 2
        if upto == 2 {
            return (task, Some(action));
        }
        let waiting = task
            .transition(
                TaskInput::NeedsUser(PendingItem::Approval {
                    action_id: action.action_id,
                }),
                fx.now(),
            )
            .unwrap();
        let mut approval = ApprovalRequest::new(
            ApprovalId::new(&fx.ids),
            task.task_id,
            action.action_id,
            action.content().action_hash().clone(),
            hash_bytes(b"grants"),
            Rendered { lines: vec![] },
            None,
            fx.now(),
            None,
        );
        store
            .record_approval_request(
                &waiting,
                &action,
                &approval,
                &fx.event(
                    &waiting,
                    Event::ApprovalRequested {
                        approval_id: approval.approval_id,
                    },
                ),
            )
            .unwrap(); // 3a
        if upto == 3 {
            return (waiting, Some(action));
        }
        approval
            .decide(UserDecision::Approve, Actor::Runtime, fx.now())
            .unwrap();
        let running_task = waiting
            .transition(TaskInput::UserDecided, fx.now())
            .unwrap();
        store
            .record_approval_decision(
                &running_task,
                &approval,
                &fx.event(
                    &running_task,
                    Event::ApprovalDecided {
                        approval_id: approval.approval_id,
                        decision: ApprovalDecision::Approved,
                    },
                ),
            )
            .unwrap(); // 3b
        approval
            .consume(action.content().action_hash(), &hash_bytes(b"grants"))
            .unwrap();
        store
            .record_approval_decision(
                &running_task,
                &approval,
                &fx.event(
                    &running_task,
                    Event::ApprovalDecided {
                        approval_id: approval.approval_id,
                        decision: ApprovalDecision::Approved,
                    },
                ),
            )
            .unwrap();
        (running_task, action)
    } else {
        store
            .record_validation(
                &step,
                Some(&action),
                &fx.event(
                    &task,
                    Event::DecisionReceived {
                        hash: hash_bytes(b"d"),
                        transport: Default::default(),
                        text_chars: None,
                    },
                ),
            )
            .unwrap(); // 2
        if upto <= 3 {
            return (task, Some(action));
        }
        (task, action)
    };
    let action = action.transition(ActionInput::Authorize, fx.now()).unwrap();
    store
        .record_authorized(&action, None, &fx.event(&task, Event::ActionAuthorized))
        .unwrap(); // 4
    if upto == 4 {
        return (task, Some(action));
    }
    let action = action.transition(ActionInput::Start, fx.now()).unwrap();
    store
        .record_running(&action, &fx.event(&task, Event::ActionStarted))
        .unwrap(); // 5
    if upto == 5 {
        return (task, Some(action));
    }
    let execution = Execution {
        execution_id: ExecutionId::new(&fx.ids),
        action_id: action.action_id,
        attempt: 1,
        started_at: fx.now(),
        ended_at: fx.now(),
        termination: Termination::Exited { exit_code: 0 },
        snapshot_before: None,
        snapshot_after: None,
        stdout_blob: None,
        stderr_blob: None,
        metrics: ExecutionMetrics::default(),
    };
    let observation = Observation {
        observation_id: ObservationId::new(&fx.ids),
        action_id: action.action_id,
        tool: "write_file".into(),
        status: ObservationStatus::Succeeded,
        cause: None,
        rolled_back: false,
        payload: json!({"bytes_written": 1}),
        stdout: String::new(),
        stderr: String::new(),
        notices: vec![],
        metrics: ExecutionMetrics::default(),
        created_at: fx.now(),
    };
    store
        .record_execution(
            &execution,
            &observation,
            &fx.event(
                &task,
                Event::ExecutionFinished {
                    attempt: 1,
                    termination: "exited".into(),
                },
            ),
        )
        .unwrap(); // 6
    if upto == 6 {
        return (task, Some(action));
    }
    let action = action.transition(ActionInput::Succeed, fx.now()).unwrap();
    store
        .record_action_terminal(
            &action,
            &fx.event(
                &task,
                Event::ActionFinished {
                    status: "succeeded".into(),
                    cause: None,
                },
            ),
        )
        .unwrap(); // 7
    if upto == 7 {
        return (task, Some(action));
    }
    step.finish(StepOutcome::Continued, fx.now());
    let mut task = task;
    task.counters.steps = 1;
    store
        .record_step_outcome(
            &step,
            &task,
            &fx.event(
                &task,
                Event::StepFinished {
                    outcome: StepOutcome::Continued,
                },
            ),
        )
        .unwrap(); // 8
    (task, Some(action))
}

/// REQ-PERS-001 / LIM-P1 / REQ-SEC-008: WAL, synchronous FULL, schema version 1, owner-only files, lock refusal, reopen.
#[test]
fn req_pers_001_open_pragmas_permissions_lock() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let store = open(&data).unwrap();
    assert_eq!(store.pragmas().unwrap(), ("wal".to_owned(), 2), "FULL is 2");
    assert_eq!(store.schema_version().unwrap(), 2);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&data).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(data.join("mimlys.db"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::metadata(data.join("blobs"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }
    let started = std::time::Instant::now();
    assert_eq!(
        open(&data).err(),
        Some(PersistenceError::Locked),
        "a second runtime is refused"
    );
    assert!(started.elapsed() >= std::time::Duration::from_millis(LIM_O3_LOCK_WAIT_MS - 50));
    drop(store);
    let reopened = open(&data).unwrap();
    assert_eq!(reopened.schema_version().unwrap(), 2);
}

/// REQ-PERS-002: a newer schema is refused naming both versions.
#[test]
fn req_pers_002_newer_schema_refused() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    {
        let store = open(&data).unwrap();
        store
            .with_conn(|c| {
                c.execute("UPDATE schema_version SET version = 99", [])
                    .unwrap()
            })
            .unwrap();
    }
    assert_eq!(
        open(&data).err(),
        Some(PersistenceError::NewerSchema {
            found: 99,
            supported: 2
        })
    );
}

/// REQ-PERS-003 / REQ-PERS-004: the eight lines are eight transactions, each durable across a reopen, in order.
#[test]
fn req_pers_003_004_eight_lines_are_eight_durable_transactions() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let fx = Fx::new();
    for upto in 0..=8 {
        let data = data.join(format!("u{upto}"));
        let (task, action) = {
            let store = open(&data).unwrap();
            sequence(&store, &fx, upto, false)
        };
        let store = open(&data).unwrap();
        let loaded = store.load_task(task.task_id).unwrap().unwrap();
        assert_eq!(loaded.state, task.state, "upto {upto}");
        if let Some(action) = action {
            let actions = store.load_actions(task.task_id).unwrap();
            assert_eq!(actions.len(), 1);
            assert_eq!(actions[0].status, action.status, "upto {upto}");
        }
        let events = store.events_since(0, None, 100).unwrap();
        // create + start + (step_started + lines up to upto)
        let expected = 2 + if upto == 0 {
            0
        } else {
            1 + upto.min(8) - if upto >= 3 { 1 } else { 0 }
        };
        assert_eq!(
            events.len(),
            expected,
            "events after upto {upto}: {:?}",
            events
                .iter()
                .map(|e| e.record.event.name())
                .collect::<Vec<_>>()
        );
        assert!(events.windows(2).all(|w| w[0].position < w[1].position));
        integrity::check(&store, fx.now()).unwrap();
    }
    // Transaction count: every method is exactly one transaction.
    let data = data.join("count");
    let store = open(&data).unwrap();
    sequence(&store, &fx, 8, false);
    // create_task, task_transition, step_started, lines 1, 2, 4, 5, 6, 7, 8 (no approval) = 10 transactions
    assert_eq!(
        store.transactions.load(std::sync::atomic::Ordering::SeqCst),
        10
    );
}

/// REQ-PERS-004: the write ordering is enforced against the stored status.
#[test]
fn req_pers_004_ordering_violations_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(&dir.path().join("data")).unwrap();
    let fx = Fx::new();
    let (task, action) = sequence(&store, &fx, 1, false);
    let action = action.unwrap();
    let authorized = action
        .clone()
        .transition(ActionInput::Authorize, fx.now())
        .unwrap();
    let running = authorized.transition(ActionInput::Start, fx.now()).unwrap();
    // running before authorized
    assert!(matches!(
        store.record_running(&running, &fx.event(&task, Event::ActionStarted)),
        Err(PersistenceError::Ordering { .. })
    ));
    // execution before running
    let execution = Execution {
        execution_id: ExecutionId::new(&fx.ids),
        action_id: action.action_id,
        attempt: 1,
        started_at: fx.now(),
        ended_at: fx.now(),
        termination: Termination::Exited { exit_code: 0 },
        snapshot_before: None,
        snapshot_after: None,
        stdout_blob: None,
        stderr_blob: None,
        metrics: ExecutionMetrics::default(),
    };
    let observation = Observation {
        observation_id: ObservationId::new(&fx.ids),
        action_id: action.action_id,
        tool: "write_file".into(),
        status: ObservationStatus::Succeeded,
        cause: None,
        rolled_back: false,
        payload: json!({}),
        stdout: String::new(),
        stderr: String::new(),
        notices: vec![],
        metrics: ExecutionMetrics::default(),
        created_at: fx.now(),
    };
    assert!(matches!(
        store.record_execution(
            &execution,
            &observation,
            &fx.event(&task, Event::ActionStarted)
        ),
        Err(PersistenceError::Ordering { .. })
    ));
    // a non-terminal status as terminal
    assert!(matches!(
        store.record_action_terminal(&authorized, &fx.event(&task, Event::ActionStarted)),
        Err(PersistenceError::Ordering { .. })
    ));
    // a refused write leaves no trace
    assert_eq!(
        store.load_actions(task.task_id).unwrap()[0].status,
        action.status
    );
}

/// REQ-PERS-005: a blob is on disk before its row, bounded by LIM-S4; orphans are detectable.
#[test]
fn req_pers_005_blobs() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let store = open(&data).unwrap();
    let fx = Fx::new();
    let task = fx.task();
    let id = store.put_blob(task.task_id, b"hello").unwrap();
    assert_eq!(store.get_blob(id).unwrap(), b"hello");
    assert!(data.join("blobs").join(id.to_string()).exists());
    assert!(matches!(
        store.put_blob(task.task_id, &vec![0u8; LIM_S4_BLOB_BYTES + 1]),
        Err(PersistenceError::Storage { .. })
    ));
    std::fs::write(data.join("blobs").join("orphan"), b"x").unwrap();
    assert_eq!(store.orphan_blobs().unwrap(), vec!["orphan".to_owned()]);
    assert!(matches!(
        store.get_blob(mimlys_core::ids::BlobId::new(&fx.ids)),
        Err(PersistenceError::NotFound { .. })
    ));
}

/// REQ-PERS-006: each consistency rule is detected and named.
#[test]
fn req_pers_006_integrity_rules() {
    let fx = Fx::new();
    let violate = |sql: &str| {
        let dir = tempfile::tempdir().unwrap();
        let store = open(&dir.path().join("data")).unwrap();
        sequence(&store, &fx, 8, true);
        integrity::check(&store, fx.now()).unwrap();
        store.with_conn(|c| c.execute_batch(sql).unwrap()).unwrap();
        match integrity::check(&store, fx.now()) {
            Err(PersistenceError::Integrity { rule, .. }) => rule,
            other => panic!("expected a violation, got {other:?}"),
        }
    };
    assert_eq!(
        violate("UPDATE tasks SET state = 'completed'; UPDATE steps SET outcome = NULL;"),
        "CONS-01"
    );
    assert_eq!(
        violate("UPDATE tasks SET state = 'completed'; UPDATE actions SET status = 'running';"),
        "CONS-02"
    );
    assert_eq!(
        violate("UPDATE approvals SET status = 'approved';"),
        "CONS-03"
    );
    assert_eq!(
        violate("UPDATE approvals SET action_hash = 'ffff';"),
        "CONS-05"
    );
    assert_eq!(
        violate(
            "UPDATE actions SET status = 'authorized'; UPDATE approvals SET status = 'approved';"
        ),
        "CONS-06"
    );
    assert_eq!(
        violate("UPDATE actions SET status = 'teleported';"),
        "CONS-07"
    );
    assert_eq!(
        violate("UPDATE tasks SET record = json_set(record, '$.counters.steps', 999);"),
        "CONS-08"
    );
}

/// REQ-PERS-007 / REQ-PERS-008: recovery of a running task per effect, and expiry of waiting items.
/// INV-12 (external design §21.2): a restart never turns an unfinished
/// action into a success. A write that was running when the process ended is
/// `unknown` after recovery, never `succeeded`; nothing is left running; the
/// task is not resumed past it (no snapshot) and a second recovery changes
/// nothing.
#[test]
fn req_pers_007_008_recovery() {
    let fx = Fx::new();
    let ctx = |auto_resume: bool| RecoveryContext {
        ids: &fx.ids,
        now: fx.now(),
        policy_hash: hash_bytes(b"policy"),
        recovery: Recovery { auto_resume },
    };
    // Running with no action in flight: halted(recovered); with auto_resume: running.
    let dir = tempfile::tempdir().unwrap();
    let store = open(&dir.path().join("data")).unwrap();
    let (task, _) = sequence(&store, &fx, 0, false);
    let out = recover_all(&store, &ctx(false), &fake_workspaces()).unwrap();
    assert_eq!(out[0].disposition, RecoveryDisposition::Clean);
    assert_eq!(
        store.load_task(task.task_id).unwrap().unwrap().state,
        TaskState::Halted {
            halt_reason: HaltReason::Recovered
        }
    );
    let dir = tempfile::tempdir().unwrap();
    let store = open(&dir.path().join("data")).unwrap();
    let (task, _) = sequence(&store, &fx, 0, false);
    recover_all(&store, &ctx(true), &fake_workspaces()).unwrap();
    assert_eq!(
        store.load_task(task.task_id).unwrap().unwrap().state,
        TaskState::Running
    );
    // A reversible write in flight: action unknown, task halted(recovered), no re-execution.
    let dir = tempfile::tempdir().unwrap();
    let store = open(&dir.path().join("data")).unwrap();
    let (task, action) = sequence(&store, &fx, 5, false);
    let out = recover_all(&store, &ctx(true), &fake_workspaces()).unwrap();
    assert_eq!(out[0].disposition, RecoveryDisposition::RolledBack);
    assert_eq!(
        store.load_task(task.task_id).unwrap().unwrap().state,
        TaskState::Halted {
            halt_reason: HaltReason::Recovered
        },
        "no snapshot in M1, so no auto-resume"
    );
    assert_eq!(
        store.load_actions(task.task_id).unwrap()[0].status,
        mimlys_core::action::ActionStatus::Unknown
    );
    assert!(store.load_running_action(task.task_id).unwrap().is_none());
    let _ = action;
    // Idempotent: a second recovery finds nothing running.
    assert!(
        recover_all(&store, &ctx(true), &fake_workspaces())
            .unwrap()
            .is_empty()
    );
    integrity::check(&store, fx.now()).unwrap();
    // An expired approval halts the task.
    let dir = tempfile::tempdir().unwrap();
    let store = open(&dir.path().join("data")).unwrap();
    let (waiting, _) = sequence(&store, &fx, 3, true);
    assert!(matches!(
        waiting.state,
        TaskState::WaitingUser {
            wait_kind: WaitKind::Approval
        }
    ));
    let mut approval = store
        .load_pending_approval(waiting.task_id)
        .unwrap()
        .unwrap();
    approval.expires_at = Some(fx.now());
    store
        .record_approval_decision(
            &waiting,
            &approval,
            &fx.event(
                &waiting,
                Event::ApprovalRequested {
                    approval_id: approval.approval_id,
                },
            ),
        )
        .unwrap();
    recover_all(&store, &ctx(false), &fake_workspaces()).unwrap();
    assert_eq!(
        store.load_task(waiting.task_id).unwrap().unwrap().state,
        TaskState::Halted {
            halt_reason: HaltReason::ApprovalExpired
        }
    );
    assert!(
        store
            .load_pending_approval(waiting.task_id)
            .unwrap()
            .is_none()
    );
    let names: Vec<String> = store
        .events_since(0, Some(waiting.task_id), 100)
        .unwrap()
        .iter()
        .map(|e| e.record.event.name().to_owned())
        .collect();
    assert!(names.contains(&"approval.decided".to_owned()));
    assert_eq!(names.last().map(String::as_str), Some("task.state_changed"));
}

/// REQ-OBS-002 / REQ-PERS-012: events stream in order per task; versions are recorded on the task.
#[test]
fn req_obs_002_events_and_versions() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(&dir.path().join("data")).unwrap();
    let fx = Fx::new();
    let (task, _) = sequence(&store, &fx, 8, false);
    let all = store.events_since(0, Some(task.task_id), 100).unwrap();
    let names: Vec<&str> = all.iter().map(|e| e.record.event.name()).collect();
    assert_eq!(
        names,
        [
            "task.created",
            "task.state_changed",
            "step.started",
            "action.proposed",
            "decision.received",
            "action.authorized",
            "action.started",
            "execution.finished",
            "action.finished",
            "step.finished"
        ]
    );
    let after = store
        .events_since(all[4].position, Some(task.task_id), 2)
        .unwrap();
    assert_eq!(after.len(), 2);
    assert_eq!(after[0].record.event.name(), "action.authorized");
    let summaries = store.list_tasks().unwrap();
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].state, "running");
    let loaded = store.load_task(task.task_id).unwrap().unwrap();
    assert_eq!(loaded.versions.runtime_version, "0.0.0-test");
    assert_eq!(loaded.versions.decision_schema_version, 1);
    assert_eq!(store.load_steps(task.task_id).unwrap().len(), 1);
    assert_eq!(store.load_observations(task.task_id).unwrap().len(), 1);
    assert_eq!(store.load_grants(task.task_id).unwrap().len(), 1);
}

/// REQ-TEST-007: crash injection at every point, before and after the commit; the store recovers to a consistent state.
#[cfg(feature = "crash-injection")]
#[test]
fn req_test_007_crash_at_every_point_recovers() {
    let exe = std::env::current_exe().unwrap();
    for point in crate::crash::POINTS {
        for phase in ["before", "after"] {
            let dir = tempfile::tempdir().unwrap();
            let data = dir.path().join("data");
            let status = std::process::Command::new(&exe)
                .args([
                    "crash_child_sequence",
                    "--exact",
                    "--nocapture",
                    "--test-threads=1",
                ])
                .env("MIMLYS_CRASH_AT", format!("{point}.{phase}"))
                .env("MIMLYS_CRASH_DIR", &data)
                .output()
                .unwrap();
            let fx = Fx::new();
            let store = open(&data).unwrap_or_else(|e| panic!("reopen after {point}.{phase}: {e}"));
            integrity::check(&store, fx.now())
                .unwrap_or_else(|e| panic!("integrity after {point}.{phase}: {e}"));
            let ctx = RecoveryContext {
                ids: &fx.ids,
                now: fx.now(),
                policy_hash: hash_bytes(b"policy"),
                recovery: Recovery { auto_resume: false },
            };
            recover_all(&store, &ctx, &fake_workspaces()).unwrap();
            integrity::check(&store, fx.now()).unwrap();
            assert!(
                store.tasks_in_state("running").unwrap().is_empty(),
                "a task is still running after recovery at {point}.{phase}"
            );
            for summary in store.list_tasks().unwrap() {
                assert!(
                    store
                        .load_running_action(summary.task_id)
                        .unwrap()
                        .is_none(),
                    "{point}.{phase}"
                );
            }
            let _ = status;
        }
    }
}

/// The child of the crash test: runs the full sequence (with approval and a blob) under `MIMLYS_CRASH_AT`.
#[cfg(feature = "crash-injection")]
#[test]
fn crash_child_sequence() {
    let Ok(dir) = std::env::var("MIMLYS_CRASH_DIR") else {
        return;
    };
    let store = open(std::path::Path::new(&dir)).unwrap();
    let fx = Fx::new();
    let (task, _) = sequence(&store, &fx, 8, true);
    store.put_blob(task.task_id, b"out").unwrap();
    // M3 points: a snapshot (files first, then the row) and a deletion (rows first, then files).
    {
        let files = crate::snapshots::SnapshotFiles::open(std::path::Path::new(&dir)).unwrap();
        let content = b"snapshot content";
        let hash = hash_bytes(content);
        let manifest =
            mimlys_core::snapshot::Manifest::new(vec![mimlys_core::snapshot::Entry::File {
                path: "f".into(),
                executable: false,
                size: content.len() as u64,
                hash: hash.clone(),
            }]);
        let id = mimlys_core::ids::SnapshotId::new(&fx.ids);
        files.begin(id, std::slice::from_ref(&hash)).unwrap();
        files.put_object(&hash, &content[..]).unwrap();
        let manifest_hash = files.put_manifest(id, &manifest).unwrap();
        let record = mimlys_core::snapshot::SnapshotRecord {
            snapshot_id: id,
            task_id: task.task_id,
            reason: mimlys_core::snapshot::SnapshotReason::Base,
            manifest_hash,
            bytes: content.len() as u64,
            files: 1,
            created_at: fx.now(),
            action_id: None,
            project_root: None,
        };
        store
            .record_snapshot(
                &record,
                &[(hash, content.len() as u64)],
                &fx.event(
                    &task,
                    Event::WorkspaceSnapshot {
                        snapshot_id: id,
                        reason: "base".into(),
                    },
                ),
            )
            .unwrap();
        files.finish(id).unwrap();
        let other = fx.task();
        store
            .create_task(&other, &[], &fx.event(&other, Event::TaskCreated))
            .unwrap();
        let rows = store
            .delete_task_data(
                other.task_id,
                false,
                &fx.event(&other, Event::TaskDeleted { audit: false }),
            )
            .unwrap();
        assert!(rows.snapshots.is_empty());
    }
    let paused = task.transition(TaskInput::PauseRequest, fx.now()).unwrap();
    store
        .record_task_transition(
            &paused,
            &fx.event(
                &paused,
                Event::TaskStateChanged {
                    from: "running".into(),
                    to: "running".into(),
                    reason: None,
                },
            ),
        )
        .unwrap();
}

mod snapshots_m3 {
    use super::*;
    use crate::snapshots::SnapshotFiles;
    use crate::sqlite::Store;
    use mimlys_core::ids::SnapshotId;
    use mimlys_core::snapshot::{Entry, Manifest, SnapshotReason, SnapshotRecord, WorkspaceState};

    fn snapshot_with(
        store: &Store,
        files: &SnapshotFiles,
        fx: &Fx,
        task: &Task,
        content: &[u8],
        reason: SnapshotReason,
    ) -> SnapshotRecord {
        let hash = hash_bytes(content);
        files.put_object(&hash, content).unwrap();
        let manifest = Manifest::new(vec![Entry::File {
            path: "f".into(),
            executable: false,
            size: content.len() as u64,
            hash: hash.clone(),
        }]);
        let id = SnapshotId::new(&fx.ids);
        files.begin(id, std::slice::from_ref(&hash)).unwrap();
        let manifest_hash = files.put_manifest(id, &manifest).unwrap();
        let record = SnapshotRecord {
            snapshot_id: id,
            task_id: task.task_id,
            reason,
            manifest_hash,
            bytes: content.len() as u64,
            files: 1,
            created_at: fx.now(),
            action_id: None,
            project_root: None,
        };
        store
            .record_snapshot(
                &record,
                &[(hash, content.len() as u64)],
                &fx.event(
                    task,
                    Event::WorkspaceSnapshot {
                        snapshot_id: id,
                        reason: reason.name().to_owned(),
                    },
                ),
            )
            .unwrap();
        files.finish(id).unwrap();
        record
    }

    /// REQ-WS-004 / REQ-PERS-009 / CONS-04: snapshot rows reference durable
    /// manifests; object counts follow the manifests; integrity checks both.
    #[test]
    fn req_pers_009_snapshot_rows_and_counts() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        let store = open(&data).unwrap();
        let files = SnapshotFiles::open(&data).unwrap();
        let fx = Fx::new();
        let (task, _) = sequence(&store, &fx, 0, false);
        let a = snapshot_with(&store, &files, &fx, &task, b"same", SnapshotReason::Base);
        let b = snapshot_with(&store, &files, &fx, &task, b"same", SnapshotReason::After);
        let c = snapshot_with(&store, &files, &fx, &task, b"other", SnapshotReason::After);
        assert_eq!(store.load_snapshots(task.task_id).unwrap().len(), 3);
        assert_eq!(
            store.load_snapshot(b.snapshot_id).unwrap().unwrap().reason,
            SnapshotReason::After
        );
        let refs = store
            .object_references(&[hash_bytes(b"same"), hash_bytes(b"other")])
            .unwrap();
        assert_eq!(refs[0].2, 2, "two manifests reference `same`");
        assert_eq!(refs[1].2, 1);
        integrity::check(&store, fx.now()).unwrap();
        store
            .record_workspace_state(task.task_id, WorkspaceState::Active, None)
            .unwrap();
        assert_eq!(
            store.load_workspace_state(task.task_id).unwrap(),
            Some(WorkspaceState::Active)
        );
        // A tampered manifest fails CONS-04.
        let path = data
            .join("snapshots/manifests")
            .join(format!("{}.json", a.snapshot_id));
        let original = std::fs::read(&path).unwrap();
        std::fs::write(&path, b"{\"entries\":[]}").unwrap();
        let err = integrity::check(&store, fx.now()).unwrap_err();
        assert!(err.to_string().contains("CONS-04"), "{err}");
        std::fs::write(&path, original).unwrap();
        integrity::check(&store, fx.now()).unwrap();
        // A wrong count fails CONS-04.
        let _ = store
            .with_conn(|conn| {
                conn.execute(
                    "UPDATE objects SET refs = 5 WHERE hash = ?1",
                    [hash_bytes(b"other").as_str()],
                )
                .map_err(|_| PersistenceError::Storage { detail: "x".into() })?;
                Ok::<(), PersistenceError>(())
            })
            .unwrap();
        assert!(
            integrity::check(&store, fx.now())
                .unwrap_err()
                .to_string()
                .contains("CONS-04")
        );
        let _ = c;
    }

    /// INV-08 (external design §21.2): the database references only
    /// materialized snapshots. A snapshot row is written after its manifest
    /// and objects are durable; the consistency check refuses a row whose
    /// manifest file is missing, and passes once the manifest is back.
    #[test]
    fn inv_08_a_snapshot_row_needs_its_materialized_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        let store = open(&data).unwrap();
        let files = SnapshotFiles::open(&data).unwrap();
        let fx = Fx::new();
        let (task, _) = sequence(&store, &fx, 0, false);
        let snap = snapshot_with(&store, &files, &fx, &task, b"content", SnapshotReason::Base);
        integrity::check(&store, fx.now()).unwrap();
        let manifest = data
            .join("snapshots/manifests")
            .join(format!("{}.json", snap.snapshot_id));
        assert!(manifest.is_file(), "the manifest was durable with the row");
        let saved = std::fs::read(&manifest).unwrap();
        let mut writable = std::fs::metadata(&manifest).unwrap().permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        writable.set_readonly(false);
        std::fs::set_permissions(&manifest, writable).unwrap();
        std::fs::remove_file(&manifest).unwrap();
        let err = integrity::check(&store, fx.now()).unwrap_err();
        assert!(err.to_string().contains("CONS-04"), "{err}");
        assert!(
            err.to_string().contains("no manifest"),
            "the row references a snapshot that is not materialized: {err}"
        );
        std::fs::write(&manifest, saved).unwrap();
        integrity::check(&store, fx.now()).unwrap();
    }

    /// RFC 006 §6: deleting a task's data releases its objects in the same
    /// transaction; shared objects survive; `audit` removes the records.
    /// REQ-WS-009 (`deleted`): the workspace state reads `deleted` after `delete_task_data`, and the task record stays unless the audit is asked for.
    /// REQ-WS-012 (reference counts in the same transaction as the rows): the objects released by the deleted task's rows are returned, an object another task still references stays with its count, the blob rows and snapshot rows go, the integrity check passes, and `audit` removes the records.
    #[test]
    fn rfc_006_delete_task_data() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        let store = open(&data).unwrap();
        let files = SnapshotFiles::open(&data).unwrap();
        let fx = Fx::new();
        let (task, _) = sequence(&store, &fx, 0, false);
        snapshot_with(&store, &files, &fx, &task, b"shared", SnapshotReason::Base);
        snapshot_with(&store, &files, &fx, &task, b"mine", SnapshotReason::After);
        let other = fx.task();
        store
            .create_task(&other, &[], &fx.event(&other, Event::TaskCreated))
            .unwrap();
        snapshot_with(&store, &files, &fx, &other, b"shared", SnapshotReason::Base);
        store.put_blob(task.task_id, b"blob").unwrap();
        assert_eq!(store.blob_bytes(task.task_id).unwrap(), 4);
        let rows = store
            .delete_task_data(
                task.task_id,
                false,
                &fx.event(&task, Event::TaskDeleted { audit: false }),
            )
            .unwrap();
        assert_eq!(
            rows.objects_at_zero,
            vec![hash_bytes(b"mine")],
            "the shared object stays referenced by the other task"
        );
        assert_eq!(rows.snapshots.len(), 2);
        assert_eq!(rows.blobs.len(), 1);
        assert!(store.load_snapshots(task.task_id).unwrap().is_empty());
        assert_eq!(
            store.load_workspace_state(task.task_id).unwrap(),
            Some(WorkspaceState::Deleted)
        );
        assert!(
            store.load_task(task.task_id).unwrap().is_some(),
            "the audit stays"
        );
        assert_eq!(
            store.object_references(&[hash_bytes(b"shared")]).unwrap()[0].2,
            1
        );
        for id in &rows.snapshots {
            files.remove_manifest(*id).unwrap();
        }
        for h in &rows.objects_at_zero {
            files.remove_object(h).unwrap();
        }
        integrity::check(&store, fx.now()).unwrap();
        store
            .delete_task_data(
                task.task_id,
                true,
                &fx.event(&task, Event::TaskDeleted { audit: true }),
            )
            .unwrap();
        assert!(
            store.load_task(task.task_id).unwrap().is_none(),
            "--audit removes the records"
        );
        assert!(
            store
                .events_since(0, Some(task.task_id), 100)
                .unwrap()
                .is_empty()
        );
        integrity::check(&store, fx.now()).unwrap();
    }
}

/// REQ-PERS-006 (handoff 009b): a store written by 0.8.0 (bundled SQLite of `rusqlite` 0.38, its
/// data in an uncheckpointed `-wal`) opens under the current `rusqlite`, keeps schema version 2,
/// passes the integrity check, lists its tasks and recovers.
#[test]
fn req_pers_006_a_store_written_by_0_8_0_opens_checks_lists_and_recovers() {
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/v0_8_0");
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    std::fs::create_dir_all(&data).unwrap();
    for name in ["mimlys.db", "mimlys.db-wal"] {
        std::fs::copy(fixture.join(name), data.join(name)).unwrap();
    }
    assert!(
        std::fs::metadata(data.join("mimlys.db-wal")).unwrap().len() > 100_000,
        "the fixture's data is in its -wal"
    );
    let fx = Fx::new();
    let store = open(&data).unwrap();
    assert_eq!(store.schema_version().unwrap(), 2, "SCHEMA_VERSION stays 2");
    assert_eq!(crate::SCHEMA_VERSION, 2);
    integrity::check(&store, fx.now()).unwrap();
    let mut tasks = store.list_tasks().unwrap();
    tasks.sort_by_key(|t| t.task_id.to_string());
    let ids: Vec<String> = tasks.iter().map(|t| t.task_id.to_string()).collect();
    assert_eq!(
        ids,
        [
            "00000000-0001-7000-8000-000000000001",
            "00000000-0015-7000-8000-000000000015"
        ]
    );
    for t in &tasks {
        assert_eq!(t.goal, "fix the failing test");
        assert_eq!(t.state, "running");
        assert!(store.load_task(t.task_id).unwrap().is_some());
    }
    // Recovery: the two tasks were running when the copy was taken. A restarted process
    // draws new identifiers; the scripted source starts above the ones in the fixture.
    let later_ids = ScriptedIds::starting_at(1_000_000);
    let ctx = RecoveryContext {
        ids: &later_ids,
        now: fx.now(),
        policy_hash: hash_bytes(b"policy"),
        recovery: Recovery { auto_resume: false },
    };
    let out = recover_all(&store, &ctx, &fake_workspaces()).unwrap();
    assert_eq!(out.len(), 2);
    for t in &tasks {
        assert_ne!(
            store.load_task(t.task_id).unwrap().unwrap().state,
            TaskState::Running,
            "a task that was running is not running after recovery"
        );
    }
    integrity::check(&store, fx.now()).unwrap();
}

/// REQ-PERS-006, REQ-PERS-009, REQ-SEC-012 (handoff 008c): a store written by 0.9.0
/// (`sha2` 0.10; its data in an uncheckpointed `-wal`, with two snapshot objects and a
/// manifest) opens, checks and lists under the current build, and its snapshot files
/// verify: the manifest against its recorded hash, every object against its name.
#[test]
fn req_pers_006_a_store_written_by_0_9_0_opens_and_verifies() {
    use mimlys_core::hash::Hash;
    use mimlys_core::ids::SnapshotId;
    use mimlys_core::snapshot::Entry;
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/v0_9_0");
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    for sub in ["snapshots/objects", "snapshots/manifests"] {
        std::fs::create_dir_all(data.join(sub)).unwrap();
        for entry in std::fs::read_dir(fixture.join(sub)).unwrap() {
            let entry = entry.unwrap();
            std::fs::copy(entry.path(), data.join(sub).join(entry.file_name())).unwrap();
        }
    }
    for name in ["mimlys.db", "mimlys.db-wal"] {
        std::fs::copy(fixture.join(name), data.join(name)).unwrap();
    }
    assert!(
        std::fs::metadata(data.join("mimlys.db-wal")).unwrap().len() > 100_000,
        "the fixture's data is in its -wal"
    );
    let fx = Fx::new();
    let store = open(&data).unwrap();
    assert_eq!(store.schema_version().unwrap(), 2);
    integrity::check(&store, fx.now()).unwrap();
    let tasks = store.list_tasks().unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(
        tasks[0].task_id.to_string(),
        "00000000-0001-7000-8000-000000000001"
    );
    assert_eq!(tasks[0].state, "running");
    assert!(store.load_task(tasks[0].task_id).unwrap().is_some());

    // The snapshot files verify with the hashes 0.9.0 wrote.
    let files = crate::snapshots::SnapshotFiles::open(&data).unwrap();
    let snapshot: SnapshotId =
        serde_json::from_str("\"00000000-000a-7000-8000-00000000000a\"").unwrap();
    let recorded =
        Hash::from_hex("bb5b91bc5cc8279c564abd7db9c54827ecbe2f428913685d9d97531253e11dd8").unwrap();
    let manifest = files.read_manifest(snapshot, &recorded).unwrap();
    assert_eq!(manifest.hash(), recorded, "the manifest's canonical hash");
    let mut checked = 0;
    for entry in &manifest.entries {
        if let Entry::File { hash, size, .. } = entry {
            let mut bytes = Vec::new();
            std::io::Read::read_to_end(&mut files.open_object(hash).unwrap(), &mut bytes).unwrap();
            assert_eq!(bytes.len() as u64, *size);
            assert_eq!(&hash_bytes(&bytes), hash, "an object is named by its hash");
            // Writing the object into an empty directory hashes it again, streaming, and
            // accepts only its name; a wrong name is refused.
            let elsewhere = tempfile::tempdir().unwrap();
            let fresh = crate::snapshots::SnapshotFiles::open(elsewhere.path()).unwrap();
            fresh.put_object(hash, &bytes[..]).unwrap();
            assert!(fresh.has_object(hash));
            assert!(
                fresh
                    .put_object(&hash_bytes(b"another name"), &bytes[..])
                    .is_err()
            );
            checked += 1;
        }
    }
    assert_eq!(checked, 2);
    // The names are the ones 0.9.0 gave them.
    assert!(
        files.has_object(
            &Hash::from_hex("f3ea8f2a2e6215bd4b5c03ca09e9cfc6e81652ecc7349fb5e82165edd04b989b")
                .unwrap()
        )
    );
    assert!(
        files.has_object(
            &Hash::from_hex("9dc177c2fde29dea8e7c29f7ddf147b7c449c99d049c62f3aac0a5933ecf76a3")
                .unwrap()
        )
    );
}

/// REQ-MAIN-002, REQ-PERS-012 (handoff 004b): the tasks of a store written before the event and
/// action schema versions were recorded read with the values of their era. The stored records
/// are 0.9.0's fixture, whose `versions` hold neither value; its tasks were made by a test
/// helper and say `0.0.0-test`, which is no release and reads as the current schemas. The
/// release a record names is then rewritten in a copy of the database (the only thing changed),
/// and the same records read with that release's values.
#[test]
fn req_main_002_the_tasks_of_an_old_store_read_with_the_values_of_their_era() {
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/v0_9_0");
    let read = |release: Option<&str>| -> Vec<(u32, u32)> {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        for sub in ["snapshots/objects", "snapshots/manifests"] {
            std::fs::create_dir_all(data.join(sub)).unwrap();
        }
        for name in ["mimlys.db", "mimlys.db-wal"] {
            std::fs::copy(fixture.join(name), data.join(name)).unwrap();
        }
        let raw = rusqlite::Connection::open(data.join("mimlys.db")).unwrap();
        let before: String = raw
            .query_row("SELECT record FROM tasks LIMIT 1", [], |r| r.get(0))
            .unwrap();
        assert!(
            !before.contains("event_schema_version") && !before.contains("action_schema_version"),
            "the fixture predates both versions"
        );
        if let Some(release) = release {
            raw.execute(
                "UPDATE tasks SET record = json_set(record, '$.versions.runtime_version', ?1)",
                [release],
            )
            .unwrap();
        }
        drop(raw);
        let store = open(&data).unwrap();
        store
            .list_tasks()
            .unwrap()
            .iter()
            .map(|t| {
                let task = store.load_task(t.task_id).unwrap().unwrap();
                (
                    task.versions.event_schema_version,
                    task.versions.action_schema_version,
                )
            })
            .collect()
    };
    let current = (
        mimlys_core::event::EVENT_SCHEMA_VERSION,
        mimlys_core::action::ACTION_SCHEMA_VERSION,
    );
    assert_eq!(read(None), [current], "a test build reads as the current");
    assert_eq!(read(Some("0.9.0")), [(2, 2)]);
    assert_eq!(read(Some("0.8.0")), [(1, 2)]);
    assert_eq!(read(Some("0.7.1")), [(1, 1)]);
}
