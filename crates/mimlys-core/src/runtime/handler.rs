//! The one handler of the control API (external design §4; REQ-ARCH-004; RFC 014, Amendment 1,
//! item 2): a request in, a response out.

use super::{Runtime, RuntimeError, Session};
use crate::api::{Envelope, Outcome, Reply, Request, Response};

impl Runtime {
    /// Handles one request of a client: the way into the verbs of external design §4.
    ///
    /// It blocks where the verb blocks (`cancel_task` waits for the step in flight, as 014a made
    /// it), so a caller that must not block runs it on a thread of its own, and it is never made
    /// from inside the step it would wait for. A failure is the response's `Failed` outcome, with
    /// the error exactly as the verb reports it.
    pub fn handle(&self, envelope: Envelope) -> Response {
        let Envelope {
            request_id,
            session,
            request,
        } = envelope;
        let outcome = match self.apply(&session, request) {
            Ok(reply) => Outcome::Done(reply),
            Err(error) => Outcome::Failed(error),
        };
        Response {
            request_id,
            outcome,
        }
    }

    /// The request's verb, with the arguments it carries.
    fn apply(&self, session: &Session, request: Request) -> Result<Reply, RuntimeError> {
        Ok(match request {
            Request::CreateTask(create) => {
                Reply::Task(Box::new(self.create_task(session, create)?))
            }
            Request::ResumeTask { task_id, extend } => {
                Reply::Task(Box::new(self.resume_task(session, task_id, extend)?))
            }
            Request::PauseTask { task_id } => {
                Reply::Task(Box::new(self.pause_task(session, task_id)?))
            }
            Request::CancelTask { task_id } => {
                Reply::Task(Box::new(self.cancel_task(session, task_id)?))
            }
            Request::ApproveAction { task_id, note } => {
                Reply::Task(Box::new(self.approve_action(session, task_id, note)?))
            }
            Request::RejectAction {
                task_id,
                note,
                halt,
            } => Reply::Task(Box::new(self.reject_action(session, task_id, note, halt)?)),
            Request::AnswerQuestion { task_id, answer } => {
                Reply::Task(Box::new(self.answer_question(session, task_id, &answer)?))
            }
            Request::GrantCapability { task_id, kind } => {
                Reply::Grant(self.grant_capability(session, task_id, kind)?)
            }
            Request::RevokeCapability { task_id, grant_id } => {
                Reply::Grant(self.revoke_capability(session, task_id, grant_id)?)
            }
            Request::RollbackWorkspace {
                task_id,
                snapshot_id,
            } => Reply::Snapshot(self.rollback_workspace(session, task_id, snapshot_id)?),
            Request::PromoteWorkspace { task_id, plan } => {
                Reply::Promotion(self.promote_workspace(session, task_id, &plan)?)
            }
            Request::RevertPromotion {
                task_id,
                promotion_id,
                overwrite_edited,
            } => Reply::Promotion(self.revert_promotion(
                session,
                task_id,
                promotion_id,
                overwrite_edited,
            )?),
            Request::DeleteTask { task_id, audit } => {
                Reply::Footprint(self.delete_task(session, task_id, audit)?)
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
}
