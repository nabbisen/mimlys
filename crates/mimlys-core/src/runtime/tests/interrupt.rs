//! A request to interrupt the step in flight interrupts only what can be undone
//! (handoff 010e; REQ-CLI-005, REQ-TASK-006; external design §7.2, §16.3,
//! §19.2), through real steps over the fakes. The tool provider here honours
//! the token it is given, as a real one does, and asks the runtime to interrupt
//! while its action runs: the verdict is the state the step leaves. The backend
//! here can ask while the model request is in flight, as a signal does.

use super::*;
use crate::observation::{Notice, ObservationStatus};
use crate::spi::{BackendDescriptor, BoxFuture, Cancellation, GenerateRequest, ModelBackend};
use std::sync::Mutex;

mod admission;
mod not_started;
mod second_check;

/// What the provider asks of the runtime while its action runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ask {
    Nothing,
    /// `Runtime::start_no_new_action`.
    NoNewAction,
    /// `Runtime::interrupt_step`.
    Undoable,
    /// `Runtime::interrupt_step_anyway`.
    Anyway,
    /// `Runtime::cancel_task`, as a client of another thread would: the call is made on a
    /// thread of its own, and the execution then waits for the token it was given, as a real
    /// one does (the call waits for the step to end, and the step ends when the execution does).
    CancelTask,
}

/// What the thread that cancelled the task saw right after: its state and its actions' statuses.
type AfterCancel = Arc<Mutex<Option<(TaskState, Vec<ActionStatus>)>>>;

/// A tool provider that asks, then honours the token it was given.
struct Asking {
    inner: FakeToolProvider,
    runtime: Mutex<Option<Arc<Runtime>>>,
    task: Mutex<Option<TaskId>>,
    session: Session,
    ask: Mutex<Ask>,
    /// What it asks when the step's environment is built (`definitions`): after
    /// the step's boundary and before its action would begin, which is where a
    /// request lands that the boundary of an approved step did not see.
    ask_at_definitions: Mutex<Ask>,
    /// Per execution: whether the token it was given was cancelled when it looked.
    saw_cancelled: Mutex<Vec<bool>>,
    /// Per execution: whether the start gate's token was cancelled when it looked.
    saw_start_gate: Mutex<Vec<bool>>,
    /// Whether the provider stops at the start gate's token, as a wait made before
    /// anything is sent does (the MCP pin check): it then reports "not started".
    stop_at_start_gate: Mutex<bool>,
    /// The task's state and the actions' statuses right after `cancel_task`.
    after_cancel_task: AfterCancel,
    /// The thread that called `cancel_task` during the action.
    cancel_thread: Mutex<Option<std::thread::JoinHandle<()>>>,
    /// What the next action does to the world while it runs (a test's own effect), once.
    effect: Mutex<Option<Box<dyn Fn() + Send>>>,
}

impl crate::spi::ToolProvider for Asking {
    fn definitions(&self) -> Vec<ToolDefinition> {
        let runtime = self.runtime.lock().unwrap().clone();
        let task = *self.task.lock().unwrap();
        if let (Some(runtime), Some(task)) = (runtime, task) {
            match *self.ask_at_definitions.lock().unwrap() {
                Ask::NoNewAction => runtime.start_no_new_action(task).unwrap(),
                Ask::Undoable => runtime.interrupt_step(task).unwrap(),
                Ask::Anyway => runtime.interrupt_step_anyway(task).unwrap(),
                Ask::Nothing | Ask::CancelTask => {}
            }
        }
        self.inner.definitions()
    }

    fn execute<'a>(
        &'a self,
        action: &'a crate::validate::AuthorizedAction,
        context: crate::spi::ExecutionContext,
    ) -> crate::spi::BoxFuture<'a, ExecutionOutcome> {
        Box::pin(async move {
            let runtime = self.runtime.lock().unwrap().clone().expect("runtime set");
            let task = self.task.lock().unwrap().expect("task set");
            match *self.ask.lock().unwrap() {
                Ask::Nothing => {}
                Ask::NoNewAction => runtime.start_no_new_action(task).unwrap(),
                Ask::Undoable => runtime.interrupt_step(task).unwrap(),
                Ask::Anyway => runtime.interrupt_step_anyway(task).unwrap(),
                Ask::CancelTask => {
                    let after = Arc::clone(&self.after_cancel_task);
                    let session = self.session.clone();
                    let canceller = Arc::clone(&runtime);
                    *self.cancel_thread.lock().unwrap() = Some(std::thread::spawn(move || {
                        canceller.cancel_task(&session, task).unwrap();
                        let view = canceller.get_task(task).unwrap();
                        let statuses = canceller
                            .persistence()
                            .load_actions(task)
                            .unwrap()
                            .into_iter()
                            .map(|a| a.status)
                            .collect();
                        *after.lock().unwrap() = Some((view.task.state, statuses));
                    }));
                    // The execution ends when it sees the request, as a real one does.
                    for _ in 0..10_000 {
                        if context.cancel.is_cancelled() {
                            break;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(1));
                    }
                }
            }
            if let Some(effect) = self.effect.lock().unwrap().take() {
                effect();
            }
            let start_gate = context.start_gate.is_cancelled();
            self.saw_start_gate.lock().unwrap().push(start_gate);
            if start_gate && *self.stop_at_start_gate.lock().unwrap() {
                return ExecutionOutcome::not_started(1);
            }
            let cancelled = context.cancel.is_cancelled();
            self.saw_cancelled.lock().unwrap().push(cancelled);
            if cancelled {
                ExecutionOutcome::cancelled(1)
            } else {
                self.inner.execute(action, context).await
            }
        })
    }
}

/// A backend that asks the runtime while the model request is in flight, as a
/// stop signal does, then answers from its script.
struct Stopping {
    inner: ScriptedBackend,
    runtime: Mutex<Option<Arc<Runtime>>>,
    task: Mutex<Option<TaskId>>,
    ask: Mutex<Ask>,
}

impl ModelBackend for Stopping {
    fn describe(&self) -> BackendDescriptor {
        self.inner.describe()
    }

    fn generate<'a>(
        &'a self,
        request: GenerateRequest,
        cancel: Cancellation,
    ) -> BoxFuture<'a, Result<Vec<u8>, BackendError>> {
        let runtime = self.runtime.lock().unwrap().clone();
        let task = *self.task.lock().unwrap();
        if let (Some(runtime), Some(task)) = (runtime, task) {
            match *self.ask.lock().unwrap() {
                Ask::NoNewAction => runtime.start_no_new_action(task).unwrap(),
                Ask::Undoable => runtime.interrupt_step(task).unwrap(),
                Ask::Anyway => runtime.interrupt_step_anyway(task).unwrap(),
                Ask::Nothing | Ask::CancelTask => {}
            }
        }
        self.inner.generate(request, cancel)
    }
}

fn effect_tool(name: &str, kind: EffectKind, reversible: bool, idempotent: bool) -> ToolDefinition {
    let mut definition = tool(name, kind);
    definition.effect = EffectView {
        kind,
        reversible,
        idempotent,
    };
    definition
}

/// Every effect class of external design §7.2 with the flags that decide its
/// rule after `unknown`, and whether the runtime can undo it: it undoes no host
/// write, a backed-up one included (§7.2, amended 2026-11-14).
fn classes() -> Vec<(ToolDefinition, bool)> {
    vec![
        (
            effect_tool("a_read", EffectKind::WorkspaceRead, true, true),
            true,
        ),
        (
            effect_tool("a_workspace_write", EffectKind::WorkspaceWrite, true, true),
            true,
        ),
        (
            effect_tool("a_host_read", EffectKind::HostRead, true, true),
            true,
        ),
        (
            effect_tool(
                "a_host_write_with_backup",
                EffectKind::HostWrite,
                true,
                false,
            ),
            false,
        ),
        (
            effect_tool("a_host_write_without", EffectKind::HostWrite, false, false),
            false,
        ),
        (
            effect_tool(
                "an_idempotent_external_read",
                EffectKind::ExternalRead,
                true,
                true,
            ),
            true,
        ),
        (
            effect_tool(
                "a_non_idempotent_read",
                EffectKind::ExternalRead,
                true,
                false,
            ),
            false,
        ),
        (
            effect_tool("an_external_write", EffectKind::ExternalWrite, true, true),
            false,
        ),
    ]
}

