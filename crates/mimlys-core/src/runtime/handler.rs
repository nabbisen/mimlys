//! The one handler of the control API (external design §4; REQ-ARCH-004; RFC 014, Amendment 1,
//! item 2): a request in, a response out.
//!
//! **A request on a task whose step is in flight** (external design §4; handoff 014b, item 5)
//! follows the rule of its verb, and no write is made to the task while its step holds it,
//! except by the step: a pause and a stop are *noted* and taken in the step's own turn, at its
//! end; a grant and a revocation *wait* for the step to end and apply from a fresh read; the
//! others need a state that has no step, and are *refused* as the state they would be refused
//! in. The slot (`runtime::steps`) is where the three are kept.

use super::steps::{Held, Note, Noted, OnStep, WriteGuard};
use super::{Runtime, RuntimeError, Session};
use crate::api::{Envelope, Notice, Outcome, Reply, Request, Response};
use crate::ids::{RequestId, TaskId};
use crate::task::{Task, TaskState};

impl Runtime {
    /// Handles one request of a client: the way into the verbs of external design §4.
    ///
    /// It blocks where the verb blocks (`cancel_task` waits for the step in flight, as 014a made
    /// it; a grant waits for it too), so a caller that must not block runs it on a thread of its
    /// own, and it is never made from inside the step it would wait for. A failure is the
    /// response's `Failed` outcome, with the error exactly as the verb reports it.
    pub fn handle(&self, envelope: Envelope) -> Response {
        self.handle_noticing(envelope, &|_| {})
    }

    /// As [`Runtime::handle`], and tells `notice` what a client says to the user while the
    /// request waits (a grant waits for the step in flight to end).
    pub fn handle_noticing(&self, envelope: Envelope, notice: &dyn Fn(Notice)) -> Response {
        let Envelope {
            request_id,
            session,
            request,
        } = envelope;
        let outcome = match self.apply(request_id, &session, request, notice) {
            Ok(reply) => Outcome::Done(reply),
            Err(error) => Outcome::Failed(error),
        };
        Response {
            request_id,
            outcome,
        }
    }

