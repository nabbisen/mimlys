//! The probe's task and its verdict, in process, on every platform, over the core's fakes
//! (REQ-MDL-011, REQ-ARCH-006): the runtime is the core's, the store, the tools and the workspaces
//! are fakes, the clock is a fake that moves only when a test moves it, and no time is read. A
//! verdict here is a state: what `drive` left in the store and what `classify` says of it.
//!
//! The stop signal is made as the signal's handler makes it (`Stop::signal`), so the same tests
//! stand for the Ctrl-C of Unix and of Windows.

use mimlys::probe::drive::{Driven, Stop, drive, observe};
use mimlys::probe::{self, Class, Detail, Verdict, classify};
use mimlys_core::action::{EffectKind, EffectView};
use mimlys_core::clock::{Clock, FakeClock};
use mimlys_core::ids::{ScriptedIds, TaskId};
use mimlys_core::path::PlatformRules;
use mimlys_core::runtime::{CreateTask, Runtime, RuntimeDeps, RuntimeError, Session};
use mimlys_core::spi::fake::{FakePersistence, FakeToolProvider, FakeWorkspaces, ScriptedBackend};
use mimlys_core::spi::{
    BackendDescriptor, BackendError, BoxFuture, Cancellation, ExecutionOutcome, GenerateRequest,
    Locality, ModelBackend,
};
use mimlys_core::task::{ScopeSpec, TaskState};
use mimlys_core::tool::{Param, ParamSchema, ParamType, Provider, ResourceProfile, ToolDefinition};
use serde_json::{Value, json};
use std::sync::Arc;

fn string(name: &str) -> Param {
    Param {
        name: name.into(),
        ty: ParamType::String {
            min_chars: 0,
            max_chars: 4096,
            max_bytes: None,
        },
        optional: false,
        description: name.into(),
    }
}

fn path() -> Param {
    Param {
        name: "path".into(),
        ty: ParamType::WorkspacePath,
        optional: false,
        description: "file".into(),
    }
}

fn tool(name: &str, params: Vec<Param>, kind: EffectKind) -> ToolDefinition {
    ToolDefinition {
        name: name.into(),
        description: name.into(),
        input: ParamSchema { params },
        output: None,
        effect: EffectView {
            kind,
            reversible: true,
            idempotent: kind == EffectKind::WorkspaceRead,
        },
        deadline_ms: 30_000,
        profile: ResourceProfile::Builtin,
        destination_arguments: vec![],
        provider: Provider::Builtin,
    }
}

fn tools() -> Arc<FakeToolProvider> {
    let tools = Arc::new(FakeToolProvider::new(vec![
        tool("read_file", vec![path()], EffectKind::WorkspaceRead),
        tool(
            "write_file",
            vec![path(), string("content")],
            EffectKind::WorkspaceWrite,
        ),
        tool(
            "replace_in_file",
            vec![path(), string("find"), string("replace")],
            EffectKind::WorkspaceWrite,
        ),
    ]));
    for name in ["read_file", "write_file", "replace_in_file"] {
        for _ in 0..16 {
            tools.script(name, ExecutionOutcome::succeeded(json!({}), 0));
        }
    }
    tools
}

fn session() -> Session {
    Session {
        id: "s1".into(),
        os_user: "tester".into(),
    }
}

struct Rig {
    runtime: Arc<Runtime>,
    clock: Arc<FakeClock>,
}

impl Rig {
    fn new(backend: Arc<dyn ModelBackend>, external: bool) -> Self {
        let clock = Arc::new(FakeClock::default());
        let runtime = Runtime::new(RuntimeDeps {
            persistence: Arc::new(FakePersistence::default()),
            backend,
            backend_name: "default".to_owned(),
            workspaces: Arc::new(FakeWorkspaces::new(tools())),
            policy: probe::policy(external),
            ids: Arc::new(ScriptedIds::default()),
            clock: Arc::clone(&clock) as Arc<dyn Clock>,
            runtime_version: "0.1.0-test".into(),
            rules: PlatformRules::Posix,
            probe: mimlys_core::confine::Probe::unavailable("no worker in these tests"),
            secrets: vec![],
            context_chars: mimlys_core::limits::LIM_C1_CONTEXT_CHARS,
            output_bytes: mimlys_core::limits::LIM_S4_OUTPUT_BYTES as usize,
        })
        .unwrap();
        Self {
            runtime: Arc::new(runtime),
            clock,
        }
    }

