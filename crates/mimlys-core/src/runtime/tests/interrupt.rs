//! A request to interrupt the step in flight interrupts only what can be undone
//! (handoff 010e; REQ-CLI-005, REQ-TASK-006; external design §7.2, §16.3,
//! §19.2), through real steps over the fakes. The tool provider here honours
//! the token it is given, as a real one does, and asks the runtime to interrupt
//! while its action runs: the verdict is the state the step leaves.

use super::*;
use crate::observation::ObservationStatus;
use std::sync::Mutex;

/// What the provider asks of the runtime while its action runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ask {
    Nothing,
    /// `Runtime::request_cancel`.
    Undoable,
    /// `Runtime::request_cancel_anyway`.
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
                Ask::Undoable => runtime.request_cancel(task),
                Ask::Anyway => runtime.request_cancel_anyway(task),
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
/// rule after `unknown`, and whether it can be undone by that rule.
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
            true,
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
    session: Session,
    task: TaskId,
}

impl Rig {
    /// A task whose model proposes `tool` once, then completes.
    fn new(definition: ToolDefinition) -> Self {
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
        provider
            .inner
            .script(&name, ExecutionOutcome::succeeded(json!({}), 1));
        let mut policy = Policy::shipped();
        policy.ceiling.tools.push(name.clone());
        policy
            .profiles
            .get_mut("workspace")
            .unwrap()
            .tools
            .push(name.clone());
        let backend = Arc::new(ScriptedBackend::with_decisions(vec![
            act(&name, "a"),
            act("read_file", "b"),
        ]));
        let runtime = Arc::new(
            Runtime::new(RuntimeDeps {
                persistence: persistence.clone(),
                backend,
                workspaces: Arc::new(FakeWorkspaces::new(provider.clone())),
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
        Self {
            runtime,
            persistence,
            provider,
            session,
            task,
        }
    }

    fn ask(&self, ask: Ask) {
        *self.provider.ask.lock().unwrap() = ask;
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
                rig.runtime.cancellation(rig.task).is_cancelled(),
                "{name}: the request is kept for the next step, which ends cancelled at once"
            );
            let next = futures_block(rig.runtime.run_step(rig.task)).unwrap();
            assert_eq!(next.outcome, StepOutcome::Cancelled, "{name}");
        }
    }
}

/// REQ-CLI-005, REQ-PERS-007: the rule the signal uses is the rule of
/// recovery, read from the other side: `can_be_undone` is true exactly where
/// `after_unknown` is not "halt `unknown_outcome`".
#[test]
fn req_cli_005_the_signal_and_recovery_share_one_rule() {
    use crate::action::{AfterUnknown, after_unknown, can_be_undone};
    for kind in [
        EffectKind::None,
        EffectKind::WorkspaceRead,
        EffectKind::WorkspaceWrite,
        EffectKind::HostRead,
        EffectKind::HostWrite,
        EffectKind::ExternalRead,
        EffectKind::ExternalWrite,
    ] {
        for reversible in [false, true] {
            for idempotent in [false, true] {
                let effect = EffectView {
                    kind,
                    reversible,
                    idempotent,
                };
                assert_eq!(
                    can_be_undone(effect),
                    after_unknown(effect) != AfterUnknown::Halt,
                    "{effect:?}"
                );
            }
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
