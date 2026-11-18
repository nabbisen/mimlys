use super::*;

fn at(ms: i64) -> Instant {
    Instant::from_unix_millis(ms).unwrap()
}

fn footprint(workspace: u64, promotions: u64) -> Footprint {
    Footprint {
        workspace_bytes: workspace,
        snapshots: 1,
        object_bytes_exclusive: 10,
        object_bytes_shared: 5,
        blob_bytes: 1,
        promotions,
    }
}

fn candidate(n: u128, promotions: u64) -> Candidate {
    Candidate::new(
        TaskId::from_uuid(uuid::Uuid::from_u128(n)),
        "completed",
        60,
        footprint(100 * n as u64, promotions),
    )
}

/// REQ-WS-014: `--state` takes terminal states only; `halted` or anything
/// else is refused with the list of what it takes.
#[test]
fn req_ws_014_state_takes_terminal_states_only() {
    for ok in [
        vec![],
        vec!["completed".to_owned()],
        vec!["failed".into(), "cancelled".into()],
    ] {
        assert!(Filter::parse(&ok, None).is_ok(), "{ok:?}");
    }
    for bad in [
        "halted",
        "running",
        "created",
        "paused",
        "waiting_user",
        "recovering",
        "done",
    ] {
        let err = Filter::parse(&[bad.to_owned()], None)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains(bad) && err.contains("completed, failed, cancelled"),
            "{err}"
        );
    }
    assert!(Filter::parse(&[], Some("soon")).is_err());
}

/// REQ-WS-014: a task that is not terminal never matches, whatever was asked.
#[test]
fn req_ws_014_a_task_that_is_not_terminal_never_matches() {
    let now = at(10_000_000);
    let long_ago = at(0);
    for filter in [
        Filter::default(),
        Filter::parse(&["completed".to_owned()], Some("1s")).unwrap(),
    ] {
        for state in [
            "created",
            "running",
            "waiting_user",
            "paused",
            "halted",
            "recovering",
        ] {
            assert!(!filter.matches(state, long_ago, now), "{state}");
        }
    }
    for state in TERMINAL_STATES {
        assert!(Filter::default().matches(state, long_ago, now), "{state}");
    }
}

/// REQ-WS-014: `--state` and `--older-than` filter; the age is inclusive.
#[test]
fn req_ws_014_state_and_older_than_filter() {
    let now = at(10 * 86_400_000);
    let filter = Filter::parse(&["failed".into(), "cancelled".into()], Some("2d")).unwrap();
    let two_days_ago = at(8 * 86_400_000);
    let just_under = at(8 * 86_400_000 + 1);
    assert!(filter.matches("failed", two_days_ago, now));
    assert!(filter.matches("cancelled", at(0), now));
    assert!(
        !filter.matches("failed", just_under, now),
        "one millisecond too new"
    );
    assert!(!filter.matches("completed", at(0), now), "not in --state");
    // Without --older-than any age matches, the newest included.
    assert!(Filter::default().matches("completed", now, now));
}

/// REQ-WS-014: a task whose promotion is still reversible is kept, listed with
/// the reason, and is not among those deleted; the total counts the others.
#[test]
fn req_ws_014_a_reversible_promotion_is_kept() {
    let found = vec![candidate(1, 0), candidate(2, 1), candidate(3, 0)];
    assert_eq!(found[1].kept, Some(KEPT_PROMOTION_REVERSIBLE));
    assert_eq!(found[0].kept, None);
    let ids: Vec<&str> = to_delete(&found)
        .iter()
        .map(|c| c.task_id.as_str())
        .collect();
    assert_eq!(ids, [found[0].task_id.as_str(), found[2].task_id.as_str()]);
    let lines = crate::messages::prune_plan(&found, false);
    assert!(
        lines
            .iter()
            .any(|l| l.contains(&found[1].task_id) && l.contains("kept: promotion reversible")),
        "{lines:?}"
    );
    let total: u64 = to_delete(&found).iter().map(|c| c.bytes()).sum();
    assert!(
        lines
            .iter()
            .any(|l| l.contains(&format!("({total} bytes)"))),
        "{lines:?}"
    );
    assert!(lines.last().unwrap().contains("audit: kept"));
    assert!(
        crate::messages::prune_plan(&found, true)
            .last()
            .unwrap()
            .contains("--audit")
    );
}

