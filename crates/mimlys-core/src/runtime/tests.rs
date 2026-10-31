//! Runtime tests over the fakes (REQ-TEST-002): the loop's ordered lines,
//! the control verbs, approvals, questions, budgets and the boundary.

use super::*;
use crate::action::ActionStatus;
use crate::action::{EffectKind, EffectView};
use crate::approval::ApprovalStatus;
use crate::clock::FakeClock;
use crate::grant::GrantKind;
use crate::ids::{ScriptedIds, TaskId};
use crate::limits::LIM_T5_DEADLINE_MS;
use crate::path::PlatformRules;
use crate::policy::{OnReject, Policy};
use crate::spi::fake::{FakePersistence, FakeToolProvider, FakeWorkspaces, ScriptedBackend};
use crate::spi::{BackendError, ExecutionOutcome};
use crate::step::{Budgets, StepOutcome};
use crate::task::{CompletionCheck, Extension, HaltReason, ScopeSpec, TaskState, WaitKind};
use crate::tool::{Param, ParamSchema, ParamType, Provider, ResourceProfile, ToolDefinition};
use serde_json::{Value, json};
use std::sync::Arc;

fn tool(name: &str, kind: EffectKind) -> ToolDefinition {
    ToolDefinition {
        name: name.into(),
        description: format!("{name}."),
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
            kind,
            reversible: true,
            idempotent: true,
        },
        deadline_ms: 30_000,
        profile: ResourceProfile::Builtin,
        destination_arguments: vec![],
        provider: Provider::Builtin,
    }
}

struct Harness {
    runtime: Runtime,
    persistence: Arc<FakePersistence>,
    backend: Arc<ScriptedBackend>,
    tools: Arc<FakeToolProvider>,
    workspaces: Arc<FakeWorkspaces>,
    clock: Arc<FakeClock>,
    session: Session,
}

impl Harness {
    fn new(decisions: Vec<Value>) -> Self {
        Self::with_policy(decisions, Policy::shipped())
    }

    fn with_policy(decisions: Vec<Value>, policy: Policy) -> Self {
        Self::with_context(
            decisions,
            policy,
            vec![],
            crate::limits::LIM_C1_CONTEXT_CHARS,
        )
    }

    fn with_context(
        decisions: Vec<Value>,
        policy: Policy,
        secrets: Vec<(String, String)>,
        context_chars: usize,
    ) -> Self {
        Self::with_backend(
            Arc::new(ScriptedBackend::with_decisions(decisions)),
            policy,
            secrets,
            context_chars,
        )
    }

    fn with_backend(
        backend: Arc<ScriptedBackend>,
        policy: Policy,
        secrets: Vec<(String, String)>,
        context_chars: usize,
    ) -> Self {
        let persistence = Arc::new(FakePersistence::default());
        let tools = Arc::new(FakeToolProvider::new(vec![
            tool("read_file", EffectKind::WorkspaceRead),
            tool("write_file", EffectKind::WorkspaceWrite),
            tool("list_dir", EffectKind::WorkspaceRead),
            tool("search_files", EffectKind::WorkspaceRead),
            tool("replace_in_file", EffectKind::WorkspaceWrite),
            tool("delete_path", EffectKind::WorkspaceWrite),
        ]));
        let clock = Arc::new(FakeClock::default());
        let workspaces = Arc::new(FakeWorkspaces::new(tools.clone()));
        let runtime = Runtime::new(RuntimeDeps {
            persistence: persistence.clone(),
            backend: backend.clone(),
            workspaces: workspaces.clone(),
            policy,
            ids: Arc::new(ScriptedIds::default()),
            clock: clock.clone(),
            runtime_version: "0.1.0-test".into(),
            rules: PlatformRules::Posix,
            probe: crate::confine::Probe::unavailable("no backend in tests"),
            secrets,
            context_chars,
        })
        .unwrap();
        Self {
            runtime,
            persistence,
            backend,
            tools,
            workspaces,
            clock,
            session: Session {
                id: "s1".into(),
                os_user: "tester".into(),
            },
        }
    }

    fn create(&self) -> TaskId {
        self.create_with(|_| {})
    }

    fn create_with(&self, edit: impl FnOnce(&mut CreateTask)) -> TaskId {
        let mut request = CreateTask {
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
            hold: false,
            from_task: None,
            from_snapshot: None,
        };
        edit(&mut request);
        self.runtime
            .create_task(&self.session, request)
            .unwrap()
            .task_id
    }

    fn step(&self, task_id: TaskId) -> StepResultView {
        futures_block(self.runtime.run_step(task_id)).unwrap()
    }

    fn state(&self, task_id: TaskId) -> TaskState {
        self.runtime.get_task(task_id).unwrap().task.state
    }

    fn event_names(&self, task_id: TaskId) -> Vec<String> {
        self.persistence
            .events()
            .into_iter()
            .filter(|e| e.task_id == Some(task_id))
            .map(|e| e.event.name().to_owned())
            .collect()
    }
}

fn act(tool: &str, path: &str) -> Value {
    json!({"decision": "act", "tool": tool, "arguments": {"path": path}})
}

/// A minimal executor for the fakes' futures: they never pend.
fn futures_block<F: std::future::Future>(future: F) -> F::Output {
    use std::task::{Context, Poll, Waker};
    let mut cx = Context::from_waker(Waker::noop());
    let mut future = std::pin::pin!(future);
    loop {
        if let Poll::Ready(v) = future.as_mut().poll(&mut cx) {
            return v;
        }
    }
}

/// REQ-TASK-001 / REQ-CAP-003: a created task starts `running` with one
/// `tool` grant per profile tool, and records creation and materialization.
#[test]
fn req_task_001_create_starts_running_with_profile_grants() {
    let h = Harness::new(vec![]);
    let id = h.create();
    let view = h.runtime.get_task(id).unwrap();
    assert_eq!(view.task.state, TaskState::Running);
    let mut names: Vec<String> = view
        .grants
        .iter()
        .filter_map(|g| match g.kind() {
            GrantKind::Tool { name } => Some(name.clone()),
            _ => None,
        })
        .collect();
    names.sort();
    let mut expected = Policy::shipped()
        .profile("workspace")
        .unwrap()
        .tools
        .clone();
    expected.sort();
    assert_eq!(names, expected, "one tool grant per profile tool");
    assert_eq!(
        h.event_names(id),
        [
            "task.created",
            "workspace.materialized",
            "workspace.snapshot",
            "task.state_changed"
        ],
        "REQ-WS-004: the base snapshot is taken at materialization"
    );
    assert_eq!(view.task.versions.tool_definition_hashes.len(), 6);
}

