//! The engine's gate among the user's runtimes (handoff 013d; REQ-SCHED-007; RFC 013 decision 7
//! with Amendment 1; external design §19.3), through real steps over the fakes and a scripted
//! admission standing for the registry. The gate is taken before a model request is sent and
//! returned when its answer is read; the wait is the step's `engine_wait_ms` and a log line; the
//! first stop request ends it with nothing sent; the request's own deadline starts when it is
//! sent. The verdict is always the state the step leaves.

use super::*;
use crate::event::{AdmissionState, Event};
use crate::fake_admission::{Script, ScriptedAdmission};
use crate::spi::{BackendDescriptor, BoxFuture, Cancellation, GenerateRequest, ModelBackend};
use std::sync::Mutex;

/// A backend that reports what the gate looked like when each request reached it: the number of
/// gates held, and the deadline the request carried.
struct Watching {
    inner: ScriptedBackend,
    gate: Arc<ScriptedAdmission>,
    held_when_sent: Mutex<Vec<usize>>,
    deadlines: Mutex<Vec<i64>>,
}

impl ModelBackend for Watching {
    fn describe(&self) -> BackendDescriptor {
        self.inner.describe()
    }

    fn generate<'a>(
        &'a self,
        request: GenerateRequest,
        cancel: Cancellation,
    ) -> BoxFuture<'a, Result<Vec<u8>, BackendError>> {
        self.held_when_sent.lock().unwrap().push(self.gate.held());
        self.deadlines.lock().unwrap().push(request.deadline_ms);
        self.inner.generate(request, cancel)
    }
}

struct Rig {
    runtime: Arc<Runtime>,
    persistence: Arc<FakePersistence>,
    backend: Arc<Watching>,
    gate: Arc<ScriptedAdmission>,
    clock: Arc<FakeClock>,
    task: TaskId,
}

fn rig(script: Vec<Script>, decisions: Vec<Value>) -> Rig {
    let gate = ScriptedAdmission::new(script);
    let persistence = Arc::new(FakePersistence::default());
    let backend = Arc::new(Watching {
        inner: ScriptedBackend::with_decisions(decisions),
        gate: gate.clone(),
        held_when_sent: Mutex::new(Vec::new()),
        deadlines: Mutex::new(Vec::new()),
    });
    let tools = Arc::new(FakeToolProvider::new(vec![tool(
        "read_file",
        EffectKind::WorkspaceRead,
    )]));
    let workspaces = Arc::new(FakeWorkspaces::new(tools));
    let clock = Arc::new(FakeClock::default());
    let runtime = Arc::new(
        Runtime::new(RuntimeDeps {
            persistence: persistence.clone(),
            backend: backend.clone(),
            workspaces,
            policy: Policy::shipped(),
            ids: Arc::new(ScriptedIds::default()),
            clock: clock.clone(),
            runtime_version: "0.1.0-test".into(),
            rules: PlatformRules::Posix,
            probe: crate::confine::Probe::unavailable("no backend in tests"),
            secrets: vec![],
            context_chars: crate::limits::LIM_C1_CONTEXT_CHARS,
            output_bytes: crate::limits::LIM_S4_OUTPUT_BYTES as usize,
        })
        .unwrap()
        .with_engine_gate(gate.clone()),
    );
    let session = Session {
        id: "s1".into(),
        os_user: "tester".into(),
    };
    let task = runtime
        .create_task(
            &session,
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
                hold: false,
                from_task: None,
                from_snapshot: None,
            },
        )
        .unwrap()
        .task_id;
    Rig {
        runtime,
        persistence,
        backend,
        gate,
        clock,
        task,
    }
}

fn complete() -> Value {
    json!({"decision": "complete", "summary": "done"})
}

fn step(rig: &Rig) -> crate::runtime::StepResultView {
    futures_block(rig.runtime.run_step(rig.task)).unwrap()
}

/// REQ-SCHED-007: the gate is taken before the request is sent, held while it is in flight, and
/// returned when its answer has been read: the backend sees one gate held, and none is held
/// when the step ends.
#[test]
fn req_sched_007_the_gate_is_held_for_the_request_and_returned_with_its_answer() {
    let rig = rig(vec![], vec![complete()]);
    let result = step(&rig);
    assert_eq!(result.outcome, StepOutcome::Completed);
    assert_eq!(*rig.backend.held_when_sent.lock().unwrap(), [1]);
    assert_eq!(rig.gate.taken(), 1);
    assert_eq!(rig.gate.held(), 0, "returned");
}

/// REQ-SCHED-007: a request that waits for the gate records the wait as the step's
/// `engine_wait_ms` and `metrics` counts it; the wait is not model time; and the task's events
/// say nothing of the wait (it is not a change of admission).
#[test]
fn req_sched_007_a_wait_for_the_gate_is_the_steps_engine_wait() {
    let rig = rig(
        vec![Script::WaitThenAdmit {
            holders: vec![4242],
            waited_ms: 1500,
        }],
        vec![complete()],
    );
    // The other runtime's request is in flight for 1500 ms of the (fake) clock.
    let clock = rig.clock.clone();
    rig.gate.on_admit(move || clock.advance_millis(1500));
    let result = step(&rig);
    assert_eq!(result.outcome, StepOutcome::Completed);
    let measures = rig.persistence.load_steps(rig.task).unwrap()[0]
        .measures
        .clone()
        .unwrap();
    assert_eq!(measures.engine_wait_ms, Some(1500));
    assert_eq!(
        measures.model_ms,
        Some(0),
        "the wait is not the model's time"
    );
    let events: Vec<String> = rig
        .persistence
        .events()
        .into_iter()
        .map(|e| e.event.name().to_owned())
        .collect();
    assert!(!events.contains(&"task.admission_changed".to_owned()));
    // A step that did not wait has no wait on its record.
    let rig = self::rig(vec![], vec![complete()]);
    step(&rig);
    let measures = rig.persistence.load_steps(rig.task).unwrap()[0]
        .measures
        .clone()
        .unwrap();
    assert_eq!(measures.engine_wait_ms, None);
}

