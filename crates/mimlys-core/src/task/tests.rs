use super::*;
use crate::clock::{Clock, FakeClock};
use crate::ids::ScriptedIds;
use crate::limits::{LIM_T1_STEPS, LIM_T5_DEADLINE_MS};
use crate::task::test_versions;

fn fixture() -> (Task, FakeClock) {
    let clock = FakeClock::default();
    let task = Task::create(
        crate::task::TaskSpec {
            task_id: TaskId::new(&ScriptedIds::default()),
            goal: "fix the failing test".into(),
            deadline_ms: LIM_T5_DEADLINE_MS,
            budgets: Budgets::default(),
            profile: "workspace".into(),
            versions: test_versions(),
            scope: crate::task::ScopeSpec {
                root: "/project".into(),
                include: vec![],
            },
        },
        clock.now(),
    )
    .unwrap();
    (task, clock)
}

fn ceiling(task: &Task) -> ExtensionCeiling {
    ExtensionCeiling {
        max_steps: 200,
        max_deadline: task.created_at.plus_millis(24 * 60 * 60 * 1000).unwrap(),
    }
}

fn resume(task: &Task) -> TaskInput {
    TaskInput::Resume {
        extend: None,
        ceiling: ceiling(task),
    }
}

fn in_state(state: TaskState) -> Task {
    let (mut task, _) = fixture();
    task.state = state;
    if let TaskState::WaitingUser { wait_kind } = &task.state {
        task.pending = Some(match wait_kind {
            WaitKind::Approval => PendingItem::Approval {
                action_id: ActionId::new(&ScriptedIds::default()),
            },
            WaitKind::Question => PendingItem::Question {
                question_id: QuestionId::new(&ScriptedIds::default()),
            },
        });
    }
    task
}

fn all_states() -> Vec<TaskState> {
    vec![
        TaskState::Created,
        TaskState::Running,
        TaskState::WaitingUser {
            wait_kind: WaitKind::Approval,
        },
        TaskState::WaitingUser {
            wait_kind: WaitKind::Question,
        },
        TaskState::Paused,
        TaskState::Halted {
            halt_reason: HaltReason::Convergence,
        },
        TaskState::Recovering,
        TaskState::Completed {
            completion: Completion::Asserted,
        },
        TaskState::Failed {
            reason: FailureReason::WorkspaceLost,
        },
        TaskState::Cancelled,
    ]
}

fn all_inputs(task: &Task) -> Vec<TaskInput> {
    vec![
        resume(task),
        TaskInput::PauseRequest,
        TaskInput::Pause,
        TaskInput::Cancel,
        TaskInput::NeedsUser(PendingItem::Question {
            question_id: QuestionId::new(&ScriptedIds::default()),
        }),
        TaskInput::UserDecided,
        TaskInput::UserHalted,
        TaskInput::Expired,
        TaskInput::Halt(HaltReason::Policy),
        TaskInput::Complete(Completion::Verified),
        TaskInput::Fail(FailureReason::WorkspaceLost),
        TaskInput::StartRecovery,
        TaskInput::Recovered {
            resume: false,
            reason: HaltReason::Recovered,
        },
    ]
}

/// REQ-TASK-001 / REQ-TASK-009: creation yields `created` with the deadline at now + LIM-T5.
#[test]
fn req_task_001_create_sets_state_deadline_and_budgets() {
    let (task, clock) = fixture();
    assert_eq!(task.state, TaskState::Created);
    assert_eq!(task.remaining_ms(clock.now()), LIM_T5_DEADLINE_MS);
    assert!(!task.deadline_passed(clock.now()));
    assert_eq!(task.budgets.steps, LIM_T1_STEPS);
    assert_eq!(task.counters, Counters::default());
    assert!(!task.pause_requested);
    assert!(task.pending.is_none());
}

