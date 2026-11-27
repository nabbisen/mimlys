//! The client (external design §4; REQ-ARCH-004, REQ-ARCH-009; handoff 014b, item 3): it makes
//! the request id and carries the session, hands the envelope to whatever transport it has, and
//! gives back what the runtime answered, as the verb answers it.

use super::*;
use mimlys_core::action::{EffectKind, EffectView};
use mimlys_core::api::{Notice, Outcome};
use mimlys_core::clock::FakeClock;
use mimlys_core::ids::ScriptedIds;
use mimlys_core::path::PlatformRules;
use mimlys_core::policy::Policy;
use mimlys_core::runtime::RuntimeDeps;
use mimlys_core::spi::fake::{FakePersistence, FakeToolProvider, FakeWorkspaces, ScriptedBackend};
use mimlys_core::task::{ScopeSpec, TaskState};
use mimlys_core::tool::{Param, ParamSchema, ParamType, Provider, ResourceProfile, ToolDefinition};
use std::sync::Mutex;

fn session() -> Session {
    Session {
        id: "pid:4242".into(),
        os_user: "tester".into(),
    }
}

/// A transport that keeps every envelope it was given and answers with what it was told to.
struct Recording {
    seen: Mutex<Vec<Envelope>>,
    answer: Box<dyn Fn(&Envelope) -> Response + Send + Sync>,
}

impl Recording {
    fn answering(answer: impl Fn(&Envelope) -> Response + Send + Sync + 'static) -> Arc<Self> {
        Arc::new(Self {
            seen: Mutex::new(Vec::new()),
            answer: Box::new(answer),
        })
    }

    fn seen(&self) -> Vec<Envelope> {
        self.seen.lock().unwrap().clone()
    }
}

impl Transport for Recording {
    fn send(&self, envelope: Envelope, _notice: &dyn Fn(Notice)) -> Response {
        let response = (self.answer)(&envelope);
        self.seen.lock().unwrap().push(envelope);
        response
    }
}

fn done(envelope: &Envelope, reply: Reply) -> Response {
    Response {
        request_id: envelope.request_id,
        outcome: Outcome::Done(reply),
    }
}

/// REQ-ARCH-004: every request carries the session and an id of its own, a version 7 UUID; no
/// two requests share an id, and a request sent again goes under the id it first had.
#[test]
fn req_arch_004_the_client_makes_one_id_per_request_and_carries_the_session() {
    let transport = Recording::answering(|e| done(e, Reply::CreationForbidden(false)));
    let client = Client::over(transport.clone(), session());
    client.creation_forbidden(None, "default").unwrap();
    client.creation_forbidden(None, "default").unwrap();
    let again = RequestId::new(&SystemIds);
    client
        .send_as(
            again,
            Request::CreationForbidden {
                profile: None,
                backend: "default".into(),
            },
        )
        .unwrap();
    let seen = transport.seen();
    assert_eq!(seen.len(), 3);
    assert!(seen.iter().all(|e| e.session == session()));
    assert!(seen.iter().all(|e| e.request_id.is_v7()));
    assert_ne!(seen[0].request_id, seen[1].request_id);
    assert_eq!(seen[2].request_id, again);
    assert_eq!(client.session(), &session());
}

/// Each typed call sends the request of its verb, with the arguments it was given.
#[test]
fn req_arch_004_a_typed_call_sends_the_request_of_its_verb() {
    let transport = Recording::answering(|e| done(e, Reply::CreationForbidden(false)));
    let client = Client::over(transport.clone(), session());
    let ids = ScriptedIds::default();
    let task = mimlys_core::ids::TaskId::new(&ids);
    // The replies do not answer these requests: only what was sent is judged.
    let _ = client.pause_task(task);
    let _ = client.cancel_task(task);
    let _ = client.approve_action(task, Some("fine".into()));
    let _ = client.reject_action(task, None, Some(true));
    let _ = client.answer_question(task, "main");
    let _ = client.resume_task(task, None);
    let _ = client.delete_task(task, true);
    let seen = transport.seen();
    let sent: Vec<&str> = seen.iter().map(|e| e.request.name()).collect();
    assert_eq!(
        sent,
        [
            "pause_task",
            "cancel_task",
            "approve_action",
            "reject_action",
            "answer_question",
            "resume_task",
            "delete_task"
        ]
    );
    assert_eq!(
        seen[2].request,
        Request::ApproveAction {
            task_id: task,
            note: Some("fine".into())
        }
    );
    assert_eq!(
        seen[3].request,
        Request::RejectAction {
            task_id: task,
            note: None,
            halt: Some(true)
        }
    );
    assert_eq!(
        seen[6].request,
        Request::DeleteTask {
            task_id: task,
            audit: true
        }
    );
}

