//! Plan first over the real store (REQ-TASK-015; handoff 015b): the gate and the wait for the
//! plan are in the task's own records, so a restart, a recovery and an expiry keep them. Each
//! process is a runtime over a store opened afresh; the fakes of `mimlys-core` stand for the
//! model and the tools.

use super::*;
use crate::recovery::{RecoveryContext, recover_all};
use mimlys_core::action::{EffectKind, EffectView};
use mimlys_core::clock::{Clock, FakeClock};
use mimlys_core::event::ApprovalDecision;
use mimlys_core::ids::{ScriptedIds, TaskId};
use mimlys_core::path::PlatformRules;
use mimlys_core::policy::{Policy, Recovery};
use mimlys_core::rejection::{Detail, StateReason};
use mimlys_core::runtime::{CreateTask, Runtime, RuntimeDeps, Session};
use mimlys_core::spi::fake::{FakeToolProvider, FakeWorkspaces, ScriptedBackend};
use mimlys_core::spi::{ExecutionOutcome, PersistenceRead};
use mimlys_core::step::StepOutcome;
use mimlys_core::task::{HaltReason, PendingItem, ScopeSpec, TaskState, WaitKind};
use mimlys_core::tool::{Param, ParamSchema, ParamType, Provider, ResourceProfile, ToolDefinition};
use serde_json::{Value, json};
use std::path::Path;
use std::sync::Arc;

/// Every process of a test has ids of its own, as a real one has.
static PROCESSES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

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

fn read_file() -> ToolDefinition {
    ToolDefinition {
        name: "read_file".into(),
        description: "read_file.".into(),
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
            kind: EffectKind::WorkspaceRead,
            reversible: true,
            idempotent: true,
        },
        deadline_ms: 30_000,
        profile: ResourceProfile::Builtin,
        destination_arguments: vec![],
        provider: Provider::Builtin,
    }
}

fn session() -> Session {
    Session {
        id: "s1".into(),
        os_user: "tester".into(),
    }
}

/// One process: a store opened on `data`, and a runtime whose model says `decisions`.
struct Process {
    runtime: Runtime,
    store: Arc<Store>,
    tools: Arc<FakeToolProvider>,
}

impl Process {
    fn start(data: &Path, decisions: Vec<Value>) -> Self {
        let store = Arc::new(crate::sqlite::open(data).unwrap());
        let tools = Arc::new(FakeToolProvider::new(vec![read_file()]));
        tools.script("read_file", ExecutionOutcome::succeeded(json!({}), 1));
        let workspaces = Arc::new(FakeWorkspaces::new(tools.clone()));
        let runtime = Runtime::new(RuntimeDeps {
            persistence: store.clone(),
            backend: Arc::new(ScriptedBackend::with_decisions(decisions)),
            backend_name: "default".to_owned(),
            workspaces,
            policy: Policy::shipped(),
            ids: Arc::new(ScriptedIds::starting_at(
                1000 * (1 + PROCESSES.fetch_add(1, std::sync::atomic::Ordering::SeqCst)),
            )),
            clock: Arc::new(FakeClock::default()),
            runtime_version: "0.1.0-test".into(),
            rules: PlatformRules::Posix,
            probe: mimlys_core::confine::Probe::unavailable("no backend in tests"),
            secrets: vec![],
            context_chars: mimlys_core::limits::LIM_C1_CONTEXT_CHARS,
            output_bytes: mimlys_core::limits::LIM_S4_OUTPUT_BYTES as usize,
        })
        .unwrap();
        Self {
            runtime,
            store,
            tools,
        }
    }

    /// What a start does before any command: recovery (REQ-PERS-007), with the plan waits that
    /// expired halted.
    fn recover(&self, now: mimlys_core::clock::Instant) {
        let ids = ScriptedIds::starting_at(
            500_000 + 1000 * PROCESSES.fetch_add(1, std::sync::atomic::Ordering::SeqCst),
        );
        let workspaces = FakeWorkspaces::new(self.tools.clone());
        recover_all(
            &self.store,
            &RecoveryContext {
                ids: &ids,
                now,
                policy_hash: mimlys_core::hash::hash_bytes(b"policy"),
                recovery: Recovery { auto_resume: false },
            },
            &workspaces,
        )
        .unwrap();
    }

    fn step(&self, task: TaskId) -> mimlys_core::runtime::StepResultView {
        block_on(self.runtime.run_step(task)).unwrap()
    }

    fn task(&self, task: TaskId) -> mimlys_core::task::Task {
        self.store.load_task(task).unwrap().unwrap()
    }
}

fn plan(steps: &[&str]) -> Value {
    json!({"decision": "plan", "steps": steps})
}

fn act() -> Value {
    json!({"decision": "act", "tool": "read_file", "arguments": {"path": "a"}})
}

fn waiting_plan() -> TaskState {
    TaskState::WaitingUser {
        wait_kind: WaitKind::Plan,
    }
}