/// REQ-TASK-002: every row of external design §5.1 is a legal transition with the expected result.
/// REQ-TEST-003 (tasks, legal): the nineteen rows of the table are all asserted, with the resulting state.
#[test]
fn req_task_002_legal_transitions_of_ed_5_1() {
    let (task, clock) = fixture();
    let now = clock.now();
    let r =
        |state: TaskState, input: TaskInput| in_state(state).transition(input, now).unwrap().state;
    let question = PendingItem::Question {
        question_id: QuestionId::new(&ScriptedIds::default()),
    };
    assert_eq!(r(TaskState::Created, resume(&task)), TaskState::Running);
    assert_eq!(
        r(TaskState::Created, TaskInput::Cancel),
        TaskState::Cancelled
    );
    assert_eq!(
        r(TaskState::Running, TaskInput::NeedsUser(question)),
        TaskState::WaitingUser {
            wait_kind: WaitKind::Question
        }
    );
    assert_eq!(r(TaskState::Running, TaskInput::Pause), TaskState::Paused);
    assert_eq!(
        r(TaskState::Running, TaskInput::Halt(HaltReason::BudgetSteps)),
        TaskState::Halted {
            halt_reason: HaltReason::BudgetSteps
        }
    );
    assert_eq!(
        r(
            TaskState::Running,
            TaskInput::Complete(Completion::Verified)
        ),
        TaskState::Completed {
            completion: Completion::Verified
        }
    );
    assert_eq!(
        r(
            TaskState::Running,
            TaskInput::Fail(FailureReason::ModelGaveUp {
                reason: "no".into()
            })
        ),
        TaskState::Failed {
            reason: FailureReason::ModelGaveUp {
                reason: "no".into()
            }
        }
    );
    assert_eq!(
        r(TaskState::Running, TaskInput::Cancel),
        TaskState::Cancelled
    );
    assert_eq!(
        r(TaskState::Running, TaskInput::StartRecovery),
        TaskState::Recovering
    );
    let waiting_approval = TaskState::WaitingUser {
        wait_kind: WaitKind::Approval,
    };
    let waiting_question = TaskState::WaitingUser {
        wait_kind: WaitKind::Question,
    };
    assert_eq!(
        r(waiting_approval.clone(), TaskInput::UserDecided),
        TaskState::Running
    );
    assert_eq!(
        r(waiting_approval.clone(), TaskInput::UserHalted),
        TaskState::Halted {
            halt_reason: HaltReason::ApprovalRejected
        }
    );
    assert_eq!(
        r(waiting_approval.clone(), TaskInput::Expired),
        TaskState::Halted {
            halt_reason: HaltReason::ApprovalExpired
        }
    );
    assert_eq!(
        r(waiting_question, TaskInput::Expired),
        TaskState::Halted {
            halt_reason: HaltReason::QuestionExpired
        }
    );
    assert_eq!(r(waiting_approval, TaskInput::Cancel), TaskState::Cancelled);
    assert_eq!(r(TaskState::Paused, resume(&task)), TaskState::Running);
    assert_eq!(
        r(TaskState::Paused, TaskInput::Cancel),
        TaskState::Cancelled
    );
    let halted = TaskState::Halted {
        halt_reason: HaltReason::Convergence,
    };
    assert_eq!(r(halted.clone(), resume(&task)), TaskState::Running);
    assert_eq!(r(halted, TaskInput::Cancel), TaskState::Cancelled);
    assert_eq!(
        r(
            TaskState::Recovering,
            TaskInput::Recovered {
                resume: false,
                reason: HaltReason::UnknownOutcome
            }
        ),
        TaskState::Halted {
            halt_reason: HaltReason::UnknownOutcome
        }
    );
    assert_eq!(
        r(
            TaskState::Recovering,
            TaskInput::Recovered {
                resume: true,
                reason: HaltReason::Recovered
            }
        ),
        TaskState::Running
    );
    assert_eq!(
        r(TaskState::Recovering, TaskInput::Cancel),
        TaskState::Cancelled
    );
}