struct Rig {
    runtime: Arc<Runtime>,
    persistence: Arc<FakePersistence>,
    provider: Arc<Asking>,
    backend: Arc<Stopping>,
    workspaces: Arc<FakeWorkspaces>,
    session: Session,
    task: TaskId,
}

impl Rig {
    /// A task whose model proposes `tool` once, then completes.
    fn new(definition: ToolDefinition) -> Self {
        Self::ending(definition, ExecutionOutcome::succeeded(json!({}), 1))
    }

    /// As `new`, and the tool's execution ends as `outcome` says.
    fn ending(definition: ToolDefinition, outcome: ExecutionOutcome) -> Self {
        Self::admitted(definition, outcome, None)
    }

    /// As `ending`, the runtime taking its slots from `admission` (RFC 013 decision 5).
    fn admitted(
        definition: ToolDefinition,
        outcome: ExecutionOutcome,
        admission: Option<Arc<crate::fake_admission::ScriptedAdmission>>,
    ) -> Self {
        let name = definition.name.clone();
        let persistence = Arc::new(FakePersistence::default());
        let provider = Arc::new(Asking {
            inner: FakeToolProvider::new(vec![
                tool("read_file", EffectKind::WorkspaceRead),
                definition,
            ]),
            runtime: Mutex::new(None),
            task: Mutex::new(None),
            session: Session {
                id: "s1".into(),
                os_user: "tester".into(),
            },
            ask: Mutex::new(Ask::Nothing),
            ask_at_definitions: Mutex::new(Ask::Nothing),
            saw_cancelled: Mutex::new(Vec::new()),
            saw_start_gate: Mutex::new(Vec::new()),
            stop_at_start_gate: Mutex::new(false),
            after_cancel_task: Arc::new(Mutex::new(None)),
            cancel_thread: Mutex::new(None),
            effect: Mutex::new(None),
        });
        provider.inner.script(&name, outcome);
        let mut policy = Policy::shipped();
        policy.ceiling.tools.push(name.clone());
        policy
            .profiles
            .get_mut("workspace")
            .unwrap()
            .tools
            .push(name.clone());
        let backend = Arc::new(Stopping {
            inner: ScriptedBackend::with_decisions(vec![act(&name, "a"), act("read_file", "b")]),
            runtime: Mutex::new(None),
            task: Mutex::new(None),
            ask: Mutex::new(Ask::Nothing),
        });
        let workspaces = Arc::new(FakeWorkspaces::new(provider.clone()));
        let runtime = Runtime::new(RuntimeDeps {
            persistence: persistence.clone(),
            backend: backend.clone(),
            backend_name: "default".to_owned(),
            workspaces: workspaces.clone(),
            policy,
            ids: Arc::new(ScriptedIds::default()),
            clock: Arc::new(FakeClock::default()),
            runtime_version: "0.1.0-test".into(),
            rules: PlatformRules::Posix,
            probe: crate::confine::Probe::unavailable("no backend in tests"),
            secrets: vec![],
            context_chars: crate::limits::LIM_C1_CONTEXT_CHARS,
            output_bytes: crate::limits::LIM_S4_OUTPUT_BYTES as usize,
        })
        .unwrap();
        let runtime = Arc::new(match admission {
            Some(admission) => runtime.with_admission(admission),
            None => runtime,
        });
        let session = provider.session.clone();
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
                    plan_first: false,
                    effort: None,
                },
            )
            .unwrap()
            .task_id;
        *provider.runtime.lock().unwrap() = Some(runtime.clone());
        *provider.task.lock().unwrap() = Some(task);
        *backend.runtime.lock().unwrap() = Some(runtime.clone());
        *backend.task.lock().unwrap() = Some(task);
        Self {
            runtime,
            persistence,
            provider,
            backend,
            workspaces,
            session,
            task,
        }
    }

    /// Waits for the thread that called `cancel_task` during the action, if there was one.
    fn join_cancel(&self) {
        if let Some(thread) = self.provider.cancel_thread.lock().unwrap().take() {
            thread.join().expect("the cancel did not panic");
        }
    }

    /// What the tool provider asks while the action runs.
    fn ask(&self, ask: Ask) {
        *self.provider.ask.lock().unwrap() = ask;
    }

    /// What the tool provider asks when the next step's environment is built:
    /// a stop signal that comes after the step's boundary and before its action
    /// begins, as in the step that continues an approved action.
    fn ask_when_the_step_begins(&self, ask: Ask) {
        *self.provider.ask_at_definitions.lock().unwrap() = ask;
    }

    /// What the backend asks while the model request is in flight.
    fn ask_during_the_request(&self, ask: Ask) {
        *self.backend.ask.lock().unwrap() = ask;
    }

    /// Runs the step that executes the action: the first step proposes it,
    /// and an action that needs approval waits for it, which is given.
    fn execute_the_action(&self) -> StepResultView {
        let mut result = futures_block(self.runtime.run_step(self.task)).unwrap();
        if result.outcome == StepOutcome::Waited {
            self.runtime
                .approve_action(&self.session, self.task, None)
                .unwrap();
            result = futures_block(self.runtime.run_step(self.task)).unwrap();
        }
        result
    }

    fn action(&self) -> crate::action::Action {
        self.persistence.load_actions(self.task).unwrap().remove(0)
    }

    fn saw(&self) -> Vec<bool> {
        self.provider.saw_cancelled.lock().unwrap().clone()
    }

    /// The calls the tool provider received: none when the action never started.
    fn calls(&self) -> usize {
        self.provider.inner.calls().len()
    }

    /// The last `task.state_changed` of the task: where it went and why.
    fn last_state_change(&self) -> Option<(String, Option<HaltReason>)> {
        self.persistence
            .events()
            .into_iter()
            .filter(|e| e.task_id == Some(self.task))
            .filter_map(|e| match e.event {
                crate::event::Event::TaskStateChanged { to, reason, .. } => Some((to, reason)),
                _ => None,
            })
            .next_back()
    }

    fn events(&self) -> Vec<String> {
        self.persistence
            .events()
            .into_iter()
            .filter(|e| e.task_id == Some(self.task))
            .map(|e| e.event.name().to_owned())
            .collect()
    }
}

/// REQ-CLI-005, external design §19.2: the first request interrupts the action
/// of every class whose rule after `unknown` is not to halt `unknown_outcome`,
/// and not the others: the token each execution is given says so, and the
/// action ends `cancelled` or runs to its end. The classes are the rows of
/// §7.2; the verdict is read from the state the step leaves.
#[test]
fn req_cli_005_the_first_request_interrupts_only_what_can_be_undone() {
    for (definition, can_be_undone) in classes() {
        let name = definition.name.clone();
        let rig = Rig::new(definition);
        rig.ask(Ask::Undoable);
        let result = rig.execute_the_action();
        assert_eq!(
            rig.saw(),
            [can_be_undone],
            "{name}: the token the action was given"
        );
        let action = rig.action();
        if can_be_undone {
            assert_eq!(action.status, ActionStatus::Cancelled, "{name}");
            assert_eq!(result.outcome, StepOutcome::Cancelled, "{name}");
            assert_eq!(result.state, TaskState::Running, "{name}");
        } else {
            assert_eq!(
                action.status,
                ActionStatus::Succeeded,
                "{name}: ran to its end"
            );
            assert_eq!(result.outcome, StepOutcome::Continued, "{name}");
            assert!(
                rig.runtime.step_interruption(rig.task).is_cancelled(),
                "{name}: the request is kept for the next step, which ends cancelled at once"
            );
            let next = futures_block(rig.runtime.run_step(rig.task)).unwrap();
            assert_eq!(next.outcome, StepOutcome::Cancelled, "{name}");
        }
    }
}

