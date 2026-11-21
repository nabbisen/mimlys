//! The runtime over the real registry, in process (handoff 013d; REQ-SCHED-006, REQ-CLI-005; RFC
//! 013 decision 5 with Amendment 1). On every platform: the stop request is made as the signal's
//! handler makes it (`Runtime::start_no_new_action`), so the same tests stand for the Ctrl-C of
//! Unix and of Windows; the registry is the real one (files, locks) in a temporary directory; the
//! runtime is the core's over its fakes. A wait has begun when the registry lists the runtime
//! waiting: a state, never a time.

use mimlys::registry::{Bounds, Class, Kind, Owner, Registry, State, Take};
use mimlys_core::action::{ActionStatus, EffectKind, EffectView};
use mimlys_core::clock::{Clock, FakeClock};
use mimlys_core::event::{AdmissionState, Event};
use mimlys_core::ids::{ScriptedIds, TaskId};
use mimlys_core::observation::Notice;
use mimlys_core::path::PlatformRules;
use mimlys_core::policy::Policy;
use mimlys_core::runtime::{CreateTask, Runtime, RuntimeDeps, Session};
use mimlys_core::spi::PersistenceRead;
use mimlys_core::spi::fake::{FakePersistence, FakeToolProvider, FakeWorkspaces, ScriptedBackend};
use mimlys_core::step::StepOutcome;
use mimlys_core::task::{ScopeSpec, TaskState};
use mimlys_core::tool::{Param, ParamSchema, ParamType, Provider, ResourceProfile, ToolDefinition};
use serde_json::json;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

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

struct Rig {
    runtime: Arc<Runtime>,
    persistence: Arc<FakePersistence>,
    tools: Arc<FakeToolProvider>,
    task: TaskId,
}

fn registry_of(dir: &Path) -> Arc<Registry> {
    Registry::new(
        dir.to_owned(),
        Bounds {
            op: Duration::from_secs(10),
            poll: Duration::from_millis(10),
        },
        Arc::new(FakeClock::default()) as Arc<dyn Clock>,
    )
}