/// REQ-WS-014: tasks are deleted one at a time in the order listed, each
/// result reported; a failure stops the run, the rest is untouched and named.
#[test]
fn req_ws_014_a_failure_midway_leaves_the_rest_untouched_and_named() {
    let found: Vec<Candidate> = (1..=5).map(|n| candidate(n, u64::from(n == 3))).collect();
    let fail_at = found[3].task_id.clone();
    let mut tried = Vec::new();
    let mut reported = Vec::new();
    let outcome = execute(
        &found,
        |c| {
            tried.push(c.task_id.clone());
            if c.task_id == fail_at {
                Err(AppError::Io {
                    detail: "disk on fire".into(),
                })
            } else {
                Ok(c.footprint.clone())
            }
        },
        |c, r| reported.push((c.task_id.clone(), r.is_ok())),
    );
    // 1, 2 deleted; 3 kept (skipped); 4 failed; 5 never tried.
    assert_eq!(
        tried,
        [
            found[0].task_id.clone(),
            found[1].task_id.clone(),
            found[3].task_id.clone()
        ]
    );
    assert_eq!(outcome.deleted.len(), 2);
    assert_eq!(
        outcome.failed,
        Some((fail_at.clone(), "disk on fire".to_owned()))
    );
    assert_eq!(outcome.untouched, [found[4].task_id.clone()]);
    assert_eq!(
        reported.len(),
        3,
        "every result is reported, the failure too"
    );
    assert_eq!(reported[2], (fail_at.clone(), false));
    let message = crate::messages::prune_stopped(&fail_at, "disk on fire", &outcome.untouched, 2);
    assert!(
        message.contains(&fail_at) && message.contains(&found[4].task_id),
        "{message}"
    );

    // Nothing fails: all but the kept one, in order.
    let mut seen = Vec::new();
    let outcome = execute(
        &found,
        |c| {
            seen.push(c.task_id.clone());
            Ok(c.footprint.clone())
        },
        |_, _| {},
    );
    assert_eq!(outcome.deleted.len(), 4);
    assert!(outcome.failed.is_none() && outcome.untouched.is_empty());
    assert!(!seen.contains(&found[2].task_id));
}

/// REQ-WS-014, RFC 006 §6: a deletion that left files behind has deleted the task, and the line
/// says so, naming the task and the paths left to the next start; any other error is `failed`.
#[test]
fn req_ws_014_a_deletion_that_left_files_says_the_task_is_deleted() {
    let task = TaskId::from_uuid(uuid::Uuid::from_u128(7));
    let left = AppError::Runtime(mimlys_core::runtime::RuntimeError::Workspace(
        mimlys_core::spi::WorkspaceError::NotRemoved {
            task_id: task,
            paths: vec!["tasks/a".into(), "tasks/b".into()],
        },
    ));
    assert_eq!(
        failure_line(&task.to_string(), &left),
        format!(
            "task {task} is deleted; 2 path(s) could not be removed and are left to the next start: tasks/a, tasks/b"
        )
    );
    let other = AppError::Io {
        detail: "disk on fire".into(),
    };
    assert!(
        failure_line(&task.to_string(), &other).starts_with(&format!("failed  {task}: ")),
        "{}",
        failure_line(&task.to_string(), &other)
    );
}

/// REQ-WS-014, RFC 006 §6: a task deleted with files left is deleted: the run goes on past it,
/// counts it among the deleted, keeps its paths in `left`, and does not stop (only another error
/// does, and then the files left are named too).
#[test]
fn req_ws_014_a_task_deleted_with_files_left_does_not_stop_the_run() {
    let found: Vec<Candidate> = (1..=4).map(|n| candidate(n, 0)).collect();
    let left = |c: &Candidate| {
        AppError::Runtime(mimlys_core::runtime::RuntimeError::Workspace(
            mimlys_core::spi::WorkspaceError::NotRemoved {
                task_id: TaskId::from_uuid(c.task_id.parse().unwrap()),
                paths: vec![format!("workspaces/{}/tree", c.task_id)],
            },
        ))
    };
    let mut tried = Vec::new();
    let outcome = execute(
        &found,
        |c| {
            tried.push(c.task_id.clone());
            if c.task_id == found[0].task_id {
                Err(left(c))
            } else {
                Ok(c.footprint.clone())
            }
        },
        |_, _| {},
    );
    assert_eq!(tried.len(), 4, "every task is tried");
    assert_eq!(
        outcome.deleted.len(),
        4,
        "the task with files left is deleted"
    );
    assert!(outcome.failed.is_none() && outcome.untouched.is_empty());
    assert_eq!(
        outcome.left,
        [(
            found[0].task_id.clone(),
            vec![format!("workspaces/{}/tree", found[0].task_id)]
        )]
    );
    let line = crate::messages::prune_left(&outcome.left);
    assert!(
        line.contains(&found[0].task_id) && line.contains("next start"),
        "{line}"
    );
    // A failure after it stops the run as before, and the message names the files left as well.
    let outcome = execute(
        &found,
        |c| {
            if c.task_id == found[0].task_id {
                Err(left(c))
            } else if c.task_id == found[1].task_id {
                Err(AppError::Io {
                    detail: "disk on fire".into(),
                })
            } else {
                Ok(c.footprint.clone())
            }
        },
        |_, _| {},
    );
    assert_eq!(outcome.left.len(), 1);
    assert_eq!(
        outcome.failed.as_ref().map(|f| f.0.as_str()),
        Some(found[1].task_id.as_str())
    );
    assert_eq!(
        outcome.untouched,
        [found[2].task_id.clone(), found[3].task_id.clone()]
    );
}