/// REQ-CLI-005, external design §16.3, §19.2: the second request interrupts an
/// action that cannot be undone, and its outcome is recorded as a crash records
/// it: the action `unknown`, its observation `unknown`, no rollback, the task
/// `halted` with `unknown_outcome` and the action listed. Never `cancelled`
/// as if nothing had happened.
#[test]
fn req_cli_005_the_second_request_leaves_the_outcome_unknown_and_halts() {
    for (definition, can_be_undone) in classes().into_iter().filter(|(_, undoable)| !undoable) {
        let name = definition.name.clone();
        let kind = definition.effect.kind;
        let rig = Rig::new(definition);
        rig.ask(Ask::Anyway);
        let result = rig.execute_the_action();
        assert!(!can_be_undone);
        assert_eq!(rig.saw(), [true], "{name}: interrupted");
        assert_eq!(rig.action().status, ActionStatus::Unknown, "{name}");
        assert_eq!(result.outcome, StepOutcome::Halted, "{name}");
        assert_eq!(
            result.state,
            TaskState::Halted {
                halt_reason: HaltReason::UnknownOutcome,
                halt_detail: None,
            },
            "{name}"
        );
        let observations = rig.persistence.load_observations(rig.task).unwrap();
        assert_eq!(observations[0].status, ObservationStatus::Unknown, "{name}");
        assert!(!observations[0].rolled_back, "{name}");
        let events = rig.events();
        assert!(
            !events.contains(&"workspace.rollback".to_owned()),
            "{name}: nothing is restored, as in recovery: {events:?}"
        );
        let last_change = rig.last_state_change();
        assert_eq!(
            last_change,
            Some(("halted".to_owned(), Some(HaltReason::UnknownOutcome))),
            "{name}: the halt is in the events"
        );
        let view = rig.runtime.get_task(rig.task).unwrap();
        assert_eq!(
            view.unknown_actions.len(),
            1,
            "{name}: the action is listed"
        );
        assert_eq!(view.unknown_actions[0].tool, name);
        assert_eq!(view.unknown_actions[0].effect, kind);
        // The task can be resumed after the user has looked: the record is a halt, not an end.
        assert!(
            rig.runtime
                .resume_task(&rig.session, rig.task, None)
                .is_ok(),
            "{name}"
        );
    }
}

/// REQ-CLI-005: the second request with only what can be undone in flight
/// cancels as the first would have: the action `cancelled` (rolled back where
/// it wrote), the step `cancelled`, the task still `running` for the caller to
/// cancel. No outcome is unknown.
#[test]
fn req_cli_005_the_second_request_on_what_can_be_undone_cancels_as_before() {
    for (definition, _) in classes().into_iter().filter(|(_, undoable)| *undoable) {
        let name = definition.name.clone();
        let rig = Rig::new(definition);
        rig.ask(Ask::Anyway);
        let result = rig.execute_the_action();
        assert_eq!(rig.action().status, ActionStatus::Cancelled, "{name}");
        assert_eq!(result.outcome, StepOutcome::Cancelled, "{name}");
        assert_eq!(result.state, TaskState::Running, "{name}");
        assert!(
            rig.runtime
                .get_task(rig.task)
                .unwrap()
                .unknown_actions
                .is_empty(),
            "{name}"
        );
    }
}

/// REQ-TASK-006: `cancel_task` ends the task `cancelled`; an action running
/// that cannot be undone is recorded with its outcome unknown, one that can be
/// undone is `cancelled`, and the task's view names the first (the command's
/// text and JSON are built from it).
#[test]
fn req_task_006_cancel_task_records_an_interrupted_action_that_cannot_be_undone_as_unknown() {
    for (definition, can_be_undone) in classes() {
        let name = definition.name.clone();
        let rig = Rig::new(definition);
        rig.ask(Ask::CancelTask);
        // The step's own end after a cancellation from outside is not the subject.
        let _ = futures_block(rig.runtime.run_step(rig.task));
        rig.join_cancel();
        if rig.runtime.get_task(rig.task).unwrap().task.state != TaskState::Cancelled {
            // An action that needs approval: approve, then run it.
            rig.runtime
                .approve_action(&rig.session, rig.task, None)
                .unwrap();
            let _ = futures_block(rig.runtime.run_step(rig.task));
            rig.join_cancel();
        }
        let (state, statuses) = rig
            .provider
            .after_cancel_task
            .lock()
            .unwrap()
            .clone()
            .unwrap_or_else(|| panic!("{name}: cancel_task was called while the action ran"));
        assert_eq!(state, TaskState::Cancelled, "{name}");
        let expected = if can_be_undone {
            ActionStatus::Cancelled
        } else {
            ActionStatus::Unknown
        };
        assert_eq!(statuses, [expected], "{name}");
        let view = rig.runtime.get_task(rig.task).unwrap();
        assert_eq!(
            view.unknown_actions.len(),
            usize::from(!can_be_undone),
            "{name}"
        );
    }
}

/// REQ-SCHED-004, REQ-TASK-005 (handoff 010e): a request made while no step runs
/// is cleared by the pause and the resume that follow, so it never reaches a step
/// that began after them. The task is paused and resumed, and the step after the
/// resume runs.
#[test]
fn req_sched_004_a_request_made_while_no_step_runs_does_not_reach_a_step_after_a_pause_and_a_resume()
 {
    let h = Harness::new(vec![
        act("read_file", "a"),
        act("read_file", "b"),
        act("read_file", "c"),
    ]);
    for _ in 0..3 {
        h.tools
            .script("read_file", ExecutionOutcome::succeeded(json!({}), 1));
    }
    let id = h.create();
    assert_eq!(h.step(id).outcome, StepOutcome::Continued);
    // No step runs: a request is made, then the task is paused and resumed.
    h.runtime.interrupt_step(id).unwrap();
    h.runtime.pause_task(&h.session, id).unwrap();
    let mut paused = false;
    for _ in 0..2 {
        // The request ends the step that meets it; the pause is taken after it.
        let result = h.step(id);
        paused = result.outcome == StepOutcome::Paused;
        if paused {
            break;
        }
    }
    assert!(paused, "the pause was taken");
    assert_eq!(h.state(id), TaskState::Paused);
    h.runtime.resume_task(&h.session, id, None).unwrap();
    assert_eq!(
        h.step(id).outcome,
        StepOutcome::Continued,
        "the first step after the resume is not interrupted"
    );
}

/// REQ-SCHED-004, REQ-TASK-005 (handoff 010e): a request made while the task is
/// paused is cleared by the resume.
#[test]
fn req_sched_004_a_request_made_while_the_task_is_paused_is_cleared_by_the_resume() {
    let h = Harness::new(vec![act("read_file", "a"), act("read_file", "b")]);
    for _ in 0..2 {
        h.tools
            .script("read_file", ExecutionOutcome::succeeded(json!({}), 1));
    }
    let id = h.create();
    assert_eq!(h.step(id).outcome, StepOutcome::Continued);
    h.runtime.pause_task(&h.session, id).unwrap();
    assert_eq!(h.step(id).outcome, StepOutcome::Paused);
    h.runtime.interrupt_step(id).unwrap();
    assert!(h.runtime.step_interruption(id).is_cancelled());
    h.runtime.resume_task(&h.session, id, None).unwrap();
    assert!(
        !h.runtime.step_interruption(id).is_cancelled(),
        "the resume cleared it"
    );
    assert_eq!(h.step(id).outcome, StepOutcome::Continued);
}

