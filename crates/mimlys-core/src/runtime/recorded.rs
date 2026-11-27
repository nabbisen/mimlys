//! The recorded outcome of a request that is sent again (external design §4, "The recorded
//! outcome"): what the request did, as its events say, and never a new error for a change that
//! took effect.
//!
//! The events a request's change wrote carry its id, and a verb holds its task while it writes
//! (a step cannot, and no other request can), so they are consecutive in the task's events. They
//! are read from the first one found, as long as they carry the id: a handful, whatever the
//! length of the task's history. The verb's result, when it is not the task, is rebuilt from the
//! store with what those events name (a grant, a snapshot, a promotion).

use super::{Runtime, RuntimeError};
use crate::api::{AlreadyApplied, Did, Reply, Request};
use crate::event::Event;
use crate::ids::{RequestId, SnapshotId, TaskId};
use crate::spi::PositionedEvent;

/// How many of a task's events are read at once while the request's own are collected.
const PAGE: usize = 32;

impl Runtime {
    /// The answer to a request whose id is recorded: `found` is the first event its change wrote.
    pub(super) fn recorded_outcome(
        &self,
        request_id: RequestId,
        request: &Request,
        found: PositionedEvent,
    ) -> Result<AlreadyApplied, RuntimeError> {
        let task_id = found.record.task_id.or(request.task_id());
        let task = task_id
            .map(|id| self.deps.persistence.load_task(id))
            .transpose()?
            .flatten();
        let events = self.events_of_request(task_id, request_id, &found)?;
        let did = match (&task, task_id) {
            (Some(_), Some(task_id)) => self.did(task_id, request, &events)?,
            // The task was deleted since: what its change made went with it.
            _ => Did::Applied { reply: None },
        };
        Ok(AlreadyApplied {
            position: found.position,
            event: found.record.event.name().to_owned(),
            task: task.map(Box::new),
            did,
        })
    }

    /// The events of the request, in order, from the first: those that carry its id, up to the
    /// first of the task's events that does not.
    fn events_of_request(
        &self,
        task_id: Option<TaskId>,
        request_id: RequestId,
        first: &PositionedEvent,
    ) -> Result<Vec<PositionedEvent>, RuntimeError> {
        let mut out = vec![first.clone()];
        let mut after = first.position;
        loop {
            let page = self.deps.persistence.events_since(after, task_id, PAGE)?;
            let full = page.len() == PAGE;
            for event in page {
                if event.record.request_id != Some(request_id) {
                    return Ok(out);
                }
                after = event.position;
                out.push(event);
            }
            if !full {
                return Ok(out);
            }
        }
    }

    /// What the change came to, from its events, with the verb's result rebuilt where it is not
    /// the task.
    fn did(
        &self,
        task_id: TaskId,
        request: &Request,
        events: &[PositionedEvent],
    ) -> Result<Did, RuntimeError> {
        let named = |pick: fn(&Event) -> Option<SnapshotId>| {
            events.iter().find_map(|e| pick(&e.record.event))
        };
        let persistence = &self.deps.persistence;
        let reply = match request {
            Request::GrantCapability { .. } => {
                let issued = events.iter().find_map(|e| match &e.record.event {
                    Event::GrantIssued { grant_id } => Some(*grant_id),
                    _ => None,
                });
                issued.and_then(|id| {
                    persistence
                        .load_grants(task_id)
                        .ok()?
                        .into_iter()
                        .find(|g| g.grant_id == id)
                        .map(Reply::Grant)
                })
            }
            Request::RevokeCapability { grant_id, .. } => persistence
                .load_grants(task_id)?
                .into_iter()
                .find(|g| g.grant_id == *grant_id)
                .map(Reply::Grant),
            Request::RollbackWorkspace { .. } => {
                let restored = named(|e| match e {
                    Event::WorkspaceRollback { snapshot_id } => Some(*snapshot_id),
                    _ => None,
                });
                match restored {
                    Some(id) => persistence.load_snapshot(id)?.map(Reply::Snapshot),
                    None => None,
                }
            }
            Request::PromoteWorkspace { .. } => {
                if let Some((event, reason, kind)) =
                    events.iter().find_map(|e| match &e.record.event {
                        Event::WorkspacePromotionFailed { reason, kind } => {
                            Some((e.record.event.name(), reason.clone(), kind.clone()))
                        }
                        _ => None,
                    })
                {
                    return Ok(Did::Failed {
                        event: event.to_owned(),
                        reason,
                        kind,
                    });
                }
                if !events
                    .iter()
                    .any(|e| matches!(e.record.event, Event::WorkspacePromoted { .. }))
                {
                    // It wrote the backup, and perhaps that the promotion began, and stopped:
                    // never "applied".
                    let last = events.last().map_or("", |e| e.record.event.name());
                    return Ok(Did::Failed {
                        event: last.to_owned(),
                        reason: format!(
                            "the promotion did not complete: its request wrote up to `{last}`, \
                             and nothing after it"
                        ),
                        // Nothing recorded what stopped it (a crash, or a store that could not
                        // be written): no kind to keep.
                        kind: None,
                    });
                }
                let backup = named(|e| match e {
                    Event::WorkspaceSnapshot { snapshot_id, .. } => Some(*snapshot_id),
                    _ => None,
                });
                persistence
                    .load_promotions(task_id)?
                    .into_iter()
                    .find(|p| Some(p.backup_snapshot) == backup)
                    .map(Reply::Promotion)
            }
            Request::RevertPromotion { promotion_id, .. } => persistence
                .load_promotions(task_id)?
                .into_iter()
                .find(|p| p.promotion_id == *promotion_id)
                .map(Reply::Promotion),
            // The verbs that return the task, and a deletion, whose footprint is not kept.
            _ => None,
        };
        Ok(Did::Applied { reply })
    }
}