/// REQ-PERS-004 / external design §16.2: an authorized action writes the
/// eight lines in order, and the executor sees exactly one call.
#[test]
fn req_pers_004_action_lines_in_order() {
    let h = Harness::new(vec![act("read_file", "src/lib.rs")]);
    h.tools.script(
        "read_file",
        ExecutionOutcome::succeeded(json!({"content": "x"}), 3),
    );
    let id = h.create();
    let result = h.step(id);
    assert_eq!(result.outcome, StepOutcome::Continued);
    assert_eq!(result.state, TaskState::Running);
    let writes = h.persistence.writes();
    let tail = &writes[writes.len() - 8..];
    assert_eq!(
        tail,
        [
            "step_started",
            "proposed",
            "validation",
            "authorized",
            "running",
            "execution",
            "action_terminal",
            "step_outcome"
        ]
    );
    assert_eq!(h.tools.calls().len(), 1);
    let actions = h.persistence.load_actions(id).unwrap();
    assert_eq!(actions.len(), 1);
    assert_eq!(actions[0].status, ActionStatus::Succeeded);
    let names = h.event_names(id);
    assert!(names.contains(&"decision.received".to_owned()));
    assert_eq!(
        names.iter().filter(|n| *n == "decision.received").count(),
        1
    );
}

/// REQ-MDL-008: the backend is called once per step and only for the decision.
#[test]
fn req_mdl_008_one_backend_call_per_step() {
    let h = Harness::new(vec![act("read_file", "a"), act("read_file", "b")]);
    h.tools
        .script("read_file", ExecutionOutcome::succeeded(json!({}), 1));
    h.tools
        .script("read_file", ExecutionOutcome::succeeded(json!({}), 1));
    let id = h.create();
    h.step(id);
    h.step(id);
    assert_eq!(h.backend.requests().len(), 2);
    assert_eq!(h.backend.requests()[0].schema_version, 1);
}

/// REQ-DEC-009 / REQ-TASK-010: a rejection becomes the next request's
/// feedback segment and three in a row halt with `budget_rejections`.
#[test]
fn req_dec_009_rejections_feed_back_then_halt() {
    let h = Harness::new(vec![
        json!({"decision": "act", "tool": "launch_missiles", "arguments": {}}),
        json!("not an object"),
        json!({"decision": "act", "tool": "read_file", "arguments": {"path": "../etc/passwd"}}),
    ]);
    let id = h.create();
    assert_eq!(h.step(id).outcome, StepOutcome::Rejected);
    assert_eq!(h.step(id).outcome, StepOutcome::Rejected);
    let last = h.backend.requests()[1]
        .messages
        .last()
        .unwrap()
        .content
        .clone();
    let feedback = h.backend.requests()[1]
        .messages
        .iter()
        .any(|m| m.content.contains("<<feedback") && m.content.contains("unknown_tool"));
    assert!(
        feedback,
        "the rejection must reach the model as a feedback segment; last: {last}"
    );
    let third = h.step(id);
    assert_eq!(third.outcome, StepOutcome::Halted);
    assert_eq!(
        third.state,
        TaskState::Halted {
            halt_reason: HaltReason::BudgetRejections
        }
    );
    assert_eq!(h.backend.requests().len(), 3);
}

/// REQ-CAP-001: a tool without a grant is invisible, so the decision is
/// rejected `unknown_tool` and nothing runs.
#[test]
fn req_cap_001_ungranted_tool_is_unknown() {
    let h = Harness::new(vec![act("write_file", "a")]);
    let id = h.create_with(|r| r.profile = Some("read_only".into()));
    let result = h.step(id);
    assert_eq!(result.outcome, StepOutcome::Rejected);
    let steps = h.persistence.load_steps(id).unwrap();
    assert_eq!(
        steps[0].rejection,
        Some(crate::rejection::RejectionCode::UnknownTool)
    );
    assert!(h.tools.calls().is_empty());
    let defs = &h.backend.requests()[0].messages[0].content;
    assert!(
        !defs.contains("write_file"),
        "an ungranted tool is not even described"
    );
}

fn approval_policy() -> Policy {
    let mut policy = Policy::shipped();
    policy.profiles.get_mut("workspace").unwrap().approval_tools = vec!["write_file".into()];
    policy
}

/// REQ-APR-001..009: an approval-listed tool waits; approval lets the same
/// step continue from line 4 with the approval consumed in that write.
#[test]
fn req_apr_approval_round_trip() {
    let h = Harness::with_policy(vec![act("write_file", "src/lib.rs")], approval_policy());
    h.tools.script(
        "write_file",
        ExecutionOutcome::succeeded(json!({"bytes": 3}), 2),
    );
    let id = h.create();
    let wait = h.step(id);
    assert_eq!(wait.outcome, StepOutcome::Waited);
    assert_eq!(
        wait.state,
        TaskState::WaitingUser {
            wait_kind: WaitKind::Approval
        }
    );
    let view = h.runtime.get_task(id).unwrap();
    let approval = view.pending_approval.expect("pending approval");
    assert_eq!(approval.status, ApprovalStatus::Pending);
    assert!(approval.display_text().contains("write_file"));
    assert!(
        h.tools.calls().is_empty(),
        "nothing runs before the user decides"
    );
    assert!(
        h.runtime.run_step(id).now_or_never_err(),
        "a waiting task has no step"
    );
    let task = h.runtime.approve_action(&h.session, id).unwrap();
    assert_eq!(task.state, TaskState::Running);
    let cont = h.step(id);
    assert_eq!(cont.outcome, StepOutcome::Continued);
    let writes = h.persistence.writes();
    let tail = &writes[writes.len() - 5..];
    assert_eq!(
        tail,
        [
            "authorized",
            "running",
            "execution",
            "action_terminal",
            "step_outcome"
        ]
    );
    assert_eq!(h.tools.calls().len(), 1);
    let approvals = h.persistence.load_approvals(id).unwrap();
    assert_eq!(approvals[0].status, ApprovalStatus::Consumed);
    assert_eq!(
        h.persistence.load_steps(id).unwrap().len(),
        1,
        "the approved action completes its own step"
    );
    assert_eq!(
        h.backend.requests().len(),
        1,
        "no second decision is requested for an approved action"
    );
}

/// REQ-APR-007: a rejection with `halt` halts `approval_rejected`; without
/// it the action is rejected and the task continues.
#[test]
fn req_apr_007_reject_continue_or_halt() {
    let h = Harness::with_policy(
        vec![act("write_file", "a"), act("write_file", "b")],
        approval_policy(),
    );
    let id = h.create();
    h.step(id);
    let task = h
        .runtime
        .reject_action(&h.session, id, Some("not that file".into()), None)
        .unwrap();
    assert_eq!(task.state, TaskState::Running);
    let actions = h.persistence.load_actions(id).unwrap();
    assert!(matches!(actions[0].status, ActionStatus::Rejected { .. }));
    assert_eq!(
        h.persistence.load_steps(id).unwrap()[0].outcome,
        Some(StepOutcome::Rejected)
    );
    h.step(id);
    let task = h
        .runtime
        .reject_action(&h.session, id, None, Some(true))
        .unwrap();
    assert_eq!(
        task.state,
        TaskState::Halted {
            halt_reason: HaltReason::ApprovalRejected
        }
    );
    assert!(h.tools.calls().is_empty());
    assert!(
        h.backend.requests()[1]
            .messages
            .iter()
            .any(|m| m.content.contains("not that file")),
        "the note reaches the model as user text"
    );
}