/// REQ-SCHED-004, REQ-PERS-007 (handoff 010e): a request that is not spent by
/// the step it was made for (the second request meets an action that cannot be
/// undone: the step halts the task, it does not end `cancelled`) is cleared by
/// the halt, so the step after the resume is not interrupted.
#[test]
fn req_sched_004_a_request_is_cleared_by_the_halt_it_caused() {
    let (definition, _) = classes()
        .into_iter()
        .find(|(d, _)| d.name == "an_external_write")
        .unwrap();
    let rig = Rig::new(definition);
    rig.ask(Ask::Anyway);
    let result = rig.execute_the_action();
    assert_eq!(result.outcome, StepOutcome::Halted);
    assert!(
        !rig.runtime.step_interruption(rig.task).is_cancelled(),
        "the halt cleared the request"
    );
    rig.ask(Ask::Nothing);
    rig.runtime
        .resume_task(&rig.session, rig.task, None)
        .unwrap();
    let next = futures_block(rig.runtime.run_step(rig.task)).unwrap();
    assert_eq!(
        next.outcome,
        StepOutcome::Continued,
        "the step after the resume runs: {next:?}"
    );
}

/// REQ-SCHED-004, REQ-APR-007 (handoff 010e): a request made while the task waits
/// for an approval is cleared when the user decides, so the approved action is
/// not interrupted by a request made before the decision.
#[test]
fn req_sched_004_a_request_made_while_the_task_waits_is_cleared_when_the_user_decides() {
    let (definition, _) = classes()
        .into_iter()
        .find(|(d, _)| d.name == "an_external_write")
        .unwrap();
    let rig = Rig::new(definition);
    let waited = futures_block(rig.runtime.run_step(rig.task)).unwrap();
    assert_eq!(waited.outcome, StepOutcome::Waited);
    rig.runtime.interrupt_step_anyway(rig.task).unwrap();
    rig.runtime
        .approve_action(&rig.session, rig.task, None)
        .unwrap();
    assert!(
        !rig.runtime.step_interruption(rig.task).is_cancelled(),
        "the decision cleared the request"
    );
    let result = futures_block(rig.runtime.run_step(rig.task)).unwrap();
    assert_eq!(result.outcome, StepOutcome::Continued, "{result:?}");
    assert_eq!(rig.action().status, ActionStatus::Succeeded);
}

/// REQ-CLI-005, REQ-OBS-001 (handoff 010e-3, item 2): a first signal during the
/// model request of a step whose action is then proposed starts no new action,
/// whatever the action's class (an external read that is not idempotent needs no
/// approval, so nothing else would stop it). The action is `cancelled` and never
/// started: no `action.started`, no execution, the tool provider received no call,
/// nothing rolled back; its observation says that the stop came before it began.
/// The step ends `cancelled`, the request is spent, and the caller pauses the
/// task. (An action that needs approval is started only after the user's
/// decision, which clears the request. A request made after the decision and
/// before the approved step begins is met by that step's boundary (see
/// `req_cli_005_no_new_step_begins_after_a_stop_signal`); one that comes after
/// the boundary, as here at the building of the step's environment, is met by
/// the check before the action begins.)
#[test]
fn req_cli_005_a_first_signal_during_the_model_request_starts_no_new_action() {
    for (definition, _) in classes() {
        let name = definition.name.clone();
        let rig = Rig::new(definition);
        rig.ask_during_the_request(Ask::NoNewAction);
        let mut result = futures_block(rig.runtime.run_step(rig.task)).unwrap();
        if result.outcome == StepOutcome::Waited {
            rig.runtime
                .approve_action(&rig.session, rig.task, None)
                .unwrap();
            rig.ask_when_the_step_begins(Ask::NoNewAction);
            result = futures_block(rig.runtime.run_step(rig.task)).unwrap();
        }
        assert_eq!(result.outcome, StepOutcome::Cancelled, "{name}");
        assert_eq!(result.state, TaskState::Running, "{name}");
        assert_eq!(rig.action().status, ActionStatus::Cancelled, "{name}");
        assert_eq!(rig.calls(), 0, "{name}: the tool received no call");
        assert!(rig.saw().is_empty(), "{name}: it was never executed");
        let events = rig.events();
        for never in ["action.started", "execution.finished", "workspace.rollback"] {
            assert!(
                !events.contains(&never.to_owned()),
                "{name}: no {never}: {events:?}"
            );
        }
        assert!(
            events.contains(&"action.finished".to_owned()),
            "{name}: {events:?}"
        );
        let observations = rig.persistence.load_observations(rig.task).unwrap();
        assert_eq!(observations.len(), 1, "{name}");
        assert_eq!(
            observations[0].status,
            ObservationStatus::Cancelled,
            "{name}"
        );
        assert_eq!(
            observations[0].notices,
            [Notice::StoppedBeforeStart],
            "{name}: it says that the stop came before the action began"
        );
        assert!(!observations[0].rolled_back, "{name}");
        assert!(
            rig.runtime
                .get_task(rig.task)
                .unwrap()
                .unknown_actions
                .is_empty(),
            "{name}: nothing is unknown, for nothing was sent"
        );
        // The step spent the request, and the task pauses.
        assert!(!rig.runtime.no_new_action_requested(rig.task), "{name}");
        rig.runtime.pause_task(&rig.session, rig.task).unwrap();
        let paused = futures_block(rig.runtime.run_step(rig.task)).unwrap();
        assert_eq!(paused.outcome, StepOutcome::Paused, "{name}");
    }
}

/// REQ-CLI-005 (handoff 010e-4, item 1): no new step begins after a stop signal.
/// A first signal that finds no step in flight (the task is `running`, between
/// two steps) is met by the next step's boundary: it ends `cancelled` at once,
/// with no model request, the request is spent by it, the caller pauses, and the
/// step after that begins normally. The same for either stronger request, which
/// imply the weakest.
#[test]
fn req_cli_005_no_new_step_begins_after_a_stop_signal() {
    type Ask = fn(&Runtime, TaskId) -> Result<(), RuntimeError>;
    let asks: [(&str, Ask); 3] = [
        ("start_no_new_action", Runtime::start_no_new_action),
        ("interrupt_step", Runtime::interrupt_step),
        ("interrupt_step_anyway", Runtime::interrupt_step_anyway),
    ];
    for (name, ask) in asks {
        let h = Harness::new(vec![
            act("read_file", "a"),
            act("read_file", "b"),
            act("read_file", "c"),
        ]);
        for _ in 0..3 {
            h.tools
                .script("read_file", ExecutionOutcome::succeeded(json!({}), 1));
        }
        let id = h.create();
        assert_eq!(h.step(id).outcome, StepOutcome::Continued, "{name}");
        let requests = h.backend.requests().len();
        assert_eq!(requests, 1, "{name}");
        // No step is in flight: the task is running between two steps.
        ask(&h.runtime, id).unwrap();
        let cancelled = h.step(id);
        assert_eq!(cancelled.outcome, StepOutcome::Cancelled, "{name}");
        assert_eq!(cancelled.state, TaskState::Running, "{name}");
        assert_eq!(
            h.backend.requests().len(),
            requests,
            "{name}: the step made no model request"
        );
        assert_eq!(h.tools.calls().len(), 1, "{name}: nor any action");
        assert!(
            !h.runtime.no_new_action_requested(id),
            "{name}: the request is spent by that step"
        );
        // The caller pauses, as after any cancelled step.
        h.runtime.pause_task(&h.session, id).unwrap();
        assert_eq!(h.step(id).outcome, StepOutcome::Paused, "{name}");
        assert_eq!(
            h.backend.requests().len(),
            requests,
            "{name}: still no request"
        );
        // And the step after the resume begins normally.
        h.runtime.resume_task(&h.session, id, None).unwrap();
        assert_eq!(h.step(id).outcome, StepOutcome::Continued, "{name}");
        assert_eq!(
            h.backend.requests().len(),
            requests + 1,
            "{name}: the next step asked the model"
        );
    }
}