/// REQ-TASK-002: every (state, input) pair not in external design §5.1 is refused with `StateError`.
/// REQ-TEST-003 (tasks, illegal): all ten states against all thirteen inputs; every pair outside the table is refused.
#[test]
fn req_task_002_illegal_transitions_are_refused() {
    let (task, clock) = fixture();
    let now = clock.now();
    let legal: &[(&str, &str)] = &[
        ("created", "resume"),
        ("created", "cancel"),
        ("running", "pause_request"),
        ("running", "pause"),
        ("running", "needs_user"),
        ("running", "halt"),
        ("running", "complete"),
        ("running", "fail"),
        ("running", "cancel"),
        ("running", "start_recovery"),
        ("waiting_user", "user_decided"),
        ("waiting_user", "user_halted"),
        ("waiting_user", "expired"),
        ("waiting_user", "cancel"),
        ("paused", "resume"),
        ("paused", "cancel"),
        ("halted", "resume"),
        ("halted", "cancel"),
        ("recovering", "recovered"),
        ("recovering", "cancel"),
    ];
    let mut checked = 0;
    for state in all_states() {
        for input in all_inputs(&task) {
            let pair = (state.name(), input.name());
            let result = in_state(state.clone()).transition(input.clone(), now);
            if legal.contains(&pair) {
                assert!(result.is_ok(), "expected legal: {pair:?}");
            } else {
                assert_eq!(
                    result,
                    Err(StateError::Illegal {
                        from: pair.0,
                        input: pair.1
                    }),
                    "expected refusal: {pair:?}"
                );
            }
            checked += 1;
        }
    }
    assert_eq!(checked, all_states().len() * 13);
}

/// REQ-TASK-003: terminal states refuse every input and the original task is never mutated.
#[test]
fn req_task_003_terminal_states_are_immutable() {
    let (task, clock) = fixture();
    for state in all_states().into_iter().filter(|s| s.is_terminal()) {
        let terminal = in_state(state);
        for input in all_inputs(&task) {
            let before = terminal.clone();
            assert!(terminal.transition(input, clock.now()).is_err());
            assert_eq!(terminal, before);
        }
    }
    assert!(!TaskState::Running.is_terminal());
    assert!(
        !TaskState::Halted {
            halt_reason: HaltReason::Recovered
        }
        .is_terminal()
    );
}

/// REQ-TASK-005: a pause request is a flag cleared at the boundary; resumption clears it too.
#[test]
fn req_task_005_pause_request_is_a_flag_until_the_boundary() {
    let (task, clock) = fixture();
    let now = clock.now();
    let running = task.transition(resume(&task), now).unwrap();
    let requested = running.transition(TaskInput::PauseRequest, now).unwrap();
    assert_eq!(requested.state, TaskState::Running);
    assert!(requested.pause_requested);
    let paused = requested.transition(TaskInput::Pause, now).unwrap();
    assert_eq!(paused.state, TaskState::Paused);
    assert!(!paused.pause_requested);
}

/// REQ-TASK-005: an extension is accepted only from a budget halt, only upward, only within the ceiling,
/// and only for steps and deadline.
#[test]
fn req_task_005_extension_rules() {
    let (task, clock) = fixture();
    let now = clock.now();
    let c = ceiling(&task);
    let budget_halt = in_state(TaskState::Halted {
        halt_reason: HaltReason::BudgetSteps,
    });
    let later = task.deadline.plus_millis(60_000).unwrap();
    let ok = budget_halt
        .transition(
            TaskInput::Resume {
                extend: Some(Extension {
                    steps: Some(LIM_T1_STEPS + 10),
                    deadline: Some(later),
                }),
                ceiling: c,
            },
            now,
        )
        .unwrap();
    assert_eq!(ok.state, TaskState::Running);
    assert_eq!(ok.budgets.steps, LIM_T1_STEPS + 10);
    assert_eq!(ok.deadline, later);

    let refuse = |halt: HaltReason, ext: Extension| {
        in_state(TaskState::Halted { halt_reason: halt })
            .transition(
                TaskInput::Resume {
                    extend: Some(ext),
                    ceiling: c,
                },
                now,
            )
            .unwrap_err()
    };
    let raise = Extension {
        steps: Some(LIM_T1_STEPS + 1),
        deadline: None,
    };
    assert!(matches!(
        refuse(HaltReason::Convergence, raise),
        StateError::Extension { .. }
    ));
    assert!(matches!(
        refuse(
            HaltReason::BudgetSteps,
            Extension {
                steps: Some(LIM_T1_STEPS),
                deadline: None
            }
        ),
        StateError::Extension { reason } if reason.contains("raised")
    ));
    assert!(matches!(
        refuse(
            HaltReason::BudgetSteps,
            Extension {
                steps: Some(c.max_steps + 1),
                deadline: None
            }
        ),
        StateError::Extension { reason } if reason.contains("ceiling")
    ));
    assert!(matches!(
        refuse(
            HaltReason::BudgetDeadline,
            Extension {
                steps: None,
                deadline: Some(task.deadline)
            }
        ),
        StateError::Extension { reason } if reason.contains("later")
    ));
    assert!(matches!(
        refuse(
            HaltReason::BudgetDeadline,
            Extension {
                steps: None,
                deadline: Some(c.max_deadline.plus_millis(1).unwrap())
            }
        ),
        StateError::Extension { reason } if reason.contains("ceiling")
    ));
    // Not even an empty extension from created or paused.
    for state in [TaskState::Created, TaskState::Paused] {
        assert!(matches!(
            in_state(state)
                .transition(
                    TaskInput::Resume {
                        extend: Some(Extension::default()),
                        ceiling: c
                    },
                    now
                )
                .unwrap_err(),
            StateError::Extension { .. }
        ));
    }
}

