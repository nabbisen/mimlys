//! The step loop (external design §6.1, §16.2; REQ-DEC-001, REQ-DEC-009,
//! REQ-DEC-010, REQ-TASK-005, REQ-TASK-009, REQ-TASK-010, REQ-CAP-010,
//! REQ-TOOL-009, REQ-MDL-003, REQ-MDL-008, REQ-PERS-004).
//!
//! One step is: boundary checks → assemble → decide → validate → (control
//! decision | action lines 1–8). The executor is reached only through an
//! `AuthorizedAction`; the backend is called only for the decision.

use super::contain::{Panicked, contained, contained_call};
use super::context::{ContextInputs, assemble};
use super::{Runtime, RuntimeError};
use crate::action::{
    Action, ActionInput, DispositionKind, Execution, FailureCause, Proposal, Termination,
    disposition,
};
use crate::approval::{ApprovalError, ApprovalRequest, ApprovalStatus, InvalidationReason, render};
use crate::clock::Instant;
use crate::decision::DECISION_SCHEMA_VERSION;
use crate::event::{Actor, Event};
use crate::grant::GrantSet;
use crate::hooks::{PolicyHooks, TaskView};
use crate::ids::{ActionId, ApprovalId, ExecutionId, ObservationId, QuestionId, StepId, TaskId};
use crate::limits::{
    LIM_A1_EXPIRY_MS, LIM_D1_DECISION_BYTES, LIM_M1_REQUEST_MS, LIM_M3_BACKEND_RETRIES,
};
use crate::native::{self, DecisionTransport};
use crate::observation::{Observation, ObservationStatus, Verdict};
use crate::panic_text::bounded_message;
use crate::path::CaseMode;
use crate::policy::Profile;
use crate::question::Question;
use crate::rejection::RejectionCode;
use crate::snapshot::{SnapshotReason, SnapshotRecord};
use crate::spi::{BackendError, Cancellation, ExecutionContext, GenerateRequest, ToolProvider};
use crate::step::StepMeasures;
use crate::step::{ActionOutcome, Step, StepOutcome, StepPhase, StepResult};
use crate::task::{
    CompletionStep, FailureReason, HaltReason, PendingItem, PlanVersion, StateError, Task,
    TaskInput, TaskState,
};
use crate::tool::{ToolDefinition, render_definitions};
use crate::validate::{AuthorizedAction, ControlDecision, Outcome, ValidationContext, validate};
use std::sync::Arc;

/// The component name of a contained panic in a tool provider.
pub(super) const PROVIDER: &str = "tool_provider";

/// What a step did, for callers of the loop.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StepResultView {
    /// How the step ended.
    pub outcome: StepOutcome,
    /// The task's state afterwards.
    pub state: TaskState,
}

/// What one decision request produced: the backend's answer, the measures,
/// and how the decision arrived (REQ-MDL-009).
struct Decided {
    result: Result<Vec<u8>, BackendError>,
    measures: StepMeasures,
    transport: DecisionTransport,
    text_chars: Option<u64>,
}

/// What one step runs under: the grants, the visible tools and the policy
/// hooks are fixed at the step boundary (REQ-CAP-010).
struct StepEnv {
    grants: GrantSet,
    profile: Profile,
    case: CaseMode,
    tools: Arc<dyn ToolProvider>,
    visible: Vec<ToolDefinition>,
    definitions: String,
    workspace_root: String,
    workspace_remaining_bytes: Option<u64>,
    /// External tools unpinned for this task, with the reason, named to the model (RFC 009 §7).
    unavailable: Vec<(String, String)>,
}

impl StepEnv {
    fn hooks<'a>(&'a self, runtime: &'a Runtime, task: &Task, now: Instant) -> PolicyHooks<'a> {
        PolicyHooks {
            grants: &self.grants,
            profile: &self.profile,
            exclusions: &runtime.exclusions,
            case: self.case,
            host_tools: &runtime.deps.policy.host_tools,
            task: TaskView {
                now,
                remaining_ms: task.remaining_ms(now),
                workspace_remaining_bytes: self.workspace_remaining_bytes,
            },
        }
    }

    fn validate(
        &self,
        runtime: &Runtime,
        task: &Task,
        pending_user_item: bool,
        raw: &[u8],
        now: Instant,
    ) -> Outcome {
        let hooks = self.hooks(runtime, task, now);
        let ctx = ValidationContext {
            task_running: task.state == TaskState::Running,
            pending_user_item,
            tools: &self.visible,
            rules: runtime.deps.rules,
            grants: &hooks,
            effects: &hooks,
            budgets: &hooks,
        };
        validate(&ctx, raw)
    }
}

impl Runtime {
    /// Runs steps until the task leaves `running` (REQ-SCHED-002).
    pub async fn run_until_wait(&self, task_id: TaskId) -> Result<TaskState, RuntimeError> {
        loop {
            let result = self.run_step(task_id).await?;
            if result.state != TaskState::Running {
                return Ok(result.state);
            }
        }
    }

    /// Runs one step of a `running` task (external design §6.1).
    pub async fn run_step(&self, task_id: TaskId) -> Result<StepResultView, RuntimeError> {
        let task = self.load(task_id)?;
        if task.state != TaskState::Running {
            return Err(RuntimeError::State(StateError::Illegal {
                from: task.state.name(),
                input: "run_step",
            }));
        }
        let now = self.deps.clock.now();
        // The boundary: a cancellation request, a requested pause, then the deadline (REQ-TASK-005, REQ-TASK-006, REQ-TASK-009).
        if self.cancellation(task_id).is_cancelled() {
            return Ok(StepResultView {
                outcome: StepOutcome::Cancelled,
                state: task.state,
            });
        }
        if task.pause_requested {
            let paused = task.transition(TaskInput::Pause, now)?;
            self.persist_task(&task, &paused)?;
            return Ok(StepResultView {
                outcome: StepOutcome::Paused,
                state: paused.state,
            });
        }
        if task.deadline_passed(now) {
            let halted = task.transition(TaskInput::Halt(HaltReason::BudgetDeadline), now)?;
            self.persist_task(&task, &halted)?;
            return Ok(StepResultView {
                outcome: StepOutcome::Halted,
                state: halted.state,
            });
        }
        // A panic in the provider's `definitions`, `refresh_pins` or
        // `drain_events` (REQ-SEC-011) halts the task like a failed `execute`:
        // every task ends in one stated outcome, with its state change
        // recorded (CLR-4). `guard` is the only source of this component.
        let env = match self.step_env(&task, now) {
            Ok(env) => env,
            Err(RuntimeError::Unavailable { component }) if component == PROVIDER => {
                let halted = task.transition(TaskInput::Halt(HaltReason::ToolUnavailable), now)?;
                self.persist_task(&task, &halted)?;
                return Ok(StepResultView {
                    outcome: StepOutcome::Halted,
                    state: halted.state,
                });
            }
            Err(e) => return Err(e),
        };
        // An approved action continues its own step from line 4.
        if let Some(approval) = self.deps.persistence.load_pending_approval(task_id)?
            && approval.status == ApprovalStatus::Approved
        {
            return self.continue_approved(task, &env, approval, now).await;
        }
        self.new_step(task, &env, now).await
    }