/// REQ-CLI-005 (handoff 010e-4, item 1): the spent request does not meet the
/// step after it. A `running` task whose step ended `cancelled` and which is not
/// paused runs its next step normally.
#[test]
fn req_cli_005_the_request_is_spent_by_the_step_it_ended() {
    let h = Harness::new(vec![act("read_file", "a"), act("read_file", "b")]);
    for _ in 0..2 {
        h.tools
            .script("read_file", ExecutionOutcome::succeeded(json!({}), 1));
    }
    let id = h.create();
    h.runtime.start_no_new_action(id).unwrap();
    assert_eq!(h.step(id).outcome, StepOutcome::Cancelled);
    assert_eq!(
        h.backend.requests().len(),
        0,
        "the task's first step never asked"
    );
    assert_eq!(h.step(id).outcome, StepOutcome::Continued);
    assert_eq!(h.backend.requests().len(), 1);
}

/// REQ-CLI-005 (handoff 010e-3, item 2): a request of either stronger kind that
/// is set when an execution would begin starts nothing either, and nothing is
/// recorded `unknown` for a call that was never sent. The requests are made
/// while the model request is in flight, as a signal makes them.
#[test]
fn req_cli_005_a_stronger_request_already_set_when_an_execution_would_begin_starts_nothing() {
    for ask in [Ask::Undoable, Ask::Anyway] {
        for name in ["an_external_write", "a_non_idempotent_read", "a_read"] {
            let (definition, _) = classes().into_iter().find(|(d, _)| d.name == name).unwrap();
            let rig = Rig::new(definition);
            rig.ask_during_the_request(ask);
            let mut result = futures_block(rig.runtime.run_step(rig.task)).unwrap();
            if result.outcome == StepOutcome::Waited {
                rig.runtime
                    .approve_action(&rig.session, rig.task, None)
                    .unwrap();
                rig.ask_when_the_step_begins(ask);
                result = futures_block(rig.runtime.run_step(rig.task)).unwrap();
            }
            assert_eq!(result.outcome, StepOutcome::Cancelled, "{name} {ask:?}");
            assert_eq!(result.state, TaskState::Running, "{name} {ask:?}");
            assert_eq!(rig.calls(), 0, "{name} {ask:?}");
            assert!(
                !rig.events().contains(&"action.started".to_owned()),
                "{name} {ask:?}"
            );
            // No action was recorded unknown for a call that was never sent.
            for action in rig.persistence.load_actions(rig.task).unwrap() {
                assert_ne!(action.status, ActionStatus::Unknown, "{name} {ask:?}");
            }
        }
    }
}

/// REQ-PERS-008, REQ-OBS-001 (handoff 010e-3, item 5): an action that cannot be
/// undone and whose execution the runtime, not the tool, ended is `unknown`,
/// never `failed`: at a deadline, a resource limit, a cancellation or a lost
/// worker (the MCP executor's outcome for a connection lost after the call was
/// sent is `host_fault{worker_died}` with a `sandbox_fault` failure; the loop's
/// own derivation gives the same with none). The observation's status is
/// `unknown`, nothing is restored, the task halts `unknown_outcome` with the
/// action listed and the halt in the events, and the model is not asked again:
/// one execution, no retry, no feedback.
#[test]
fn req_pers_008_an_action_that_cannot_be_undone_and_was_ended_by_the_runtime_is_unknown() {
    use crate::action::{FailureCause, HostFaultDetail, Termination};
    let ended_by = |termination: Termination, failure: Option<FailureCause>| ExecutionOutcome {
        termination,
        failure,
        ..ExecutionOutcome::succeeded(json!({}), 1)
    };
    let outcomes: Vec<(&str, ExecutionOutcome)> = vec![
        ("deadline", ExecutionOutcome::timed_out(1)),
        ("fuel_exhausted", ended_by(Termination::FuelExhausted, None)),
        ("memory_limit", ended_by(Termination::MemoryLimit, None)),
        ("output_limit", ended_by(Termination::OutputLimit, None)),
        ("process_limit", ended_by(Termination::ProcessLimit, None)),
        ("cancelled", ExecutionOutcome::cancelled(1)),
        (
            "worker_died",
            ExecutionOutcome::host_fault(HostFaultDetail::WorkerDied, 1),
        ),
        (
            "worker_died_as_the_mcp_executor_reports_it",
            ended_by(
                Termination::HostFault {
                    detail: HostFaultDetail::WorkerDied,
                },
                Some(FailureCause::SandboxFault {
                    detail: HostFaultDetail::WorkerDied,
                }),
            ),
        ),
    ];
    for (definition, can_be_undone) in classes().into_iter().filter(|(_, undoable)| !undoable) {
        assert!(!can_be_undone);
        for (how, outcome) in &outcomes {
            let name = format!("{} ended by {how}", definition.name);
            let kind = definition.effect.kind;
            let rig = Rig::ending(definition.clone(), outcome.clone());
            let result = rig.execute_the_action();
            assert_eq!(rig.action().status, ActionStatus::Unknown, "{name}");
            assert_eq!(rig.calls(), 1, "{name}: no retry");
            assert_eq!(result.outcome, StepOutcome::Halted, "{name}");
            assert_eq!(
                result.state,
                TaskState::Halted {
                    halt_reason: HaltReason::UnknownOutcome,
                    halt_detail: None,
                },
                "{name}"
            );
            let observations = rig.persistence.load_observations(rig.task).unwrap();
            assert_eq!(observations.len(), 1, "{name}");
            assert_eq!(observations[0].status, ObservationStatus::Unknown, "{name}");
            assert!(!observations[0].rolled_back, "{name}");
            assert!(
                !rig.events().contains(&"workspace.rollback".to_owned()),
                "{name}: nothing is restored"
            );
            assert_eq!(
                rig.last_state_change(),
                Some(("halted".to_owned(), Some(HaltReason::UnknownOutcome))),
                "{name}: the halt is in the events"
            );
            let view = rig.runtime.get_task(rig.task).unwrap();
            assert_eq!(view.unknown_actions.len(), 1, "{name}");
            assert_eq!(view.unknown_actions[0].effect, kind, "{name}");
        }
    }
}

/// REQ-PERS-008 (handoff 010e-3, item 5): the rule applies only when the runtime
/// ended the execution. An end the tool reported itself (an exit code, a
/// structured failure), or an execution that never started, keeps the table of
/// §7.5: the action is `failed`, the task goes on (or halts `tool_unavailable`),
/// and nothing is unknown. And an action that can be undone, ended by its
/// deadline, stays `failed` and is rolled back where it wrote: the unknown
/// outcome is for what cannot be undone.
#[test]
fn req_pers_008_the_rule_is_for_what_the_runtime_ended_and_what_cannot_be_undone() {
    use crate::action::HostFaultDetail;
    let external_write = || {
        classes()
            .into_iter()
            .find(|(d, _)| d.name == "an_external_write")
            .unwrap()
            .0
    };
    // The tool reported its own failure: the model is told, the task goes on.
    let rig = Rig::ending(
        external_write(),
        ExecutionOutcome::tool_error("rejected", json!({}), 1),
    );
    let result = rig.execute_the_action();
    assert_eq!(result.outcome, StepOutcome::Continued);
    assert!(matches!(rig.action().status, ActionStatus::Failed { .. }));
    assert_eq!(
        rig.runtime.get_task(rig.task).unwrap().task.state,
        TaskState::Running
    );
    // Nothing was started: the tool could not be reached.
    let rig = Rig::ending(
        external_write(),
        ExecutionOutcome::host_fault(HostFaultDetail::Unavailable, 1),
    );
    let result = rig.execute_the_action();
    assert!(matches!(rig.action().status, ActionStatus::Failed { .. }));
    assert_eq!(
        result.state,
        TaskState::Halted {
            halt_reason: HaltReason::ToolUnavailable,
            halt_detail: None,
        }
    );
    assert!(
        rig.runtime
            .get_task(rig.task)
            .unwrap()
            .unknown_actions
            .is_empty()
    );
    // A workspace write that times out is rolled back and failed, not unknown.
    let rig = Rig::ending(
        effect_tool("a_workspace_write", EffectKind::WorkspaceWrite, true, false),
        ExecutionOutcome::timed_out(1),
    );
    let result = rig.execute_the_action();
    assert!(matches!(rig.action().status, ActionStatus::Failed { .. }));
    assert_eq!(result.outcome, StepOutcome::Continued);
    assert_eq!(result.state, TaskState::Running);
    let observations = rig.persistence.load_observations(rig.task).unwrap();
    assert_eq!(observations[0].status, ObservationStatus::Failed);
    assert!(observations[0].rolled_back, "rolled back");
    assert!(rig.events().contains(&"workspace.rollback".to_owned()));
    assert!(
        rig.runtime
            .get_task(rig.task)
            .unwrap()
            .unknown_actions
            .is_empty()
    );
}

