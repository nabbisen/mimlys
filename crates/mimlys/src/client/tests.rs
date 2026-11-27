//! The client (external design §4; REQ-ARCH-004, REQ-ARCH-009; handoff 014b, item 3): it makes
//! the request id and carries the session, hands the envelope to whatever transport it has, and
//! gives back what the runtime answered, as the verb answers it.

use super::*;
use mimlys_core::action::{EffectKind, EffectView};
use mimlys_core::api::{Did, Notice, Outcome};
use mimlys_core::clock::{Clock, FakeClock};
use mimlys_core::ids::ScriptedIds;
use mimlys_core::path::PlatformRules;
use mimlys_core::policy::Policy;
use mimlys_core::runtime::RuntimeDeps;
use mimlys_core::spi::PersistenceRead;
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

/// External design §4: a pause met while a step is in flight is noted, and its reply comes at once
/// and says so: the client returns the task as it is, and the request is not an error.
#[test]
fn req_task_005_a_pause_that_was_noted_returns_the_task_as_it_is() {
    let ids = ScriptedIds::default();
    let task_id = mimlys_core::ids::TaskId::new(&ids);
    let transport =
        Recording::answering(move |e| done(e, Reply::Noted(Box::new(noted_task(task_id)))));
    let client = Client::over(transport, session());
    let task = client.pause_task(task_id).unwrap();
    assert_eq!(task.task_id, task_id);
    assert!(!task.pause_requested, "not written yet");
}

fn noted_task(task_id: mimlys_core::ids::TaskId) -> Task {
    let mut task = Task::create(
        mimlys_core::task::TaskSpec {
            task_id,
            goal: "g".into(),
            deadline_ms: mimlys_core::limits::LIM_T5_DEADLINE_MS,
            budgets: mimlys_core::step::Budgets::default(),
            profile: "workspace".into(),
            versions: mimlys_core::task::TaskVersions {
                policy_hash: mimlys_core::hash::hash_bytes(b"policy"),
                decision_schema_version: 1,
                event_schema_version: mimlys_core::event::EVENT_SCHEMA_VERSION,
                action_schema_version: mimlys_core::action::ACTION_SCHEMA_VERSION,
                tool_definition_hashes: Default::default(),
                runtime_version: "0.0.0-test".into(),
                backend: None,
                tool_imports: Default::default(),
            },
            scope: ScopeSpec {
                root: "/project".into(),
                include: vec![],
            },
            plan_first: false,
        },
        FakeClock::default().now(),
    )
    .unwrap();
    task.state = TaskState::Running;
    task
}

/// A runtime over the fakes, and a client of it, in-process: what the commands have, with the
/// store and the workspaces in the test's hands.
struct World {
    client: Client,
    persistence: Arc<FakePersistence>,
    workspaces: Arc<FakeWorkspaces>,
}