    /// The probe's task, as the command makes it.
    fn create(&self) -> Result<TaskId, RuntimeError> {
        Ok(self
            .runtime
            .create_task(
                &session(),
                CreateTask {
                    goal: probe::GOAL.into(),
                    scope: ScopeSpec {
                        root: "/data/tmp/probe-x".into(),
                        include: vec![],
                    },
                    profile: Some(probe::PROFILE.into()),
                    grants: vec![],
                    budgets: None,
                    deadline_ms: Some(probe::WALL_MS),
                    completion_check: None,
                    hold: false,
                    from_task: None,
                    from_snapshot: None,
                    plan_first: false,
                    effort: None,
                },
            )?
            .task_id)
    }

    /// Drives the task and judges it, the file being as asked or not as `as_asked` says.
    async fn verdict(&self, id: TaskId, as_asked: bool) -> Verdict {
        let stop = Stop::new();
        assert_eq!(
            drive(&self.runtime, id, &stop).await.unwrap(),
            Driven::Ended
        );
        let (observed, _) = observe(&self.runtime, id, || as_asked).unwrap();
        classify(&observed)
    }
}

fn scripted(replies: Vec<Value>) -> Arc<ScriptedBackend> {
    Arc::new(ScriptedBackend::with_decisions(replies))
}

fn act(tool: &str, arguments: Value) -> Value {
    json!({"decision": "act", "tool": tool, "arguments": arguments})
}

fn edit() -> Value {
    act(
        "replace_in_file",
        json!({"path": "probe.txt", "find": "red", "replace": "blue"}),
    )
}

fn complete() -> Value {
    json!({"decision": "complete", "summary": "done"})
}

fn class(verdict: Verdict) -> (Class, Option<Detail>) {
    match verdict {
        Verdict::Class(class, detail) => (class, detail),
        other => panic!("not a class: {other:?}"),
    }
}

/// REQ-MDL-011: the steps of a model that acts and completes, read from the store, are `acts`
/// when the runtime finds the file as asked, and `incomplete` (`not_verified`) when it does not:
/// the same task and the same replies, the verdict the runtime's reading. This is the seam the
/// model's word never crosses.
#[tokio::test]
async fn req_mdl_011_the_verdict_is_the_runtimes_reading_of_the_file() {
    for (as_asked, expected) in [
        (true, (Class::Acts, None)),
        (false, (Class::Incomplete, Some(Detail::NotVerified))),
    ] {
        let rig = Rig::new(scripted(vec![edit(), complete()]), false);
        let id = rig.create().unwrap();
        assert_eq!(class(rig.verdict(id, as_asked).await), expected);
        assert!(matches!(
            rig.runtime.get_task(id).unwrap().task.state,
            TaskState::Completed { .. }
        ));
    }
}

/// REQ-MDL-011: a model that completes without acting is `plans_only` whatever the file reading
/// says (the file is the probe's own and was not touched), and it says how it ended: it said it
/// was done, and the file was not as asked (`not_verified`).
#[tokio::test]
async fn req_mdl_011_completing_without_acting_is_plans_only() {
    let rig = Rig::new(scripted(vec![complete()]), false);
    let id = rig.create().unwrap();
    assert_eq!(
        class(rig.verdict(id, false).await),
        (Class::PlansOnly, Some(Detail::NotVerified))
    );
}

/// The probe's task run to its end over `replies`, and what it came to.
async fn rejected_run(replies: Vec<Value>) -> (probe::Observed, (Class, Option<Detail>)) {
    let rig = Rig::new(scripted(replies), false);
    let id = rig.create().unwrap();
    let stop = Stop::new();
    assert_eq!(drive(&rig.runtime, id, &stop).await.unwrap(), Driven::Ended);
    let (observed, _) = observe(&rig.runtime, id, || false).unwrap();
    let verdict = classify(&observed);
    (observed, class(verdict))
}