/// REQ-TASK-007: the fourteen halt reasons with their snake_case names.
#[test]
fn req_task_007_halt_reasons_are_the_fourteen_of_ed_5_2() {
    let names: Vec<String> = HaltReason::ALL
        .iter()
        .map(|r| {
            serde_json::to_string(r)
                .unwrap()
                .trim_matches('"')
                .to_owned()
        })
        .collect();
    assert_eq!(
        names,
        [
            "budget_steps",
            "budget_rejections",
            "budget_failures",
            "budget_deadline",
            "convergence",
            "approval_rejected",
            "approval_expired",
            "question_expired",
            "recovered",
            "unknown_outcome",
            "workspace_quota",
            "backend_unavailable",
            "tool_unavailable",
            "policy"
        ]
    );
}

/// REQ-TASK-009: the deadline is absolute; a stopped clock that jumps past it reports it as passed.
#[test]
fn req_task_009_deadline_is_absolute() {
    let (task, clock) = fixture();
    clock.advance_millis(LIM_T5_DEADLINE_MS - 1);
    assert!(!task.deadline_passed(clock.now()));
    clock.advance_millis(1);
    assert!(task.deadline_passed(clock.now()));
    assert_eq!(task.remaining_ms(clock.now()), 0);
}

/// REQ-TASK-010 / REQ-TASK-011: terminal fields are carried flat in the serialized state.
#[test]
fn req_task_010_terminal_fields_serialize_flat() {
    let completed = in_state(TaskState::Completed {
        completion: Completion::Verified,
    });
    let json = serde_json::to_value(&completed).unwrap();
    assert_eq!(json["state"], "completed");
    assert_eq!(json["completion"], "verified");
    let failed = in_state(TaskState::Failed {
        reason: FailureReason::ModelGaveUp {
            reason: "stuck".into(),
        },
    });
    let json = serde_json::to_value(&failed).unwrap();
    assert_eq!(json["state"], "failed");
    assert_eq!(json["reason"]["failure_reason"], "model_gave_up");
    let halted = in_state(TaskState::Halted {
        halt_reason: HaltReason::WorkspaceQuota,
    });
    let json = serde_json::to_value(&halted).unwrap();
    assert_eq!(json["state"], "halted");
    assert_eq!(json["halt_reason"], "workspace_quota");
    let back: Task = serde_json::from_value(json).unwrap();
    assert_eq!(back, halted);
}

/// REQ-TASK-002: waiting records the pending item and clears it on every exit.
#[test]
fn req_task_002_pending_item_is_set_and_cleared() {
    let (task, clock) = fixture();
    let now = clock.now();
    let action_id = ActionId::new(&ScriptedIds::default());
    let running = task.transition(resume(&task), now).unwrap();
    let waiting = running
        .transition(
            TaskInput::NeedsUser(PendingItem::Approval { action_id }),
            now,
        )
        .unwrap();
    assert_eq!(waiting.pending, Some(PendingItem::Approval { action_id }));
    assert_eq!(
        waiting.state,
        TaskState::WaitingUser {
            wait_kind: WaitKind::Approval
        }
    );
    for input in [
        TaskInput::UserDecided,
        TaskInput::UserHalted,
        TaskInput::Expired,
        TaskInput::Cancel,
    ] {
        assert!(waiting.transition(input, now).unwrap().pending.is_none());
    }
}

