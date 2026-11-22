use super::*;
use crate::clock::{Clock, FakeClock};
use crate::hash::hash_bytes;
use crate::ids::ScriptedIds;

fn every_event() -> Vec<Event> {
    let ids = ScriptedIds::default();
    vec![
        Event::TaskCreated,
        Event::TaskStateChanged {
            from: "running".into(),
            to: "halted".into(),
            reason: Some(HaltReason::Convergence),
            rollback_failed: false,
            halt_detail: None,
        },
        Event::TaskAdmissionChanged {
            state: AdmissionState::Waiting,
            slots_taken: Some(2),
            reason: None,
            waited_ms: None,
        },
        Event::TaskAdmissionChanged {
            state: AdmissionState::Admitted,
            slots_taken: Some(2),
            reason: None,
            waited_ms: Some(1500),
        },
        Event::TaskAdmissionChanged {
            state: AdmissionState::Unadmitted,
            slots_taken: None,
            reason: Some("the registry is not writable".into()),
            waited_ms: None,
        },
        Event::TaskPauseRequested,
        Event::TaskDeleted { audit: false },
        Event::StepStarted,
        Event::StepFinished {
            outcome: StepOutcome::Continued,
        },
        Event::DecisionReceived {
            hash: hash_bytes(b"d"),
            transport: Default::default(),
            text_chars: None,
        },
        Event::DecisionRejected {
            code: RejectionCode::Malformed,
            reason: None,
            grant_kind: None,
        },
        Event::ActionProposed,
        Event::ActionAuthorized,
        Event::ActionStarted,
        Event::ExecutionFinished {
            attempt: 1,
            termination: "exited".into(),
        },
        Event::ActionFinished {
            status: "failed".into(),
            cause: Some("timeout".into()),
        },
        Event::ApprovalRequested {
            approval_id: ApprovalId::new(&ids),
        },
        Event::ApprovalDecided {
            approval_id: ApprovalId::new(&ids),
            decision: ApprovalDecision::Approved,
        },
        Event::ApprovalInvalidated {
            approval_id: ApprovalId::new(&ids),
        },
        Event::QuestionAsked {
            question_id: QuestionId::new(&ids),
        },
        Event::QuestionAnswered {
            question_id: QuestionId::new(&ids),
        },
        Event::GrantIssued {
            grant_id: GrantId::new(&ids),
        },
        Event::GrantRevoked {
            grant_id: GrantId::new(&ids),
        },
        Event::WorkspaceMaterialized,
        Event::WorkspaceSnapshot {
            snapshot_id: SnapshotId::new(&ids),
            reason: "before_action".into(),
        },
        Event::WorkspaceRollback {
            snapshot_id: SnapshotId::new(&ids),
        },
        Event::WorkspacePromoted {
            files: vec!["src/lib.rs".into()],
        },
        Event::WorkspacePromotionStarted {
            files: vec!["src/lib.rs".into()],
        },
        Event::WorkspacePromotionFailed {
            reason: "restored".into(),
        },
        Event::WorkspaceUnreadable {
            path: "d".into(),
            kind: None,
        },
        Event::TaskRecovered {
            disposition: RecoveryDisposition::RolledBack,
        },
        Event::ToolPinned {
            tool: "rypsok.web_search".into(),
            hash: hash_bytes(b"t"),
        },
        Event::ToolUnpinned {
            tool: "rypsok.web_search".into(),
        },
        Event::ServerStarted {
            server: "rypsok".into(),
            version: Some("0.7.0".into()),
        },
        Event::ServerStopped {
            server: "rypsok".into(),
            reason: "shutdown".into(),
        },
        Event::RuntimeStarted {
            tier: Tier::Core,
            backend: crate::spi::BackendDescriptor {
                kind: "fake".into(),
                model: None,
                locality: crate::spi::Locality::Local,
            },
        },
        Event::RuntimeStopped,
        Event::RuntimeOrphansKilled { count: 2 },
        Event::SnapshotJournalsRecovered {
            finished: 1,
            undone: 2,
            objects_removed: 3,
        },
    ]
}