/// REQ-OBS-001 (handoff 010e-3, item 6): every halt decided inside `execute` is
/// in the events as the state change it is, with its reason. For a disposition
/// that halts (`tool_unavailable`) and for an unknown outcome the last
/// `task.state_changed` is the halt, once.
#[test]
fn req_obs_001_every_halt_decided_inside_execute_is_a_state_change_in_the_events() {
    use crate::action::HostFaultDetail;
    let state_changes = |rig: &Rig| -> Vec<(String, Option<HaltReason>)> {
        rig.persistence
            .events()
            .into_iter()
            .filter(|e| e.task_id == Some(rig.task))
            .filter_map(|e| match e.event {
                crate::event::Event::TaskStateChanged { to, reason, .. } => Some((to, reason)),
                _ => None,
            })
            .collect()
    };
    // A disposition that halts: the tool cannot be reached.
    let rig = Rig::ending(
        effect_tool("a_read", EffectKind::WorkspaceRead, true, true),
        ExecutionOutcome::host_fault(HostFaultDetail::Unavailable, 1),
    );
    let result = rig.execute_the_action();
    assert_eq!(
        result.state,
        TaskState::Halted {
            halt_reason: HaltReason::ToolUnavailable,
            halt_detail: None,
        }
    );
    assert_eq!(
        rig.last_state_change(),
        Some(("halted".to_owned(), Some(HaltReason::ToolUnavailable)))
    );
    assert_eq!(
        state_changes(&rig)
            .iter()
            .filter(|(to, _)| to == "halted")
            .count(),
        1,
        "once"
    );
    // An unknown outcome: the same, with its reason.
    let rig = Rig::ending(
        effect_tool("an_external_write", EffectKind::ExternalWrite, false, false),
        ExecutionOutcome::timed_out(1),
    );
    rig.execute_the_action();
    assert_eq!(
        rig.last_state_change(),
        Some(("halted".to_owned(), Some(HaltReason::UnknownOutcome)))
    );
    assert_eq!(
        state_changes(&rig)
            .iter()
            .filter(|(to, _)| to == "halted")
            .count(),
        1,
        "once"
    );
}

/// REQ-OBS-001 (handoff 010e-3, item 6): a restore that fails after a failed
/// write leaves the outcome unknown and halts the task `unknown_outcome` inside
/// `execute`; that halt is in the events too, with its reason.
#[test]
fn req_obs_001_a_failed_restore_halts_the_task_and_the_halt_is_in_the_events() {
    let rig = Rig::ending(
        effect_tool("a_workspace_write", EffectKind::WorkspaceWrite, true, false),
        ExecutionOutcome::tool_error("rejected", json!({}), 1),
    );
    rig.workspaces.fail_restores();
    let result = rig.execute_the_action();
    assert_eq!(
        result.state,
        TaskState::Halted {
            halt_reason: HaltReason::UnknownOutcome,
            halt_detail: None,
        }
    );
    assert_eq!(result.outcome, StepOutcome::Halted);
    assert_eq!(
        rig.last_state_change(),
        Some(("halted".to_owned(), Some(HaltReason::UnknownOutcome)))
    );
}

/// REQ-WS-010: a restore that fails after a cancelled
/// write is not silent. The execution was cancelled and `on_cancel` is
/// `rollback`, so the workspace was to be restored; it was not. The task halts
/// `unknown_outcome` as it does after a failed write, the observation says that
/// nothing was rolled back (and no `workspace.rollback` is recorded), and the
/// log line names the action and the snapshot.
#[test]
fn req_ws_010_a_failed_restore_after_a_cancelled_write_halts_the_task() {
    let buffers = super::per_thread_log();
    let me = std::thread::current().id();
    buffers.lock().unwrap().remove(&me);

    let rig = Rig::ending(
        effect_tool("a_workspace_write", EffectKind::WorkspaceWrite, true, false),
        ExecutionOutcome::cancelled(1),
    );
    rig.workspaces.fail_restores();
    let result = rig.execute_the_action();

    assert_eq!(
        result.state,
        TaskState::Halted {
            halt_reason: HaltReason::UnknownOutcome,
            halt_detail: None,
        }
    );
    assert_eq!(result.outcome, StepOutcome::Halted);
    assert_eq!(
        rig.last_state_change(),
        Some(("halted".to_owned(), Some(HaltReason::UnknownOutcome)))
    );
    let action = rig.action();
    assert_eq!(action.status, ActionStatus::Cancelled);
    let observations = rig.persistence.load_observations(rig.task).unwrap();
    assert_eq!(observations.len(), 1);
    assert_eq!(observations[0].status, ObservationStatus::Cancelled);
    assert!(!observations[0].rolled_back, "it says: not rolled back");
    assert!(!observations[0].notices.contains(&Notice::RolledBack));
    assert!(!rig.events().contains(&"workspace.rollback".to_owned()));
    assert!(rig.workspaces.restores().is_empty());

    let written = buffers.lock().unwrap().remove(&me).unwrap_or_default();
    let log = String::from_utf8(written).unwrap();
    let line: serde_json::Value = log
        .lines()
        .map(|l| serde_json::from_str::<serde_json::Value>(l).expect("one JSON log line"))
        .find(|l| l["level"] == "WARN")
        .unwrap_or_else(|| panic!("a warning was logged: {log}"));
    let fields = &line["fields"];
    assert_eq!(fields["action_id"], action.action_id.to_string(), "{line}");
    assert_eq!(
        fields["snapshot_id"],
        action.snapshot_before.unwrap().to_string(),
        "{line}"
    );
    assert!(
        fields["error"]
            .as_str()
            .is_some_and(|e| e.contains("restore")),
        "{line}"
    );
}

/// REQ-WS-010: a cancelled write whose restore succeeds is rolled back and the
/// task is not halted (the step ends `cancelled`, as before).
#[test]
fn req_ws_010_a_restore_that_succeeds_after_a_cancelled_write_leaves_the_task_running() {
    let rig = Rig::ending(
        effect_tool("a_workspace_write", EffectKind::WorkspaceWrite, true, false),
        ExecutionOutcome::cancelled(1),
    );
    let result = rig.execute_the_action();
    assert_eq!(result.outcome, StepOutcome::Cancelled);
    assert_eq!(result.state, TaskState::Running);
    let observations = rig.persistence.load_observations(rig.task).unwrap();
    assert!(observations[0].rolled_back);
    assert_eq!(rig.workspaces.restores().len(), 1);
}

impl Rig {
    /// What the action does to the world while it runs.
    fn effect(&self, effect: impl Fn() + Send + 'static) {
        *self.provider.effect.lock().unwrap() = Some(Box::new(effect));
    }

    fn unreadable_events(&self) -> Vec<(String, bool)> {
        self.persistence
            .events()
            .into_iter()
            .filter(|e| e.task_id == Some(self.task))
            .filter_map(|e| match e.event {
                crate::event::Event::WorkspaceUnreadable { path, .. } => {
                    Some((path, e.action_id.is_some()))
                }
                _ => None,
            })
            .collect()
    }