    fn step_env(&self, task: &Task, now: Instant) -> Result<StepEnv, RuntimeError> {
        let persistence = &self.deps.persistence;
        let mut grants = GrantSet::new();
        for g in persistence.load_grants(task.task_id)? {
            grants.add(g);
        }
        grants.expire(now);
        // Reconcile against the ceiling: a grant the policy no longer permits is revoked (REQ-CAP-010).
        for grant_id in grants.reconcile(&self.deps.policy.ceiling, now) {
            if grants.revoke(grant_id).is_ok()
                && let Some(grant) = grants.all().iter().find(|g| g.grant_id == grant_id)
            {
                persistence.record_grant_status(
                    grant,
                    &self.event(
                        Actor::Runtime,
                        Some(task.task_id),
                        None,
                        None,
                        Event::GrantRevoked { grant_id },
                    ),
                )?;
            }
        }
        let tools = self.deps.workspaces.tools(task.task_id)?;
        // RFC 009 §7: a dirty server list is refreshed here; a changed
        // definition is unpinned once, named to the model, and hidden.
        let unavailable = self.refresh_pins(task, tools.as_ref())?;
        let names = grants.visible_tools(now);
        let visible: Vec<ToolDefinition> = self
            .guard(PROVIDER, || tools.definitions())?
            .into_iter()
            .filter(|d| names.contains(&d.name))
            .collect();
        let definitions = render_definitions(&visible).map_err(|e| RuntimeError::Denied {
            detail: e.to_string(),
        })?;
        let info = self.deps.workspaces.open(task.task_id)?;
        Ok(StepEnv {
            grants,
            profile: self.profile_of(task)?,
            case: info.case_mode,
            tools,
            visible,
            definitions,
            workspace_root: info.root,
            workspace_remaining_bytes: self.deps.workspaces.remaining_bytes(task.task_id)?,
            unavailable,
        })
    }

    /// Records `tool.unpinned` once per changed tool and the provider's
    /// server events; returns every tool unpinned so far for the task.
    fn refresh_pins(
        &self,
        task: &Task,
        tools: &dyn ToolProvider,
    ) -> Result<Vec<(String, String)>, RuntimeError> {
        let changed = self.guard(PROVIDER, || tools.refresh_pins(&task.versions.tool_imports))?;
        self.record_provider_events(tools)?;
        let mut unpinned = self.unpinned.lock().expect("unpinned lock");
        let set = unpinned.entry(task.task_id).or_default();
        for tool in changed {
            if set.insert(tool.clone()) {
                self.deps.persistence.append_event(&self.event(
                    Actor::Runtime,
                    Some(task.task_id),
                    None,
                    None,
                    Event::ToolUnpinned { tool },
                ))?;
            }
        }
        Ok(set
            .iter()
            .map(|t| (t.clone(), "definition_changed".to_owned()))
            .collect())
    }

    /// Appends the server events a provider produced (RFC 009 §1). The
    /// events are audit records (REQ-OBS-001): losing them is not a log
    /// matter, so a panic in `drain_events` is an error like the other calls.
    pub(crate) fn record_provider_events(
        &self,
        tools: &dyn ToolProvider,
    ) -> Result<(), RuntimeError> {
        let events = self.guard(PROVIDER, || tools.drain_events())?;
        self.append_provider_events(events)
    }

    /// Appends events a provider produced and the caller already drained.
    pub(crate) fn append_provider_events(&self, events: Vec<Event>) -> Result<(), RuntimeError> {
        for event in events {
            self.deps.persistence.append_event(&self.event(
                Actor::Runtime,
                None,
                None,
                None,
                event,
            ))?;
        }
        Ok(())
    }

    /// The decision transport in force (REQ-MDL-009), asked of the backend
    /// before every decision, with the same containment and LIM-M3 retries as
    /// the request itself. A configured `tool_calls` that the engine refuses
    /// comes back as `Unavailable`; the step then halts `backend_unavailable`.
    async fn resolve_transport(
        &self,
        cancel: &Cancellation,
    ) -> Result<DecisionTransport, BackendError> {
        let mut attempt = 0;
        loop {
            let call = contained(
                || self.deps.backend.decision_transport(cancel.clone()),
                |panicked| self.log_contained_panic("model_backend", panicked),
            );
            match call.await {
                Err(panicked) => {
                    self.log_contained_panic("model_backend", &panicked);
                    return Err(BackendError::Unavailable {
                        detail: "adapter_panic".into(),
                    });
                }
                Ok(Err(BackendError::Unavailable { .. })) if attempt < LIM_M3_BACKEND_RETRIES => {
                    attempt += 1
                }
                Ok(other) => return other,
            }
        }
    }

