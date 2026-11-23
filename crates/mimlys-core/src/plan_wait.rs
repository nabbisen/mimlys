//! The wait of a plan-first task for its plan's approval (REQ-TASK-015; RFC 015 decision 6,
//! Amendment 1; external design §5, "Plan first").
//!
//! The wait lives on **the step** that made the plan, as an optional field of its record: the step
//! stays open in the `wait` phase while the user decides, exactly as an action's step does, and
//! the decision, its note and the instant it was made are recorded with it. A plan version is
//! immutable (REQ-DEC-011), so the decision is not on the version; and no table is added, so no
//! store migration is needed (RFC 015, Amendment 1, item 6): a record that has no `plan_wait` is
//! every record written before 0.13.

use crate::clock::Instant;
use crate::event::{Actor, ApprovalDecision};
use crate::limits::LIM_D2_REASON_CHARS;
use crate::observation::sanitize;
use serde::{Deserialize, Serialize};

/// What the user decided about the plan, and when.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanDecided {
    /// Approved, rejected, or expired (LIM-A1).
    pub decision: ApprovalDecision,
    /// When.
    pub at: Instant,
    /// Who: the user's session, or the runtime for an expiry.
    pub by: Actor,
    /// The user's note, sanitized and bounded as a rejection's note is (LIM-D2). For an approval
    /// it reaches the model as `<<user kind="approval_note">>`, for a rejection as
    /// `<<user kind="rejection_note">>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// The wait of a step for the approval of the plan version it made.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanWait {
    /// The plan version that waits.
    pub version: u32,
    /// When the wait began.
    pub requested_at: Instant,
    /// When it expires, when it does (LIM-A1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<Instant>,
    /// What was decided, once something was. Absent while the wait is pending, and when the wait
    /// ended with no decision (the task was cancelled).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decided: Option<PlanDecided>,
}

/// A plan's note was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("the note exceeds LIM-D2")]
pub struct NoteTooLong;

impl PlanWait {
    /// A wait that begins at `now`.
    pub fn request(version: u32, now: Instant, expires_at: Option<Instant>) -> Self {
        Self {
            version,
            requested_at: now,
            expires_at,
            decided: None,
        }
    }

    /// Whether the wait still waits for the user.
    pub fn is_pending(&self) -> bool {
        self.decided.is_none()
    }

    /// Records the user's decision. A note is bounded by LIM-D2, as a rejection's note is, and
    /// sanitized (it reaches the model as the user's own words, never as markup).
    pub fn decide(
        &mut self,
        decision: ApprovalDecision,
        by: Actor,
        note: Option<String>,
        now: Instant,
    ) -> Result<(), NoteTooLong> {
        if note
            .as_ref()
            .is_some_and(|n| n.chars().count() > LIM_D2_REASON_CHARS)
        {
            return Err(NoteTooLong);
        }
        self.decided = Some(PlanDecided {
            decision,
            at: now,
            by,
            note: note.map(|n| sanitize(&n).0),
        });
        Ok(())
    }

    /// Expires a pending wait whose `expires_at` has passed (LIM-A1). Whether it did.
    pub fn expire(&mut self, now: Instant) -> bool {
        if self.is_pending() && self.expires_at.is_some_and(|t| now >= t) {
            self.decided = Some(PlanDecided {
                decision: ApprovalDecision::Expired,
                at: now,
                by: Actor::Runtime,
                note: None,
            });
            true
        } else {
            false
        }
    }

    /// The note of an approval, when the wait was approved with one.
    pub fn approval_note(&self) -> Option<&str> {
        self.decided
            .as_ref()
            .filter(|d| d.decision == ApprovalDecision::Approved)
            .and_then(|d| d.note.as_deref())
    }

    /// The note of a rejection, when the wait was rejected with one.
    pub fn rejection_note(&self) -> Option<&str> {
        self.decided
            .as_ref()
            .filter(|d| d.decision == ApprovalDecision::Rejected)
            .and_then(|d| d.note.as_deref())
    }
}