/// REQ-MDL-011 (handoff 015a-5, item 7): a step counts as acting when its decision was an `act`,
/// accepted or rejected after parsing, not only when an action record was made. A model whose
/// every call to `write_file` is invalid, whatever the rejection (`schema`, `unknown_tool`,
/// `invalid_argument`, `denied`), proposed actions, and badly: it is `incomplete`, not
/// `plans_only`; the guards of the task end it (`not_finished`). The same replies that are not
/// `act`s are `plans_only`.
#[tokio::test]
async fn req_mdl_011_a_rejected_act_is_acting() {
    let rejected: [(&str, Value); 4] = [
        (
            "schema",
            act("write_file", json!({"path": "probe.txt", "content": 5})),
        ),
        (
            "unknown_tool",
            act(
                "write_the_file",
                json!({"path": "probe.txt", "content": "x"}),
            ),
        ),
        (
            "invalid_argument",
            act("write_file", json!({"path": "a\u{0}b", "content": "x"})),
        ),
        (
            "denied",
            act(
                "write_file",
                json!({"path": "../outside.txt", "content": "x"}),
            ),
        ),
    ];
    for (code, reply) in rejected {
        let (observed, verdict) = rejected_run(vec![reply; 8]).await;
        assert!(
            observed.steps.iter().all(|s| s.replied && s.acted),
            "{code}: {:?}",
            observed.steps
        );
        assert_eq!(
            verdict,
            (Class::Incomplete, Some(Detail::NotFinished)),
            "{code}: a rejected `act` is acting"
        );
    }
    // Rejected, and then the model says it is done: it acted, and the file was not as asked.
    let (_, verdict) = rejected_run(vec![
        act("write_file", json!({"path": "probe.txt", "content": 5})),
        complete(),
    ])
    .await;
    assert_eq!(verdict, (Class::Incomplete, Some(Detail::NotVerified)));
    // Rejected, and then asking: it acted first.
    let (_, verdict) = rejected_run(vec![
        act("write_the_file", json!({})),
        json!({"decision": "ask_user", "question": "how?"}),
    ])
    .await;
    assert_eq!(verdict, (Class::Incomplete, Some(Detail::AskedUser)));
}

/// REQ-MDL-011: what is not an `act` is not acting. A decision that is not an `act` is not, and a
/// reply that does not parse as a decision is not known to be one: an `act` with no `arguments`
/// is refused by the envelope's own schema, before any tool is named, and is a reply that was not
/// accepted (the model said something, it is not `no_structured_call`), without being acting.
#[tokio::test]
async fn req_mdl_011_what_is_not_an_act_is_not_acting() {
    let (observed, verdict) = rejected_run(vec![
        json!({"decision": "act", "tool": "write_file"}),
        json!({"decision": "act", "tool": "write_file"}),
        json!({"decision": "act", "tool": "write_file"}),
        json!({"decision": "act", "tool": "write_file"}),
    ])
    .await;
    assert!(
        observed.steps.iter().all(|s| s.replied && !s.malformed),
        "{:?}",
        observed.steps
    );
    assert!(
        observed.steps.iter().all(|s| !s.acted),
        "{:?}",
        observed.steps
    );
    assert_eq!(verdict, (Class::PlansOnly, Some(Detail::NotFinished)));
}

/// REQ-MDL-011: the classes from the store, one by one: plans until the steps run out, prose, a
/// question, giving up, an engine that fails, and an edit that never ends.
#[tokio::test]
async fn req_mdl_011_each_class_from_what_the_store_holds() {
    let plans: Vec<Value> = (0..12)
        .map(|n| json!({"decision": "plan", "steps": [format!("step {n}")]}))
        .collect();
    let rig = Rig::new(scripted(plans), false);
    let id = rig.create().unwrap();
    assert_eq!(
        class(rig.verdict(id, false).await),
        (Class::PlansOnly, Some(Detail::NotFinished))
    );
    let (_, measures) = observe(&rig.runtime, id, || false).unwrap();
    assert_eq!(measures.steps, 8, "LIM-E2: the steps of the budget");

    let ask = Rig::new(
        scripted(vec![
            act("read_file", json!({"path": "probe.txt"})),
            json!({"decision": "ask_user", "question": "which?"}),
        ]),
        false,
    );
    let id = ask.create().unwrap();
    assert_eq!(
        class(ask.verdict(id, false).await),
        (Class::Incomplete, Some(Detail::AskedUser))
    );

    let give_up = Rig::new(
        scripted(vec![
            act("read_file", json!({"path": "probe.txt"})),
            json!({"decision": "give_up", "reason": "no"}),
        ]),
        false,
    );
    let id = give_up.create().unwrap();
    assert_eq!(
        class(give_up.verdict(id, false).await),
        (Class::Incomplete, Some(Detail::GaveUp))
    );

    let never = Rig::new(
        scripted(
            (0..12)
                .map(|n| {
                    act(
                        "replace_in_file",
                        json!({"path": "probe.txt", "find": format!("x{n}"), "replace": "y"}),
                    )
                })
                .collect(),
        ),
        false,
    );
    let id = never.create().unwrap();
    assert_eq!(
        class(never.verdict(id, false).await),
        (Class::Incomplete, Some(Detail::NotFinished))
    );

    let prose = Arc::new(ScriptedBackend::with_decisions(vec![]));
    for _ in 0..5 {
        prose.push(Ok(b"Sure, I will do that.".to_vec()));
    }
    let rig = Rig::new(prose, false);
    let id = rig.create().unwrap();
    assert_eq!(
        class(rig.verdict(id, false).await),
        (Class::NoStructuredCall, None)
    );

    let down = Arc::new(ScriptedBackend::with_decisions(vec![]));
    for _ in 0..4 {
        down.push(Err(BackendError::Unavailable {
            detail: "refused".into(),
        }));
    }
    let rig = Rig::new(down, false);
    let id = rig.create().unwrap();
    // No class: it is said, and the command does not record it.
    assert_eq!(rig.verdict(id, false).await, Verdict::Unreachable);
}