    /// The model request with LIM-M3 retries of an identical request
    /// (REQ-MDL-003), with the context's and the model's measures (REQ-CTX-010).
    async fn decide(
        &self,
        task: &Task,
        env: &StepEnv,
        now: Instant,
    ) -> Result<Decided, RuntimeError> {
        let persistence = &self.deps.persistence;
        let cancel = self.cancellation(task.task_id);
        let transport = match self.resolve_transport(&cancel).await {
            Ok(transport) => transport,
            Err(error) => {
                return Ok(Decided {
                    result: Err(error),
                    measures: StepMeasures::default(),
                    transport: DecisionTransport::Envelope,
                    text_chars: None,
                });
            }
        };
        // The functions are a view of the pinned list (REQ-MDL-009): a tool
        // whose function name is invalid or collides is left out here, with
        // the reason, and named to the model like any unavailable tool.
        let offer = match transport {
            DecisionTransport::ToolCalls => native::offer(&env.visible),
            DecisionTransport::Envelope => native::Offer::default(),
        };
        let (offered, definitions, unavailable) = match transport {
            DecisionTransport::ToolCalls => {
                let offered: Vec<ToolDefinition> = env
                    .visible
                    .iter()
                    .filter(|t| !offer.unavailable.iter().any(|(name, _)| name == &t.name))
                    .cloned()
                    .collect();
                let definitions =
                    render_definitions(&offered).map_err(|e| RuntimeError::Denied {
                        detail: e.to_string(),
                    })?;
                let mut unavailable = env.unavailable.clone();
                unavailable.extend(offer.unavailable.iter().cloned());
                (offered, definitions, unavailable)
            }
            DecisionTransport::Envelope => (
                env.visible.clone(),
                env.definitions.clone(),
                env.unavailable.clone(),
            ),
        };
        let plan = persistence.load_plan(task.task_id)?;
        let steps = persistence.load_steps(task.task_id)?;
        let actions = persistence.load_actions(task.task_id)?;
        let observations = persistence.load_observations(task.task_id)?;
        let questions = persistence.load_questions(task.task_id)?;
        let approvals = persistence.load_approvals(task.task_id)?;
        let history = match task.forked_from {
            Some(source) => Some(self.history_of(source)?),
            None => None,
        };
        let assembled = assemble(&ContextInputs {
            task,
            tools: &offered,
            tool_definitions: &definitions,
            grants: &env.grants,
            plan: plan.as_ref(),
            steps: &steps,
            actions: &actions,
            observations: &observations,
            questions: &questions,
            approvals: &approvals,
            history: history.as_ref(),
            unavailable: &unavailable,
            budget: self.deps.context_chars,
            now,
        });
        let mut measures = StepMeasures {
            context: assembled.measures,
            model_ms: None,
            usage: None,
        };
        {
            let mut last = self.last_requests.lock().expect("last requests lock");
            if let Some(previous) = last.get(&task.task_id) {
                measures.context.prefix_chars =
                    crate::context::common_prefix_chars(previous, &assembled.messages) as u64;
            }
            last.insert(task.task_id, assembled.messages.clone());
        }
        let messages = assembled.messages;
        let request = GenerateRequest {
            messages,
            schema_version: DECISION_SCHEMA_VERSION,
            max_bytes: LIM_D1_DECISION_BYTES,
            deadline_ms: LIM_M1_REQUEST_MS.min(task.remaining_ms(now).max(1)),
            transport,
            functions: offer.functions,
        };
        let started = self.deps.clock.now();
        let mut attempt = 0;
        let result = loop {
            // A panic in the adapter is contained here (REQ-SEC-011): that
            // request is `backend_unavailable`, and it is not repeated, since
            // the adapter's state after a panic is unknown.
            let call = contained(
                || self.deps.backend.generate(request.clone(), cancel.clone()),
                |panicked| self.log_contained_panic("model_backend", panicked),
            );
            match call.await {
                Err(panicked) => {
                    self.log_contained_panic("model_backend", &panicked);
                    break Err(BackendError::Unavailable {
                        detail: "adapter_panic".into(),
                    });
                }
                Ok(Err(BackendError::Unavailable { .. })) if attempt < LIM_M3_BACKEND_RETRIES => {
                    attempt += 1
                }
                Ok(other) => break other,
            }
        };
        if result.is_ok() {
            measures.model_ms = Some(started.millis_until(self.deps.clock.now()).max(0));
            measures.usage = match contained_call(|| self.deps.backend.last_usage()) {
                Ok(usage) => usage,
                Err(panicked) => {
                    self.log_contained_panic("model_backend", &panicked);
                    None
                }
            };
        }
        let text_chars = match result {
            Ok(_) => match contained_call(|| self.deps.backend.last_text_chars()) {
                Ok(chars) => chars,
                Err(panicked) => {
                    self.log_contained_panic("model_backend", &panicked);
                    None
                }
            },
            Err(_) => None,
        };
        Ok(Decided {
            result,
            measures,
            transport,
            text_chars,
        })
    }

    /// Logs that a panic in an adapter was contained (REQ-SEC-011). The
    /// message is the adapter's own text: secrets are redacted, invisible and
    /// control characters removed, whitespace collapsed and the length cut at
    /// LIM-I5. It goes to the log only, never into an observation, an event
    /// or a prompt, and no stack trace is part of it.
    pub(super) fn log_contained_panic(&self, adapter: &'static str, panicked: &Panicked) {
        let message = bounded_message(&panicked.message, Some(&self.redactor));
        tracing::error!(
            target: "mimlys_core",
            adapter,
            panic_message = %message,
            "a panic in an adapter was contained"
        );
    }

    /// Calls a synchronous method of an adapter under containment
    /// (REQ-SEC-011). A panic is logged as above and is `unavailable` for
    /// the component, which the caller turns into its own fail-closed
    /// outcome.
    pub(super) fn guard<R>(
        &self,
        adapter: &'static str,
        call: impl FnOnce() -> R,
    ) -> Result<R, RuntimeError> {
        contained_call(call).map_err(|panicked| {
            self.log_contained_panic(adapter, &panicked);
            RuntimeError::Unavailable {
                component: adapter.to_owned(),
            }
        })
    }

