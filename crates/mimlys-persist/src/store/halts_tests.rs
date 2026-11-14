//! The runtime over the real store (handoff 010e-3; REQ-OBS-001, REQ-PERS-008,
//! REQ-CLI-005): the halts decided inside `execute` and the action closed before
//! it began are in the store as the events and rows the commands read. The
//! fakes of `mimlys-core` stand for the model and the tools; the store is real,
//! so `metrics` counts what it counts in a run.

use mimlys_core::action::{ActionStatus, EffectKind, EffectView, HostFaultDetail};
use mimlys_core::clock::FakeClock;
use mimlys_core::ids::{ScriptedIds, TaskId};
use mimlys_core::observation::{Notice, ObservationStatus};
use mimlys_core::path::PlatformRules;
use mimlys_core::policy::Policy;
use mimlys_core::runtime::{CreateTask, Runtime, RuntimeDeps, Session};
use mimlys_core::spi::fake::{FakeToolProvider, FakeWorkspaces, ScriptedBackend};
use mimlys_core::spi::{ExecutionOutcome, PersistenceRead};
use mimlys_core::step::StepOutcome;
use mimlys_core::task::{HaltReason, ScopeSpec, TaskState};
use mimlys_core::tool::{Param, ParamSchema, ParamType, Provider, ResourceProfile, ToolDefinition};
use serde_json::json;
use std::sync::Arc;

/// A minimal executor for the fakes' futures: they never pend.
fn block_on<F: std::future::Future>(future: F) -> F::Output {
    use std::task::{Context, Poll, Waker};
    let mut cx = Context::from_waker(Waker::noop());
    let mut future = std::pin::pin!(future);
    loop {
        if let Poll::Ready(value) = future.as_mut().poll(&mut cx) {
            return value;
        }
    }
}

fn tool(name: &str, kind: EffectKind, idempotent: bool) -> ToolDefinition {
    ToolDefinition {
        name: name.into(),
        description: format!("{name}."),
        input: ParamSchema {
            params: vec![Param {
                name: "path".into(),
                ty: ParamType::WorkspacePath,
                optional: false,
                description: "file".into(),
            }],
        },
        output: None,
        effect: EffectView {
            kind,
            reversible: true,
            idempotent,
        },
        deadline_ms: 30_000,
        profile: ResourceProfile::Builtin,
        destination_arguments: vec![],
        provider: Provider::Builtin,
    }
}

struct Run {
    runtime: Runtime,
    store: Arc<crate::store::Store>,
    workspaces: Arc<FakeWorkspaces>,
    task: TaskId,
    _dir: tempfile::TempDir,
}

impl Run {
    /// A task over the real store whose model proposes `definition` once and
    /// whose tool ends as `outcome` says.
    fn new(definition: ToolDefinition, outcome: ExecutionOutcome) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(crate::sqlite::open(&dir.path().join("data")).unwrap());
        let name = definition.name.clone();
        let tools = Arc::new(FakeToolProvider::new(vec![definition]));
        tools.script(&name, outcome);
        let workspaces = Arc::new(FakeWorkspaces::new(tools));
        let mut policy = Policy::shipped();
        policy.ceiling.tools.push(name.clone());
        policy
            .profiles
            .get_mut("workspace")
            .unwrap()
            .tools
            .push(name.clone());
        let runtime = Runtime::new(RuntimeDeps {
            persistence: store.clone(),
            backend: Arc::new(ScriptedBackend::with_decisions(vec![
                json!({"decision": "act", "tool": name, "arguments": {"path": "a"}}),
            ])),
            workspaces: workspaces.clone(),
            policy,
            ids: Arc::new(ScriptedIds::default()),
            clock: Arc::new(FakeClock::default()),
            runtime_version: "0.1.0-test".into(),
            rules: PlatformRules::Posix,
            probe: mimlys_core::confine::Probe::unavailable("no backend in tests"),
            secrets: vec![],
            context_chars: mimlys_core::limits::LIM_C1_CONTEXT_CHARS,
        })
        .unwrap();
        let session = Session {
            id: "s1".into(),
            os_user: "tester".into(),
        };
        let task = runtime
            .create_task(
                &session,
                CreateTask {
                    goal: "g".into(),
                    scope: ScopeSpec {
                        root: "/home/u/project".into(),
                        include: vec![],
                    },
                    profile: None,
                    grants: vec![],
                    budgets: None,
                    deadline_ms: None,
                    completion_check: None,
                    hold: false,
                    from_task: None,
                    from_snapshot: None,
                },
            )
            .unwrap()
            .task_id;
        Self {
            runtime,
            store,
            workspaces,
            task,
            _dir: dir,
        }
    }

    fn step(&self) -> mimlys_core::runtime::StepResultView {
        block_on(self.runtime.run_step(self.task)).unwrap()
    }

    /// The `(to, reason)` of every `task.state_changed` of the task, in order.
    fn state_changes(&self) -> Vec<(String, Option<HaltReason>)> {
        let mut out = Vec::new();
        let mut position = 0;
        loop {
            let batch = self
                .store
                .events_since(position, Some(self.task), 1000)
                .unwrap();
            let Some(last) = batch.last() else {
                return out;
            };
            position = last.position;
            for e in batch {
                if let mimlys_core::event::Event::TaskStateChanged { to, reason, .. } =
                    e.record.event
                {
                    out.push((to, reason));
                }
            }
        }
    }

    fn halts(&self, reason: &str) -> u64 {
        self.store
            .metrics(Some(self.task))
            .unwrap()
            .halts_by_reason
            .get(reason)
            .copied()
            .unwrap_or(0)
    }
}