/// A runtime of process `pid` over the registry in `dir`, with one slot to share and one task
/// whose model proposes `read_file` once.
fn rig(dir: &Path, pid: u32) -> Rig {
    let persistence = Arc::new(FakePersistence::default());
    let tools = Arc::new(FakeToolProvider::new(vec![read_file()]));
    let backend = Arc::new(ScriptedBackend::with_decisions(vec![
        json!({"decision": "act", "tool": "read_file", "arguments": {"path": "a.txt"}}),
    ]));
    let workspaces = Arc::new(FakeWorkspaces::new(tools.clone()));
    let admission = mimlys::registry::slots(
        registry_of(dir),
        1,
        Owner {
            pid,
            data_dir: "/data/second".into(),
        },
    );
    let runtime = Arc::new(
        Runtime::new(RuntimeDeps {
            persistence: persistence.clone(),
            backend,
            workspaces,
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
        .unwrap()
        .with_admission(admission),
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
        tools,
        task,
    }
}

fn event_names(rig: &Rig) -> Vec<String> {
    rig.persistence
        .events()
        .into_iter()
        .filter(|e| e.task_id == Some(rig.task))
        .map(|e| e.event.name().to_owned())
        .collect()
}

/// The first runtime's slot, taken as its execution takes it.
async fn holder(dir: &Path) -> mimlys::registry::Handle {
    let class = Class {
        kind: Kind::Slot,
        key: None,
        url: None,
        limit: 1,
    };
    let owner = Owner {
        pid: 1111,
        data_dir: "/data/first".into(),
    };
    match registry_of(dir)
        .take(&class, Some("first".into()), &owner, None)
        .await
        .unwrap()
    {
        Take::Granted { handle, .. } => handle,
        Take::Busy { .. } => panic!("the registry is empty"),
    }
}

/// Until the registry lists process `pid` waiting.
async fn until_waiting(dir: &Path, pid: u32) {
    let registry = registry_of(dir);
    loop {
        let waiting = registry
            .list()
            .unwrap()
            .live
            .iter()
            .any(|s| s.entry.state == State::Waiting && s.entry.pid == pid);
        if waiting {
            return;
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
}

/// REQ-SCHED-006, REQ-CLI-005, RFC 013 Amendment 1, on every platform: a first stop request during
/// the wait for a slot over the real registry leaves the action not begun: `stopped_before_start`,
/// nothing recorded `running`, no `action.started`, no call, the task still `running` for the
/// caller to pause, and no waiting entry left in the registry. The holder's slot is untouched.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn req_sched_006_a_stop_request_ends_a_wait_over_the_real_registry() {
    let dir = tempfile::tempdir().unwrap();
    let _held = holder(dir.path()).await;
    let rig = Arc::new(rig(dir.path(), 2222));
    let stepping = {
        let rig = rig.clone();
        tokio::spawn(async move { rig.runtime.run_step(rig.task).await })
    };
    until_waiting(dir.path(), 2222).await;
    rig.runtime.start_no_new_action(rig.task);
    let result = stepping.await.unwrap().unwrap();
    assert_eq!(result.outcome, StepOutcome::Cancelled);
    assert_eq!(result.state, TaskState::Running, "the caller pauses it");
    assert!(rig.tools.calls().is_empty(), "no call reached the tool");
    let names = event_names(&rig);
    assert!(!names.contains(&"action.started".to_owned()), "{names:?}");
    assert!(
        !rig.persistence.writes().contains(&"running"),
        "nothing was recorded running"
    );
    let action = rig.persistence.load_actions(rig.task).unwrap().remove(0);
    assert_eq!(action.status, ActionStatus::Cancelled);
    let observations = rig.persistence.load_observations(rig.task).unwrap();
    assert_eq!(observations.len(), 1);
    assert_eq!(observations[0].notices, [Notice::StoppedBeforeStart]);
    let admission: Vec<AdmissionState> = rig
        .persistence
        .events()
        .into_iter()
        .filter_map(|e| match e.event {
            Event::TaskAdmissionChanged { state, .. } => Some(state),
            _ => None,
        })
        .collect();
    assert_eq!(admission, [AdmissionState::Waiting]);
    let live = registry_of(dir.path()).list().unwrap().live;
    assert_eq!(live.len(), 1, "only the holder's entry is left: {live:?}");
    assert_eq!(live[0].entry.pid, 1111);
}

/// REQ-SCHED-006, on every platform: the second runtime's execution is admitted when the first's
/// slot is returned; both changes are recorded, in order, before the action begins, and the slot
/// goes back with the execution.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn req_sched_006_a_wait_over_the_real_registry_ends_when_the_slot_is_returned() {
    let dir = tempfile::tempdir().unwrap();
    let held = holder(dir.path()).await;
    let rig = Arc::new(rig(dir.path(), 2222));
    let stepping = {
        let rig = rig.clone();
        tokio::spawn(async move { rig.runtime.run_step(rig.task).await })
    };
    until_waiting(dir.path(), 2222).await;
    assert!(rig.tools.calls().is_empty(), "nothing ran while it waited");
    drop(held);
    let result = stepping.await.unwrap().unwrap();
    assert_eq!(result.outcome, StepOutcome::Continued);
    assert_eq!(rig.tools.calls().len(), 1);
    let names = event_names(&rig);
    let first = names
        .iter()
        .position(|n| n == "task.admission_changed")
        .unwrap();
    let last = names
        .iter()
        .rposition(|n| n == "task.admission_changed")
        .unwrap();
    let started = names.iter().position(|n| n == "action.started").unwrap();
    assert!(first < last && last < started, "{names:?}");
    assert!(
        registry_of(dir.path()).list().unwrap().live.is_empty(),
        "the slot went back with the execution"
    );
}