/// An answer to another request, or of another kind than the request's, is the defect of a
/// transport: an `internal` error that names it, never a result made up from it.
#[test]
fn req_arch_004_an_answer_that_does_not_answer_is_an_internal_error() {
    let ids = ScriptedIds::default();
    let task = mimlys_core::ids::TaskId::new(&ids);
    let other_id = Recording::answering(|e| {
        let mut response = done(e, Reply::CreationForbidden(true));
        response.request_id = RequestId::new(&ScriptedIds::starting_at(77));
        response
    });
    let error = Client::over(other_id, session())
        .creation_forbidden(None, "default")
        .unwrap_err();
    assert_eq!(error.kind(), "internal", "{error}");
    let wrong_kind = Recording::answering(|e| done(e, Reply::CreationForbidden(true)));
    let error = Client::over(wrong_kind, session())
        .pause_task(task)
        .unwrap_err();
    assert_eq!(error.kind(), "internal", "{error}");
    assert!(error.to_string().contains("creation_forbidden"), "{error}");
}

/// The runtime's error reaches the caller as the verb gives it.
#[test]
fn req_arch_004_the_runtime_s_error_reaches_the_caller_as_the_verb_gives_it() {
    let transport = Recording::answering(|e| Response {
        request_id: e.request_id,
        outcome: Outcome::Failed(RuntimeError::NotFound {
            what: "task".into(),
        }),
    });
    let ids = ScriptedIds::default();
    let error = Client::over(transport, session())
        .cancel_task(mimlys_core::ids::TaskId::new(&ids))
        .unwrap_err();
    assert_eq!(
        error,
        RuntimeError::NotFound {
            what: "task".into()
        }
    );
}

fn read_file() -> ToolDefinition {
    ToolDefinition {
        name: "read_file".into(),
        description: "read.".into(),
        input: ParamSchema {
            params: vec![Param {
                name: "path".into(),
                ty: ParamType::WorkspacePath,
                optional: false,
                description: "file".into(),
            }],
        },
        output: None,
        effect: EffectView {
            kind: EffectKind::WorkspaceRead,
            reversible: true,
            idempotent: true,
        },
        deadline_ms: 30_000,
        profile: ResourceProfile::Builtin,
        destination_arguments: vec![],
        provider: Provider::Builtin,
    }
}

/// The in-process transport reaches the runtime of the process: a task made through the client
/// is the runtime's, the client's session is the actor of its events, and the ask of the
/// creation check is answered by the runtime's policy.
#[test]
fn req_arch_009_the_in_process_transport_reaches_the_runtime_of_the_process() {
    let persistence = Arc::new(FakePersistence::default());
    let tools = Arc::new(FakeToolProvider::new(vec![read_file()]));
    let runtime = Arc::new(
        Runtime::new(RuntimeDeps {
            persistence: persistence.clone(),
            backend: Arc::new(ScriptedBackend::with_decisions(vec![])),
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
    let client = Client::in_process(Arc::clone(&runtime), session());
    let task = client
        .create_task(CreateTask {
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
            hold: true,
            from_task: None,
            from_snapshot: None,
            plan_first: false,
            effort: None,
        })
        .unwrap();
    assert_eq!(task.state, TaskState::Created);
    assert_eq!(
        runtime.get_task(task.task_id).unwrap().task.state,
        TaskState::Created
    );
    let created = persistence
        .events()
        .into_iter()
        .find(|e| e.event.name() == "task.created")
        .unwrap();
    assert_eq!(created.actor, session().actor());
    let cancelled = client.cancel_task(task.task_id).unwrap();
    assert_eq!(cancelled.state.name(), "cancelled");
    assert!(!client.creation_forbidden(None, "default").unwrap());
    // A refused request is the verb's own error.
    let refused = client.resume_task(task.task_id, None).unwrap_err();
    assert_eq!(refused.kind(), "state");
}
