//! A task found `recovering` is recovered again (external design §16.3; REQ-PERS-007).
//! The states a recovery that ended part-way leaves are made the way `recover_all`
//! makes them, step by step, and, with the feature `crash-injection`, by a process
//! that aborts at the commit of a step.

use super::*;
use crate::integrity;
use crate::sqlite::open;
use crate::store::tests::{Fx, sequence};
use mimlys_core::action::{Action, ActionInput, ActionStatus, EffectKind};
use mimlys_core::hash::hash_bytes;
use mimlys_core::ids::{ActionId, SnapshotId, StepId};
use mimlys_core::policy::Recovery;
use mimlys_core::snapshot::{SnapshotReason, SnapshotRecord};
use mimlys_core::spi::fake::{FakeToolProvider, FakeWorkspaces};
use mimlys_core::spi::{PersistenceRead, WorkspaceProvider};
use mimlys_core::step::Step;

fn workspaces() -> FakeWorkspaces {
    FakeWorkspaces::new(std::sync::Arc::new(FakeToolProvider::new(vec![])))
}

/// A task `running` with one action `running`, whose snapshot before it was recorded.
struct InFlight {
    task: Task,
    action: Action,
    snapshot: SnapshotId,
}

fn in_flight(store: &Store, data: &std::path::Path, kind: EffectKind) -> InFlight {
    let fx = Fx::new();
    let (task, _) = sequence(store, &fx, 0, false);
    let step = Step::start(StepId::new(&fx.ids), task.task_id, 0, fx.now());
    store
        .record_step_started(&step, &fx.event(&task, Event::StepStarted))
        .unwrap();
    let snapshot = SnapshotId::new(&fx.ids);
    let files = SnapshotFiles::open(data).unwrap();
    files.begin(snapshot, &[]).unwrap();
    let manifest_hash = files
        .put_manifest(snapshot, &mimlys_core::snapshot::Manifest::new(vec![]))
        .unwrap();
    store
        .record_snapshot(
            &SnapshotRecord {
                snapshot_id: snapshot,
                task_id: task.task_id,
                reason: SnapshotReason::Before,
                manifest_hash,
                bytes: 0,
                files: 0,
                created_at: fx.now(),
                action_id: None,
                project_root: None,
            },
            &[],
            &fx.event(
                &task,
                Event::WorkspaceSnapshot {
                    snapshot_id: snapshot,
                    reason: "before".into(),
                },
            ),
        )
        .unwrap();
    files.finish(snapshot).unwrap();
    let action = fx.action(&task, &step, kind, false);
    store
        .record_proposed(&step, &action, &fx.event(&task, Event::ActionProposed))
        .unwrap();
    store
        .record_validation(
            &step,
            Some(&action),
            &fx.event(
                &task,
                Event::DecisionReceived {
                    hash: hash_bytes(b"d"),
                    transport: Default::default(),
                    text_chars: None,
                },
            ),
        )
        .unwrap();
    let mut action = action.transition(ActionInput::Authorize, fx.now()).unwrap();
    if kind == EffectKind::WorkspaceWrite {
        action.snapshot_before = Some(snapshot);
    }
    store
        .record_authorized(&action, None, &fx.event(&task, Event::ActionAuthorized))
        .unwrap();
    let action = action.transition(ActionInput::Start, fx.now()).unwrap();
    store
        .record_running(&action, &fx.event(&task, Event::ActionStarted))
        .unwrap();
    InFlight {
        task,
        action,
        snapshot,
    }
}

/// How far an earlier recovery got before the process ended.
#[derive(Clone, Copy, Debug)]
enum Reached {
    /// `StartRecovery` was committed.
    Start,
    /// The action was marked `unknown` as well.
    Mark,
    /// `task.recovered` was recorded as well; the task was not yet moved on.
    Recorded,
}