/// A backend that answers its first request with an `act` and moves the clock past LIM-E3 doing
/// so: the model that was slow, as the clock tells it.
struct SlowAfterOneAct {
    clock: Arc<FakeClock>,
    asked: std::sync::atomic::AtomicUsize,
}

impl ModelBackend for SlowAfterOneAct {
    fn describe(&self) -> BackendDescriptor {
        BackendDescriptor {
            kind: "slow".into(),
            model: None,
            locality: Locality::Local,
        }
    }

    fn generate<'a>(
        &'a self,
        _request: GenerateRequest,
        _cancel: Cancellation,
    ) -> BoxFuture<'a, Result<Vec<u8>, BackendError>> {
        Box::pin(async move {
            self.asked.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.clock.advance_millis(probe::WALL_MS + 1);
            Ok(serde_json::to_vec(&act("read_file", json!({"path": "probe.txt"}))).unwrap())
        })
    }
}

/// REQ-MDL-011, LIM-E3: the wall time is the task's deadline. A model that acted and then took
/// more than LIM-E3 is `incomplete` (`not_finished`), and the next step is not even asked of it;
/// a deadline that passes before any request was sent measured nothing, and says so rather than
/// call the backend unreachable.
#[tokio::test]
async fn req_mdl_011_the_wall_time_is_the_tasks_deadline() {
    let clock = Arc::new(FakeClock::default());
    let backend = Arc::new(SlowAfterOneAct {
        clock: Arc::clone(&clock),
        asked: std::sync::atomic::AtomicUsize::new(0),
    });
    let mut rig = Rig::new(backend.clone(), false);
    // The rig's own clock is the one the backend moves.
    rig.clock = Arc::clone(&clock);
    let runtime = Runtime::new(RuntimeDeps {
        persistence: Arc::new(FakePersistence::default()),
        backend: backend.clone(),
        backend_name: "default".to_owned(),
        workspaces: Arc::new(FakeWorkspaces::new(tools())),
        policy: probe::policy(false),
        ids: Arc::new(ScriptedIds::default()),
        clock: Arc::clone(&clock) as Arc<dyn Clock>,
        runtime_version: "0.1.0-test".into(),
        rules: PlatformRules::Posix,
        probe: mimlys_core::confine::Probe::unavailable("no worker in these tests"),
        secrets: vec![],
        context_chars: mimlys_core::limits::LIM_C1_CONTEXT_CHARS,
        output_bytes: mimlys_core::limits::LIM_S4_OUTPUT_BYTES as usize,
    })
    .unwrap();
    rig.runtime = Arc::new(runtime);
    let id = rig.create().unwrap();
    assert_eq!(
        class(rig.verdict(id, false).await),
        (Class::Incomplete, Some(Detail::NotFinished))
    );
    assert_eq!(backend.asked.load(std::sync::atomic::Ordering::SeqCst), 1);

    // The deadline passes before the first request: nothing was sent, nothing was measured.
    let rig = Rig::new(scripted(vec![edit(), complete()]), false);
    let id = rig.create().unwrap();
    rig.clock.advance_millis(probe::WALL_MS + 1);
    assert_eq!(rig.verdict(id, false).await, Verdict::NotMeasured);
}

