//! Questions from the model to the user (external design §10.3;
//! REQ-APR-010): the same waiting mechanism and expiry as approvals; the
//! answer enters the next step as `user` text.

use crate::clock::Instant;
use crate::event::Actor;
use crate::ids::{QuestionId, StepId, TaskId};
use crate::limits::LIM_D4_TEXT_CHARS;
use crate::observation::sanitize;
use serde::{Deserialize, Serialize};

/// The status of a question.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuestionStatus {
    /// Waiting for the user.
    Pending,
    /// Answered.
    Answered,
    /// Expired.
    Expired,
}

/// A question operation was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum QuestionError {
    /// Not pending.
    #[error("the question is not pending")]
    NotPending,
    /// The answer exceeds LIM-D4.
    #[error("the answer exceeds LIM-D4")]
    AnswerTooLong,
    /// The answer is empty.
    #[error("the answer is empty")]
    AnswerEmpty,
}

/// A question to the user (external design §10.3).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Question {
    /// Identifier.
    pub question_id: QuestionId,
    /// The task.
    pub task_id: TaskId,
    /// The step that asked.
    pub step_id: StepId,
    /// The question, sanitized and bounded by LIM-D4.
    pub text: String,
    /// When asked.
    pub asked_at: Instant,
    /// Expiry, when any (LIM-A1).
    pub expires_at: Option<Instant>,
    /// Status.
    pub status: QuestionStatus,
    /// The answer, sanitized and bounded by LIM-D4.
    pub answer: Option<String>,
    /// Who answered.
    pub answered_by: Option<Actor>,
    /// When answered or expired.
    pub answered_at: Option<Instant>,
}

impl Question {
    /// Records a pending question; the model's text is sanitized and bounded.
    pub fn ask(
        question_id: QuestionId,
        task_id: TaskId,
        step_id: StepId,
        text: &str,
        now: Instant,
        expires_at: Option<Instant>,
    ) -> Self {
        let (clean, _) = sanitize(text);
        Self {
            question_id,
            task_id,
            step_id,
            text: clean.chars().take(LIM_D4_TEXT_CHARS).collect(),
            asked_at: now,
            expires_at,
            status: QuestionStatus::Pending,
            answer: None,
            answered_by: None,
            answered_at: None,
        }
    }

    /// Records the user's answer (REQ-APR-010).
    pub fn answer(&mut self, text: &str, by: Actor, now: Instant) -> Result<(), QuestionError> {
        if self.status != QuestionStatus::Pending {
            return Err(QuestionError::NotPending);
        }
        let (clean, _) = sanitize(text);
        let count = clean.chars().count();
        if count == 0 {
            return Err(QuestionError::AnswerEmpty);
        }
        if count > LIM_D4_TEXT_CHARS {
            return Err(QuestionError::AnswerTooLong);
        }
        self.answer = Some(clean);
        self.answered_by = Some(by);
        self.answered_at = Some(now);
        self.status = QuestionStatus::Answered;
        Ok(())
    }

    /// Expires a pending question whose `expires_at` has passed (REQ-APR-006).
    pub fn expire(&mut self, now: Instant) -> bool {
        if self.status == QuestionStatus::Pending && self.expires_at.is_some_and(|t| now >= t) {
            self.status = QuestionStatus::Expired;
            self.answered_at = Some(now);
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests;