/// The state a recovery leaves when it ends after `reached`, written the way `recover_all`
/// writes it: the restore (a snapshot restore leaves no row) comes before the mark.
fn left_by_a_recovery(store: &Store, flight: &InFlight, reached: Reached) {
    let fx = Fx::starting_at(500_000);
    let recovering = flight
        .task
        .transition(TaskInput::StartRecovery, fx.now())
        .unwrap();
    store
        .record_task_transition(
            &recovering,
            &fx.event(&recovering, state_changed(&flight.task, &recovering)),
        )
        .unwrap();
    if matches!(reached, Reached::Start) {
        return;
    }
    let unknown = flight
        .action
        .transition(ActionInput::MarkUnknown, fx.now())
        .unwrap();
    let mut event = fx.event(
        &recovering,
        Event::ActionFinished {
            status: "unknown".into(),
            cause: None,
        },
    );
    event.action_id = Some(unknown.action_id);
    store.record_action_terminal(&unknown, &event).unwrap();
    if matches!(reached, Reached::Mark) {
        return;
    }
    store
        .append_event(&fx.event(
            &recovering,
            Event::TaskRecovered {
                disposition: RecoveryDisposition::RolledBack,
            },
        ))
        .unwrap();
}

/// A start of the process: identifiers above those of the states it finds.
fn start_again(
    store: &Store,
    workspaces: &dyn WorkspaceProvider,
    auto_resume: bool,
) -> Vec<Recovered> {
    start_with_ids(store, workspaces, auto_resume, 1_000_000)
}

fn start_with_ids(
    store: &Store,
    workspaces: &dyn WorkspaceProvider,
    auto_resume: bool,
    first_id: u64,
) -> Vec<Recovered> {
    let fx = Fx::starting_at(first_id);
    recover_all(
        store,
        &RecoveryContext {
            ids: &fx.ids,
            now: fx.now(),
            policy_hash: hash_bytes(b"policy"),
            recovery: Recovery { auto_resume },
        },
        workspaces,
    )
    .unwrap()
}

fn names(store: &Store, task: &Task) -> Vec<String> {
    store
        .events_since(0, Some(task.task_id), 1_000)
        .unwrap()
        .iter()
        .map(|e| e.record.event.name().to_owned())
        .collect()
}

fn count(names: &[String], name: &str) -> usize {
    names.iter().filter(|n| n.as_str() == name).count()
}

fn state_of(store: &Store, task: &Task) -> TaskState {
    store.load_task(task.task_id).unwrap().unwrap().state
}

fn status_of(store: &Store, task: &Task, action: ActionId) -> ActionStatus {
    store
        .load_actions(task.task_id)
        .unwrap()
        .into_iter()
        .find(|a| a.action_id == action)
        .unwrap()
        .status
}

fn temp_store() -> (tempfile::TempDir, std::path::PathBuf, Store) {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let store = open(&data).unwrap();
    (dir, data, store)
}

/// REQ-PERS-007: a recovery that ended after `StartRecovery` (or after the mark, or after
/// `task.recovered`) is finished by the next start: the snapshot is restored, the action
/// is `unknown` once, `task.recovered` is recorded once, and the task leaves `recovering`.
#[test]
fn req_pers_007_a_task_found_recovering_is_recovered_again() {
    for reached in [Reached::Start, Reached::Mark, Reached::Recorded] {
        let (_dir, data, store) = temp_store();
        let flight = in_flight(&store, &data, EffectKind::WorkspaceWrite);
        left_by_a_recovery(&store, &flight, reached);
        assert_eq!(
            state_of(&store, &flight.task),
            TaskState::Recovering,
            "{reached:?}"
        );

        let workspaces = workspaces();
        let out = start_again(&store, &workspaces, false);

        assert_eq!(out.len(), 1, "{reached:?}");
        assert_eq!(out[0].disposition, RecoveryDisposition::RolledBack);
        assert_eq!(
            state_of(&store, &flight.task),
            TaskState::Halted {
                halt_reason: HaltReason::Recovered,
                halt_detail: None,
            },
            "{reached:?}"
        );
        assert_eq!(
            workspaces.restores(),
            [flight.snapshot],
            "{reached:?}: restored again, whatever the earlier pass did"
        );
        assert_eq!(
            status_of(&store, &flight.task, flight.action.action_id),
            ActionStatus::Unknown,
            "{reached:?}"
        );
        assert!(
            store
                .load_running_action(flight.task.task_id)
                .unwrap()
                .is_none()
        );
        let names = names(&store, &flight.task);
        assert_eq!(count(&names, "task.recovered"), 1, "{reached:?}: {names:?}");
        assert_eq!(
            count(&names, "action.finished"),
            1,
            "{reached:?}: the action is marked once: {names:?}"
        );
        integrity::check(&store, Fx::new().now()).unwrap();
        // The next start has nothing to do.
        assert!(start_again(&store, &workspaces, false).is_empty());
    }
}

