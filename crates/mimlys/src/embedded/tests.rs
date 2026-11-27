//! Embedded mode's driver (handoff 014b, item 4; RFC 014, Amendment 1; external design §4):
//! a task that a request made `running` is stepped by the driver of the process, with no one
//! else calling a step; the driver ends when the command ends; a follower that waits is woken by
//! what the driver does. Judged by state: the task's, and who is still alive.

use super::*;
use crate::client::Client;
use mimlys_core::clock::FakeClock;
use mimlys_core::ids::{ScriptedIds, TaskId};
use mimlys_core::path::PlatformRules;
use mimlys_core::policy::Policy;
use mimlys_core::runtime::{CreateTask, RuntimeDeps, Session};
use mimlys_core::spi::fake::{FakePersistence, FakeToolProvider, FakeWorkspaces, ScriptedBackend};
use mimlys_core::task::{ScopeSpec, TaskState};
use serde_json::json;

fn rig(decisions: Vec<serde_json::Value>) -> (Arc<Runtime>, Arc<InProcess>, Client) {
    let persistence = Arc::new(FakePersistence::default());
    let tools = Arc::new(FakeToolProvider::new(vec![]));
    let runtime = Arc::new(
        Runtime::new(RuntimeDeps {
            persistence,
            backend: Arc::new(ScriptedBackend::with_decisions(decisions)),
            backend_name: "default".to_owned(),
            workspaces: Arc::new(FakeWorkspaces::new(tools)),
            policy: Policy::shipped(),
            ids: Arc::new(ScriptedIds::default()),
            clock: Arc::new(FakeClock::default()),
            runtime_version: "0.1.0-test".into(),
            rules: PlatformRules::Posix,
            probe: mimlys_core::confine::Probe::unavailable("no backend in tests"),
            secrets: vec![],
            context_chars: mimlys_core::limits::LIM_C1_CONTEXT_CHARS,
            output_bytes: mimlys_core::limits::LIM_S4_OUTPUT_BYTES as usize,
        })
        .unwrap(),
    );
    let transport = InProcess::new(Arc::clone(&runtime));
    let client = Client::over(
        transport.clone(),
        Session {
            id: "pid:1".into(),
            os_user: "tester".into(),
        },
    );
    (runtime, transport, client)
}

fn create(hold: bool) -> CreateTask {
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
        hold,
        from_task: None,
        from_snapshot: None,
        plan_first: false,
        effort: None,
    }
}

/// Waits, a follower's way, until the task leaves `running`; a bound for a failure only.
async fn until_not_running(runtime: &Runtime, client: &Client, id: TaskId) -> TaskState {
    for _ in 0..600 {
        let state = runtime.get_task(id).unwrap().task.state;
        if state != TaskState::Running {
            return state;
        }
        client.changed().await;
    }
    panic!("the task never left running");
}

/// REQ-SCHED-002, REQ-ARCH-009: a task that a request made `running` is stepped by the driver of
/// the process, to its end, though nothing but the client was called: the follower runs no step.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn req_sched_002_the_driver_of_the_process_runs_the_steps_of_a_task_a_request_started() {
    let (runtime, _transport, client) = rig(vec![
        json!({"decision": "plan", "steps": ["look"]}),
        json!({"decision": "complete", "summary": "done"}),
    ]);
    let task = client.create_task(create(false)).unwrap();
    assert_eq!(task.state, TaskState::Running);
    let state = until_not_running(&runtime, &client, task.task_id).await;
    assert!(matches!(state, TaskState::Completed { .. }), "{state:?}");
    assert_eq!(
        runtime
            .persistence()
            .load_steps(task.task_id)
            .unwrap()
            .len(),
        2,
        "two steps, run by the driver"
    );
}

/// A task held in `created` is nobody's until a request starts it; one that a follower attaches to
/// while it is `running` (a recovered task, made so by the start) is the driver's from then on.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn req_sched_002_a_task_is_stepped_only_once_a_request_or_an_attach_hands_it_over() {
    let (runtime, transport, client) =
        rig(vec![json!({"decision": "complete", "summary": "done"})]);
    let task = client.create_task(create(true)).unwrap();
    assert!(!transport.driver().owns(task.task_id));
    client.changed().await;
    assert_eq!(
        runtime.get_task(task.task_id).unwrap().task.state,
        TaskState::Created
    );
    client.resume_task(task.task_id, None).unwrap();
    let state = until_not_running(&runtime, &client, task.task_id).await;
    assert!(matches!(state, TaskState::Completed { .. }), "{state:?}");
    // A follower attaching to a task that is not running leaves it alone.
    client.attach(task.task_id);
    assert!(
        until_released(&transport, task.task_id).await,
        "a task that is not running is let go"
    );
}

async fn until_released(transport: &InProcess, id: TaskId) -> bool {
    for _ in 0..200 {
        if !transport.driver().owns(id) {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    false
}

/// RFC 014, Amendment 1, item 1: the driver of embedded mode lives as long as the command: once
/// the client and its transport are gone, the loop ends and nothing of the process holds the
/// runtime.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn req_arch_009_the_driver_ends_with_the_command() {
    let (runtime, transport, client) =
        rig(vec![json!({"decision": "complete", "summary": "done"})]);
    let task = client.create_task(create(false)).unwrap();
    until_not_running(&runtime, &client, task.task_id).await;
    let weak = Arc::downgrade(&transport);
    drop(client);
    drop(transport);
    for _ in 0..400 {
        if weak.upgrade().is_none() {
            // The loop held the runtime too: it is ours alone now.
            for _ in 0..400 {
                if Arc::strong_count(&runtime) == 1 {
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
            panic!("the driver's loop still holds the runtime");
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    panic!("the transport is still held");
}

/// External design §4: a follower that finds the driver ended by a panic ends in the same way,
/// and never hangs.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[should_panic(expected = "a step panicked")]
async fn req_arch_009_a_follower_ends_as_the_driver_does_when_a_step_panics() {
    let (_runtime, transport, client) = rig(vec![]);
    // A loop of the driver that ended in a panic.
    let ended = tokio::spawn(async { panic!("a step panicked") });
    *transport.task.lock().unwrap() = Some(ended);
    for _ in 0..200 {
        client.changed().await;
    }
}

/// Outside an executor there is no loop to start, and a request is handled all the same: a
/// command that has none (a synchronous one) steps nothing.
#[test]
fn req_arch_009_without_an_executor_a_request_is_handled_and_no_loop_starts() {
    let (runtime, transport, client) = rig(vec![]);
    let task = client.create_task(create(false)).unwrap();
    assert_eq!(task.state, TaskState::Running);
    assert!(transport.task.lock().unwrap().is_none());
    assert_eq!(
        runtime.get_task(task.task_id).unwrap().task.state,
        TaskState::Running
    );
}