/// REQ-OBS-001 (handoff 010e-3, item 6): a halt decided inside `execute` is in
/// the store as a `task.state_changed` with its reason, and `metrics` counts it
/// in `halts_by_reason`: a disposition that halts (`tool_unavailable`), and an
/// unknown outcome (an external read that is not idempotent, ended by its
/// deadline). Each exactly once.
#[test]
fn req_obs_001_a_halt_decided_inside_execute_is_counted_by_the_metrics() {
    let run = Run::new(
        tool("a_read", EffectKind::WorkspaceRead, true),
        ExecutionOutcome::host_fault(HostFaultDetail::Unavailable, 1),
    );
    let result = run.step();
    assert_eq!(result.outcome, StepOutcome::Halted);
    assert_eq!(
        result.state,
        TaskState::Halted {
            halt_reason: HaltReason::ToolUnavailable
        }
    );
    let halts: Vec<_> = run
        .state_changes()
        .into_iter()
        .filter(|(to, _)| to == "halted")
        .collect();
    assert_eq!(
        halts,
        [("halted".to_owned(), Some(HaltReason::ToolUnavailable))]
    );
    assert_eq!(run.halts("tool_unavailable"), 1);

    let run = Run::new(
        tool("a_non_idempotent_read", EffectKind::ExternalRead, false),
        ExecutionOutcome::timed_out(1),
    );
    let result = run.step();
    assert_eq!(
        result.state,
        TaskState::Halted {
            halt_reason: HaltReason::UnknownOutcome
        }
    );
    assert_eq!(run.halts("unknown_outcome"), 1);
    let actions = run.store.load_actions(run.task).unwrap();
    assert_eq!(actions[0].status, ActionStatus::Unknown);
    let observations = run.store.load_observations(run.task).unwrap();
    assert_eq!(observations[0].status, ObservationStatus::Unknown);
}

/// REQ-CLI-005 (handoff 010e-3, item 2), over the real store: an action closed
/// `cancelled` before it began is one write: the row is `cancelled`, there is no
/// execution, the observation row says the stop came before it began, and the
/// events have no `action.started`.
#[test]
fn req_cli_005_an_action_closed_before_it_began_is_one_consistent_write() {
    let run = Run::new(
        tool("a_non_idempotent_read", EffectKind::ExternalRead, false),
        ExecutionOutcome::succeeded(json!({}), 1),
    );
    run.runtime.start_no_new_action(run.task);
    let result = run.step();
    assert_eq!(result.outcome, StepOutcome::Cancelled);
    let actions = run.store.load_actions(run.task).unwrap();
    assert_eq!(actions[0].status, ActionStatus::Cancelled);
    let observations = run.store.load_observations(run.task).unwrap();
    assert_eq!(observations.len(), 1);
    assert_eq!(observations[0].status, ObservationStatus::Cancelled);
    assert_eq!(observations[0].notices, [Notice::StoppedBeforeStart]);
    let metrics = run.store.metrics(Some(run.task)).unwrap();
    assert_eq!(metrics.events_by_name.get("action.started"), None);
    assert_eq!(metrics.events_by_name.get("execution.finished"), None);
    assert_eq!(metrics.actions_by_status.get("cancelled"), Some(&1));
    // The store refuses the same write twice: the action is no longer `authorized`.
    let again = mimlys_core::spi::Persistence::record_not_started(
        run.store.as_ref(),
        &actions[0],
        &observations[0],
        &mimlys_core::event::EventRecord {
            event_id: mimlys_core::ids::EventId::new(&ScriptedIds::starting_at(9000)),
            at: actions[0].created_at,
            actor: mimlys_core::event::Actor::Runtime,
            task_id: Some(run.task),
            step_id: None,
            action_id: Some(actions[0].action_id),
            policy_hash: mimlys_core::hash::hash_bytes(b"p"),
            event: mimlys_core::event::Event::ActionFinished {
                status: "cancelled".into(),
                cause: None,
            },
        },
    );
    assert!(again.is_err(), "ordering is enforced");
}

/// REQ-OBS-001 (handoff 010e-3, item 6): a failed restore after a failed write
/// leaves the outcome unknown and halts the task inside `execute`; the halt is a
/// `task.state_changed` with its reason, and `metrics` counts it.
#[test]
fn req_obs_001_a_failed_restore_is_counted_by_the_metrics() {
    let mut write = tool("a_workspace_write", EffectKind::WorkspaceWrite, false);
    write.effect.reversible = true;
    let run = Run::new(
        write,
        ExecutionOutcome::tool_error("rejected", json!({}), 1),
    );
    run.workspaces.fail_restores();
    let result = run.step();
    assert_eq!(
        result.state,
        TaskState::Halted {
            halt_reason: HaltReason::UnknownOutcome
        }
    );
    assert_eq!(run.halts("unknown_outcome"), 1);
    let last = run.state_changes().pop();
    assert_eq!(
        last,
        Some(("halted".to_owned(), Some(HaltReason::UnknownOutcome)))
    );
}