/// REQ-PERS-007: the restore comes before the mark, in the record too: the event of the
/// restore precedes the action's `unknown`.
#[test]
fn req_pers_007_the_restore_comes_before_the_mark() {
    let (_dir, data, store) = temp_store();
    let flight = in_flight(&store, &data, EffectKind::WorkspaceWrite);
    start_again(&store, &workspaces(), false);
    let names = names(&store, &flight.task);
    let position = |name: &str| names.iter().position(|n| n == name).unwrap();
    assert!(
        position("workspace.rollback") < position("action.finished"),
        "{names:?}"
    );
    assert!(
        position("action.finished") < position("task.recovered"),
        "{names:?}"
    );
}

/// REQ-PERS-007, REQ-PERS-008: an action that cannot be undone, marked `unknown` by a
/// recovery that then ended, still halts the task `unknown_outcome` at the next start, with
/// the action listed, whatever `auto_resume` says. It is not taken for "no action in flight".
#[test]
fn req_pers_008_an_action_marked_by_an_earlier_recovery_still_halts_unknown_outcome() {
    for reached in [Reached::Start, Reached::Mark, Reached::Recorded] {
        let (_dir, data, store) = temp_store();
        let flight = in_flight(&store, &data, EffectKind::ExternalWrite);
        left_by_a_recovery(&store, &flight, reached);

        let workspaces = workspaces();
        let out = start_again(&store, &workspaces, true);

        assert_eq!(out[0].disposition, RecoveryDisposition::UnknownOutcome);
        assert_eq!(
            state_of(&store, &flight.task),
            TaskState::Halted {
                halt_reason: HaltReason::UnknownOutcome,
                halt_detail: None,
            },
            "{reached:?}"
        );
        assert!(workspaces.restores().is_empty(), "nothing to restore");
        assert_eq!(
            status_of(&store, &flight.task, flight.action.action_id),
            ActionStatus::Unknown
        );
    }
}

/// REQ-PERS-007: a restore that fails at the next start leaves the outcome unknown and the
/// task halted `unknown_outcome`, as at the first start; auto-resume does not resume it.
#[test]
fn req_pers_007_a_restore_that_fails_when_recovered_again_halts_unknown_outcome() {
    let (_dir, data, store) = temp_store();
    let flight = in_flight(&store, &data, EffectKind::WorkspaceWrite);
    left_by_a_recovery(&store, &flight, Reached::Mark);
    let workspaces = workspaces();
    workspaces.fail_restores();

    let out = start_again(&store, &workspaces, true);

    assert_eq!(out[0].disposition, RecoveryDisposition::UnknownOutcome);
    assert_eq!(
        state_of(&store, &flight.task),
        TaskState::Halted {
            halt_reason: HaltReason::UnknownOutcome,
            halt_detail: None,
        }
    );
    assert!(
        names(&store, &flight.task)
            .iter()
            .all(|n| n != "workspace.rollback")
    );
}

/// REQ-PERS-007: with `auto_resume`, a task whose snapshot was restored runs again after
/// the second pass, as after the first.
#[test]
fn req_pers_007_a_task_recovered_again_resumes_by_policy() {
    let (_dir, data, store) = temp_store();
    let flight = in_flight(&store, &data, EffectKind::WorkspaceWrite);
    left_by_a_recovery(&store, &flight, Reached::Mark);
    start_again(&store, &workspaces(), true);
    assert_eq!(state_of(&store, &flight.task), TaskState::Running);
}

