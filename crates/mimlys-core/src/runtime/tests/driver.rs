//! The driver (handoff 014b, item 4; RFC 014, Amendment 1; REQ-SCHED-001, REQ-SCHED-002): it
//! owns the tasks that requests made `running`, runs their steps one at a time until they wait
//! or end, and lets a task go when a request moved it. Judged by state: what the driver owns and
//! what the store holds.

use super::*;
use crate::api::{Envelope, Reply, Request};
use crate::driver::{Driver, Turn};
use crate::ids::RequestId;

/// A driver over a runtime of its own on the harness store and fakes: the harness is what the
/// test reads the store through.
fn driven(decisions: Vec<Value>, policy: Policy) -> (Driver, Harness) {
    let h = Harness::with_policy(decisions, policy);
    let runtime = Runtime::new(RuntimeDeps {
        persistence: h.persistence.clone(),
        backend: h.backend.clone(),
        backend_name: "default".to_owned(),
        workspaces: h.workspaces.clone(),
        policy: Policy::shipped(),
        ids: Arc::new(ScriptedIds::starting_at(500_000)),
        clock: h.clock.clone(),
        runtime_version: "0.1.0-test".into(),
        rules: PlatformRules::Posix,
        probe: crate::confine::Probe::unavailable("no backend in tests"),
        secrets: vec![],
        context_chars: crate::limits::LIM_C1_CONTEXT_CHARS,
        output_bytes: crate::limits::LIM_S4_OUTPUT_BYTES as usize,
    })
    .unwrap();
    (Driver::new(Arc::new(runtime)), h)
}

fn request(h: &Harness, request: Request) -> Envelope {
    Envelope {
        request_id: RequestId::new(&ScriptedIds::starting_at(9_000)),
        session: h.session.clone(),
        request,
    }
}

fn create(hold: bool) -> Request {
    Request::CreateTask(CreateTask {
        goal: "fix the failing test".into(),
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
    })
}

fn task_of(reply: Reply) -> TaskId {
    match reply {
        Reply::Task(task) => task.task_id,
        other => panic!("not a task: {other:?}"),
    }
}

/// A request that makes a task `running` hands the task to the driver; one that does not, does
/// not.
#[test]
fn req_sched_002_a_request_that_makes_a_task_running_hands_it_to_the_driver() {
    let (driver, h) = driven(vec![], Policy::shipped());
    let held = task_of(
        driver
            .handle(request(&h, create(true)))
            .into_result()
            .unwrap(),
    );
    assert!(!driver.owns(held), "a task held in `created` is not run");
    let resumed = driver
        .handle(request(
            &h,
            Request::ResumeTask {
                task_id: held,
                extend: None,
            },
        ))
        .into_result()
        .unwrap();
    assert_eq!(task_of(resumed), held);
    assert!(driver.owns(held), "a resume hands the task over");
    // A refused request hands nothing over.
    let cancelled = task_of(
        driver
            .handle(request(&h, Request::CancelTask { task_id: held }))
            .into_result()
            .unwrap(),
    );
    assert_eq!(cancelled, held);
    let refused = driver.handle(request(
        &h,
        Request::ResumeTask {
            task_id: held,
            extend: None,
        },
    ));
    assert!(refused.into_result().is_err());
}

/// REQ-SCHED-002: the driver runs the steps of the task it owns, one at a time, until the task
/// waits; then it lets the task go, and has nothing to do. A step is run by no one else.
#[test]
fn req_sched_002_the_driver_runs_the_steps_until_the_task_waits_and_then_lets_it_go() {
    let (driver, h) = driven(
        vec![
            act("read_file", "a"),
            json!({"decision": "ask_user", "question": "Which branch?"}),
        ],
        Policy::shipped(),
    );
    h.tools
        .script("read_file", ExecutionOutcome::succeeded(json!({}), 1));
    let id = task_of(
        driver
            .handle(request(&h, create(false)))
            .into_result()
            .unwrap(),
    );
    assert!(driver.owns(id));
    let first = futures_block(driver.step_once());
    assert!(
        matches!(first, Turn::Ran { task_id, outcome: StepOutcome::Continued, state: TaskState::Running } if task_id == id),
        "{first:?}"
    );
    assert!(driver.owns(id), "a task that still runs keeps its place");
    let second = futures_block(driver.step_once());
    assert!(
        matches!(
            &second,
            Turn::Ran {
                state: TaskState::WaitingUser {
                    wait_kind: WaitKind::Question
                },
                ..
            }
        ),
        "{second:?}"
    );
    assert!(!driver.owns(id), "a task that waits is let go");
    assert_eq!(futures_block(driver.step_once()), Turn::Idle);
    // The answer makes it running again, and hands it back.
    let answered = driver.handle(request(
        &h,
        Request::AnswerQuestion {
            task_id: id,
            answer: "main".into(),
        },
    ));
    assert!(answered.into_result().is_ok());
    assert!(driver.owns(id));
}

/// A request that holds the task (a cancel in progress, a grant that waits) keeps a step from
/// beginning: the driver asks again, and the task keeps its place.
#[test]
fn req_sched_002_a_task_held_by_a_request_is_deferred_and_keeps_its_place() {
    let (driver, h) = driven(vec![], Policy::shipped());
    let id = task_of(
        driver
            .handle(request(&h, create(false)))
            .into_result()
            .unwrap(),
    );
    let (cancelling, _) = driver.runtime().steps.begin_cancel(id).unwrap();
    assert_eq!(futures_block(driver.step_once()), Turn::Deferred(id));
    assert!(driver.owns(id));
    drop(cancelling);
    assert!(matches!(
        futures_block(driver.step_once()),
        Turn::Ran { .. }
    ));
}

/// A task a request moved is not stepped: the driver lets it go and runs nothing.
#[test]
fn req_sched_002_a_task_that_is_no_longer_running_is_let_go() {
    let (driver, h) = driven(vec![], Policy::shipped());
    let id = task_of(
        driver
            .handle(request(&h, create(false)))
            .into_result()
            .unwrap(),
    );
    driver
        .handle(request(&h, Request::CancelTask { task_id: id }))
        .into_result()
        .unwrap();
    assert_eq!(futures_block(driver.step_once()), Turn::Released(id));
    assert!(!driver.owns(id));
    assert_eq!(h.persistence.load_steps(id).unwrap().len(), 0);
    assert_eq!(futures_block(driver.step_once()), Turn::Idle);
}

/// A failure of a step is kept for the follower of the task, once, and the task is let go.
#[test]
fn req_sched_002_the_error_of_a_step_is_kept_for_the_task_s_follower() {
    let (driver, h) = driven(vec![], Policy::shipped());
    let id = task_of(
        driver
            .handle(request(&h, create(false)))
            .into_result()
            .unwrap(),
    );
    // A store that cannot be read ends the step with an error that is not the state's.
    h.persistence.fail_write("step_started");
    let turn = futures_block(driver.step_once());
    assert_eq!(turn, Turn::Failed(id), "{turn:?}");
    assert!(!driver.owns(id));
    let error = driver.take_failure(id).expect("the error is kept");
    assert_eq!(error.kind(), "internal");
    assert!(driver.take_failure(id).is_none(), "once");
}