/// REQ-APR-009 / REQ-CAP-005: revoking a grant invalidates the pending approval.
#[test]
fn req_apr_009_revocation_invalidates_pending_approval() {
    let h = Harness::with_policy(vec![act("write_file", "a")], approval_policy());
    let id = h.create();
    h.step(id);
    let grants = h.runtime.get_task(id).unwrap().grants;
    let write = grants
        .iter()
        .find(|g| matches!(g.kind(), GrantKind::Tool { name } if name == "write_file"))
        .unwrap();
    h.runtime
        .revoke_capability(&h.session, id, write.grant_id)
        .unwrap();
    assert_eq!(h.state(id), TaskState::Running);
    let approvals = h.persistence.load_approvals(id).unwrap();
    assert!(matches!(approvals[0].status, ApprovalStatus::Invalidated));
    assert!(h.runtime.approve_action(&h.session, id).is_err());
    assert!(h.tools.calls().is_empty());
}

/// REQ-APR-009: an approval given, then a grant revoked before the step
/// continues, is not honoured.
#[test]
fn req_apr_009_approved_then_revoked_does_not_run() {
    let h = Harness::with_policy(vec![act("write_file", "a")], approval_policy());
    let id = h.create();
    h.step(id);
    h.runtime.approve_action(&h.session, id).unwrap();
    let grants = h.runtime.get_task(id).unwrap().grants;
    let write = grants
        .iter()
        .find(|g| matches!(g.kind(), GrantKind::Tool { name } if name == "write_file"))
        .unwrap();
    h.runtime
        .revoke_capability(&h.session, id, write.grant_id)
        .unwrap();
    let actions = h.persistence.load_actions(id).unwrap();
    assert!(
        matches!(actions[0].status, ActionStatus::Rejected { .. }),
        "{:?}",
        actions[0].status
    );
    assert_eq!(
        h.persistence.load_steps(id).unwrap()[0].outcome,
        Some(StepOutcome::Rejected)
    );
    assert_eq!(h.state(id), TaskState::Running);
    h.backend.push(Ok(serde_json::to_vec(
        &json!({"decision": "complete", "summary": "x"}),
    )
    .unwrap()));
    assert_eq!(h.step(id).outcome, StepOutcome::Completed);
    assert!(h.tools.calls().is_empty());
}

/// REQ-TASK-006: cancel from `waiting_user` cancels the action and
/// invalidates the approval.
#[test]
fn req_task_006_cancel_while_waiting() {
    let h = Harness::with_policy(vec![act("write_file", "a")], approval_policy());
    let id = h.create();
    h.step(id);
    let task = h.runtime.cancel_task(&h.session, id).unwrap();
    assert_eq!(task.state, TaskState::Cancelled);
    assert_eq!(
        h.persistence.load_actions(id).unwrap()[0].status,
        ActionStatus::Cancelled
    );
    assert!(matches!(
        h.persistence.load_approvals(id).unwrap()[0].status,
        ApprovalStatus::Invalidated
    ));
    assert!(
        h.runtime.cancel_task(&h.session, id).is_err(),
        "terminal states accept nothing"
    );
}

/// REQ-APR-010: `ask_user` waits for a question; the answer reaches the next request.
#[test]
fn req_apr_010_question_round_trip() {
    let h = Harness::new(vec![
        json!({"decision": "ask_user", "question": "Which branch?"}),
        json!({"decision": "complete", "summary": "done"}),
    ]);
    let id = h.create();
    let wait = h.step(id);
    assert_eq!(
        wait.state,
        TaskState::WaitingUser {
            wait_kind: WaitKind::Question
        }
    );
    assert!(h.runtime.get_task(id).unwrap().pending_question.is_some());
    assert!(
        h.runtime.approve_action(&h.session, id).is_err(),
        "a question is not an approval"
    );
    let task = h.runtime.answer_question(&h.session, id, "main").unwrap();
    assert_eq!(task.state, TaskState::Running);
    h.step(id);
    let request = &h.backend.requests()[1];
    assert!(
        request
            .messages
            .iter()
            .any(|m| m.content.contains("Which branch?"))
    );
    assert!(
        request
            .messages
            .iter()
            .any(|m| m.content.contains("<<user kind=\"answer\"") && m.content.contains("main"))
    );
}

/// REQ-TASK-007 / external design §5.3: completion is `asserted` without a
/// check and `verified` when the check's action succeeds.
#[test]
fn req_task_007_completion_rule() {
    let h = Harness::new(vec![json!({"decision": "complete", "summary": "done"})]);
    let id = h.create();
    let result = h.step(id);
    assert_eq!(result.outcome, StepOutcome::Completed);
    assert_eq!(
        result.state,
        TaskState::Completed {
            completion: crate::task::Completion::Asserted
        }
    );

    let h = Harness::new(vec![
        json!({"decision": "complete", "summary": "done"}),
        json!({"decision": "complete", "summary": "done"}),
    ]);
    h.tools.script(
        "read_file",
        ExecutionOutcome::tool_error("missing", json!({}), 1),
    );
    h.tools
        .script("read_file", ExecutionOutcome::succeeded(json!({}), 1));
    let id = h.create_with(|r| {
        r.completion_check = Some(CompletionCheck {
            tool: "read_file".into(),
            arguments: json!({"path": "tests/ok"}).as_object().unwrap().clone(),
        })
    });
    let first = h.step(id);
    assert_eq!(
        first.outcome,
        StepOutcome::Continued,
        "a failed check continues the task: {:?}",
        first.state
    );
    assert_eq!(first.state, TaskState::Running);
    let second = h.step(id);
    assert_eq!(
        second.state,
        TaskState::Completed {
            completion: crate::task::Completion::Verified
        }
    );
    assert_eq!(h.tools.calls().len(), 2);
    assert_eq!(
        h.backend.requests().len(),
        2,
        "the check never asks the model"
    );
}

/// REQ-TASK-011: `give_up` fails the task with the model's reason.
#[test]
fn req_task_011_give_up_fails() {
    let h = Harness::new(vec![
        json!({"decision": "give_up", "reason": "no such test"}),
    ]);
    let id = h.create();
    let result = h.step(id);
    assert_eq!(result.outcome, StepOutcome::Failed);
    assert!(matches!(result.state, TaskState::Failed { .. }));
}

/// REQ-TASK-005: a pause takes effect at the next boundary; resume continues.
#[test]
fn req_task_005_pause_at_boundary_and_resume() {
    let h = Harness::new(vec![act("read_file", "a")]);
    h.tools
        .script("read_file", ExecutionOutcome::succeeded(json!({}), 1));
    let id = h.create();
    let task = h.runtime.pause_task(&h.session, id).unwrap();
    assert_eq!(task.state, TaskState::Running);
    assert!(task.pause_requested);
    let result = h.step(id);
    assert_eq!(result.outcome, StepOutcome::Paused);
    assert_eq!(result.state, TaskState::Paused);
    assert!(
        h.backend.requests().is_empty(),
        "no decision is requested at a pause"
    );
    let task = h.runtime.resume_task(&h.session, id, None).unwrap();
    assert_eq!(task.state, TaskState::Running);
    assert!(!task.pause_requested);
    assert_eq!(h.step(id).outcome, StepOutcome::Continued);
}