    async fn new_step(
        &self,
        task: Task,
        env: &StepEnv,
        now: Instant,
    ) -> Result<StepResultView, RuntimeError> {
        let persistence = &self.deps.persistence;
        let index = persistence.load_steps(task.task_id)?.len() as u32;
        let mut step = Step::start(
            StepId::new(self.deps.ids.as_ref()),
            task.task_id,
            index,
            now,
        );
        persistence.record_step_started(
            &step,
            &self.event(
                Actor::Runtime,
                Some(task.task_id),
                Some(step.step_id),
                None,
                Event::StepStarted,
            ),
        )?;

        step.phase = StepPhase::Decide;
        let Decided {
            result: decided,
            measures,
            transport,
            text_chars,
        } = self.decide(&task, env, now).await?;
        step.measures = Some(measures);
        let raw = match decided {
            Ok(bytes) => bytes,
            Err(BackendError::Cancelled) => {
                return self.finish_step_into(
                    task.clone(),
                    task,
                    step,
                    StepOutcome::Cancelled,
                    now,
                );
            }
            Err(_) => {
                let halted =
                    task.transition(TaskInput::Halt(HaltReason::BackendUnavailable), now)?;
                return self.finish_step_into(task, halted, step, StepOutcome::Halted, now);
            }
        };

        step.phase = StepPhase::Validate;
        let outcome = env.validate(self, &task, task.pending.is_some(), &raw, now);
        step.decision_hash = outcome.decision_hash().cloned();
        let received = step.decision_hash.clone().map(|hash| {
            self.event(
                Actor::Model,
                Some(task.task_id),
                Some(step.step_id),
                None,
                Event::DecisionReceived {
                    hash,
                    transport,
                    text_chars,
                },
            )
        });
        match outcome {
            Outcome::Rejected { rejection, .. } => {
                if let Some(received) = received {
                    persistence.append_event(&received)?;
                }
                step.rejection = Some(rejection.code);
                persistence.record_validation(
                    &step,
                    None,
                    &self.event(
                        Actor::Runtime,
                        Some(task.task_id),
                        Some(step.step_id),
                        None,
                        Event::DecisionRejected {
                            code: rejection.code,
                        },
                    ),
                )?;
                self.finish_step(task, step, StepOutcome::Rejected, None, now)
            }
            Outcome::Control { decision, .. } => {
                let received = received.expect("a parsed decision has a hash");
                self.control(task, step, decision, received, env, now).await
            }
            Outcome::Authorized(authorized) => {
                drop(received);
                let action = self.propose(
                    &task,
                    &step,
                    &authorized,
                    env,
                    authorized.approval_required(),
                    now,
                );
                step.action_id = Some(action.action_id);
                persistence.record_proposed(
                    &step,
                    &action,
                    &self.event(
                        Actor::Model,
                        Some(task.task_id),
                        Some(step.step_id),
                        Some(action.action_id),
                        Event::ActionProposed,
                    ),
                )?; // 1
                persistence.record_validation(
                    &step,
                    Some(&action),
                    &self.event(
                        Actor::Model,
                        Some(task.task_id),
                        Some(step.step_id),
                        Some(action.action_id),
                        Event::DecisionReceived {
                            hash: authorized.decision_hash().clone(),
                            transport,
                            text_chars,
                        },
                    ),
                )?; // 2
                if authorized.approval_required() {
                    return self.request_approval(task, step, action, &authorized, env, now);
                }
                let action = action
                    .transition(ActionInput::Authorize, now)
                    .map_err(ordering)?;
                let action = self.snapshot_before(&task, action, &authorized, now)?;
                persistence.record_authorized(
                    &action,
                    None,
                    &self.event(
                        Actor::Runtime,
                        Some(task.task_id),
                        Some(step.step_id),
                        Some(action.action_id),
                        Event::ActionAuthorized,
                    ),
                )?; // 4
                let (step, action_outcome, task) = self
                    .execute(task, step, action, &authorized, env, now)
                    .await?;
                self.finish_step(
                    task,
                    step,
                    continued_or_cancelled(action_outcome),
                    action_outcome,
                    now,
                )
            }
        }
    }

    fn propose(
        &self,
        task: &Task,
        step: &Step,
        authorized: &AuthorizedAction,
        env: &StepEnv,
        approval_required: bool,
        now: Instant,
    ) -> Action {
        Action::propose(
            Proposal {
                action_id: ActionId::new(self.deps.ids.as_ref()),
                task_id: task.task_id,
                step_id: step.step_id,
                tool: authorized.tool().to_owned(),
                arguments: serde_json::Value::Object(authorized.arguments().clone()),
                grant_set_hash: env.grants.hash(now),
                effect: authorized.effect(),
                approval_required,
                deadline_ms: authorized.deadline_ms(),
            },
            now,
        )
    }

    /// Line 3: the approval request; the task waits (REQ-APR-001..006).
    fn request_approval(
        &self,
        task: Task,
        mut step: Step,
        action: Action,
        authorized: &AuthorizedAction,
        env: &StepEnv,
        now: Instant,
    ) -> Result<StepResultView, RuntimeError> {
        let persistence = &self.deps.persistence;
        let waiting_action = action
            .transition(ActionInput::RequireApproval, now)
            .map_err(ordering)?;
        let tool = env
            .visible
            .iter()
            .find(|t| t.name == authorized.tool())
            .cloned()
            .expect("the validator saw the tool");
        let host_tool = (authorized.tool() == crate::host_tool::RUN_HOST_TOOL)
            .then(|| authorized.arguments().get("tool").and_then(|v| v.as_str()))
            .flatten();
        let used: Vec<String> = env
            .grants
            .active(now)
            .filter_map(|g| match g.kind() {
                crate::grant::GrantKind::Tool { name } if name == authorized.tool() => {
                    Some(format!("tool:{name}"))
                }
                crate::grant::GrantKind::HostTool { name } if Some(name.as_str()) == host_tool => {
                    Some(format!("host_tool:{name}"))
                }
                _ => None,
            })
            .collect();
        // Every host-tool approval shows the tier in force (REQ-CONF-004, ED §13.4).
        let tier = host_tool.map(|_| {
            self.deps
                .probe
                .tier_with(&self.deps.policy.accept_best_effort)
                .describe()
        });
        let rendered = render(
            authorized,
            &tool,
            &env.workspace_root,
            &used,
            None,
            tier.as_deref(),
        );
        let approval = ApprovalRequest::new(
            ApprovalId::new(self.deps.ids.as_ref()),
            task.task_id,
            action.action_id,
            action.content().action_hash().clone(),
            env.grants.hash(now),
            rendered,
            authorized.reason(),
            now,
            LIM_A1_EXPIRY_MS.and_then(|ms| now.plus_millis(ms).ok()),
        );
        let waiting = task.transition(
            TaskInput::NeedsUser(PendingItem::Approval {
                action_id: action.action_id,
            }),
            now,
        )?;
        step.phase = StepPhase::Wait;
        persistence.record_approval_request(
            &waiting,
            &waiting_action,
            &approval,
            &self.event(
                Actor::Runtime,
                Some(task.task_id),
                Some(step.step_id),
                Some(action.action_id),
                Event::ApprovalRequested {
                    approval_id: approval.approval_id,
                },
            ),
        )?; // 3
        persistence.append_event(&self.event(
            Actor::Runtime,
            Some(task.task_id),
            None,
            None,
            state_changed(&task, &waiting),
        ))?;
        Ok(StepResultView {
            outcome: StepOutcome::Waited,
            state: waiting.state,
        })
    }