/// REQ-MDL-011, REQ-SEC-010: the probe's profile has no `external`, and the rule that a remote
/// backend needs one is not set aside for the probe: a remote backend under the profile of a local
/// one is refused when the task is created (and at every start), nothing is sent; the profile
/// that the command builds for a remote backend, named and allowed, has it, and the same task is
/// made.
#[tokio::test]
async fn req_sec_010_a_remote_backend_needs_the_probe_profile_that_says_external() {
    let remote = Arc::new(ScriptedBackend::with_decisions(vec![edit(), complete()]).remote());
    let refused = Rig::new(remote.clone(), false).create().unwrap_err();
    assert!(
        refused
            .to_string()
            .contains("forbids external communication"),
        "{refused}"
    );
    assert!(remote.requests().is_empty(), "nothing was sent");
    let allowed = Rig::new(remote.clone(), true);
    let id = allowed.create().unwrap();
    assert_eq!(class(allowed.verdict(id, true).await), (Class::Acts, None));
    assert_eq!(remote.requests().len(), 2);
    // A start after the backend became remote halts `policy`, whichever way the task was made.
    let local = Arc::new(ScriptedBackend::with_decisions(vec![edit(), complete()]));
    let rig = Rig::new(local.clone(), false);
    let id = rig.create().unwrap();
    local.set_remote(true);
    let (observed, _) = {
        let stop = Stop::new();
        drive(&rig.runtime, id, &stop).await.unwrap();
        observe(&rig.runtime, id, || true).unwrap()
    };
    assert!(
        matches!(
            observed.end,
            probe::End::Halted(mimlys_core::task::HaltReason::Policy)
        ),
        "{:?}",
        observed.end
    );
    assert!(local.requests().is_empty(), "nothing was sent");
    assert_eq!(classify(&observed), Verdict::Fault);
}

/// REQ-MDL-011: a stop signal that comes before the task exists is kept and applied when it does:
/// the driving ends without a request to the model.
#[tokio::test]
async fn req_mdl_011_a_stop_before_the_task_exists_is_applied_when_it_does() {
    let backend = scripted(vec![edit(), complete()]);
    let rig = Rig::new(backend.clone(), false);
    let stop = Stop::new();
    stop.signal();
    assert!(stop.requested());
    let id = rig.create().unwrap();
    assert_eq!(
        drive(&rig.runtime, id, &stop).await.unwrap(),
        Driven::Stopped
    );
    assert!(
        backend.requests().is_empty(),
        "nothing was asked of the model"
    );
}

/// A backend whose request stays open until it is cancelled: the model that is thinking.
struct Thinking {
    started: tokio::sync::Notify,
    cancelled: std::sync::atomic::AtomicBool,
}

impl ModelBackend for Thinking {
    fn describe(&self) -> BackendDescriptor {
        BackendDescriptor {
            kind: "thinking".into(),
            model: None,
            locality: Locality::Local,
        }
    }

    fn generate<'a>(
        &'a self,
        _request: GenerateRequest,
        cancel: Cancellation,
    ) -> BoxFuture<'a, Result<Vec<u8>, BackendError>> {
        Box::pin(async move {
            self.started.notify_one();
            // A state, never a time: the request ends when it is cancelled, however long that is.
            while !cancel.is_cancelled() {
                tokio::task::yield_now().await;
            }
            self.cancelled
                .store(true, std::sync::atomic::Ordering::SeqCst);
            Err(BackendError::Cancelled)
        })
    }
}

/// REQ-MDL-011, REQ-CLI-005: a stop signal during the model's request cancels the request, as for
/// a task, and the driving ends as stopped; the task is left for the command to delete.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn req_mdl_011_a_stop_signal_cancels_the_request_in_flight() {
    let backend = Arc::new(Thinking {
        started: tokio::sync::Notify::new(),
        cancelled: std::sync::atomic::AtomicBool::new(false),
    });
    let rig = Rig::new(backend.clone(), false);
    let id = rig.create().unwrap();
    let stop = Stop::new();
    let driving = {
        let runtime = Arc::clone(&rig.runtime);
        let stop = Arc::clone(&stop);
        tokio::spawn(async move { drive(&runtime, id, &stop).await })
    };
    backend.started.notified().await;
    stop.signal();
    // A bound for a failure only (a stop that is never applied would leave the request open for
    // ever): the verdict is the state the driving ends in.
    let ended = tokio::time::timeout(std::time::Duration::from_secs(60), driving)
        .await
        .expect("the stop ended the request");
    assert_eq!(ended.unwrap().unwrap(), Driven::Stopped);
    assert!(
        backend.cancelled.load(std::sync::atomic::Ordering::SeqCst),
        "the request was cancelled"
    );
}