    /// The task held for a request that needs no step in flight: refused, as `state`, while a
    /// step is in flight and the task is `running`; held once the step has ended otherwise.
    fn held_without_step(
        &self,
        task_id: TaskId,
        verb: &'static str,
    ) -> Result<WriteGuard<'_>, RuntimeError> {
        self.held(
            task_id,
            OnStep::UnlessRunning(
                verb,
                &|| Ok(self.load(task_id)?.state == TaskState::Running),
            ),
        )
    }

    fn held(&self, task_id: TaskId, on_step: OnStep<'_>) -> Result<WriteGuard<'_>, RuntimeError> {
        match self.steps.begin_write(task_id, on_step)? {
            Held::Claimed(guard) => Ok(guard),
            // Only a request that is noted is noted.
            Held::Noted => Err(RuntimeError::Conflict {
                detail: format!("task {task_id} was not claimed"),
            }),
        }
    }

    /// A pause or a stop: noted when a step is in flight (the reply says so), applied by
    /// `apply` when none is.
    fn noted_or(
        &self,
        task_id: TaskId,
        noted: Noted,
        apply: impl FnOnce() -> Result<Task, RuntimeError>,
    ) -> Result<Reply, RuntimeError> {
        match self.steps.begin_write(task_id, OnStep::Note(noted))? {
            Held::Noted => Ok(Reply::Noted(Box::new(self.load(task_id)?))),
            Held::Claimed(_held) => Ok(Reply::Task(Box::new(apply()?))),
        }
    }

    /// The request's verb, with the arguments it carries.
    fn apply(
        &self,
        request_id: RequestId,
        session: &Session,
        request: Request,
        notice: &dyn Fn(Notice),
    ) -> Result<Reply, RuntimeError> {
        let note = || Note {
            request_id,
            session: session.clone(),
        };
        Ok(match request {
            Request::CreateTask(create) => {
                Reply::Task(Box::new(self.create_task(session, create)?))
            }
            Request::ResumeTask { task_id, extend } => {
                let _held = self.held_without_step(task_id, "resume_task")?;
                Reply::Task(Box::new(self.resume_task(session, task_id, extend)?))
            }
            Request::PauseTask { task_id } => {
                self.noted_or(task_id, Noted::Pause(note()), || {
                    self.pause_task(session, task_id)
                })?
            }
            Request::CancelTask { task_id } => {
                Reply::Task(Box::new(self.cancel_task(session, task_id)?))
            }
            Request::ApproveAction { task_id, note } => {
                let _held = self.held_without_step(task_id, "approve_action")?;
                Reply::Task(Box::new(self.approve_action(session, task_id, note)?))
            }
            Request::RejectAction {
                task_id,
                note,
                halt,
            } => {
                let _held = self.held_without_step(task_id, "reject_action")?;
                Reply::Task(Box::new(self.reject_action(session, task_id, note, halt)?))
            }
            Request::AnswerQuestion { task_id, answer } => {
                let _held = self.held_without_step(task_id, "answer_question")?;
                Reply::Task(Box::new(self.answer_question(session, task_id, &answer)?))
            }
            Request::GrantCapability { task_id, kind } => {
                let _held = self.held(
                    task_id,
                    OnStep::Wait(&|| notice(Notice::WaitingForStep { task_id })),
                )?;
                Reply::Grant(self.grant_capability(session, task_id, kind)?)
            }
            Request::RevokeCapability { task_id, grant_id } => {
                let _held = self.held(
                    task_id,
                    OnStep::Wait(&|| notice(Notice::WaitingForStep { task_id })),
                )?;
                Reply::Grant(self.revoke_capability(session, task_id, grant_id)?)
            }
            Request::RollbackWorkspace {
                task_id,
                snapshot_id,
            } => {
                let _held = self.held_without_step(task_id, "rollback_workspace")?;
                Reply::Snapshot(self.rollback_workspace(session, task_id, snapshot_id)?)
            }
            Request::PromoteWorkspace { task_id, plan } => {
                let _held = self.held_without_step(task_id, "promote_workspace")?;
                Reply::Promotion(self.promote_workspace(session, task_id, &plan)?)
            }
            Request::RevertPromotion {
                task_id,
                promotion_id,
                overwrite_edited,
            } => {
                let _held = self.held_without_step(task_id, "revert_promotion")?;
                Reply::Promotion(self.revert_promotion(
                    session,
                    task_id,
                    promotion_id,
                    overwrite_edited,
                )?)
            }
            Request::DeleteTask { task_id, audit } => {
                let _held = self.held_without_step(task_id, "delete_task")?;
                Reply::Footprint(self.delete_task(session, task_id, audit)?)
            }
            Request::StopAtBoundary { task_id } => {
                // The request first: an action the step has not started yet is closed without
                // starting, and the pause is taken at the boundary (REQ-CLI-005).
                self.start_no_new_action(task_id)?;
                self.noted_or(task_id, Noted::Pause(note()), || {
                    self.pause_task(session, task_id)
                })?
            }
            Request::InterruptStep { task_id } => {
                self.interrupt_step(task_id)?;
                Reply::Interrupted {
                    step_in_flight: self.steps.step_in_flight(task_id),
                }
            }
            Request::StopAnyway { task_id } => {
                self.interrupt_step_anyway(task_id)?;
                self.noted_or(task_id, Noted::Stop(note()), || {
                    let task = self.load(task_id)?;
                    if task.state == TaskState::Running {
                        self.cancel_idle_task(session, task_id)
                    } else {
                        Ok(task)
                    }
                })?
            }
            Request::PlanEffort {
                profile,
                effort,
                from_task,
            } => {
                let name = profile.unwrap_or_else(|| self.deps.policy.default_profile.clone());
                let profile = self.deps.policy.profile(&name).ok_or_else(|| {
                    RuntimeError::InvalidRequest {
                        detail: format!("unknown profile {name}"),
                    }
                })?;
                Reply::EffortPlan(self.plan_effort(profile, effort, from_task)?)
            }
            Request::ModelChange { task_id } => {
                Reply::ModelChange(self.model_change(&self.load(task_id)?)?)
            }
            Request::StartRefusal { task_id } => {
                Reply::StartRefusal(self.start_refusal(&self.load(task_id)?)?)
            }
            Request::CreationForbidden { profile, backend } => {
                Reply::CreationForbidden(self.creation_forbidden(profile.as_deref(), &backend)?)
            }
        })
    }

    /// What was noted for the end of the step of `task_id`, applied in the step's own turn: a
    /// stop cancels a task the step left `running`, and a pause is requested of it, so that the
    /// step that follows takes it at its boundary. A task the step left anywhere else (waiting,
    /// halted, paused, ended) is not touched: the step's own end said what happened to it.
    pub(super) fn apply_noted(
        &self,
        task_id: TaskId,
        running: &mut bool,
        pause: Option<Note>,
        stop: Option<Note>,
    ) {
        if !*running {
            return;
        }
        if let Some(stop) = stop {
            if let Err(error) = self.cancel_idle_task(&stop.session, task_id) {
                tracing::warn!(
                    target: "mimlys_core",
                    task = %task_id,
                    %error,
                    "the stop noted for the end of the step could not be applied"
                );
            }
            *running = false;
            return;
        }
        if let Some(pause) = pause
            && let Err(error) = self.pause_task(&pause.session, task_id)
        {
            tracing::warn!(
                target: "mimlys_core",
                task = %task_id,
                %error,
                "the pause noted for the end of the step could not be applied"
            );
        }
    }
}