    /// Continues the step of an approved action: the approval is checked
    /// against the action and the grant set again, consumed with the
    /// authorization (one transition), then lines 5–8 run (REQ-APR-008, REQ-APR-009).
    async fn continue_approved(
        &self,
        task: Task,
        env: &StepEnv,
        mut approval: ApprovalRequest,
        now: Instant,
    ) -> Result<StepResultView, RuntimeError> {
        let persistence = &self.deps.persistence;
        let action = persistence
            .load_actions(task.task_id)?
            .into_iter()
            .find(|a| a.action_id == approval.action_id)
            .ok_or(RuntimeError::NotFound {
                what: "action".into(),
            })?;
        let mut step = persistence
            .load_steps(task.task_id)?
            .into_iter()
            .find(|s| s.step_id == action.step_id)
            .ok_or(RuntimeError::NotFound {
                what: "step".into(),
            })?;
        let raw = serde_json::to_vec(&serde_json::json!({
            "decision": "act", "tool": action.content().tool(), "arguments": action.content().arguments(),
        }))
        .expect("serializes");
        let authorized = match env.validate(self, &task, false, &raw, now) {
            Outcome::Authorized(a)
                if a.tool() == action.content().tool()
                    && serde_json::Value::Object(a.arguments().clone())
                        == *action.content().arguments() =>
            {
                a
            }
            _ => return self.invalidate_pending(task, step, action, approval, now),
        };
        if let Err(
            ApprovalError::Invalidated(_)
            | ApprovalError::WrongStatus { .. }
            | ApprovalError::NoteTooLong,
        ) = approval.consume(action.content().action_hash(), &env.grants.hash(now))
        {
            return self.invalidate_pending(task, step, action, approval, now);
        }
        let action = action
            .transition(ActionInput::Authorize, now)
            .map_err(ordering)?;
        let action = self.snapshot_before(&task, action, &authorized, now)?;
        step.phase = StepPhase::Execute;
        persistence.record_authorized(
            &action,
            Some(&approval),
            &self.event(
                Actor::Runtime,
                Some(task.task_id),
                Some(step.step_id),
                Some(action.action_id),
                Event::ActionAuthorized,
            ),
        )?; // 4
        let (step, action_outcome, task) = self
            .execute(task, step, action, &authorized, env, now)
            .await?;
        self.finish_step(
            task,
            step,
            continued_or_cancelled(action_outcome),
            action_outcome,
            now,
        )
    }

    /// The approval no longer matches the action or the grant set: it is
    /// invalidated, the action rejected and the step ends (REQ-APR-009).
    fn invalidate_pending(
        &self,
        task: Task,
        mut step: Step,
        action: Action,
        mut approval: ApprovalRequest,
        now: Instant,
    ) -> Result<StepResultView, RuntimeError> {
        let persistence = &self.deps.persistence;
        approval.invalidate(InvalidationReason::GrantsChanged);
        persistence.record_approval_decision(
            &task,
            &approval,
            &self.event(
                Actor::Runtime,
                Some(task.task_id),
                Some(step.step_id),
                Some(action.action_id),
                Event::ApprovalInvalidated {
                    approval_id: approval.approval_id,
                },
            ),
        )?;
        let rejected = action
            .transition(ActionInput::Reject(RejectionCode::Denied), now)
            .map_err(ordering)?;
        persistence.record_action_terminal(
            &rejected,
            &self.event(
                Actor::Runtime,
                Some(task.task_id),
                Some(step.step_id),
                Some(action.action_id),
                Event::ActionFinished {
                    status: rejected.status.name().to_owned(),
                    cause: None,
                },
            ),
        )?;
        step.rejection = Some(RejectionCode::Denied);
        self.finish_step(task, step, StepOutcome::Rejected, None, now)
    }

    async fn control(
        &self,
        task: Task,
        mut step: Step,
        decision: ControlDecision,
        received: crate::event::EventRecord,
        env: &StepEnv,
        now: Instant,
    ) -> Result<StepResultView, RuntimeError> {
        let persistence = &self.deps.persistence;
        match decision {
            ControlDecision::Plan { steps } => {
                let version = persistence
                    .load_plan(task.task_id)?
                    .map_or(1, |p| p.version + 1);
                let plan = PlanVersion {
                    task_id: task.task_id,
                    version,
                    steps,
                    created_at: now,
                };
                persistence.record_plan(&plan, &received)?;
                self.finish_step(task, step, StepOutcome::Continued, None, now)
            }
            ControlDecision::AskUser { question } => {
                persistence.append_event(&received)?;
                let question = Question::ask(
                    QuestionId::new(self.deps.ids.as_ref()),
                    task.task_id,
                    step.step_id,
                    &question,
                    now,
                    LIM_A1_EXPIRY_MS.and_then(|ms| now.plus_millis(ms).ok()),
                );
                let waiting = task.transition(
                    TaskInput::NeedsUser(PendingItem::Question {
                        question_id: question.question_id,
                    }),
                    now,
                )?;
                step.phase = StepPhase::Wait;
                persistence.record_question(
                    &waiting,
                    &question,
                    &self.event(
                        Actor::Model,
                        Some(task.task_id),
                        Some(step.step_id),
                        None,
                        Event::QuestionAsked {
                            question_id: question.question_id,
                        },
                    ),
                )?;
                self.finish_step_into(task, waiting, step, StepOutcome::Waited, now)
            }
            ControlDecision::Complete { .. } => {
                persistence.append_event(&received)?;
                match task.completion_step(None) {
                    CompletionStep::Finish(completion) => {
                        let completed = task.transition(TaskInput::Complete(completion), now)?;
                        self.finish_step_into(task, completed, step, StepOutcome::Completed, now)
                    }
                    CompletionStep::RunCheck => {
                        self.run_completion_check(task, step, env, now).await
                    }
                    CompletionStep::CheckFailed => unreachable!("no check result yet"),
                }
            }
            ControlDecision::GiveUp { reason } => {
                persistence.append_event(&received)?;
                let failed =
                    task.transition(TaskInput::Fail(FailureReason::ModelGaveUp { reason }), now)?;
                self.finish_step_into(task, failed, step, StepOutcome::Failed, now)
            }
        }
    }

