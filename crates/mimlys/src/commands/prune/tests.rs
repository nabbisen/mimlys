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