/// REQ-OBS-001 / external design §17.1: the event names, exactly as the list below gives them
/// and in its order, in dotted form, each event round-tripping through serde. (The list is the
/// count: no number is stated here to drift from it.)
#[test]
fn req_obs_001_event_vocabulary_matches_ed_17_1() {
    let events = every_event();
    let names: Vec<&str> = events.iter().map(Event::name).collect();
    assert_eq!(
        names,
        [
            "task.created",
            "task.state_changed",
            "task.admission_changed",
            "task.admission_changed",
            "task.admission_changed",
            "task.pause_requested",
            "task.deleted",
            "step.started",
            "step.finished",
            "decision.received",
            "decision.rejected",
            "action.proposed",
            "action.authorized",
            "action.started",
            "execution.finished",
            "action.finished",
            "approval.requested",
            "approval.decided",
            "approval.invalidated",
            "question.asked",
            "question.answered",
            "grant.issued",
            "grant.revoked",
            "workspace.materialized",
            "workspace.snapshot",
            "workspace.rollback",
            "workspace.promoted",
            "workspace.promotion_started",
            "workspace.promotion_failed",
            "workspace.unreadable",
            "task.recovered",
            "tool.pinned",
            "tool.unpinned",
            "server.started",
            "server.stopped",
            "runtime.started",
            "runtime.stopped",
            "runtime.orphans_killed",
            "snapshot.journals_recovered",
        ]
    );
    for event in events {
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["event"], event.name());
        let back: Event = serde_json::from_value(json).unwrap();
        assert_eq!(back, event);
    }
}

/// REQ-OBS-001: the envelope carries time, actor, references and policy hash, flat.
#[test]
fn req_obs_001_event_record_envelope_is_flat() {
    let ids = ScriptedIds::default();
    let record = EventRecord {
        event_id: EventId::new(&ids),
        at: FakeClock::default().now(),
        actor: Actor::User {
            session: "pid:4242".into(),
            os_user: "nabbisen".into(),
        },
        task_id: Some(TaskId::new(&ids)),
        step_id: None,
        action_id: None,
        policy_hash: hash_bytes(b"policy"),
        event: Event::TaskStateChanged {
            from: "created".into(),
            to: "running".into(),
            reason: None,
            rollback_failed: false,
            halt_detail: None,
        },
    };
    let json = serde_json::to_value(&record).unwrap();
    assert_eq!(json["actor"], "user");
    assert_eq!(json["os_user"], "nabbisen");
    assert_eq!(json["event"], "task.state_changed");
    assert_eq!(json["to"], "running");
    assert_eq!(json["at"], "2026-01-01T00:00:00.000Z");
    assert!(json.get("reason").is_none());
    let back: EventRecord = serde_json::from_value(json).unwrap();
    assert_eq!(back, record);
    let runtime = serde_json::to_value(Actor::Runtime).unwrap();
    assert_eq!(runtime["actor"], "runtime");
}

/// External design §5.2: the halt's `task.state_changed` carries `halt_detail` when the halt has
/// one, and an event written before it existed reads without and is written back the same.
#[test]
fn req_sec_010_the_halt_event_carries_its_rule() {
    let event = Event::TaskStateChanged {
        from: "running".into(),
        to: "halted".into(),
        reason: Some(HaltReason::Policy),
        rollback_failed: false,
        halt_detail: Some(crate::task::HaltDetail::RemoteBackendForbidden {
            backend: "default".into(),
            profile: "workspace".into(),
        }),
    };
    let json = serde_json::to_value(&event).unwrap();
    assert_eq!(json["event"], "task.state_changed");
    assert_eq!(json["reason"], "policy");
    assert_eq!(
        json["halt_detail"],
        serde_json::json!({
            "rule": "remote_backend_forbidden",
            "backend": "default",
            "profile": "workspace"
        })
    );
    assert_eq!(serde_json::from_value::<Event>(json).unwrap(), event);
    let old = serde_json::json!({
        "event": "task.state_changed", "from": "running", "to": "halted", "reason": "policy"
    });
    let read: Event = serde_json::from_value(old.clone()).unwrap();
    assert_eq!(
        read,
        Event::TaskStateChanged {
            from: "running".into(),
            to: "halted".into(),
            reason: Some(HaltReason::Policy),
            rollback_failed: false,
            halt_detail: None,
        }
    );
    assert_eq!(serde_json::to_value(&read).unwrap(), old);
}