    /// The completion check runs as a runtime-proposed action under the
    /// same validator; it never needs approval (REQ-TASK-007, external design §5.3).
    async fn run_completion_check(
        &self,
        task: Task,
        mut step: Step,
        env: &StepEnv,
        now: Instant,
    ) -> Result<StepResultView, RuntimeError> {
        let check = task
            .completion_check
            .clone()
            .expect("a check is configured");
        let raw = serde_json::to_vec(&serde_json::json!({"decision": "act", "tool": check.tool, "arguments": check.arguments})).expect("serializes");
        let authorized = match env.validate(self, &task, false, &raw, now) {
            Outcome::Authorized(a) if !a.approval_required() => a,
            _ => {
                let halted = task.transition(TaskInput::Halt(HaltReason::Policy), now)?;
                return self.finish_step_into(task, halted, step, StepOutcome::Halted, now);
            }
        };
        let persistence = &self.deps.persistence;
        let action = self.propose(&task, &step, &authorized, env, false, now);
        step.action_id = Some(action.action_id);
        persistence.record_proposed(
            &step,
            &action,
            &self.event(
                Actor::Runtime,
                Some(task.task_id),
                Some(step.step_id),
                Some(action.action_id),
                Event::ActionProposed,
            ),
        )?;
        persistence.record_validation(
            &step,
            Some(&action),
            &self.event(
                Actor::Runtime,
                Some(task.task_id),
                Some(step.step_id),
                Some(action.action_id),
                Event::DecisionReceived {
                    hash: authorized.decision_hash().clone(),
                    // The runtime's own check, not a model's decision: the envelope's form.
                    transport: DecisionTransport::Envelope,
                    text_chars: None,
                },
            ),
        )?;
        let action = action
            .transition(ActionInput::Authorize, now)
            .map_err(ordering)?;
        let action = self.snapshot_before(&task, action, &authorized, now)?;
        persistence.record_authorized(
            &action,
            None,
            &self.event(
                Actor::Runtime,
                Some(task.task_id),
                Some(step.step_id),
                Some(action.action_id),
                Event::ActionAuthorized,
            ),
        )?;
        let (step, action_outcome, task) = self
            .execute(task, step, action, &authorized, env, now)
            .await?;
        match (
            action_outcome,
            task.completion_step(Some(action_outcome == Some(ActionOutcome::Succeeded))),
        ) {
            (Some(ActionOutcome::Succeeded), CompletionStep::Finish(completion)) => {
                let completed = task.transition(TaskInput::Complete(completion), now)?;
                self.finish_step_into(task, completed, step, StepOutcome::Completed, now)
            }
            _ => self.finish_step(
                task,
                step,
                continued_or_cancelled(action_outcome),
                action_outcome,
                now,
            ),
        }
    }