/// REQ-TASK-010 / ED §5.3: the completion rule — asserted without a check, verified only after a passed check.
#[test]
fn req_task_010_completion_rule() {
    let (task, _) = fixture();
    assert_eq!(
        task.completion_step(None),
        CompletionStep::Finish(Completion::Asserted)
    );
    assert_eq!(
        task.completion_step(Some(false)),
        CompletionStep::Finish(Completion::Asserted),
        "no check configured: the result is irrelevant"
    );
    let checked = task.with_completion_check(CompletionCheck {
        tool: "run_script".into(),
        arguments: serde_json::Map::new(),
    });
    assert_eq!(checked.completion_step(None), CompletionStep::RunCheck);
    assert_eq!(
        checked.completion_step(Some(true)),
        CompletionStep::Finish(Completion::Verified)
    );
    assert_eq!(
        checked.completion_step(Some(false)),
        CompletionStep::CheckFailed
    );
    let json = serde_json::to_value(&checked).unwrap();
    assert_eq!(json["completion_check"]["tool"], "run_script");
    let back: Task = serde_json::from_value(json).unwrap();
    assert_eq!(back, checked);
}

/// A `versions` record as 0.9.0 and earlier wrote it: no event or action schema version.
fn old_versions_record(runtime_version: &str) -> serde_json::Value {
    serde_json::json!({
        "policy_hash": test_versions().policy_hash,
        "decision_schema_version": 1,
        "tool_definition_hashes": {},
        "runtime_version": runtime_version,
    })
}

/// REQ-MAIN-002, REQ-PERS-012: a task created now records the event and the
/// action schema version beside the decision schema version, and a record
/// that has them reads them back.
#[test]
fn req_main_002_a_new_task_records_both_versions() {
    let (task, _) = fixture();
    assert_eq!(
        task.versions.event_schema_version,
        crate::event::EVENT_SCHEMA_VERSION
    );
    assert_eq!(
        task.versions.action_schema_version,
        crate::action::ACTION_SCHEMA_VERSION
    );
    let json = serde_json::to_value(&task.versions).unwrap();
    assert_eq!(
        json["event_schema_version"],
        crate::event::EVENT_SCHEMA_VERSION
    );
    assert_eq!(
        json["action_schema_version"],
        crate::action::ACTION_SCHEMA_VERSION
    );
    let back: TaskVersions = serde_json::from_value(json).unwrap();
    assert_eq!(back, task.versions);
}

/// REQ-MAIN-002: a task recorded before the two versions existed reads with
/// the values of its release: events were at 1 up to 0.8.0 and are at 2 from
/// 0.9.0; actions were at 1 up to 0.7.1 and are at 2 from 0.8.0.
#[test]
fn req_main_002_an_old_record_reads_with_the_values_of_its_era() {
    for (release, event, action) in [
        ("0.7.0", 1, 1),
        ("0.7.1", 1, 1),
        ("0.8.0", 1, 2),
        ("0.9.0", 2, 2),
        ("0.9.1", 2, 2),
        ("0.10.0", 2, 2),
    ] {
        let v: TaskVersions = serde_json::from_value(old_versions_record(release)).unwrap();
        assert_eq!(v.event_schema_version, event, "events of {release}");
        assert_eq!(v.action_schema_version, action, "actions of {release}");
        assert_eq!(v.runtime_version, release);
    }
}

/// REQ-MAIN-002: a version that is no release (a development or test build,
/// or text that is not a version) is built from the current code and reads
/// as the current schemas; a recorded value is never overridden by the era.
#[test]
fn req_main_002_a_build_that_is_no_release_reads_as_current_and_a_recorded_value_stays() {
    for build in ["0.0.0-test", "0.10.0-dev", "nightly", ""] {
        let v: TaskVersions = serde_json::from_value(old_versions_record(build)).unwrap();
        assert_eq!(
            v.event_schema_version,
            crate::event::EVENT_SCHEMA_VERSION,
            "{build}"
        );
        assert_eq!(
            v.action_schema_version,
            crate::action::ACTION_SCHEMA_VERSION,
            "{build}"
        );
    }
    let mut record = old_versions_record("0.7.0");
    record["event_schema_version"] = 7.into();
    record["action_schema_version"] = 9.into();
    let v: TaskVersions = serde_json::from_value(record).unwrap();
    assert_eq!((v.event_schema_version, v.action_schema_version), (7, 9));
}