fn orphan(n: u128, bytes: u64) -> Orphan {
    Orphan {
        name: uuid::Uuid::from_u128(n).to_string(),
        bytes,
        reason: orphans::REASON,
    }
}

/// REQ-WS-014 (handoff 013c): the plan lists each workspace no task refers to as `no task: left
/// by a refused creation`, with its footprint and a total; after the plan of the tasks when
/// there is one (`also`), alone when there is not; and a plan with none is what it was.
#[test]
fn req_ws_014_the_plan_lists_the_workspaces_no_task_refers_to() {
    let found = [orphan(1, 2048), orphan(2, 0)];
    let lines = crate::messages::prune_no_task_plan(&found, true);
    assert!(lines[0].starts_with("Pruning also removes 2 workspace(s) that no task refers to"));
    for o in &found {
        let line = lines.iter().find(|l| l.contains(&o.name)).unwrap();
        assert!(
            line.contains("no task: left by a refused creation"),
            "{line}"
        );
    }
    assert!(
        lines
            .iter()
            .any(|l| l.contains("2 KiB") || l.contains("2048"))
    );
    assert!(
        lines
            .last()
            .unwrap()
            .contains("2 workspace(s) with no task")
    );
    let alone = crate::messages::prune_no_task_plan(&found[..1], false);
    assert!(
        alone[0].starts_with("Pruning removes 1 workspace(s)"),
        "{alone:?}"
    );
    // The question says what it asks about.
    assert_eq!(
        crate::messages::prune_confirm(true, false),
        "Delete these tasks?"
    );
    assert_eq!(
        crate::messages::prune_confirm(true, true),
        "Delete these tasks and workspaces?"
    );
    assert_eq!(
        crate::messages::prune_confirm(false, true),
        "Delete these workspaces?"
    );
    // The closing line is what it was when there is none.
    assert_eq!(
        crate::messages::prune_done(2, 0, 1024),
        format!(
            "pruned 2 task(s), {} freed",
            crate::messages::bytes_text(1024)
        )
    );
    assert!(crate::messages::prune_done(2, 1, 1024).contains("1 workspace(s) with no task"));
}

/// REQ-WS-014 (handoff 013c): every workspace is tried and reported; one with paths left does
/// not stop the run, is not counted as removed and keeps its paths.
#[test]
fn req_ws_014_a_workspace_with_paths_left_does_not_stop_the_run() {
    let found = [orphan(1, 10), orphan(2, 20), orphan(3, 30)];
    let mut tried = Vec::new();
    let mut reported = Vec::new();
    let outcome = remove_orphans(
        &found,
        |o| {
            tried.push(o.name.clone());
            if o.name == found[1].name {
                vec![format!("workspaces/{}/tree", o.name)]
            } else {
                vec![]
            }
        },
        |o, left| reported.push((o.name.clone(), left.len())),
    );
    assert_eq!(tried.len(), 3);
    assert_eq!(
        reported.iter().map(|(_, n)| *n).collect::<Vec<_>>(),
        [0, 1, 0]
    );
    assert_eq!(outcome.removed, [found[0].clone(), found[2].clone()]);
    assert_eq!(
        outcome.left,
        [(
            found[1].name.clone(),
            vec![format!("workspaces/{}/tree", found[1].name)]
        )]
    );
    let line = crate::messages::prune_no_task_left(&outcome.left);
    assert!(
        line.contains(&found[1].name) && line.contains("mimlys task prune"),
        "{line}"
    );
}