    /// Lines 5–7 with runtime retries (REQ-TOOL-009). Returns the step, the
    /// action's outcome (`None` only when the execution was cancelled) and
    /// the task, halted when the disposition says so.
    async fn execute(
        &self,
        task: Task,
        mut step: Step,
        mut action: Action,
        authorized: &AuthorizedAction,
        env: &StepEnv,
        now: Instant,
    ) -> Result<(Step, Option<ActionOutcome>, Task), RuntimeError> {
        let persistence = &self.deps.persistence;
        let tool_actor = Actor::Tool {
            name: authorized.tool().to_owned(),
        };
        let mut unknown_after_restore = false;
        loop {
            step.phase = StepPhase::Execute;
            action = action
                .transition(ActionInput::Start, now)
                .map_err(ordering)?;
            persistence.record_running(
                &action,
                &self.event(
                    Actor::Runtime,
                    Some(task.task_id),
                    Some(step.step_id),
                    Some(action.action_id),
                    Event::ActionStarted,
                ),
            )?; // 5
            let started = self.deps.clock.now();
            // A panic in the provider is contained here (REQ-SEC-011): that
            // execution is `host_fault{unavailable}`, like a worker that
            // could not be set up; the observation carries no panic text.
            let call = contained(
                || {
                    env.tools.execute(
                        authorized,
                        ExecutionContext {
                            deadline_ms: authorized.deadline_ms(),
                            cancel: self.cancellation(task.task_id),
                        },
                    )
                },
                |panicked| self.log_contained_panic(PROVIDER, panicked),
            );
            let outcome = match call.await {
                Ok(outcome) => outcome,
                Err(panicked) => {
                    self.log_contained_panic(PROVIDER, &panicked);
                    let wall_ms = started.millis_until(self.deps.clock.now()).max(0);
                    crate::spi::ExecutionOutcome::host_fault(
                        crate::action::HostFaultDetail::Unavailable,
                        wall_ms,
                    )
                }
            };
            let ended = self.deps.clock.now();
            step.phase = StepPhase::Observe;
            let (status, cause) = match (&outcome.failure, &outcome.termination) {
                (Some(cause), _) => (ObservationStatus::Failed, Some(cause.clone())),
                (None, Termination::Cancelled) => (ObservationStatus::Cancelled, None),
                (None, termination) => match FailureCause::from_termination(termination) {
                    Some(cause) => (ObservationStatus::Failed, Some(cause)),
                    None => (ObservationStatus::Succeeded, None),
                },
            };
            // Transaction equals action (REQ-TOOL-010, REQ-WS-010): a write that failed is
            // restored to its pre-execution snapshot before its observation; one that was
            // cancelled is restored unless the profile keeps; one that succeeded is snapshotted.
            let mut rolled_back = false;
            let mut snapshot_after = None;
            let mut notices = outcome.notices.clone();
            if let Some(before) = action.snapshot_before {
                let restore = match status {
                    ObservationStatus::Failed => true,
                    ObservationStatus::Cancelled => {
                        env.profile.on_cancel == crate::policy::OnCancel::Rollback
                    }
                    _ => false,
                };
                if restore {
                    match self.restore(&task, &step, &action, before) {
                        Ok(()) => {
                            rolled_back = true;
                            notices.push(crate::observation::Notice::RolledBack);
                        }
                        // A failed restore leaves the outcome unknown: the task halts (REQ-PERS-008).
                        Err(_) => unknown_after_restore = true,
                    }
                } else if status == ObservationStatus::Succeeded {
                    let after = self.take_snapshot(
                        &task,
                        SnapshotReason::After,
                        Some(action.action_id),
                        now,
                    )?;
                    snapshot_after = Some(after.snapshot_id);
                }
            }
            let execution = Execution {
                execution_id: ExecutionId::new(self.deps.ids.as_ref()),
                action_id: action.action_id,
                attempt: action.executions,
                started_at: started,
                ended_at: ended,
                termination: outcome.termination.clone(),
                snapshot_before: action.snapshot_before,
                snapshot_after,
                stdout_blob: None,
                stderr_blob: None,
                metrics: outcome.metrics.clone(),
            };
            let outcome_with_notices = crate::spi::ExecutionOutcome {
                notices,
                ..outcome.clone()
            };
            let mut observation = Observation::from_outcome(
                ObservationId::new(self.deps.ids.as_ref()),
                action.action_id,
                authorized.tool(),
                &outcome_with_notices,
                Verdict {
                    status,
                    cause: cause.clone(),
                    rolled_back,
                },
                ended,
            );
            observation.redact(&self.redactor);
            step.observation_id = Some(observation.observation_id);
            let cause_name = cause.as_ref().map(FailureCause::name).map(str::to_owned);
            persistence.record_execution(
                &execution,
                &observation,
                &self.event(
                    tool_actor.clone(),
                    Some(task.task_id),
                    Some(step.step_id),
                    Some(action.action_id),
                    Event::ExecutionFinished {
                        attempt: execution.attempt,
                        termination: termination_name(&execution.termination),
                    },
                ),
            )?; // 6
            let input = match (status, &cause) {
                (ObservationStatus::Succeeded, _) => ActionInput::Succeed,
                (ObservationStatus::Cancelled, _) => ActionInput::Cancel,
                (_, Some(cause)) => ActionInput::Fail(cause.clone()),
                (_, None) => ActionInput::Fail(FailureCause::ToolError {
                    detail: "unknown".into(),
                }),
            };
            let terminal = action.transition(input, ended).map_err(ordering)?;
            persistence.record_action_terminal(
                &terminal,
                &self.event(
                    Actor::Runtime,
                    Some(task.task_id),
                    Some(step.step_id),
                    Some(action.action_id),
                    Event::ActionFinished {
                        status: terminal.status.name().to_owned(),
                        cause: cause_name,
                    },
                ),
            )?; // 7
            step.phase = StepPhase::Evaluate;
            let Some(cause) = cause else {
                return Ok((
                    step,
                    (status == ObservationStatus::Succeeded).then_some(ActionOutcome::Succeeded),
                    task,
                ));
            };
            if unknown_after_restore {
                let halted = task.transition(TaskInput::Halt(HaltReason::UnknownOutcome), ended)?;
                return Ok((step, Some(ActionOutcome::Failed), halted));
            }
            let d = disposition(
                &cause,
                authorized.effect(),
                terminal.executions,
                &task.budgets,
            );
            match d.kind {
                DispositionKind::Retry => match terminal.rearm_for_retry(&task.budgets) {
                    Ok(rearmed) => {
                        persistence.record_authorized(
                            &rearmed,
                            None,
                            &self.event(
                                Actor::Runtime,
                                Some(task.task_id),
                                Some(step.step_id),
                                Some(action.action_id),
                                Event::ActionAuthorized,
                            ),
                        )?;
                        action = rearmed;
                    }
                    Err(_) => return Ok((step, Some(ActionOutcome::Failed), task)),
                },
                DispositionKind::Halt { reason } => {
                    let halted = task.transition(TaskInput::Halt(reason), ended)?;
                    return Ok((step, Some(ActionOutcome::Failed), halted));
                }
                DispositionKind::Feedback => return Ok((step, Some(ActionOutcome::Failed), task)),
            }
        }
    }

    /// The source task's history for a fork, bounded by LIM-C5 (REQ-TASK-004).
    fn history_of(&self, source: TaskId) -> Result<super::context::History, RuntimeError> {
        let persistence = &self.deps.persistence;
        let task = self.load(source)?;
        let plan = persistence
            .load_plan(source)?
            .map(|p| p.steps)
            .unwrap_or_default();
        let actions = persistence.load_actions(source)?;
        let steps = persistence.load_steps(source)?;
        let skip = steps
            .len()
            .saturating_sub(crate::limits::LIM_C5_FORK_HISTORY_STEPS);
        let steps = steps
            .into_iter()
            .skip(skip)
            .map(|s| {
                let action = s
                    .action_id
                    .and_then(|id| actions.iter().find(|a| a.action_id == id));
                super::context::HistoryStep {
                    decision: match (&action, s.rejection, s.outcome) {
                        (Some(_), _, _) => "act".into(),
                        (None, Some(_), _) => "rejected".into(),
                        (None, None, Some(StepOutcome::Completed)) => "complete".into(),
                        (None, None, Some(StepOutcome::Failed)) => "give_up".into(),
                        (None, None, Some(StepOutcome::Waited)) => "ask_user".into(),
                        _ => "plan".into(),
                    },
                    tool: action.map(|a| a.content().tool().to_owned()),
                    outcome: s
                        .outcome
                        .map(|o| format!("{o:?}").to_ascii_lowercase())
                        .unwrap_or_else(|| "open".into()),
                    status: action.map(|a| a.status.name().to_owned()),
                }
            })
            .collect();
        Ok(super::context::History {
            task_id: source.to_string(),
            goal: task.goal,
            plan,
            steps,
        })
    }

    /// The pre-execution snapshot of a write-effect action, recorded with the
    /// authorization (line 4, REQ-WS-004).
    fn snapshot_before(
        &self,
        task: &Task,
        mut action: Action,
        authorized: &AuthorizedAction,
        now: Instant,
    ) -> Result<Action, RuntimeError> {
        if authorized.effect().kind.is_write() && action.snapshot_before.is_none() {
            let record =
                self.take_snapshot(task, SnapshotReason::Before, Some(action.action_id), now)?;
            action.snapshot_before = Some(record.snapshot_id);
        }
        Ok(action)
    }

