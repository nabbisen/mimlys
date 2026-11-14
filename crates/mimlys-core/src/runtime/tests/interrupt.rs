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
    /// `Runtime::cancel_task`, as a client of another thread would.
    CancelTask,
}

/// A tool provider that asks, then honours the token it was given.
struct Asking {
    inner: FakeToolProvider,
    runtime: Mutex<Option<Arc<Runtime>>>,
    task: Mutex<Option<TaskId>>,
    session: Session,
    ask: Mutex<Ask>,
    /// Per execution: whether the token it was given was cancelled when it looked.
    saw_cancelled: Mutex<Vec<bool>>,
    /// The task's state and the actions' statuses right after `cancel_task`.
    after_cancel_task: Mutex<Option<(TaskState, Vec<ActionStatus>)>>,
}

impl crate::spi::ToolProvider for Asking {
    fn definitions(&self) -> Vec<ToolDefinition> {
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
                Ask::NoNewAction => runtime.start_no_new_action(task),
                Ask::Undoable => runtime.interrupt_step(task),
                Ask::Anyway => runtime.interrupt_step_anyway(task),
                Ask::CancelTask => {
                    runtime.cancel_task(&self.session, task).unwrap();
                    let view = runtime.get_task(task).unwrap();
                    let statuses = runtime
                        .persistence()
                        .load_actions(task)
                        .unwrap()
                        .into_iter()
                        .map(|a| a.status)
                        .collect();
                    *self.after_cancel_task.lock().unwrap() = Some((view.task.state, statuses));
                }
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
                Ask::NoNewAction => runtime.start_no_new_action(task),
                Ask::Undoable => runtime.interrupt_step(task),
                Ask::Anyway => runtime.interrupt_step_anyway(task),
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
            saw_cancelled: Mutex::new(Vec::new()),
            after_cancel_task: Mutex::new(None),
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
        let runtime = Arc::new(
            Runtime::new(RuntimeDeps {
                persistence: persistence.clone(),
                backend: backend.clone(),
                workspaces: workspaces.clone(),
                policy,
                ids: Arc::new(ScriptedIds::default()),
                clock: Arc::new(FakeClock::default()),
                runtime_version: "0.1.0-test".into(),
                rules: PlatformRules::Posix,
                probe: crate::confine::Probe::unavailable("no backend in tests"),
                secrets: vec![],
                context_chars: crate::limits::LIM_C1_CONTEXT_CHARS,
            })
            .unwrap(),
        );
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

    /// What the tool provider asks while the action runs.
    fn ask(&self, ask: Ask) {
        *self.provider.ask.lock().unwrap() = ask;
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
                .approve_action(&self.session, self.task)
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
                halt_reason: HaltReason::UnknownOutcome
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
        if rig.runtime.get_task(rig.task).unwrap().task.state != TaskState::Cancelled {
            // An action that needs approval: approve, then run it.
            rig.runtime.approve_action(&rig.session, rig.task).unwrap();
            let _ = futures_block(rig.runtime.run_step(rig.task));
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
    h.runtime.interrupt_step(id);
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
    h.runtime.interrupt_step(id);
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
    rig.runtime.interrupt_step_anyway(rig.task);
    rig.runtime.approve_action(&rig.session, rig.task).unwrap();
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
/// decision, which clears the request, so the request is made again after it, as
/// a signal during the approved step would.)
#[test]
fn req_cli_005_a_first_signal_during_the_model_request_starts_no_new_action() {
    for (definition, _) in classes() {
        let name = definition.name.clone();
        let rig = Rig::new(definition);
        rig.ask_during_the_request(Ask::NoNewAction);
        let mut result = futures_block(rig.runtime.run_step(rig.task)).unwrap();
        if result.outcome == StepOutcome::Waited {
            rig.runtime.approve_action(&rig.session, rig.task).unwrap();
            rig.runtime.start_no_new_action(rig.task);
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
                rig.runtime.approve_action(&rig.session, rig.task).unwrap();
                match ask {
                    Ask::Undoable => rig.runtime.interrupt_step(rig.task),
                    _ => rig.runtime.interrupt_step_anyway(rig.task),
                }
                result = futures_block(rig.runtime.run_step(rig.task)).unwrap();
            }
            assert_eq!(result.outcome, StepOutcome::Cancelled, "{name} {ask:?}");
            assert_eq!(result.state, TaskState::Running, "{name} {ask:?}");
            assert_eq!(rig.calls(), 0, "{name} {ask:?}");
            assert!(
                !rig.events().contains(&"action.started".to_owned()),
                "{name} {ask:?}"
            );
            // Whether the step ended at the boundary or at the check, no action
            // was recorded unknown for a call that was never sent.
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
                    halt_reason: HaltReason::UnknownOutcome
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
            halt_reason: HaltReason::ToolUnavailable
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
            halt_reason: HaltReason::ToolUnavailable
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
            halt_reason: HaltReason::UnknownOutcome
        }
    );
    assert_eq!(result.outcome, StepOutcome::Halted);
    assert_eq!(
        rig.last_state_change(),
        Some(("halted".to_owned(), Some(HaltReason::UnknownOutcome)))
    );
}