/// REQ-TASK-015: a restart while the task waits for its plan, a recovery, a rejection, a second
/// restart and an approval: the gate holds at each (the record's, not the process's), and the
/// approved version holds after the last restart.
#[test]
fn req_task_015_the_gate_and_the_approved_version_hold_across_restarts() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    // Process 1: the plan waits.
    let id = {
        let p = Process::start(&data, vec![plan(&["read a"])]);
        let id = p
            .runtime
            .create_task(
                &session(),
                CreateTask {
                    goal: "g".into(),
                    scope: ScopeSpec {
                        root: "/home/u/project".into(),
                        include: vec![],
                    },
                    profile: Some("read_only".into()),
                    grants: vec![],
                    budgets: None,
                    deadline_ms: None,
                    completion_check: None,
                    hold: false,
                    from_task: None,
                    from_snapshot: None,
                    plan_first: true,
                },
            )
            .unwrap()
            .task_id;
        assert_eq!(p.step(id).state, waiting_plan());
        id
    };
    // Process 2: a recovery leaves the wait as it is; the user rejects with a note; the model
    // proposes an `act`, which is still `plan_required`.
    {
        let p = Process::start(&data, vec![act()]);
        p.recover(mimlys_core::clock::FakeClock::default().now());
        let task = p.task(id);
        assert_eq!(task.state, waiting_plan());
        assert_eq!(task.pending, Some(PendingItem::Plan { version: 1 }));
        p.runtime
            .reject_action(&session(), id, Some("again".into()), Some(false))
            .unwrap();
        assert!(p.task(id).plan_required(), "the rejection kept the gate");
    }
    // Process 3: the process that rejected ended with the task running, so the start halts it
    // `recovered`, and the user resumes it. The gate is read from the record: an `act` is
    // rejected `plan_required`.
    {
        let p = Process::start(&data, vec![act(), plan(&["read a", "read b"])]);
        p.recover(mimlys_core::clock::FakeClock::default().now());
        assert!(matches!(
            p.task(id).state,
            TaskState::Halted {
                halt_reason: HaltReason::Recovered,
                ..
            }
        ));
        p.runtime.resume_task(&session(), id, None).unwrap();
        let result = p.step(id);
        assert_eq!(result.outcome, StepOutcome::Rejected);
        let steps = p.store.load_steps(id).unwrap();
        assert_eq!(
            steps.last().unwrap().rejection_detail,
            Some(Detail::State {
                reason: StateReason::PlanRequired
            })
        );
        assert!(p.store.load_actions(id).unwrap().is_empty());
        // The next plan waits again.
        assert_eq!(p.step(id).state, waiting_plan());
        assert_eq!(p.task(id).pending, Some(PendingItem::Plan { version: 2 }));
    }
    // Process 4: the user approves with a note.
    {
        let p = Process::start(&data, vec![]);
        p.runtime
            .approve_action(&session(), id, Some("go".into()))
            .unwrap();
        assert_eq!(p.task(id).plan_approved, Some(2));
    }
    // Process 5: the approved version holds, the gate is open, and the `act` runs.
    {
        let p = Process::start(&data, vec![act(), plan(&["x"])]);
        p.recover(mimlys_core::clock::FakeClock::default().now());
        let task = p.task(id);
        assert_eq!((task.plan_approved, task.plan_required()), (Some(2), false));
        p.runtime.resume_task(&session(), id, None).unwrap();
        assert_eq!(p.step(id).outcome, StepOutcome::Continued);
        assert_eq!(p.store.load_actions(id).unwrap().len(), 1);
        // And a later plan does not wait.
        assert_eq!(p.step(id).state, TaskState::Running);
        mimlys_core::newer::refuse_if_newer(p.store.as_ref(), id).unwrap();
    }
}

/// REQ-TASK-015, LIM-A1: a plan wait whose expiry has passed is found at the start and halts the
/// task `approval_expired`, as an approval does: the step ends halted with the decision recorded
/// as expired, the events say so, and a resume keeps the gate.
#[test]
fn req_task_015_an_expired_plan_wait_halts_approval_expired() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let id = {
        let p = Process::start(&data, vec![plan(&["read a"])]);
        let id = p
            .runtime
            .create_task(
                &session(),
                CreateTask {
                    goal: "g".into(),
                    scope: ScopeSpec {
                        root: "/home/u/project".into(),
                        include: vec![],
                    },
                    profile: Some("read_only".into()),
                    grants: vec![],
                    budgets: None,
                    deadline_ms: None,
                    completion_check: None,
                    hold: false,
                    from_task: None,
                    from_snapshot: None,
                    plan_first: true,
                },
            )
            .unwrap()
            .task_id;
        p.step(id);
        // The default is no expiry (LIM-A1); a wait that was given one is made so here.
        let now = mimlys_core::clock::FakeClock::default().now();
        let task = p.task(id);
        let mut step = p.store.load_steps(id).unwrap().remove(0);
        step.plan_wait.as_mut().unwrap().expires_at = Some(now);
        p.store.record_plan_step(&step, &task, None, &[]).unwrap();
        id
    };
    let p = Process::start(&data, vec![]);
    p.recover(mimlys_core::clock::FakeClock::default().now());
    let task = p.task(id);
    assert_eq!(
        task.state,
        TaskState::Halted {
            halt_reason: HaltReason::ApprovalExpired,
            halt_detail: None
        }
    );
    assert!(task.pending.is_none() && task.plan_required());
    let step = p.store.load_steps(id).unwrap().remove(0);
    assert_eq!(step.outcome, Some(StepOutcome::Halted));
    let decided = step.plan_wait.unwrap().decided.unwrap();
    assert_eq!(decided.decision, ApprovalDecision::Expired);
    let names: Vec<String> = p
        .store
        .events_since(0, Some(id), 100)
        .unwrap()
        .iter()
        .map(|e| e.record.event.name().to_owned())
        .collect();
    let tail: Vec<&str> = names.iter().rev().take(3).map(String::as_str).collect();
    assert_eq!(
        tail,
        [
            "task.state_changed",
            "step.finished",
            "plan.approval_decided"
        ]
    );
    // Found again at the next start, nothing more is done.
    p.recover(mimlys_core::clock::FakeClock::default().now());
    assert_eq!(p.task(id), task);
}