/// REQ-SCHED-007, REQ-CLI-005, RFC 013 Amendment 1: the first stop request, whichever of the three
/// it is, ends the wait for the gate with nothing sent: the backend is never called, the step ends
/// `cancelled` (as an interrupted request does), the task is still `running` for the caller to
/// pause, and no gate is held.
#[test]
fn req_sched_007_the_first_stop_request_ends_the_wait_and_nothing_is_sent() {
    type Request = fn(&Runtime, TaskId);
    let requests: [(&str, Request); 3] = [
        ("start_no_new_action", Runtime::start_no_new_action),
        ("interrupt_step", Runtime::interrupt_step),
        ("interrupt_step_anyway", Runtime::interrupt_step_anyway),
    ];
    for (name, request) in requests {
        let rig = rig(vec![Script::WaitUntilStopped], vec![complete()]);
        {
            let runtime = rig.runtime.clone();
            let task = rig.task;
            rig.gate.on_admit(move || request(&runtime, task));
        }
        let result = step(&rig);
        assert_eq!(result.outcome, StepOutcome::Cancelled, "{name}");
        assert_eq!(result.state, TaskState::Running, "{name}");
        assert!(
            rig.backend.held_when_sent.lock().unwrap().is_empty(),
            "{name}: no request was sent"
        );
        assert_eq!(rig.gate.held(), 0, "{name}");
    }
}

/// REQ-SCHED-007: the request's own deadline starts when it is sent: the deadline it carries is
/// worked out after the wait, from the task's time then left (a wait of an hour of the task's
/// two does not leave the request the hour it started with), and never more than LIM-M1.
#[test]
fn req_sched_007_the_requests_deadline_starts_when_it_is_sent() {
    let rig = rig(
        vec![Script::WaitThenAdmit {
            holders: vec![4242],
            waited_ms: 1,
        }],
        vec![complete()],
    );
    let task = rig.runtime.get_task(rig.task).unwrap().task;
    let remaining_at_start = task.remaining_ms(rig.clock.now());
    // The wait takes all but 30 s of the task's time.
    let clock = rig.clock.clone();
    let spend = remaining_at_start - 30_000;
    rig.gate.on_admit(move || clock.advance_millis(spend));
    step(&rig);
    assert_eq!(
        *rig.backend.deadlines.lock().unwrap(),
        [30_000],
        "the 30 s left after the wait, not the time left before it"
    );
    // A task with plenty of time is held to LIM-M1.
    let rig = self::rig(vec![], vec![complete()]);
    step(&rig);
    assert_eq!(
        *rig.backend.deadlines.lock().unwrap(),
        [crate::limits::LIM_M1_REQUEST_MS]
    );
}

/// REQ-SCHED-007 (013d-4): the task's deadline ends the wait for the gate with nothing sent, and
/// the task halts `budget_deadline`, as it does in the wait for a slot: the engine never received
/// the request, so it is not told that the engine failed (`backend_unavailable`).
#[test]
fn req_sched_007_the_deadline_ends_the_wait_and_halts_as_a_deadline() {
    let rig = rig(vec![Script::WaitUntilDeadline], vec![complete()]);
    let result = step(&rig);
    assert_eq!(result.outcome, StepOutcome::Halted);
    assert!(
        matches!(
            result.state,
            TaskState::Halted {
                halt_reason: HaltReason::BudgetDeadline
            }
        ),
        "{:?}",
        result.state
    );
    let last_change = rig
        .persistence
        .events()
        .into_iter()
        .filter(|e| e.task_id == Some(rig.task))
        .filter_map(|e| match e.event {
            Event::TaskStateChanged { to, reason, .. } => Some((to, reason)),
            _ => None,
        })
        .next_back();
    assert_eq!(
        last_change,
        Some(("halted".into(), Some(HaltReason::BudgetDeadline)))
    );
    assert!(rig.backend.held_when_sent.lock().unwrap().is_empty());
    assert_eq!(rig.gate.held(), 0);
}

/// REQ-SCHED-006, REQ-SCHED-007: a registry that cannot be used lets the request go on without
/// the gate, and the task's events say so once, whichever of the two (slot or gate) met it
/// first; the answer is read as usual.
#[test]
fn req_sched_007_a_registry_that_cannot_be_used_lets_the_request_go_on_and_says_so_once() {
    let reason = "the registry is not writable (permission denied)".to_owned();
    let rig = rig(
        vec![
            Script::Unadmitted(reason.clone()),
            Script::Unadmitted(reason.clone()),
        ],
        vec![
            json!({"decision": "act", "tool": "read_file", "arguments": {"path": "a.txt"}}),
            complete(),
        ],
    );
    assert_eq!(step(&rig).outcome, StepOutcome::Continued);
    assert_eq!(step(&rig).outcome, StepOutcome::Completed);
    let said: Vec<Option<String>> = rig
        .persistence
        .events()
        .into_iter()
        .filter_map(|e| match e.event {
            Event::TaskAdmissionChanged {
                state: AdmissionState::Unadmitted,
                reason,
                ..
            } => Some(reason),
            _ => None,
        })
        .collect();
    assert_eq!(said, [Some(format!("engine gate: {reason}"))]);
    assert_eq!(rig.backend.held_when_sent.lock().unwrap().len(), 2);
}