    /// Takes a snapshot and records it: files first, the row, then the seal
    /// (REQ-PERS-009). A manifest equal to the task's latest snapshot reuses it.
    pub(crate) fn take_snapshot(
        &self,
        task: &Task,
        reason: SnapshotReason,
        action_id: Option<ActionId>,
        now: Instant,
    ) -> Result<SnapshotRecord, RuntimeError> {
        let persistence = &self.deps.persistence;
        let previous = persistence.load_snapshots(task.task_id)?.into_iter().last();
        let prepared = self.deps.workspaces.snapshot(
            task.task_id,
            crate::ids::SnapshotId::new(self.deps.ids.as_ref()),
            reason,
            action_id,
            previous.as_ref(),
            now,
        )?;
        if prepared.reused {
            return Ok(prepared.record);
        }
        persistence.record_snapshot(
            &prepared.record,
            &prepared.objects,
            &self.event(
                Actor::Runtime,
                Some(task.task_id),
                None,
                action_id,
                Event::WorkspaceSnapshot {
                    snapshot_id: prepared.record.snapshot_id,
                    reason: reason.name().to_owned(),
                },
            ),
        )?;
        self.deps.workspaces.seal(prepared.record.snapshot_id)?;
        Ok(prepared.record)
    }

    /// Restores a snapshot and records `workspace.rollback` (REQ-WS-006).
    fn restore(
        &self,
        task: &Task,
        step: &Step,
        action: &Action,
        snapshot_id: crate::ids::SnapshotId,
    ) -> Result<(), RuntimeError> {
        let record =
            self.deps
                .persistence
                .load_snapshot(snapshot_id)?
                .ok_or(RuntimeError::NotFound {
                    what: "snapshot".into(),
                })?;
        self.deps.workspaces.restore(task.task_id, &record)?;
        self.deps.persistence.append_event(&self.event(
            Actor::Runtime,
            Some(task.task_id),
            Some(step.step_id),
            Some(action.action_id),
            Event::WorkspaceRollback { snapshot_id },
        ))?;
        Ok(())
    }

    /// Line 8: the step outcome, the counters and the task's state; a budget
    /// reached at this boundary halts the task (REQ-TASK-010).
    fn finish_step(
        &self,
        task: Task,
        step: Step,
        outcome: StepOutcome,
        action: Option<ActionOutcome>,
        now: Instant,
    ) -> Result<StepResultView, RuntimeError> {
        let mut after = task.clone();
        let budget = after.counters.apply(
            &StepResult {
                outcome,
                action,
                decision_hash: step.decision_hash.clone(),
                snapshot: None,
            },
            &after.budgets,
        );
        if let (Some(reason), TaskState::Running) = (budget, &after.state) {
            after = after.transition(TaskInput::Halt(reason), now)?;
        }
        // REQ-WS-008: the quota is read from the latest snapshot at the boundary.
        if after.state == TaskState::Running
            && self.deps.workspaces.remaining_bytes(after.task_id)? == Some(0)
        {
            after = after.transition(TaskInput::Halt(HaltReason::WorkspaceQuota), now)?;
        }
        let outcome = if matches!(after.state, TaskState::Halted { .. }) {
            StepOutcome::Halted
        } else {
            outcome
        };
        self.finish_step_into(task, after, step, outcome, now)
    }

    fn finish_step_into(
        &self,
        before: Task,
        after: Task,
        mut step: Step,
        outcome: StepOutcome,
        now: Instant,
    ) -> Result<StepResultView, RuntimeError> {
        let persistence = &self.deps.persistence;
        step.finish(outcome, now);
        persistence.record_step_outcome(
            &step,
            &after,
            &self.event(
                Actor::Runtime,
                Some(after.task_id),
                Some(step.step_id),
                step.action_id,
                Event::StepFinished { outcome },
            ),
        )?; // 8
        if before.state != after.state {
            persistence.append_event(&self.event(
                Actor::Runtime,
                Some(after.task_id),
                None,
                None,
                state_changed(&before, &after),
            ))?;
        }
        Ok(StepResultView {
            outcome,
            state: after.state,
        })
    }

    pub(crate) fn persist_task(&self, before: &Task, after: &Task) -> Result<(), RuntimeError> {
        self.deps.persistence.record_task_transition(
            after,
            &self.event(
                Actor::Runtime,
                Some(after.task_id),
                None,
                None,
                state_changed(before, after),
            ),
        )?;
        Ok(())
    }

    pub(crate) fn load(&self, task_id: TaskId) -> Result<Task, RuntimeError> {
        self.deps
            .persistence
            .load_task(task_id)?
            .ok_or(RuntimeError::NotFound {
                what: "task".into(),
            })
    }

    pub(crate) fn profile_of(&self, task: &Task) -> Result<Profile, RuntimeError> {
        self.deps
            .policy
            .profile(&task.profile)
            .cloned()
            .ok_or_else(|| RuntimeError::InvalidRequest {
                detail: format!("profile {} is not in the policy", task.profile),
            })
    }
}

/// An execution that was cancelled (no action outcome) ends its step
/// `cancelled`; any other ends it `continued`.
fn continued_or_cancelled(outcome: Option<ActionOutcome>) -> StepOutcome {
    if outcome.is_none() {
        StepOutcome::Cancelled
    } else {
        StepOutcome::Continued
    }
}

/// The termination's name for the `execution.finished` event.
fn termination_name(termination: &Termination) -> String {
    serde_json::to_value(termination)
        .ok()
        .and_then(|v| {
            v.get("termination")
                .and_then(|t| t.as_str())
                .map(str::to_owned)
        })
        .unwrap_or_default()
}

/// The `task.state_changed` event between two task values.
pub(crate) fn state_changed(before: &Task, after: &Task) -> Event {
    Event::TaskStateChanged {
        from: before.state.name().to_owned(),
        to: after.state.name().to_owned(),
        reason: match &after.state {
            TaskState::Halted { halt_reason } => Some(*halt_reason),
            _ => None,
        },
    }
}

/// An illegal action transition can only come from a record the runtime did
/// not write in order; it is reported as a persistence ordering error.
fn ordering(e: crate::action::ActionStateError) -> RuntimeError {
    RuntimeError::Persistence(crate::spi::PersistenceError::Ordering {
        detail: e.to_string(),
    })
}