/// REQ-TASK-009: the deadline halts at the boundary; an extension resumes.
#[test]
fn req_task_009_deadline_halts_and_extension_resumes() {
    let h = Harness::new(vec![act("read_file", "a")]);
    h.tools
        .script("read_file", ExecutionOutcome::succeeded(json!({}), 1));
    let id = h.create();
    h.clock.advance_millis(LIM_T5_DEADLINE_MS + 1);
    let result = h.step(id);
    assert_eq!(
        result.state,
        TaskState::Halted {
            halt_reason: HaltReason::BudgetDeadline
        }
    );
    assert!(
        h.runtime.resume_task(&h.session, id, None).is_err(),
        "a budget halt needs an extension"
    );
    let later = h.clock.now().plus_millis(60_000).unwrap();
    let task = h
        .runtime
        .resume_task(
            &h.session,
            id,
            Some(Extension {
                steps: None,
                deadline: Some(later),
            }),
        )
        .unwrap();
    assert_eq!(task.state, TaskState::Running);
    assert_eq!(h.step(id).outcome, StepOutcome::Continued);
}

/// REQ-TASK-010: the step budget halts `budget_steps`.
#[test]
fn req_task_010_step_budget_halts() {
    let h = Harness::new((0..3).map(|i| act("read_file", &format!("f{i}"))).collect());
    for _ in 0..3 {
        h.tools
            .script("read_file", ExecutionOutcome::succeeded(json!({}), 1));
    }
    let id = h.create_with(|r| {
        r.budgets = Some(Budgets {
            steps: 2,
            ..Budgets::default()
        })
    });
    assert_eq!(h.step(id).outcome, StepOutcome::Continued);
    let second = h.step(id);
    assert_eq!(second.outcome, StepOutcome::Halted);
    assert_eq!(
        second.state,
        TaskState::Halted {
            halt_reason: HaltReason::BudgetSteps
        }
    );
}

/// REQ-MDL-003: an unreachable backend is retried LIM-M3 times with the
/// same request, then the task halts `backend_unavailable`.
#[test]
fn req_mdl_003_backend_retries_then_halts() {
    let h = Harness::new(vec![]);
    for _ in 0..3 {
        h.backend.push(Err(BackendError::Unavailable {
            detail: "connection refused".into(),
        }));
    }
    let id = h.create();
    let result = h.step(id);
    assert_eq!(result.outcome, StepOutcome::Halted);
    assert_eq!(
        result.state,
        TaskState::Halted {
            halt_reason: HaltReason::BackendUnavailable
        }
    );
    let requests = h.backend.requests();
    assert_eq!(requests.len(), 3);
    assert_eq!(
        requests[0], requests[2],
        "retries send an identical request"
    );
}

/// REQ-TOOL-009: a timeout of an idempotent action is retried once within
/// LIM-T3; a tool error is fed back without a retry.
#[test]
fn req_tool_009_retry_and_feedback() {
    let h = Harness::new(vec![act("read_file", "a"), act("read_file", "b")]);
    h.tools
        .script("read_file", ExecutionOutcome::timed_out(30_000));
    h.tools
        .script("read_file", ExecutionOutcome::succeeded(json!({}), 1));
    h.tools.script(
        "read_file",
        ExecutionOutcome::tool_error("not_found", json!({}), 1),
    );
    let id = h.create();
    let first = h.step(id);
    assert_eq!(first.outcome, StepOutcome::Continued);
    let actions = h.persistence.load_actions(id).unwrap();
    assert_eq!(actions[0].status, ActionStatus::Succeeded);
    assert_eq!(actions[0].executions, 2);
    assert_eq!(
        h.persistence
            .load_executions(actions[0].action_id)
            .unwrap()
            .len(),
        2
    );
    let second = h.step(id);
    assert_eq!(second.outcome, StepOutcome::Continued);
    let actions = h.persistence.load_actions(id).unwrap();
    assert!(matches!(actions[1].status, ActionStatus::Failed { .. }));
    assert_eq!(actions[1].executions, 1);
    assert_eq!(h.tools.calls().len(), 3);
}

