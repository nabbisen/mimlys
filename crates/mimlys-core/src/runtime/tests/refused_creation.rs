//! A refused creation leaves no workspace (handoff 013c, item 2; REQ-TASK-001):
//! from the materialization until the task is recorded, an error from any
//! source (the provider, the store, the clock, the scope) removes the workspace
//! that was made, after the provider is let go of (Windows keeps a directory an
//! open handle is on); after the task is recorded the workspace is its own.

use super::*;

/// What must hold after a refused creation: no task, no workspace left, and the
/// provider let go before each removal (the fake's own holder, and the harness's).
fn nothing_left(h: &Harness, what: &str) {
    assert!(h.persistence.list_tasks().unwrap().is_empty(), "{what}");
    assert_eq!(h.workspaces.materialized(), vec![], "{what}: a workspace");
    let removals = h.workspaces.removed_with_holders();
    assert!(!removals.is_empty(), "{what}: nothing was removed");
    for holders in removals {
        // `Harness::tools` and the workspaces: no handle of the creation's own.
        assert_eq!(holders, 2, "{what}: the provider was still held");
    }
}

/// REQ-TASK-001: a materialization that fails after it made the workspace (a
/// full disk) leaves no workspace.
#[test]
fn req_task_001_a_materialization_that_fails_midway_leaves_no_workspace() {
    let h = Harness::new(vec![]);
    h.workspaces.fail_materialize();
    let error = h
        .runtime
        .create_task(&h.session, create_request())
        .unwrap_err();
    assert!(matches!(error, RuntimeError::Workspace(_)), "{error:?}");
    nothing_left(&h, "materialize");
}

/// REQ-TASK-001: a provider that cannot be opened, and one that panics (at its
/// definitions, its imports or its events: contained, an error to the caller),
/// leave no workspace.
#[test]
fn req_task_001_a_provider_that_fails_leaves_no_workspace() {
    let h = Harness::new(vec![]);
    h.workspaces.fail_tools();
    assert!(h.runtime.create_task(&h.session, create_request()).is_err());
    nothing_left(&h, "tools");

    for method in ["definitions", "imports", "drain_events"] {
        let h = Harness::new(vec![]);
        h.tools.panic_in(method, "provider exploded");
        assert!(
            h.runtime.create_task(&h.session, create_request()).is_err(),
            "{method}"
        );
        nothing_left(&h, method);
    }
}

/// REQ-TASK-001: a store that refuses the task (a full disk) leaves no workspace.
#[test]
fn req_task_001_a_store_that_refuses_the_task_leaves_no_workspace() {
    let h = Harness::new(vec![]);
    h.persistence.fail_write("create_task");
    let error = h
        .runtime
        .create_task(&h.session, create_request())
        .unwrap_err();
    assert!(matches!(error, RuntimeError::Persistence(_)), "{error:?}");
    nothing_left(&h, "create_task");
}

/// REQ-TASK-001: a clock that cannot give the task a deadline refuses the
/// request after the workspace was made, and leaves no workspace.
#[test]
fn req_task_001_a_clock_past_its_range_leaves_no_workspace() {
    let h = Harness::new(vec![]);
    let last = jiff::Timestamp::MAX.as_second() * 1000 - 1000;
    h.clock
        .set(crate::clock::Instant::from_unix_millis(last).unwrap());
    let error = h
        .runtime
        .create_task(&h.session, create_request())
        .unwrap_err();
    assert!(
        matches!(error, RuntimeError::InvalidRequest { .. }),
        "{error:?}"
    );
    nothing_left(&h, "clock");
}

/// REQ-TASK-001: a fork refused after its workspace was made leaves no
/// workspace, and the source's is untouched.
#[test]
fn req_task_001_a_refused_fork_leaves_no_workspace_and_the_sources_stays() {
    let h = Harness::new(vec![]);
    let source = h.create_with(|r| r.hold = true);
    h.runtime.cancel_task(&h.session, source).unwrap();
    assert_eq!(h.workspaces.materialized(), vec![source]);
    h.tools.panic_in("definitions", "provider exploded");
    let mut fork = create_request();
    fork.from_task = Some(source);
    assert!(h.runtime.create_task(&h.session, fork).is_err());
    assert_eq!(h.workspaces.materialized(), vec![source]);
    assert_eq!(h.persistence.list_tasks().unwrap().len(), 1);
    for holders in h.workspaces.removed_with_holders() {
        assert_eq!(holders, 2, "the provider was still held");
    }
    assert!(!h.workspaces.removed_with_holders().is_empty());
}

/// REQ-TASK-001: until the task is recorded, and no longer: a failure after the
/// task's row is written leaves the task, and the workspace is its own.
#[test]
fn req_task_001_after_the_task_is_recorded_the_workspace_is_its_own() {
    let h = Harness::new(vec![]);
    h.persistence.fail_write("workspace_state");
    assert!(h.runtime.create_task(&h.session, create_request()).is_err());
    let tasks = h.persistence.list_tasks().unwrap();
    assert_eq!(tasks.len(), 1, "the task is recorded");
    assert_eq!(h.workspaces.materialized(), vec![tasks[0].task_id]);
    assert!(h.workspaces.removed_with_holders().is_empty());
}

/// REQ-TASK-001: a refusal for the limit on the definitions (009i) is one of the
/// same, through the guard: no workspace, no task.
#[test]
fn req_task_001_a_profile_over_lim_i1_leaves_no_workspace() {
    let mut policy = Policy::shipped();
    let bigs: Vec<ToolDefinition> = ["big_a", "big_b", "big_c"]
        .iter()
        .map(|name| ToolDefinition {
            description: "d".repeat(3000),
            ..tool(name, EffectKind::WorkspaceRead)
        })
        .collect();
    for t in &bigs {
        policy.ceiling.tools.push(t.name.clone());
    }
    let h = Harness::with_backend_and_tools(
        Arc::new(ScriptedBackend::with_decisions(vec![])),
        policy,
        vec![],
        crate::limits::LIM_C1_CONTEXT_CHARS,
        bigs,
    );
    let mut request = create_request();
    request.grants = ["big_a", "big_b", "big_c"]
        .iter()
        .map(|name| GrantKind::Tool {
            name: (*name).to_owned(),
        })
        .collect();
    let error = h.runtime.create_task(&h.session, request).unwrap_err();
    assert!(matches!(error, RuntimeError::Denied { .. }), "{error:?}");
    nothing_left(&h, "definitions");
}