fn world() -> World {
    let persistence = Arc::new(FakePersistence::default());
    let workspaces = Arc::new(FakeWorkspaces::new(Arc::new(FakeToolProvider::new(vec![
        read_file(),
    ]))));
    let runtime = Arc::new(
        Runtime::new(RuntimeDeps {
            persistence: persistence.clone(),
            backend: Arc::new(ScriptedBackend::with_decisions(vec![])),
            backend_name: "default".to_owned(),
            workspaces: workspaces.clone(),
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
    World {
        client: Client::in_process(runtime, session()),
        persistence,
        workspaces,
    }
}

impl World {
    /// A task held in `created`.
    fn task(&self) -> Task {
        self.client
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
            .unwrap()
    }

    /// A plan of one added file against the task's base snapshot.
    fn plan(&self, task_id: mimlys_core::ids::TaskId) -> PromotionPlan {
        let base = self.persistence.load_snapshots(task_id).unwrap()[0].clone();
        PromotionPlan {
            task_id,
            base_snapshot: base.snapshot_id,
            current_hash: mimlys_core::hash::hash_bytes(b""),
            changes: vec![mimlys_core::snapshot::Change {
                path: "a.txt".into(),
                kind: mimlys_core::snapshot::ChangeKind::Added,
                bytes: 1,
            }],
            excluded: vec![],
            text_diffs: vec![],
        }
    }

    /// A request sent twice under one id: both answers, and the events as the first left them
    /// (the repeat wrote none).
    fn twice(
        &self,
        request: Request,
    ) -> (Result<Reply, RuntimeError>, Result<Reply, RuntimeError>) {
        let id = RequestId::new(&SystemIds);
        let first = self.client.send_as(id, request.clone());
        let events = self.persistence.events();
        let second = self.client.send_as(id, request);
        assert_eq!(self.persistence.events(), events, "the repeat wrote");
        (first, second)
    }
}

/// External design §4, "The recorded outcome": a repeat of a request that **took effect** is a
/// success, as the first answer was, through the client: the task as it is now for a verb that
/// returns one, and for the others what the first answer held, read back (the grant, the snapshot,
/// the promotion), so that a command prints what it would have printed. It is never an error.
#[test]
fn req_arch_004_a_repeat_of_a_request_that_took_effect_is_a_success_through_the_client() {
    let w = world();
    let task = w.task();
    let id = task.task_id;
    // The others: what the first answer held, whichever the verb.
    let held = |request: Request| {
        let (first, second) = w.twice(request);
        let first = first.unwrap();
        assert_eq!(second.unwrap(), first);
        first
    };
    let Reply::Grant(grant) = held(Request::GrantCapability {
        task_id: id,
        kind: mimlys_core::grant::GrantKind::Tool {
            name: "write_file".into(),
        },
    }) else {
        panic!("a grant")
    };
    let Reply::Grant(revoked) = held(Request::RevokeCapability {
        task_id: id,
        grant_id: grant.grant_id,
    }) else {
        panic!("a grant")
    };
    assert_eq!(revoked.grant_id, grant.grant_id);
    assert!(matches!(
        held(Request::RollbackWorkspace {
            task_id: id,
            snapshot_id: None
        }),
        Reply::Snapshot(_)
    ));
    let Reply::Promotion(promotion) = held(Request::PromoteWorkspace {
        task_id: id,
        plan: w.plan(id),
    }) else {
        panic!("a promotion")
    };
    assert!(matches!(
        held(Request::RevertPromotion {
            task_id: id,
            promotion_id: promotion.promotion_id,
            overwrite_edited: false,
        }),
        Reply::Promotion(_)
    ));
    // A verb that returns the task.
    let (first, second) = w.twice(Request::CancelTask { task_id: id });
    let (Ok(Reply::Task(first)), Ok(Reply::Task(second))) = (first, second) else {
        panic!("the task, twice")
    };
    assert_eq!(first.state, TaskState::Cancelled);
    assert_eq!(second, first);
}

/// External design §4: a promotion that **failed after it wrote** is answered, when sent again,
/// with the recorded failure and its reason (never as applied); the first answer was the same
/// words.
#[test]
fn req_arch_004_a_repeat_of_a_promotion_that_failed_after_it_wrote_is_the_recorded_failure() {
    let w = world();
    let id = w.task().task_id;
    w.workspaces.on_promote_apply(Box::new(|_| {
        Err(mimlys_core::spi::WorkspaceError::Storage {
            detail: "injected".into(),
        })
    }));
    let (first, second) = w.twice(Request::PromoteWorkspace {
        task_id: id,
        plan: w.plan(id),
    });
    let first = first.unwrap_err();
    assert!(matches!(first, RuntimeError::PromotionFailed { .. }));
    let second = second.unwrap_err();
    assert!(
        matches!(second, RuntimeError::Recorded { .. }),
        "{second:?}"
    );
    assert_eq!(second.to_string(), first.to_string(), "the reason");
    assert!(second.to_string().contains("injected"), "{second}");
    // A failure, not a success: nothing says that it was done.
    assert!(
        !w.persistence
            .events()
            .iter()
            .any(|e| e.event.name() == "workspace.promoted")
    );
}

/// External design §4: a request that was **refused** wrote nothing, so a repeat of it is answered
/// afresh: refused again, with the same words, and applied once what refused it has changed.
#[test]
fn req_arch_004_a_repeat_of_a_refused_request_is_answered_afresh_through_the_client() {
    let w = world();
    let id = w.task().task_id;
    let request = Request::ApproveAction {
        task_id: id,
        note: None,
    };
    let (first, second) = w.twice(request.clone());
    let first = first.unwrap_err();
    assert_eq!(first.kind(), "state", "{first}");
    assert_eq!(second.unwrap_err(), first);
    // Refused afresh each time: the id is not recorded, so a revocation of an unknown grant is
    // answered `not_found` each time, and so is the same request after the world changed.
    let unknown = mimlys_core::ids::GrantId::new(&ScriptedIds::default());
    let (first, second) = w.twice(Request::RevokeCapability {
        task_id: id,
        grant_id: unknown,
    });
    assert_eq!(first.unwrap_err().kind(), "not_found");
    assert_eq!(second.unwrap_err().kind(), "not_found");
}

/// External design §4: a **deletion repeated after it took effect** is answered that the task is
/// gone, never `not_found`, when the id is the one the client sent: whether the audit stayed (the
/// request is found by its event) or went with the task (nothing is found, and the client knows
/// the id). A deletion that was refused is refused again, and a first deletion of a task that
/// never existed is `not_found`.
#[test]
fn req_arch_004_a_deletion_repeated_after_it_took_effect_is_answered_that_the_task_is_gone() {
    let w = world();
    for audit in [false, true] {
        let id = w.task().task_id;
        // Not terminal: refused, and refused again, as the state.
        let again = RequestId::new(&SystemIds);
        assert_eq!(
            w.client
                .delete_task_as(again, id, audit)
                .unwrap_err()
                .kind(),
            "state"
        );
        assert_eq!(
            w.client
                .delete_task_as(again, id, audit)
                .unwrap_err()
                .kind(),
            "state"
        );
        w.client.cancel_task(id).unwrap();
        let sent = RequestId::new(&SystemIds);
        let first = w.client.delete_task_as(sent, id, audit).unwrap();
        assert!(matches!(first, Deleted::Footprint(_)), "{first:?}");
        let events = w.persistence.events().len();
        assert_eq!(
            w.client.delete_task_as(sent, id, audit).unwrap(),
            Deleted::Gone,
            "audit {audit}"
        );
        assert!(
            w.persistence.events().len() <= events,
            "nothing was written"
        );
    }
    // An id that was not sent before, for a task that is not there: `not_found`, as it is.
    let ids = ScriptedIds::starting_at(900);
    let nobody = mimlys_core::ids::TaskId::new(&ids);
    let error = w.client.delete_task(nobody, false).unwrap_err();
    assert_eq!(error.kind(), "not_found", "{error}");
    // ...and the same request sent again is answered the same: it was refused, so it is not an id
    // under which a deletion took effect.
    let sent = RequestId::new(&SystemIds);
    for _ in 0..2 {
        let error = w.client.delete_task_as(sent, nobody, false).unwrap_err();
        assert_eq!(error.kind(), "not_found", "{error}");
    }
}

/// A transport that answers every request with the same recorded answer, as a runtime does for an
/// id it recorded.
fn answering_with(did: mimlys_core::api::Did, task: Option<Task>) -> Client {
    let transport = Recording::answering(move |e| Response {
        request_id: e.request_id,
        outcome: Outcome::AlreadyApplied(mimlys_core::api::AlreadyApplied {
            position: 12,
            event: "task.state_changed".into(),
            task: task.clone().map(Box::new),
            task_deleted: true,
            did: did.clone(),
        }),
    });
    Client::over(transport, session())
}

/// A request that took effect on a task deleted since, with nothing left to answer with, is
/// answered that it was applied and the task is gone, whichever the verb (the typed answer says
/// so), and never as an error: not `not_found`, not "applied already" (014b-6, item 2).
#[test]
fn req_arch_004_a_request_that_took_effect_on_a_task_deleted_since_finds_it_gone() {
    let ids = ScriptedIds::default();
    let task = mimlys_core::ids::TaskId::new(&ids);
    let client = answering_with(mimlys_core::api::Did::Applied { reply: None }, None);
    assert_eq!(client.delete_task(task, false).unwrap(), Deleted::Gone);
    assert_eq!(
        client.rollback_workspace(task, None).unwrap(),
        Applied::Gone
    );
    let grant = mimlys_core::grant::GrantKind::Tool {
        name: "write_file".into(),
    };
    assert_eq!(client.grant_capability(task, grant).unwrap(), Applied::Gone);
    let grant_id = mimlys_core::ids::GrantId::new(&ids);
    assert_eq!(
        client.revoke_capability(task, grant_id).unwrap(),
        Applied::Gone
    );
}

/// What the user is told of a request that took effect before: that it was done earlier, and,
/// when its task was deleted since or there is no reply left to give, that the task is gone.
#[test]
fn req_arch_004_the_client_says_that_the_task_is_gone_since() {
    let ids = ScriptedIds::default();
    let task = mimlys_core::ids::TaskId::new(&ids);
    let answered = |task_deleted: bool, reply: Option<Reply>| {
        applied_before(
            ("grant_capability", false),
            Some(task),
            AlreadyApplied {
                position: 7,
                event: "grant.issued".into(),
                task: None,
                task_deleted,
                did: Did::Applied { reply },
            },
        )
        .unwrap()
    };
    let done = crate::messages::done_earlier("grant_capability", 7, "grant.issued");
    let gone = crate::messages::task_gone(&task.to_string());
    let reply = Reply::CreationForbidden(false);
    // Applied earlier, the task still there: done earlier, and nothing more.
    let (answer, said) = answered(false, Some(reply.clone()));
    assert!(matches!(answer, Answered::Reply(_)));
    assert_eq!(said, vec![done.clone()]);
    // Applied earlier, the task deleted since and the reply kept: the reply, and the task is gone.
    let (answer, said) = answered(true, Some(reply));
    assert!(matches!(answer, Answered::Reply(_)));
    assert_eq!(said, vec![done.clone(), gone.clone()]);
    // No reply left: the task is gone.
    let (answer, said) = answered(true, None);
    assert!(matches!(answer, Answered::Gone));
    assert_eq!(said, vec![done, gone]);
}

/// External design §4, "a request whose task was deleted since", through the client and a real
/// runtime: while the task's events are kept, the repeat is a success; the grant is read back and
/// what went with the task's data is `Gone`; after `--audit` it is `not_found`.
#[test]
fn req_arch_004_a_repeat_after_the_task_was_deleted_is_applied_unless_the_audit_went() {
    let w = world();
    let id = w.task().task_id;
    let grant = Request::GrantCapability {
        task_id: id,
        kind: mimlys_core::grant::GrantKind::Tool {
            name: "write_file".into(),
        },
    };
    let rollback = Request::RollbackWorkspace {
        task_id: id,
        snapshot_id: None,
    };
    let promote = Request::PromoteWorkspace {
        task_id: id,
        plan: w.plan(id),
    };
    let sent: Vec<(RequestId, &Request)> = [&grant, &rollback, &promote]
        .into_iter()
        .map(|r| (RequestId::new(&SystemIds), r))
        .collect();
    let first: Vec<Applied<Reply>> = sent
        .iter()
        .map(|(i, r)| w.client.applied_as(*i, (*r).clone()).unwrap())
        .collect();
    w.client.cancel_task(id).unwrap();
    w.client.delete_task(id, false).unwrap();
    let events = w.persistence.events();
    let again: Vec<Applied<Reply>> = sent
        .iter()
        .map(|(i, r)| w.client.applied_as(*i, (*r).clone()).unwrap())
        .collect();
    assert_eq!(w.persistence.events(), events, "the repeats wrote");
    assert_eq!(again[0], first[0], "the grant, read back");
    assert_eq!(again[1], Applied::Gone, "the snapshot went with the data");
    assert_eq!(again[2], Applied::Gone, "the promotion went with the data");

    // With the audit, nothing of the request is known: `not_found`, as the task is.
    let other = w.task().task_id;
    let grant = Request::GrantCapability {
        task_id: other,
        kind: mimlys_core::grant::GrantKind::Tool {
            name: "write_file".into(),
        },
    };
    let sent = RequestId::new(&SystemIds);
    w.client.applied_as(sent, grant.clone()).unwrap();
    w.client.cancel_task(other).unwrap();
    w.client.delete_task(other, true).unwrap();
    let error = w.client.applied_as(sent, grant).unwrap_err();
    assert_eq!(error.kind(), "not_found", "{error}");
}

/// External design §4 (review of 014b-5): a failure keeps its kind on a repeat, through the
/// client: a promotion that failed as a `conflict` is answered as a `conflict`, with the same
/// words.
#[test]
fn req_arch_004_a_repeat_of_a_conflict_is_a_conflict_through_the_client() {
    let w = world();
    let id = w.task().task_id;
    w.workspaces.on_promote_apply(Box::new(|_| {
        Err(mimlys_core::spi::WorkspaceError::Conflict {
            paths: vec!["a.txt".into()],
        })
    }));
    let (first, second) = w.twice(Request::PromoteWorkspace {
        task_id: id,
        plan: w.plan(id),
    });
    let first = first.unwrap_err();
    assert_eq!(first.kind(), "conflict", "{first}");
    let second = second.unwrap_err();
    assert!(
        matches!(second, RuntimeError::Recorded { .. }),
        "{second:?}"
    );
    assert_eq!(second.kind(), "conflict", "{second}");
    assert_eq!(second.to_string(), first.to_string());
    // The two answers as a script reads them (`--json`), whole: the same kind, the same words and
    // the same detail, the paths of the conflict included (014b-7, item 2).
    let as_json = |error: &RuntimeError| crate::app::AppError::Runtime(error.clone()).to_json();
    assert_eq!(as_json(&second), as_json(&first));
    assert_eq!(
        as_json(&first)["error"]["detail"]["paths"],
        serde_json::json!(["a.txt"])
    );
}