/// REQ-TASK-012: one active task per overlapping scope.
#[test]
fn req_task_012_scope_overlap_conflict() {
    let h = Harness::new(vec![]);
    let _first = h.create();
    let err = h
        .runtime
        .create_task(
            &h.session,
            CreateTask {
                goal: "another".into(),
                scope: ScopeSpec {
                    root: "/home/u/project/sub".into(),
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
        .unwrap_err();
    assert_eq!(err.kind(), "conflict");
    // REQ-SCHED-001: with one task running, another scope's task is held in `created` (LIM-X1).
    let held = h.create_with(|r| {
        r.scope.root = "/home/u/other".into();
        r.hold = true;
    });
    assert_eq!(h.state(held), TaskState::Created);
    let err = h.runtime.resume_task(&h.session, held, None).unwrap_err();
    assert_eq!(
        err.kind(),
        "conflict",
        "LIM-X1 refuses a second running task"
    );
    h.runtime.cancel_task(&h.session, _first).unwrap();
    assert_eq!(
        h.runtime.resume_task(&h.session, held, None).unwrap().state,
        TaskState::Running
    );
}

/// REQ-TASK-006 / REQ-SCHED-004: a cancellation request reaches the step in
/// flight: the execution ends `cancelled`, the step ends `cancelled`, and
/// the task is then cancelled by the control verb.
#[test]
fn req_sched_004_cancellation_reaches_the_step() {
    let h = Harness::new(vec![act("read_file", "a"), act("read_file", "b")]);
    h.tools.script("read_file", ExecutionOutcome::cancelled(1));
    let id = h.create();
    h.runtime.request_cancel(id);
    let result = h.step(id);
    assert_eq!(
        result.outcome,
        StepOutcome::Cancelled,
        "a request before the boundary ends the step at once"
    );
    assert_eq!(result.state, TaskState::Running);
    assert!(h.backend.requests().is_empty());
    let task = h.runtime.cancel_task(&h.session, id).unwrap();
    assert_eq!(task.state, TaskState::Cancelled);

    // A request during the execution: the fake tool reports `cancelled`.
    let h = Harness::new(vec![act("read_file", "a")]);
    h.tools.script("read_file", ExecutionOutcome::cancelled(1));
    let id = h.create();
    let result = h.step(id);
    assert_eq!(result.outcome, StepOutcome::Cancelled);
    let actions = h.persistence.load_actions(id).unwrap();
    assert_eq!(actions[0].status, ActionStatus::Cancelled);
    assert_eq!(
        h.persistence.load_steps(id).unwrap()[0].outcome,
        Some(StepOutcome::Cancelled)
    );
}

/// REQ-TASK-001: invalid requests are refused before anything is written.
#[test]
fn req_task_001_invalid_requests() {
    let h = Harness::new(vec![]);
    let base = || CreateTask {
        goal: "g".into(),
        scope: ScopeSpec {
            root: "/p".into(),
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
    };
    let mut empty = base();
    empty.goal = String::new();
    assert_eq!(
        h.runtime.create_task(&h.session, empty).unwrap_err().kind(),
        "invalid_request"
    );
    let mut profile = base();
    profile.profile = Some("nope".into());
    assert_eq!(
        h.runtime
            .create_task(&h.session, profile)
            .unwrap_err()
            .kind(),
        "invalid_request"
    );
    let mut network = base();
    network.grants = vec![GrantKind::Network {
        domains: vec!["example.com".into()],
        ports: vec![443],
    }];
    assert_eq!(
        h.runtime
            .create_task(&h.session, network)
            .unwrap_err()
            .kind(),
        "denied",
        "outside the ceiling"
    );
    assert!(h.persistence.writes().is_empty());
    let held = h.runtime.create_task(&h.session, base()).unwrap();
    assert_eq!(held.state, TaskState::Created);
}

/// REQ-OBS-002 / REQ-OBS-003 / REQ-OBS-006: events, metrics and status read back.
#[test]
fn req_obs_events_metrics_status() {
    let h = Harness::with_policy(vec![act("write_file", "a")], approval_policy());
    let id = h.create();
    h.step(id);
    let events = h.runtime.get_events(Some(id), 0, 100).unwrap();
    assert!(events.windows(2).all(|w| w[0].position < w[1].position));
    let after = h
        .runtime
        .get_events(Some(id), events[2].position, 100)
        .unwrap();
    assert_eq!(after.len(), events.len() - 3);
    let metrics = h.runtime.get_metrics(None).unwrap();
    assert_eq!(metrics.tasks_by_state.get("waiting_user"), Some(&1));
    let status = h.runtime.get_runtime_status().unwrap();
    assert_eq!(status.pending, vec![(id, WaitKind::Approval)]);
    assert!(status.running.is_empty());
    assert_eq!(status.tier, "core");
}

/// REQ-CAP-004: a grant issued at run time is visible at the next boundary.
#[test]
fn req_cap_004_grant_visible_next_step() {
    let h = Harness::new(vec![act("write_file", "a"), act("write_file", "a")]);
    h.tools
        .script("write_file", ExecutionOutcome::succeeded(json!({}), 1));
    let id = h.create_with(|r| r.profile = Some("read_only".into()));
    assert_eq!(h.step(id).outcome, StepOutcome::Rejected);
    h.runtime
        .grant_capability(
            &h.session,
            id,
            GrantKind::Tool {
                name: "write_file".into(),
            },
        )
        .unwrap();
    assert_eq!(h.step(id).outcome, StepOutcome::Continued);
    assert_eq!(h.tools.calls().len(), 1);
}

/// REQ-APR-007: the profile's `on_reject` decides the default.
#[test]
fn req_apr_007_on_reject_default_from_profile() {
    let mut policy = approval_policy();
    policy.profiles.get_mut("workspace").unwrap().on_reject = OnReject::Halt;
    let h = Harness::with_policy(vec![act("write_file", "a")], policy);
    let id = h.create();
    h.step(id);
    let task = h.runtime.reject_action(&h.session, id, None, None).unwrap();
    assert!(matches!(task.state, TaskState::Halted { .. }));
}

trait NowOrNever {
    fn now_or_never_err(self) -> bool;
}

impl<F: std::future::Future<Output = Result<StepResultView, RuntimeError>>> NowOrNever for F {
    fn now_or_never_err(self) -> bool {
        futures_block(self).is_err()
    }
}

/// REQ-APR-009 / INV-06: a grant issued while an approval is pending
/// withdraws the request and asks again under the new grant set; revoking
/// a grant the action does not use does the same; the task keeps waiting.
#[test]
fn req_apr_009_grant_change_rebinds_pending_approval() {
    let h = Harness::with_policy(vec![act("write_file", "a")], approval_policy());
    h.tools
        .script("write_file", ExecutionOutcome::succeeded(json!({}), 1));
    let id = h.create();
    h.step(id);
    let first = h.runtime.get_task(id).unwrap().pending_approval.unwrap();
    h.runtime
        .grant_capability(
            &h.session,
            id,
            GrantKind::Tool {
                name: "read_file".into(),
            },
        )
        .unwrap();
    let view = h.runtime.get_task(id).unwrap();
    assert_eq!(
        view.task.state,
        TaskState::WaitingUser {
            wait_kind: WaitKind::Approval
        }
    );
    let second = view.pending_approval.unwrap();
    assert_ne!(second.approval_id, first.approval_id);
    assert_eq!(second.action_id, first.action_id);
    assert_ne!(second.grant_set_hash, first.grant_set_hash);
    let approvals = h.persistence.load_approvals(id).unwrap();
    assert_eq!(approvals[0].status, ApprovalStatus::Invalidated);
    assert_eq!(approvals[1].status, ApprovalStatus::Pending);
    // An unrelated grant revoked: withdrawn and asked again.
    let unrelated = view
        .grants
        .iter()
        .find(|g| matches!(g.kind(), GrantKind::Tool { name } if name == "delete_path"))
        .unwrap()
        .grant_id;
    h.runtime
        .revoke_capability(&h.session, id, unrelated)
        .unwrap();
    let third = h.runtime.get_task(id).unwrap().pending_approval.unwrap();
    assert_ne!(third.approval_id, second.approval_id);
    // Approving the current request runs the action once.
    h.runtime.approve_action(&h.session, id).unwrap();
    assert_eq!(h.step(id).outcome, StepOutcome::Continued);
    assert_eq!(h.tools.calls().len(), 1);
    assert_eq!(
        h.persistence.load_approvals(id).unwrap()[2].status,
        ApprovalStatus::Consumed
    );
    assert_eq!(h.persistence.load_steps(id).unwrap().len(), 1);
}

/// REQ-WS-004 / REQ-TOOL-010: a write action takes a snapshot before and,
/// when it succeeds, after; a failed write is restored before its
/// observation, which says `rolled_back`; a read action takes none.
#[test]
fn req_tool_010_transaction_equals_action() {
    let h = Harness::new(vec![
        act("write_file", "a"),
        act("read_file", "a"),
        act("write_file", "b"),
    ]);
    h.tools
        .script("write_file", ExecutionOutcome::succeeded(json!({}), 1));
    h.tools
        .script("read_file", ExecutionOutcome::succeeded(json!({}), 1));
    h.tools.script(
        "write_file",
        ExecutionOutcome::tool_error("disk_full", json!({}), 1),
    );
    let id = h.create();
    let workspaces = h.workspaces.clone();
    assert_eq!(workspaces.snapshots().len(), 1, "the base at creation");
    assert_eq!(h.step(id).outcome, StepOutcome::Continued);
    let snaps = workspaces.snapshots();
    // The fake's manifests are all equal, so the before snapshot reuses the base and the after is new… also equal: reused.
    let actions = h.persistence.load_actions(id).unwrap();
    assert!(
        actions[0].snapshot_before.is_some(),
        "a write action records its pre-execution snapshot"
    );
    let executions = h.persistence.load_executions(actions[0].action_id).unwrap();
    assert_eq!(executions[0].snapshot_before, actions[0].snapshot_before);
    assert!(
        executions[0].snapshot_after.is_some(),
        "a successful write records its after snapshot"
    );
    assert!(!snaps.is_empty());
    assert_eq!(h.step(id).outcome, StepOutcome::Continued);
    let actions = h.persistence.load_actions(id).unwrap();
    assert!(
        actions[1].snapshot_before.is_none(),
        "a read takes no snapshot"
    );
    assert_eq!(h.step(id).outcome, StepOutcome::Continued);
    let actions = h.persistence.load_actions(id).unwrap();
    assert!(matches!(actions[2].status, ActionStatus::Failed { .. }));
    assert_eq!(
        workspaces.restores(),
        vec![actions[2].snapshot_before.unwrap()],
        "the failed write was restored to its before snapshot"
    );
    let observations = h.persistence.load_observations(id).unwrap();
    assert!(observations[2].rolled_back, "{:?}", observations[2]);
    assert!(
        observations[2]
            .notices
            .iter()
            .any(|n| matches!(n, crate::observation::Notice::RolledBack))
    );
    assert!(!observations[0].rolled_back);
    let names = h.event_names(id);
    assert!(names.contains(&"workspace.rollback".to_owned()));
}

/// REQ-WS-010: a cancelled write is restored unless the profile keeps it.
#[test]
fn req_ws_010_cancel_restores_unless_keep() {
    let h = Harness::new(vec![act("write_file", "a")]);
    h.tools.script("write_file", ExecutionOutcome::cancelled(1));
    let id = h.create();
    assert_eq!(h.step(id).outcome, StepOutcome::Cancelled);
    assert_eq!(
        h.workspaces.restores().len(),
        1,
        "on_cancel = rollback by default"
    );
    let observations = h.persistence.load_observations(id).unwrap();
    assert!(observations[0].rolled_back);

    let mut policy = Policy::shipped();
    policy.profiles.get_mut("workspace").unwrap().on_cancel = crate::policy::OnCancel::Keep;
    let h = Harness::with_policy(vec![act("write_file", "a")], policy);
    h.tools.script("write_file", ExecutionOutcome::cancelled(1));
    let id = h.create();
    assert_eq!(h.step(id).outcome, StepOutcome::Cancelled);
    assert!(h.workspaces.restores().is_empty(), "on_cancel = keep");
    assert!(!h.persistence.load_observations(id).unwrap()[0].rolled_back);
}

/// REQ-WS-006 / REQ-WS-007 / RFC 006 §6: the user's rollback is valid when
/// the task is not running; deletion needs a terminal task and removes the
/// rows; the footprint is reported.
#[test]
fn req_ws_006_rollback_and_delete_verbs() {
    let h = Harness::new(vec![json!({"decision": "complete", "summary": "done"})]);
    let id = h.create();
    let err = h
        .runtime
        .rollback_workspace(&h.session, id, None)
        .unwrap_err();
    assert_eq!(err.kind(), "state", "not while running");
    let err = h.runtime.delete_task(&h.session, id, false).unwrap_err();
    assert_eq!(err.kind(), "state", "not before the task is terminal");
    h.step(id);
    let record = h.runtime.rollback_workspace(&h.session, id, None).unwrap();
    assert_eq!(record.reason, crate::snapshot::SnapshotReason::Base);
    assert_eq!(h.workspaces.restores(), vec![record.snapshot_id]);
    let view = h.runtime.get_task(id).unwrap();
    assert_eq!(view.snapshots.len(), 1);
    assert_eq!(view.footprint.snapshots, 1);
    let footprint = h.runtime.delete_task(&h.session, id, false).unwrap();
    assert_eq!(footprint.snapshots, 1);
    assert!(h.persistence.load_snapshots(id).unwrap().is_empty());
    assert!(
        h.event_names(id).contains(&"task.deleted".to_owned()),
        "the audit stays"
    );
    assert!(
        h.runtime.get_task(id).is_ok(),
        "the task record stays without --audit"
    );
    let err = h.runtime.delete_task(&h.session, id, true);
    assert!(err.is_ok());
    assert!(h.runtime.get_task(id).is_err(), "--audit removes the task");
}

/// REQ-TASK-004: a fork materializes from the source's snapshot, copies the
/// active grants, attaches the history, and leaves the source untouched.
#[test]
fn req_task_004_fork() {
    let h = Harness::new(vec![
        json!({"decision": "plan", "steps": ["one", "two"]}),
        act("read_file", "a"),
        json!({"decision": "complete", "summary": "done"}),
        json!({"decision": "complete", "summary": "forked done"}),
    ]);
    h.tools
        .script("read_file", ExecutionOutcome::succeeded(json!({}), 1));
    let source = h.create_with(|r| r.profile = Some("read_only".into()));
    h.step(source);
    h.step(source);
    assert_eq!(h.step(source).outcome, StepOutcome::Completed);
    let err = h.runtime.create_task(
        &h.session,
        CreateTask {
            goal: "continue".into(),
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
            from_task: Some(source),
            from_snapshot: None,
        },
    );
    let fork = err.unwrap();
    assert_eq!(fork.forked_from, Some(source));
    let view = h.runtime.get_task(fork.task_id).unwrap();
    assert_eq!(
        view.grants.len(),
        3,
        "the source's read_only grants, not the workspace profile's"
    );
    assert_eq!(
        view.snapshots[0].reason,
        crate::snapshot::SnapshotReason::Fork
    );
    h.runtime
        .resume_task(&h.session, fork.task_id, None)
        .unwrap();
    assert_eq!(h.step(fork.task_id).outcome, StepOutcome::Completed);
    let request = h.backend.requests().last().unwrap().clone();
    let history = request
        .messages
        .iter()
        .find(|m| m.content.starts_with("<<history"))
        .expect("history segment");
    assert!(
        history
            .content
            .contains("\"goal\":\"fix the failing test\""),
        "{}",
        history.content
    );
    assert!(history.content.contains("\"tool\":\"read_file\""));
    assert!(history.content.contains("\"plan\":[\"one\",\"two\"]"));
    assert_eq!(
        h.runtime.get_task(source).unwrap().task.state,
        TaskState::Completed {
            completion: crate::task::Completion::Asserted
        }
    );
}

// ---------------------------------------------------------------------------
// Context assembly (RFC 007; REQ-CTX-001, 003, 004, 007, 010; REQ-TEST-010).
// ---------------------------------------------------------------------------

/// The scripted scenario of the context tests: `n` read actions on paths
/// `p0 … p(n-1)`; step 1 prints forty identical lines; step 3 fails and is
/// never superseded; step 5 fails and step 6 succeeds on the same path.
fn context_scenario(n: usize) -> Vec<Value> {
    (0..n)
        .map(|i| {
            let path = if i == 6 {
                "p5".to_owned()
            } else {
                format!("p{i}")
            };
            act("read_file", &path)
        })
        .collect()
}

fn script_context_outcomes(h: &Harness, n: usize) {
    for i in 0..n {
        let mut outcome = if i == 3 || i == 5 {
            ExecutionOutcome::tool_error("missing", json!({}), 1)
        } else {
            ExecutionOutcome::succeeded(json!({"content": format!("file {i}\n")}), 1)
        };
        if i == 1 {
            outcome.stdout = std::iter::repeat_n("warning: x\n", 40)
                .collect::<String>()
                .into_bytes();
        }
        h.tools.script("read_file", outcome);
    }
}

fn request_chars(h: &Harness, k: usize) -> usize {
    crate::context::content_chars(&h.backend.requests()[k].messages)
}

fn history_segment(messages: &[crate::spi::Message]) -> Option<(usize, usize, Vec<Value>)> {
    let m = messages
        .iter()
        .find(|m| m.content.starts_with("<<feedback kind=\"history\""))?;
    let head = m.content.lines().next().unwrap();
    let attr = |name: &str| -> usize {
        let key = format!("{name}=\"");
        let start = head.find(&key).unwrap() + key.len();
        head[start..].split('"').next().unwrap().parse().unwrap()
    };
    let body: Vec<Value> = serde_json::from_str(m.content.lines().nth(1).unwrap()).unwrap();
    Some((attr("summarized"), attr("dropped"), body))
}

fn measures(h: &Harness, id: TaskId, index: usize) -> crate::step::StepMeasures {
    h.persistence.load_steps(id).unwrap()[index]
        .measures
        .clone()
        .expect("every step is measured")
}

/// REQ-CTX-001 / C-07: segments 1–4 are byte-identical across steps; a plan
/// is appended after them; a grant changes the capability summary once.
#[test]
fn req_ctx_001_stable_prefix_across_steps() {
    let mut decisions = context_scenario(10);
    decisions.insert(4, json!({"decision": "plan", "steps": ["read", "fix"]}));
    let h = Harness::new(decisions);
    script_context_outcomes(&h, 10);
    let id = h.create_with(|r| r.profile = Some("read_only".into()));
    for _ in 0..8 {
        assert_eq!(h.step(id).outcome, StepOutcome::Continued);
    }
    let requests = h.backend.requests();
    assert_eq!(requests.len(), 8);
    let first = &requests[0].messages;
    assert_eq!(first[0].role, crate::spi::Role::System);
    assert!(
        first[0]
            .content
            .starts_with(super::context::SYSTEM_INSTRUCTIONS)
    );
    assert!(
        first[0]
            .content
            .contains("<<tool_definitions schema_version=")
    );
    assert!(
        first[0]
            .content
            .contains("<<feedback kind=\"capabilities\">>")
    );
    assert!(first[1].content.starts_with("<<user kind=\"goal\">>"));
    assert!(first.last().unwrap().content.contains("decision_request"));
    for (k, r) in requests.iter().enumerate() {
        assert_eq!(
            r.messages[0], first[0],
            "segment 1–3 changed at request {k}"
        );
        assert_eq!(r.messages[1], first[1], "segment 4 changed at request {k}");
        if k >= 5 {
            assert!(
                r.messages[2].content.starts_with("<<plan version=\"1\">>"),
                "the plan follows the goal from the step after the plan decision"
            );
            assert_eq!(r.messages[2].role, crate::spi::Role::Assistant);
        } else {
            assert!(!r.messages[2].content.starts_with("<<plan"));
        }
        let m = measures(&h, id, k);
        assert_eq!(m.context.compressed_steps, 0);
        assert!(!m.context.over_budget);
        if k > 0 {
            let stable = crate::context::common_prefix_chars(&first[..2], &r.messages[..2]);
            assert!(
                m.context.prefix_chars as usize >= stable,
                "request {k}: prefix {} < stable {stable}",
                m.context.prefix_chars
            );
        } else {
            assert_eq!(m.context.prefix_chars, 0);
        }
        assert!(history_segment(&r.messages).is_none());
    }
    // A grant changes the capability summary once; the goal stays.
    h.runtime
        .grant_capability(
            &h.session,
            id,
            GrantKind::Tool {
                name: "write_file".into(),
            },
        )
        .unwrap();
    assert_eq!(h.step(id).outcome, StepOutcome::Continued);
    assert_eq!(h.step(id).outcome, StepOutcome::Continued);
    let requests = h.backend.requests();
    assert_ne!(requests[8].messages[0], first[0]);
    assert!(requests[8].messages[0].content.contains("\"write_file\""));
    assert_eq!(requests[9].messages[0], requests[8].messages[0]);
    assert_eq!(requests[9].messages[1], first[1]);
}

/// REQ-CTX-003 / REQ-CTX-004: under a tight budget the rules apply in order
/// (collapse, summarize beyond LIM-C3, drop oldest summaries in batches),
/// the retained set survives, and the measures say what happened.
#[test]
fn req_ctx_003_budget_rules_and_retained_set() {
    const N: usize = 14;
    // A generous run fixes the sizes the tight runs are derived from.
    let generous = Harness::new(context_scenario(N));
    script_context_outcomes(&generous, N);
    let id = generous.create_with(|r| r.profile = Some("read_only".into()));
    for _ in 0..N {
        generous.step(id);
    }
    assert_eq!(generous.backend.requests().len(), N);
    let last = request_chars(&generous, N - 1);
    let third = request_chars(&generous, 2);
    let first = request_chars(&generous, 0);

    // Rule 2 alone: the third request (which carries step 1's observation)
    // is over by one; collapsing its forty repeated lines makes it fit
    // without any summary.
    let tight = Harness::with_context(context_scenario(3), Policy::shipped(), vec![], third - 1);
    script_context_outcomes(&tight, 3);
    let id = tight.create_with(|r| r.profile = Some("read_only".into()));
    for _ in 0..3 {
        tight.step(id);
    }
    let r2 = &tight.backend.requests()[2].messages;
    assert!(
        r2.iter()
            .any(|m| m.content.contains("[… repeated 40 times …]")),
        "repeated lines collapsed"
    );
    let m = measures(&tight, id, 2);
    assert_eq!(m.context.compressed_steps, 0);
    assert!(!m.context.over_budget);
    assert!(m.context.context_chars < m.context.chars_before_fit);
    assert!((m.context.context_chars as usize) < third);

    // Rules 3 and 4: a budget between the stable prefix and the full history.
    let budget = first + (last - first) / 3;
    let h = Harness::with_context(context_scenario(N), Policy::shipped(), vec![], budget);
    script_context_outcomes(&h, N);
    let id = h.create_with(|r| r.profile = Some("read_only".into()));
    for _ in 0..N {
        assert_eq!(h.step(id).outcome, StepOutcome::Continued);
    }
    let requests = h.backend.requests();
    let mut saw_summaries = false;
    let mut saw_drops = false;
    for (k, r) in requests.iter().enumerate() {
        let m = measures(&h, id, k);
        let chars = crate::context::content_chars(&r.messages);
        assert_eq!(m.context.context_chars as usize, chars);
        assert!(
            chars <= budget || m.context.over_budget,
            "request {k}: {chars} > {budget} without over_budget"
        );
        let Some((summarized, dropped, body)) = history_segment(&r.messages) else {
            assert_eq!(m.context.compressed_steps, 0);
            continue;
        };
        saw_summaries |= summarized > 0;
        saw_drops |= dropped > 0;
        assert_eq!(summarized, m.context.compressed_steps as usize);
        assert_eq!(dropped, m.context.dropped_steps as usize);
        assert_eq!(body.len(), summarized);
        // Steps before the request: k. Recent LIM-C3 stay full; the rest are summaries or dropped.
        assert_eq!(
            summarized + dropped,
            k.saturating_sub(crate::limits::LIM_C3_RECENT_STEPS)
        );
        for s in &body {
            assert!(
                serde_json::to_string(s).unwrap().chars().count()
                    <= crate::limits::LIM_C4_SUMMARY_CHARS
            );
        }
        let present: Vec<u64> = body.iter().map(|s| s["step"].as_u64().unwrap()).collect();
        // Retained: step 3 failed on p3 and nothing later succeeded on p3.
        if k > 3 + crate::limits::LIM_C3_RECENT_STEPS {
            assert!(
                present.contains(&3),
                "request {k}: retained failure dropped: {present:?}"
            );
            let three = body.iter().find(|s| s["step"] == 3).unwrap();
            assert_eq!(three["cause"], "tool_error");
            assert_eq!(three["status"], "failed");
        }
        // Dropped ones are the oldest non-retained summaries (step 5's failure was superseded by step 6).
        let candidates: Vec<u64> = (0..k as u64).filter(|i| *i != 3).take(dropped).collect();
        for c in &candidates {
            assert!(
                !present.contains(c),
                "request {k}: step {c} should be dropped: {present:?}"
            );
        }
        // Dropping goes in batches of LIM-C3 unless the droppable set ran out.
        let eligible = k.saturating_sub(crate::limits::LIM_C3_RECENT_STEPS);
        let droppable = eligible - usize::from(3 < eligible);
        assert!(
            dropped % crate::limits::LIM_C3_RECENT_STEPS == 0 || dropped == droppable,
            "request {k}: dropped {dropped} is neither a batch nor everything ({droppable})"
        );
    }
    assert!(saw_summaries, "the tight budget never summarized");
    assert!(saw_drops, "the tight budget never dropped");
    let metrics = h.runtime.get_metrics(Some(id)).unwrap().context;
    assert_eq!(metrics.measured_steps, N as u64);
    assert!(metrics.compressed_steps > 0);
    assert!(metrics.dropped_steps > 0);
    assert!(metrics.compression_ratio.unwrap() > 1.0);
    assert!(metrics.prefix_ratio.unwrap() > 0.0);
    assert!(metrics.context_chars_avg.unwrap() > 0);
    assert_eq!(metrics.prompt_tokens, 0);
    assert!(metrics.chars_per_prompt_token.is_none());

    // Over budget: a budget below the stable prefix cannot be met; the
    // request still goes out and says so.
    let h = Harness::with_context(context_scenario(2), Policy::shipped(), vec![], first / 2);
    script_context_outcomes(&h, 2);
    let id = h.create_with(|r| r.profile = Some("read_only".into()));
    h.step(id);
    h.step(id);
    assert!(measures(&h, id, 1).context.over_budget);
    assert_eq!(h.backend.requests().len(), 2);
}

/// REQ-CTX-007 / REQ-PRIV-002: configured secrets never reach a stored
/// observation or the next request; a notice names what was redacted.
#[test]
fn req_ctx_007_secrets_redacted_before_persistence() {
    let secret = "sk-live-9f8e7d6c".to_owned();
    let h = Harness::with_context(
        context_scenario(2),
        Policy::shipped(),
        vec![("backend.api_key".into(), secret.clone())],
        crate::limits::LIM_C1_CONTEXT_CHARS,
    );
    let mut outcome = ExecutionOutcome::succeeded(
        json!({"content": format!("key={secret}\n"), "nested": [format!("x{secret}y")]}),
        1,
    );
    outcome.stdout = format!("{{\"token\":\"{secret}\"}}\n").into_bytes();
    outcome.stderr = format!("auth {secret} failed\n").into_bytes();
    h.tools.script("read_file", outcome);
    h.tools
        .script("read_file", ExecutionOutcome::succeeded(json!({}), 1));
    let id = h.create_with(|r| r.profile = Some("read_only".into()));
    h.step(id);
    h.step(id);
    let observations = h.persistence.load_observations(id).unwrap();
    let text = serde_json::to_string(&observations[0]).unwrap();
    assert!(!text.contains(&secret), "{text}");
    assert_eq!(
        observations[0].payload["content"],
        "key=[redacted:backend.api_key]\n"
    );
    assert_eq!(
        observations[0].payload["nested"][0],
        "x[redacted:backend.api_key]y"
    );
    assert_eq!(
        observations[0].stdout,
        "{\"token\":\"[redacted:backend.api_key]\"}\n"
    );
    assert_eq!(
        observations[0].stderr,
        "auth [redacted:backend.api_key] failed\n"
    );
    assert!(observations[0].notices.iter().any(|n| matches!(
        n,
        crate::observation::Notice::SecretRedacted { name } if name == "backend.api_key"
    )));
    let request = serde_json::to_string(&h.backend.requests()[1].messages).unwrap();
    assert!(!request.contains(&secret));
    assert!(request.contains("[redacted:backend.api_key]"));
}

/// REQ-TEST-016: the same history assembles the same bytes.
#[test]
fn req_test_016_assembly_is_pure() {
    let runs: Vec<Vec<crate::spi::GenerateRequest>> = (0..2)
        .map(|_| {
            let h = Harness::new(context_scenario(5));
            script_context_outcomes(&h, 5);
            let id = h.create_with(|r| r.profile = Some("read_only".into()));
            for _ in 0..5 {
                h.step(id);
            }
            h.backend.requests()
        })
        .collect();
    assert_eq!(runs[0].len(), 5);
    for (k, (a, b)) in runs[0].iter().zip(&runs[1]).enumerate() {
        assert_eq!(a.messages, b.messages, "request {k}");
    }
}

/// REQ-SEC-010 / D-21: a remote backend is refused for a profile whose
/// `external` is false, by name; a profile with `external = true` admits
/// it; the backend is recorded at start and on the task's versions.
#[test]
fn req_sec_010_remote_backend_needs_external_profile() {
    let mut policy = Policy::shipped();
    policy.remote_backend_allowed = true;
    let mut open = policy.profiles["workspace"].clone();
    open.name = "open".into();
    open.external = true;
    policy.profiles.insert("open".into(), open);
    let h = Harness::with_backend(
        Arc::new(ScriptedBackend::with_decisions(vec![act("read_file", "a")]).remote()),
        policy,
        vec![],
        crate::limits::LIM_C1_CONTEXT_CHARS,
    );
    let err = h
        .runtime
        .create_task(
            &h.session,
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
        .unwrap_err();
    assert!(
        matches!(&err, RuntimeError::Denied { detail } if detail.contains("profile workspace forbids external communication (external = false)")),
        "{err}"
    );
    let id = h.create_with(|r| r.profile = Some("open".into()));
    let task = h.runtime.get_task(id).unwrap().task;
    let backend = task.versions.backend.unwrap();
    assert_eq!(backend.kind, "scripted");
    assert_eq!(backend.locality, crate::spi::Locality::Remote);
    h.runtime.record_started().unwrap();
    let started = h
        .persistence
        .events()
        .into_iter()
        .find(|e| e.event.name() == "runtime.started")
        .unwrap();
    assert!(matches!(
        started.event,
        crate::event::Event::RuntimeStarted {
            backend: crate::spi::BackendDescriptor {
                locality: crate::spi::Locality::Remote,
                ..
            },
            ..
        }
    ));
    assert_eq!(
        h.runtime.get_runtime_status().unwrap().backend.locality,
        crate::spi::Locality::Remote
    );
}