    /// No action is left that will never finish: every action is closed, or waits for
    /// the user to resume (an approved action).
    fn statuses(&self) -> Vec<ActionStatus> {
        self.persistence
            .load_actions(self.task)
            .unwrap()
            .into_iter()
            .map(|a| a.status)
            .collect()
    }

    fn halted_for_the_path(&self, result: &StepResultView, path: &str) {
        assert_eq!(result.outcome, StepOutcome::Halted);
        assert_eq!(
            result.state,
            TaskState::Halted {
                halt_reason: HaltReason::WorkspaceUnreadable,
                halt_detail: None,
            }
        );
        assert_eq!(
            self.runtime.get_task(self.task).unwrap().task.state,
            result.state,
            "the halt is the stored state"
        );
        assert_eq!(
            self.last_state_change(),
            Some(("halted".to_owned(), Some(HaltReason::WorkspaceUnreadable)))
        );
        assert_eq!(
            self.runtime.get_task(self.task).unwrap().unreadable,
            Some(path.to_owned()),
            "the view names the path to restore"
        );
        assert!(
            self.persistence
                .load_running_action(self.task)
                .unwrap()
                .is_none(),
            "no action is left running"
        );
    }
}

/// REQ-WS-004: part of the tree cannot be read when the snapshot before a write is taken.
/// The action has not begun: it is closed `cancelled` (nothing ran, the tool received no
/// call), the task halts `workspace_unreadable` with the path in `workspace.unreadable`,
/// and the step ends `halted`. When the permissions are restored, the resume goes on.
#[test]
fn req_ws_004_a_tree_that_cannot_be_read_before_a_write_halts_the_task() {
    let rig = Rig::new(effect_tool(
        "a_workspace_write",
        EffectKind::WorkspaceWrite,
        true,
        true,
    ));
    rig.workspaces.make_unreadable_to_snapshots("d");

    let result = rig.execute_the_action();

    rig.halted_for_the_path(&result, "d");
    assert_eq!(rig.statuses(), [ActionStatus::Cancelled]);
    assert_eq!(rig.calls(), 0, "the tool received no call");
    assert!(!rig.events().contains(&"action.started".to_owned()));
    assert_eq!(rig.unreadable_events(), [("d".to_owned(), true)]);
    let steps = rig.persistence.load_steps(rig.task).unwrap();
    assert_eq!(steps.len(), 1);
    assert_eq!(
        steps[0].outcome,
        Some(StepOutcome::Halted),
        "the step ended"
    );

    rig.workspaces.make_readable();
    rig.runtime
        .resume_task(&rig.session, rig.task, None)
        .unwrap();
    assert_eq!(rig.runtime.get_task(rig.task).unwrap().unreadable, None);
    let next = futures_block(rig.runtime.run_step(rig.task)).unwrap();
    assert_eq!(next.outcome, StepOutcome::Continued, "{next:?}");
}

/// REQ-WS-004: a read of the tree that fails for a reason other than its permissions
/// (`ReadFailed`, which names the path and the kind of the failure and asks for nothing) halts
/// the task the same way as an unreadable path does: the same halt reason, the same event with
/// the path, the same field of the view, the action closed `cancelled`, the step `halted`.
#[test]
fn req_ws_004_a_read_that_fails_for_another_reason_halts_the_task_the_same_way() {
    let rig = Rig::new(effect_tool(
        "a_workspace_write",
        EffectKind::WorkspaceWrite,
        true,
        true,
    ));
    rig.workspaces
        .make_unreadable_to_snapshots_for_another_reason("d", "entity not found");

    let result = rig.execute_the_action();

    rig.halted_for_the_path(&result, "d");
    assert_eq!(rig.statuses(), [ActionStatus::Cancelled]);
    assert_eq!(rig.calls(), 0, "the tool received no call");
    assert_eq!(rig.unreadable_events(), [("d".to_owned(), true)]);

    rig.workspaces.make_readable();
    rig.runtime
        .resume_task(&rig.session, rig.task, None)
        .unwrap();
    assert_eq!(rig.runtime.get_task(rig.task).unwrap().unreadable, None);
    let next = futures_block(rig.runtime.run_step(rig.task)).unwrap();
    assert_eq!(next.outcome, StepOutcome::Continued, "{next:?}");
}

/// REQ-WS-004: part of the tree cannot be read when the snapshot after a write is taken
/// (the action itself changed the permissions). The action ran and succeeded: it is
/// recorded as it ended, and the task halts `workspace_unreadable` with the path; the task
/// is not left `running` with an action that never finishes. The resume goes on.
#[test]
fn req_ws_004_a_tree_that_cannot_be_read_after_a_write_halts_the_task() {
    let rig = Rig::new(effect_tool(
        "a_workspace_write",
        EffectKind::WorkspaceWrite,
        true,
        true,
    ));
    let workspaces = rig.workspaces.clone();
    rig.effect(move || workspaces.make_unreadable_to_snapshots("d/inner"));

    let result = rig.execute_the_action();

    rig.halted_for_the_path(&result, "d/inner");
    assert_eq!(rig.statuses(), [ActionStatus::Succeeded]);
    assert_eq!(rig.calls(), 1);
    let observations = rig.persistence.load_observations(rig.task).unwrap();
    assert_eq!(observations.len(), 1);
    assert_eq!(observations[0].status, ObservationStatus::Succeeded);
    assert_eq!(rig.unreadable_events(), [("d/inner".to_owned(), true)]);

    rig.workspaces.make_readable();
    rig.runtime
        .resume_task(&rig.session, rig.task, None)
        .unwrap();
    let next = futures_block(rig.runtime.run_step(rig.task)).unwrap();
    assert_eq!(next.outcome, StepOutcome::Continued, "{next:?}");
}

/// REQ-WS-006: a failed write whose restore cannot read part of the tree halts the task
/// `workspace_unreadable` with the path (not `unknown_outcome`: the tree, not the outcome,
/// is what the user can fix); the observation says that nothing was rolled back.
#[test]
fn req_ws_006_a_restore_that_cannot_read_the_tree_halts_with_the_path() {
    let rig = Rig::ending(
        effect_tool("a_workspace_write", EffectKind::WorkspaceWrite, true, false),
        ExecutionOutcome::tool_error("rejected", json!({}), 1),
    );
    let workspaces = rig.workspaces.clone();
    rig.effect(move || workspaces.make_unreadable("d"));

    let result = rig.execute_the_action();

    rig.halted_for_the_path(&result, "d");
    assert_eq!(
        rig.statuses(),
        [ActionStatus::Failed {
            cause: crate::action::FailureCause::ToolError {
                detail: "rejected".into()
            }
        }]
    );
    let observations = rig.persistence.load_observations(rig.task).unwrap();
    assert!(!observations[0].rolled_back);
    assert!(!rig.events().contains(&"workspace.rollback".to_owned()));
}

/// REQ-WS-008: the quota is measured by walking the tree. At the start of a step, part of
/// it that cannot be read halts the task with the path before the step begins; no model
/// request is made and no step is left open. The resume goes on.
#[test]
fn req_ws_008_a_tree_that_cannot_be_read_at_the_start_of_a_step_halts_the_task() {
    let rig = Rig::new(effect_tool("a_read", EffectKind::WorkspaceRead, true, true));
    rig.workspaces.make_unreadable("d");

    let result = futures_block(rig.runtime.run_step(rig.task)).unwrap();

    rig.halted_for_the_path(&result, "d");
    assert!(rig.statuses().is_empty(), "no action was proposed");
    assert!(rig.persistence.load_steps(rig.task).unwrap().is_empty());
    assert_eq!(rig.unreadable_events(), [("d".to_owned(), false)]);

    rig.workspaces.make_readable();
    rig.runtime
        .resume_task(&rig.session, rig.task, None)
        .unwrap();
    let next = futures_block(rig.runtime.run_step(rig.task)).unwrap();
    assert_eq!(next.outcome, StepOutcome::Continued, "{next:?}");
}