/// REQ-PERS-007: a task that was never running is not touched, and a `recovering` task is
/// not mistaken for another task's action: an older action of the task that is `unknown`
/// from before this recovery is not the one in flight.
#[test]
fn req_pers_007_an_unknown_action_from_before_is_not_the_one_in_flight() {
    let (_dir, data, store) = temp_store();
    // An earlier action of the task ended `unknown` (a halt the user resumed from) ...
    let flight = in_flight(&store, &data, EffectKind::ExternalWrite);
    let fx = Fx::starting_at(300_000);
    let unknown = flight
        .action
        .transition(ActionInput::MarkUnknown, fx.now())
        .unwrap();
    let mut event = fx.event(
        &flight.task,
        Event::ActionFinished {
            status: "unknown".into(),
            cause: None,
        },
    );
    event.action_id = Some(unknown.action_id);
    store.record_action_terminal(&unknown, &event).unwrap();
    // ... and the process ended after `StartRecovery` with nothing in flight.
    let recovering = flight
        .task
        .transition(TaskInput::StartRecovery, fx.now())
        .unwrap();
    store
        .record_task_transition(
            &recovering,
            &fx.event(&recovering, state_changed(&flight.task, &recovering)),
        )
        .unwrap();

    let out = start_again(&store, &workspaces(), false);

    assert_eq!(out[0].disposition, RecoveryDisposition::Clean);
    assert_eq!(
        state_of(&store, &flight.task),
        TaskState::Halted {
            halt_reason: HaltReason::Recovered,
            halt_detail: None,
        }
    );
}

/// REQ-TEST-007, REQ-PERS-007: the process aborts at the commit of `StartRecovery`, and again
/// at the commit of the mark; the next start completes the recovery and the task leaves
/// `recovering`. The child is this test binary run for `recovery_child`.
#[cfg(feature = "crash-injection")]
#[test]
fn req_pers_007_a_recovery_that_the_process_does_not_finish_is_finished_by_the_next_start() {
    let exe = std::env::current_exe().unwrap();
    for (point, kind, left_in) in [
        ("task_transition", EffectKind::WorkspaceWrite, "recovering"),
        ("action_terminal", EffectKind::WorkspaceWrite, "recovering"),
        ("task_transition", EffectKind::ExternalWrite, "recovering"),
        ("action_terminal", EffectKind::ExternalWrite, "recovering"),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        let flight = {
            let store = open(&data).unwrap();
            in_flight(&store, &data, kind)
        };
        let output = std::process::Command::new(&exe)
            .args([
                "recovery::found_recovering_tests::recovery_child",
                "--exact",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("MIMLYS_CRASH_AT", format!("{point}.after"))
            .env("MIMLYS_CRASH_DIR", &data)
            .output()
            .unwrap();
        assert!(
            !output.status.success(),
            "the child aborts at {point}.after"
        );

        let store = open(&data).unwrap();
        assert_eq!(
            store.tasks_in_state(left_in).unwrap(),
            [flight.task.task_id],
            "the crash left the task {left_in} ({point}, {kind:?})"
        );
        let marked = point == "action_terminal";
        assert_eq!(
            store
                .load_running_action(flight.task.task_id)
                .unwrap()
                .is_none(),
            marked,
            "{point}: the mark was committed or not"
        );

        let workspaces = workspaces();
        let out = start_again(&store, &workspaces, false);

        assert_eq!(out.len(), 1);
        assert!(store.tasks_in_state("recovering").unwrap().is_empty());
        let expected = if kind == EffectKind::WorkspaceWrite {
            HaltReason::Recovered
        } else {
            HaltReason::UnknownOutcome
        };
        assert_eq!(
            state_of(&store, &flight.task),
            TaskState::Halted {
                halt_reason: expected,
                halt_detail: None,
            },
            "{point}, {kind:?}"
        );
        assert_eq!(
            status_of(&store, &flight.task, flight.action.action_id),
            ActionStatus::Unknown
        );
        let names = names(&store, &flight.task);
        assert_eq!(count(&names, "task.recovered"), 1, "{point}: {names:?}");
        integrity::check(&store, Fx::new().now()).unwrap();
    }
}

/// The child of the crash test: runs the recovery of the store it is given, under
/// `MIMLYS_CRASH_AT`.
#[cfg(feature = "crash-injection")]
#[test]
fn recovery_child() {
    let Ok(dir) = std::env::var("MIMLYS_CRASH_DIR") else {
        return;
    };
    let store = open(std::path::Path::new(&dir)).unwrap();
    start_with_ids(&store, &workspaces(), false, 2_000_000);
}