/// REQ-WS-008: at the end of a step (an action that is not a write: no snapshot is taken)
/// the quota check meets a tree that cannot be read: the task halts with the path; the
/// action, which succeeded, is recorded as it ended.
#[test]
fn req_ws_008_a_tree_that_cannot_be_read_at_the_end_of_a_step_halts_the_task() {
    let rig = Rig::new(effect_tool("a_read", EffectKind::WorkspaceRead, true, true));
    let workspaces = rig.workspaces.clone();
    rig.effect(move || workspaces.make_unreadable("d"));

    let result = rig.execute_the_action();

    rig.halted_for_the_path(&result, "d");
    assert_eq!(rig.statuses(), [ActionStatus::Succeeded]);
    let steps = rig.persistence.load_steps(rig.task).unwrap();
    assert_eq!(
        steps[0].outcome,
        Some(StepOutcome::Halted),
        "the step ended"
    );
}

/// REQ-WS-004, REQ-APR-008: an approved action whose snapshot cannot be taken because part
/// of the tree cannot be read is not closed: the task halts with the path, the action stays
/// approved with its step open, and the resume continues it as it continues any approved
/// action.
#[test]
fn req_ws_004_an_approved_action_continues_after_the_tree_can_be_read_again() {
    let (definition, _) = classes()
        .into_iter()
        .find(|(d, _)| d.name == "an_external_write")
        .unwrap();
    let rig = Rig::new(definition);
    let waited = futures_block(rig.runtime.run_step(rig.task)).unwrap();
    assert_eq!(waited.outcome, StepOutcome::Waited);
    rig.runtime
        .approve_action(&rig.session, rig.task, None)
        .unwrap();
    rig.workspaces.make_unreadable_to_snapshots("d");

    let result = futures_block(rig.runtime.run_step(rig.task)).unwrap();

    rig.halted_for_the_path(&result, "d");
    assert_eq!(rig.calls(), 0);
    assert!(
        !rig.action().status.is_terminal(),
        "the action is not closed: {:?}",
        rig.action().status
    );

    rig.workspaces.make_readable();
    rig.runtime
        .resume_task(&rig.session, rig.task, None)
        .unwrap();
    let next = futures_block(rig.runtime.run_step(rig.task)).unwrap();
    assert_eq!(next.outcome, StepOutcome::Continued, "{next:?}");
    assert_eq!(rig.action().status, ActionStatus::Succeeded);
    assert_eq!(rig.calls(), 1);
}

impl Rig {
    /// The `rollback_failed` field of the task's last halt event.
    fn halt_followed_a_failed_rollback(&self) -> bool {
        self.persistence
            .events()
            .into_iter()
            .filter(|e| e.task_id == Some(self.task))
            .filter_map(|e| match e.event {
                crate::event::Event::TaskStateChanged {
                    reason: Some(_),
                    rollback_failed,
                    ..
                } => Some(rollback_failed),
                _ => None,
            })
            .next_back()
            .expect("the task halted")
    }

    /// The kinds the task's `workspace.unreadable` events carry.
    fn unreadable_kinds(&self) -> Vec<Option<String>> {
        self.persistence
            .events()
            .into_iter()
            .filter(|e| e.task_id == Some(self.task))
            .filter_map(|e| match e.event {
                crate::event::Event::WorkspaceUnreadable { kind, .. } => Some(kind),
                _ => None,
            })
            .collect()
    }
}

/// REQ-WS-006, REQ-OBS-001: a halt that follows a rollback that failed says so in its event
/// (`task.state_changed`, `rollback_failed`) and in the view of the task, whether the reason is
/// `unknown_outcome` (the restore failed) or `workspace_unreadable` (the restore could not read
/// the tree); and a halt for the same reasons that follows no rollback does not say so: the
/// snapshot before an action, and the snapshot after it, are not rollbacks. A tree that fails to
/// read for a reason other than its permissions carries the kind of the failure in
/// `workspace.unreadable`, a permission carries none.
#[test]
fn req_ws_006_a_halt_after_a_failed_rollback_says_so_and_the_others_do_not() {
    let failing_write = || {
        Rig::ending(
            effect_tool("a_workspace_write", EffectKind::WorkspaceWrite, true, false),
            ExecutionOutcome::tool_error("rejected", json!({}), 1),
        )
    };
    // The restore fails: `unknown_outcome`, after a rollback that failed.
    let rig = failing_write();
    rig.workspaces.fail_restores();
    rig.execute_the_action();
    assert!(rig.halt_followed_a_failed_rollback());
    let view = rig.runtime.get_task(rig.task).unwrap();
    assert!(view.rollback_failed && view.unreadable.is_none());
    // The restore cannot read the tree (a permission): `workspace_unreadable`, after a failed rollback.
    let rig = failing_write();
    let workspaces = rig.workspaces.clone();
    rig.effect(move || workspaces.make_unreadable("d"));
    rig.execute_the_action();
    assert!(rig.halt_followed_a_failed_rollback());
    assert_eq!(rig.unreadable_kinds(), [None]);
    let view = rig.runtime.get_task(rig.task).unwrap();
    assert!(view.rollback_failed);
    assert_eq!(
        (view.unreadable.as_deref(), view.unreadable_kind),
        (Some("d"), None)
    );
    // The same for another reason: the kind of the failure is recorded and shown.
    let rig = failing_write();
    let workspaces = rig.workspaces.clone();
    rig.effect(move || {
        workspaces.make_unreadable_to_snapshots_for_another_reason("d", "entity not found")
    });
    rig.execute_the_action();
    assert!(rig.halt_followed_a_failed_rollback());
    assert_eq!(
        rig.unreadable_kinds(),
        [Some("entity not found".to_owned())]
    );
    let view = rig.runtime.get_task(rig.task).unwrap();
    assert!(view.rollback_failed);
    assert_eq!(view.unreadable_kind.as_deref(), Some("entity not found"));
    // No rollback: the snapshot before the action could not be taken.
    let rig = Rig::new(effect_tool(
        "a_workspace_write",
        EffectKind::WorkspaceWrite,
        true,
        true,
    ));
    rig.workspaces
        .make_unreadable_to_snapshots_for_another_reason("d", "entity not found");
    rig.execute_the_action();
    assert!(!rig.halt_followed_a_failed_rollback());
    assert!(!rig.runtime.get_task(rig.task).unwrap().rollback_failed);
    // No rollback: the action succeeded and the snapshot after it could not be taken.
    let rig = Rig::new(effect_tool(
        "a_workspace_write",
        EffectKind::WorkspaceWrite,
        true,
        true,
    ));
    let workspaces = rig.workspaces.clone();
    rig.effect(move || workspaces.make_unreadable_to_snapshots("d/inner"));
    rig.execute_the_action();
    assert!(!rig.halt_followed_a_failed_rollback());
    assert_eq!(rig.unreadable_kinds(), [None]);
    // An older record has neither field: it reads as no failed rollback and no kind.
    let old: crate::event::Event = serde_json::from_value(
        json!({"event": "task.state_changed", "from": "running", "to": "halted", "reason": "unknown_outcome"}),
    )
    .unwrap();
    assert!(matches!(
        old,
        crate::event::Event::TaskStateChanged {
            rollback_failed: false,
            ..
        }
    ));
    let old: crate::event::Event =
        serde_json::from_value(json!({"event": "workspace.unreadable", "path": "d"})).unwrap();
    assert!(matches!(
        old,
        crate::event::Event::WorkspaceUnreadable { kind: None, .. }
    ));
    // And a new record without either writes neither field (additive: no schema version moves).
    let json = serde_json::to_value(crate::event::Event::WorkspaceUnreadable {
        path: "d".into(),
        kind: None,
    })
    .unwrap();
    assert_eq!(json, json!({"event": "workspace.unreadable", "path": "d"}));
}
